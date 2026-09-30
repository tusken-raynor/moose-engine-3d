"""Blender exporter for Moose models (.mmdl, see "Model Format Spec.md").

Two ways to run it:
- From the command line, for one file or a batch:
      blender model.blend --background --python tools/blender/moose_export.py -- out.mmdl
  (exports the selected objects, or every visible mesh if none is selected).
- Inside Blender: open this file in the Text Editor and Run Script. It adds
  File > Export > Moose Model (.mmdl).

It needs tools/moose_model.py next to its folder (it finds it from this file's place), so
run it from the repo rather than installing it as an add-on.

What it exports:
- Mesh objects as polygons, n-gons kept. A polygon that isn't flat and convex, or whose
  corners belong to different bones, is split into its triangles (which stay flat as the
  bones move). Modifiers are applied, with the armature at its rest pose.
- Objects named proxy_* (any case) as shadow proxies: not drawn, they cast the model's
  shadows, one part per bone. Give each bone a box.
- Attributes: `color` (u8 3, linear 0-255) from the active color attribute, or else the
  face's material's viewport color; and `uv` (f32 2, v growing downward) if any mesh has a
  UV map.
- An armature (the one the meshes deform with, or one selected): every bone, parents first,
  with its rest pose relative to its parent. Each vertex moves with the bone of its
  strongest vertex group; a vertex in no bone's group goes with the first bone.
- Every action that animates the armature's bones, sampled at every frame of its range at
  the scene's frame rate. An action loops unless its name ends in _once, or it has a
  manual frame range without Cyclic. A looping action whose last frame repeats its first
  drops the repeat.

Axes: Blender is Z up; Moose is Y up, facing -Z. By default a model built facing Blender's
front view (toward -Y) faces Moose's forward: Blender (x, y, z) becomes (-x, z, y). Turn
off "Faces -Y" for Blender's OBJ export axes instead, (x, z, -y). The model's origin is the
armature's (or the world's, without one), and its feet should stand on it.

The conversion logic is in plain functions without Blender (tested by
tools/blender/test_moose_export.py); only gather() and the operator touch bpy.
"""

import math
import os
import re
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from moose_model import PROXY, write_model  # noqa: E402

try:
    import bpy
    import mathutils
except ImportError:  # Outside Blender: the plain functions are still usable.
    bpy = None
    mathutils = None

bl_info = {
    "name": "Moose Model (.mmdl)",
    "author": "Moose engine",
    "version": (1, 0),
    "blender": (3, 6, 0),
    "location": "File > Export > Moose Model (.mmdl)",
    "description": "Exports meshes, shadow proxies, an armature and its actions",
    "category": "Import-Export",
}

# The engine's tolerance for flat and convex polygons (moose-assets geom.rs), a bit tighter.
TOLERANCE = 5e-5


# ---- Plain functions (no Blender)

def sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def length(a):
    return math.sqrt(dot(a, a))


def flat_and_convex(points):
    """Whether `points` (an outline, in order) make a flat convex polygon with some area,
    as the engine's loader checks."""
    n = len(points)
    if n < 3:
        return False
    centroid = tuple(sum(p[i] for p in points) / n for i in range(3))
    size = max(length(sub(p, centroid)) for p in points)
    tol = TOLERANCE * max(size, 1.0)
    normal = [0.0, 0.0, 0.0]
    for i in range(n):
        p, q = sub(points[i], centroid), sub(points[(i + 1) % n], centroid)
        normal[0] += (p[1] - q[1]) * (p[2] + q[2])
        normal[1] += (p[2] - q[2]) * (p[0] + q[0])
        normal[2] += (p[0] - q[0]) * (p[1] + q[1])
    area2 = length(normal)
    if area2 <= TOLERANCE * size * size * 4:
        return False
    normal = tuple(v / area2 for v in normal)
    if any(abs(dot(normal, sub(p, centroid))) > tol for p in points):
        return False
    turning = 0.0
    for i in range(n):
        a, b, c = points[(i - 1) % n], points[i], points[(i + 1) % n]
        e1, e2 = sub(b, a), sub(c, b)
        if length(e2) <= tol:
            return False
        angle = math.atan2(dot(cross(e1, e2), normal), dot(e1, e2))
        if angle < -5e-4:
            return False
        turning += angle
    return abs(turning - 2 * math.pi) < 5e-4


def face_outlines(corners, points, bones, triangles):
    """How to write one face: as itself, if it is flat, convex and on one bone; otherwise as
    its triangles (those with area). `corners` are its point indices in order, `triangles`
    its triangulation as triples of positions in `corners`."""
    outline = [points[c] for c in corners]
    if len({bones[c] for c in corners}) == 1 and flat_and_convex(outline):
        return [list(range(len(corners)))]
    return [list(t) for t in triangles if flat_and_convex([outline[k] for k in t])]


def dominant_bone(weights, fallback=0):
    """The bone with the largest weight among (bone, weight) pairs, or `fallback`."""
    best = max(weights, key=lambda bw: bw[1], default=None)
    return best[0] if best and best[1] > 0 else fallback


def parents_first(names, parents):
    """An order of bones (by index) in which each comes after its parent (None for a root):
    the original order, with each bone moved after its parent if needed."""
    order, placed = [], set()

    def place(i, seen=()):
        if i in placed:
            return
        if i in seen:
            raise ValueError(f"bone {names[i]} is its own ancestor")
        if parents[i] is not None:
            place(parents[i], seen + (i,))
        order.append(i)
        placed.add(i)

    for i in range(len(names)):
        place(i)
    return order


def token(name):
    """`name` as one word the formats accept (letters, digits, _ . -)."""
    return re.sub(r"[^A-Za-z0-9_.-]", "_", name) or "_"


def loops(name, manual_range=False, cyclic=False):
    """Whether an action loops (see the module's notes)."""
    if name.lower().endswith("_once"):
        return False
    return cyclic or not manual_range


def drop_repeated_end(frames, close=1e-4):
    """For a looping animation: its frames without the last if it repeats the first."""
    if len(frames) < 2:
        return frames

    def same(a, b):
        (ta, qa), (tb, qb) = a, b
        # q and -q are the same turn.
        return (max(abs(x - y) for x, y in zip(ta, tb)) < close
                and abs(abs(sum(x * y for x, y in zip(qa, qb))) - 1.0) < close)

    if all(same(a, b) for a, b in zip(frames[0], frames[-1])):
        return frames[:-1]
    return frames


class Rows:
    """An attribute's values table: each distinct row once."""

    def __init__(self):
        self.rows, self.index = [], {}

    def add(self, row):
        key = tuple(row)
        if key not in self.index:
            self.index[key] = len(self.rows)
            self.rows.append(list(row))
        return self.index[key]


class Model:
    """A model as it is gathered: parts (objects) added one at a time, then written."""

    def __init__(self, name, uv=False):
        self.name = name
        self.uv = uv
        self.positions, self.position_bones, self.polygons = [], [], []
        self.colors, self.uvs = Rows(), Rows()
        self.bones, self.animations = [], []

    def add_part(self, points, bones, faces, proxy=False):
        """A mesh: `points` in model space, each point's bone, and `faces` as
        (corners, colors, uvs, triangles): its point indices in order, a color (linear
        0-1) and a uv (Blender's, v up) per corner, and its triangulation as triples of
        positions in `corners`."""
        base = len(self.positions)
        self.positions += [tuple(p) for p in points]
        self.position_bones += list(bones)
        for corners, colors, uvs, triangles in faces:
            for outline in face_outlines(corners, points, bones, triangles):
                refs = []
                for k in outline:
                    rows = [self.colors.add([round(255 * min(1.0, max(0.0, c))) for c in colors[k][:3]])]
                    if self.uv:
                        u, v = uvs[k] if uvs else (0.0, 0.0)
                        rows.append(self.uvs.add([round(u, 6), round(1.0 - v, 6)]))
                    refs.append((base + corners[k], rows))
                self.polygons.append((PROXY if proxy else 0, refs))

    def text(self):
        attributes = [("color", "u8", 3, self.colors.rows)]
        if self.uv:
            attributes.append(("uv", "f32", 2, self.uvs.rows))
        # Positions no polygon uses are dropped by the loader; keep the file lean anyway.
        used = sorted({p for _, refs in self.polygons for p, _ in refs})
        remap = {old: new for new, old in enumerate(used)}
        positions = [self.positions[p] for p in used]
        position_bones = [self.position_bones[p] for p in used]
        polygons = [(flags, [(remap[p], rows) for p, rows in refs]) for flags, refs in self.polygons]
        drawn = sum(1 for flags, _ in polygons if not flags & PROXY)
        about = [f"Exported from Blender: {drawn} polygons, {len(polygons) - drawn} shadow proxies, "
                 f"{len(self.bones)} bones, {len(self.animations)} animations."]
        return write_model(self.name, positions, position_bones, attributes, polygons,
                           self.bones, self.animations, about)


# ---- Blender

def axes(faces_minus_y):
    """Blender to Moose model space."""
    if faces_minus_y:
        rows = ((-1, 0, 0, 0), (0, 0, 1, 0), (0, 1, 0, 0), (0, 0, 0, 1))
    else:
        rows = ((1, 0, 0, 0), (0, 0, 1, 0), (0, -1, 0, 0), (0, 0, 0, 1))
    return mathutils.Matrix(rows)


def pose(matrix):
    """A 4x4 matrix as ((x, y, z), (qx, qy, qz, qw)), dropping any scale."""
    t, q, _ = matrix.decompose()
    return (tuple(t), (q.x, q.y, q.z, q.w))


def find_armature(objects):
    for obj in objects:
        if obj.type == "ARMATURE":
            return obj
    for obj in objects:
        for m in getattr(obj, "modifiers", ()):
            if m.type == "ARMATURE" and m.object:
                return m.object
        if obj.parent and obj.parent.type == "ARMATURE":
            return obj.parent
    return None


def gather(context, name, faces_minus_y=True):
    """The selected objects (or every visible mesh) as a Model."""
    objects = list(context.selected_objects) or [o for o in context.view_layer.objects if o.visible_get()]
    meshes = [o for o in objects if o.type == "MESH"]
    if not meshes:
        raise ValueError("nothing to export: select mesh objects")
    armature = find_armature(objects)
    to_moose = axes(faces_minus_y)
    model = Model(name, uv=any(o.data.uv_layers for o in meshes))

    # Bones, parents first.
    bone_names, order = [], []
    if armature:
        data_bones = list(armature.data.bones)
        names = [b.name for b in data_bones]
        parents = [names.index(b.parent.name) if b.parent else None for b in data_bones]
        order = parents_first(names, parents)
        bone_names = [names[i] for i in order]
        for i in order:
            b = data_bones[i]
            if b.parent:
                local = b.parent.matrix_local.inverted() @ b.matrix_local
                parent = bone_names.index(b.parent.name)
            else:
                local = to_moose @ b.matrix_local
                parent = None
            t, q = pose(local)
            model.bones.append((token(b.name), parent, t, q))

    # Meshes at the rest pose.
    rest_was = armature.data.pose_position if armature else None
    if armature:
        armature.data.pose_position = "REST"
    context.view_layer.update()
    depsgraph = context.evaluated_depsgraph_get()
    space = to_moose @ (armature.matrix_world.inverted() if armature else mathutils.Matrix.Identity(4))
    try:
        for obj in meshes:
            evaluated = obj.evaluated_get(depsgraph)
            mesh = evaluated.to_mesh()
            try:
                add_mesh(model, obj, mesh, space @ evaluated.matrix_world, bone_names)
            finally:
                evaluated.to_mesh_clear()
    finally:
        if armature:
            armature.data.pose_position = rest_was
            context.view_layer.update()

    if armature:
        model.animations = sample_actions(context, armature, order, to_moose)
    return model


def add_mesh(model, obj, mesh, matrix, bone_names):
    proxy = obj.name.lower().startswith("proxy_")
    mirrored = matrix.determinant() < 0
    points = [tuple(matrix @ v.co) for v in mesh.vertices]
    # Each vertex's bone: its strongest group that names a bone.
    group_bone = {g.index: bone_names.index(g.name) for g in obj.vertex_groups if g.name in bone_names}
    fallback = 0
    if obj.parent_type == "BONE" and obj.parent_bone in bone_names:
        fallback = bone_names.index(obj.parent_bone)
    bones = [dominant_bone([(group_bone[g.group], g.weight) for g in v.groups if g.group in group_bone], fallback)
             for v in mesh.vertices]
    if not bone_names:
        bones = [None] * len(points)

    color_layer = mesh.color_attributes.active_color if hasattr(mesh, "color_attributes") else None
    uv_layer = mesh.uv_layers.active
    mesh.calc_loop_triangles()
    triangles_of = {}
    for tri in mesh.loop_triangles:
        triangles_of.setdefault(tri.polygon_index, []).append(tuple(tri.loops))
    faces = []
    for poly in mesh.polygons:
        loop_ids = list(poly.loop_indices)
        corners = [mesh.loops[i].vertex_index for i in loop_ids]
        if color_layer is not None:
            if color_layer.domain == "CORNER":
                colors = [tuple(color_layer.data[i].color) for i in loop_ids]
            else:
                colors = [tuple(color_layer.data[c].color) for c in corners]
        else:
            slots = obj.material_slots
            material = slots[min(poly.material_index, len(slots) - 1)].material if len(slots) else None
            color = tuple(material.diffuse_color) if material else (0.8, 0.8, 0.8)
            colors = [color] * len(corners)
        uvs = [tuple(uv_layer.data[i].uv) for i in loop_ids] if uv_layer else None
        triangles = [tuple(loop_ids.index(i) for i in tri) for tri in triangles_of.get(poly.index, [])]
        if mirrored:
            # A mirroring transform turns the winding over: put it back.
            corners, colors = corners[::-1], colors[::-1]
            uvs = uvs[::-1] if uvs else uvs
            last = len(corners) - 1
            triangles = [tuple(last - k for k in reversed(t)) for t in triangles]
        faces.append((corners, colors, uvs, triangles))
    model.add_part(points, bones, faces, proxy)


def sample_actions(context, armature, order, to_moose):
    scene = context.scene
    fps = scene.render.fps / scene.render.fps_base
    data_bones = list(armature.data.bones)
    pose_bones = [armature.pose.bones[data_bones[i].name] for i in order]
    if armature.animation_data is None:
        armature.animation_data_create()
    action_was, frame_was = armature.animation_data.action, scene.frame_current
    bone_paths = {f'pose.bones["{b.name}"]' for b in pose_bones}
    animations = []
    try:
        for action in bpy.data.actions:
            if not any(path.startswith(p) for path in action_paths(action) for p in bone_paths):
                continue
            armature.animation_data.action = action
            slots = getattr(action, "slots", None)
            if slots and hasattr(armature.animation_data, "action_slot"):
                armature.animation_data.action_slot = slots[0]
            start, end = (int(round(f)) for f in action.frame_range)
            frames = []
            for f in range(start, end + 1):
                scene.frame_set(f)
                poses = []
                for pb in pose_bones:
                    local = pb.parent.matrix.inverted() @ pb.matrix if pb.parent else to_moose @ pb.matrix
                    poses.append(pose(local))
                frames.append(poses)
            looping = loops(action.name, action.use_frame_range, getattr(action, "use_cyclic", False))
            if looping:
                frames = drop_repeated_end(frames)
            animations.append((token(action.name), fps, looping, frames))
    finally:
        armature.animation_data.action = action_was
        scene.frame_set(frame_was)
    return animations


def action_paths(action):
    """The data paths an action animates: its F-curves' (layered actions, Blender 4.4 on,
    keep them in channel bags)."""
    layers = getattr(action, "layers", None)
    if layers:
        for layer in layers:
            for strip in layer.strips:
                for bag in getattr(strip, "channelbags", ()):
                    for fc in bag.fcurves:
                        yield fc.data_path
    else:
        for fc in getattr(action, "fcurves", ()):
            yield fc.data_path


def export(context, path, faces_minus_y=True):
    name = token(os.path.splitext(os.path.basename(path))[0])
    model = gather(context, name, faces_minus_y)
    with open(path, "w") as f:
        f.write(model.text())
    return model


if bpy is not None:
    from bpy_extras.io_utils import ExportHelper

    class ExportMoose(bpy.types.Operator, ExportHelper):
        """Export the selection as a Moose model"""
        bl_idname = "export_scene.moose_mmdl"
        bl_label = "Export Moose Model"
        filename_ext = ".mmdl"
        filter_glob: bpy.props.StringProperty(default="*.mmdl", options={"HIDDEN"})
        faces_minus_y: bpy.props.BoolProperty(
            name="Faces -Y",
            description="The model faces Blender's front view (-Y); off: OBJ export axes",
            default=True,
        )

        def execute(self, context):
            try:
                model = export(context, self.filepath, self.faces_minus_y)
            except ValueError as e:
                self.report({"ERROR"}, str(e))
                return {"CANCELLED"}
            self.report({"INFO"}, f"wrote {self.filepath}: {len(model.polygons)} polygons, "
                                  f"{len(model.bones)} bones, {len(model.animations)} animations")
            return {"FINISHED"}

    def menu(self, context):
        self.layout.operator(ExportMoose.bl_idname, text="Moose Model (.mmdl)")

    def register():
        bpy.utils.register_class(ExportMoose)
        bpy.types.TOPBAR_MT_file_export.append(menu)

    def unregister():
        bpy.types.TOPBAR_MT_file_export.remove(menu)
        bpy.utils.unregister_class(ExportMoose)

    if __name__ == "__main__":
        if "--" in sys.argv:
            out = sys.argv[sys.argv.index("--") + 1]
            model = export(bpy.context, out)
            print(f"wrote {out}: {len(model.polygons)} polygons, {len(model.bones)} bones, "
                  f"{len(model.animations)} animations")
        else:
            register()
