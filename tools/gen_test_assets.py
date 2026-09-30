"""Generate the test assets: assets/models/crate.obj, ball.obj and walker.mmdl (an animated
figure), the levels in assets/levels, the
placeholder floor texture assets/textures/test_floor.png and the wall textures
(brick_wall.png, panel_wall.png, stone_wall.png).

Usage (from repo root): python3 tools/gen_test_assets.py .

Polygons are listed in perimeter order, then auto-oriented so the Newell
normal points toward a reference point (level: into the sector; crate: out
of the cube). CCW-front convention.
"""
import math, os, struct, sys, zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from moose_model import PROXY, quat_axis, write_model

ROOT = sys.argv[1]


def sub(a, b): return (a[0]-b[0], a[1]-b[1], a[2]-b[2])
def dot(a, b): return a[0]*b[0] + a[1]*b[1] + a[2]*b[2]


def newell(pts):
    n = [0.0, 0.0, 0.0]
    for i, p in enumerate(pts):
        q = pts[(i+1) % len(pts)]
        n[0] += (p[1]-q[1]) * (p[2]+q[2])
        n[1] += (p[2]-q[2]) * (p[0]+q[0])
        n[2] += (p[0]-q[0]) * (p[1]+q[1])
    return tuple(n)


def centroid(pts):
    k = len(pts)
    return tuple(sum(p[i] for p in pts) / k for i in range(3))


def orient(pts, toward):
    """Return pts ordered so the normal points toward `toward`."""
    n = newell(pts)
    if dot(n, sub(toward, centroid(pts))) < 0:
        return list(reversed(pts))
    return list(pts)


def orient_away(pts, away_from):
    n = newell(pts)
    if dot(n, sub(centroid(pts), away_from)) < 0:
        return list(reversed(pts))
    return list(pts)


def lerp3(a, b, t): return tuple(a[i] + (b[i]-a[i]) * t for i in range(3))
def scale3(c, s): return tuple(min(1.0, x * s) for x in c)
def fmt(x): return f"{x:.2f}" if abs(x - round(x, 2)) < 1e-9 else f"{x:.4f}"


# ---------------------------------------------------------------- crate.obj
def crate_obj():
    h = 0.5
    top = (0.78, 0.60, 0.36)
    side_fb = (0.62, 0.45, 0.26)   # faces on +/-Z
    side_lr = (0.52, 0.37, 0.21)   # faces on +/-X
    bottom = (0.30, 0.21, 0.12)
    center = (0.0, 0.5, 0.0)
    faces = [
        ("top",    [(-h, 1, -h), (h, 1, -h), (h, 1, h), (-h, 1, h)], lambda p: top),
        ("bottom", [(-h, 0, -h), (h, 0, -h), (h, 0, h), (-h, 0, h)], lambda p: bottom),
        # Sides darken toward the bottom so interpolation is visible.
        ("front",  [(-h, 0, h), (h, 0, h), (h, 1, h), (-h, 1, h)],
         lambda p: side_fb if p[1] > 0.5 else scale3(side_fb, 0.7)),
        ("back",   [(-h, 0, -h), (h, 0, -h), (h, 1, -h), (-h, 1, -h)],
         lambda p: side_fb if p[1] > 0.5 else scale3(side_fb, 0.7)),
        ("right",  [(h, 0, -h), (h, 0, h), (h, 1, h), (h, 1, -h)],
         lambda p: side_lr if p[1] > 0.5 else scale3(side_lr, 0.7)),
        ("left",   [(-h, 0, -h), (-h, 0, h), (-h, 1, h), (-h, 1, -h)],
         lambda p: side_lr if p[1] > 0.5 else scale3(side_lr, 0.7)),
    ]
    lines = [
        "# Moose v2 test prop: 1 m crate.",
        "# Origin at bottom-center (y = 0 rests on the floor). Units: meters, +Y up.",
        "# Vertex colors use the 'v x y z r g b' extension (0-1 floats).",
        "# 4 vertices per face so each face carries its own tint and its own whole texture",
        "# (uv 0-1, v growing downward, upright on the sides). Faces are",
        "# counter-clockwise viewed from outside (outward facing).",
        "",
        "o crate",
    ]
    def uv(name, p):
        # Sides: u to the right seen from outside, v down from the top edge. Top: seen from
        # above with -Z up; bottom: seen from below with +Z up.
        x, y, z = p
        if name == "top":
            return (x + h, z + h)
        if name == "bottom":
            return (x + h, h - z)
        right = {"front": (x, 1), "back": (-x, 1), "right": (-z, 1), "left": (z, 1)}[name]
        return (right[0] * right[1] + h, 1 - y)

    vlines, tlines, flines = [], [], []
    idx = 1
    for name, pts, color in faces:
        pts = orient_away(pts, center)
        ids = []
        for p in pts:
            c = color(p)
            vlines.append("v " + " ".join(fmt(x) for x in p) + "  " + " ".join(f"{x:.3f}" for x in c))
            tlines.append("vt " + " ".join(fmt(t) for t in uv(name, p)))
            ids.append(f"{idx}/{idx}")
            idx += 1
        flines.append(f"f {' '.join(ids)}  # {name}")
    return "\n".join(lines + vlines + [""] + tlines + [""] + flines) + "\n"


def walker_model():
    """A figure of boxes on a skeleton, walking in place (see Model Format Spec.md): each
    box moves with one bone, and each bone has a box shadow proxy like its part. Origin at
    its feet, facing -Z."""
    # Bones: name, parent, rest place relative to the parent (no rest turn).
    bones = [
        ("hips", None, (0.0, 0.9, 0.0)),
        ("spine", 0, (0.0, 0.1, 0.0)),
        ("head", 1, (0.0, 0.6, 0.0)),
        ("leg_l", 0, (-0.15, 0.0, 0.0)),
        ("leg_r", 0, (0.15, 0.0, 0.0)),
        ("arm_l", 1, (-0.34, 0.55, 0.0)),
        ("arm_r", 1, (0.34, 0.55, 0.0)),
    ]
    shirt, trousers, skin = (0.22, 0.38, 0.66), (0.16, 0.20, 0.36), (0.86, 0.66, 0.50)
    # Parts: bone, box (model space at rest), color.
    parts = [
        (0, ((-0.24, 0.88, -0.12), (0.24, 1.0, 0.12)), trousers),
        (1, ((-0.22, 1.0, -0.12), (0.22, 1.6, 0.12)), shirt),
        (2, ((-0.12, 1.62, -0.13), (0.12, 1.92, 0.13)), skin),
        (3, ((-0.24, 0.0, -0.08), (-0.06, 0.88, 0.08)), trousers),
        (4, ((0.06, 0.0, -0.08), (0.24, 0.88, 0.08)), trousers),
        (5, ((-0.40, 0.95, -0.06), (-0.26, 1.58, 0.06)), shirt),
        (6, ((0.26, 0.95, -0.06), (0.40, 1.58, 0.06)), shirt),
    ]
    positions, position_bones, polygons, colors = [], [], [], []

    def color_row(c):
        colors.append([round(255 * min(1.0, x)) for x in c])
        return len(colors) - 1

    def box(bone, lo, hi, color, flags):
        first = len(positions)
        for k in range(8):
            positions.append((hi[0] if k & 1 else lo[0], hi[1] if k & 2 else lo[1],
                              hi[2] if k & 4 else lo[2]))
            position_bones.append(bone)
        center = tuple((lo[i] + hi[i]) / 2 for i in range(3))
        # Faces by the corners they keep, with a shade: sides, top, bottom.
        faces = [([0, 2, 6, 4], 0.8), ([1, 3, 7, 5], 0.8), ([0, 1, 5, 4], 0.6),
                 ([2, 3, 7, 6], 1.2), ([0, 1, 3, 2], 0.9), ([4, 5, 7, 6], 0.9)]
        for corners, shade in faces:
            pts = [positions[first + k] for k in corners]
            ordered = orient_away(pts, center)
            ids = [first + corners[pts.index(p)] for p in ordered]
            row = color_row(scale3(color, shade))
            polygons.append((flags, [(i, [row]) for i in ids]))

    for bone, (lo, hi), color in parts:
        box(bone, lo, hi, color, 0)
    for bone, (lo, hi), color in parts:
        box(bone, lo, hi, color, PROXY)

    x = (1.0, 0.0, 0.0)
    y = (0.0, 1.0, 0.0)
    rest = [t for (_, _, t) in bones]
    ident = (0.0, 0.0, 0.0, 1.0)

    def pose(turns, bob=0.0):
        frame = []
        for b, t in enumerate(rest):
            if b == 0:
                t = (t[0], t[1] + bob, t[2])
            frame.append((t, turns.get(b, ident)))
        return frame

    walk = []
    for f in range(24):
        a = 2 * math.pi * f / 24
        swing = math.sin(a)
        walk.append(pose({1: quat_axis(y, 0.1 * swing),
                          3: quat_axis(x, 0.5 * swing), 4: quat_axis(x, -0.5 * swing),
                          5: quat_axis(x, -0.4 * swing), 6: quat_axis(x, 0.4 * swing)},
                         bob=0.025 * math.cos(2 * a)))
    idle = [pose({}), pose({1: quat_axis(x, 0.04)}, bob=-0.01)]
    return write_model(
        "walker", positions, position_bones,
        [("color", "u8", 3, colors)], polygons,
        bones=[(n, p, t, ident) for (n, p, t) in bones],
        animations=[("walk", 24.0, True, walk), ("idle", 1.0, True, idle)],
        about=["Moose v2 test actor: a figure of boxes walking in place.",
               "Each part moves with one bone; the second set of boxes are its shadow proxies."])


def ball_obj(radius=0.25, segments=24, rings=12):
    """A smooth ball (UV sphere) around the origin: `rings` bands of latitude, `segments`
    around, quads except triangles at the poles. Each vertex's normal is the average of the
    (unit) normals of every face sharing its position, so shading across faces is smooth."""
    pts = [(0.0, radius, 0.0)]
    for r in range(1, rings):
        lat = math.pi * r / rings
        y, ring = radius * math.cos(lat), radius * math.sin(lat)
        for s in range(segments):
            lon = 2 * math.pi * s / segments
            pts.append((ring * math.sin(lon), y, ring * math.cos(lon)))
    pts.append((0.0, -radius, 0.0))
    bottom = len(pts) - 1

    def at(r, s):  # ring 1..rings-1
        return 1 + (r - 1) * segments + s % segments

    faces = []
    for s in range(segments):
        faces.append([0, at(1, s), at(1, s + 1)])
        faces.append([bottom, at(rings - 1, s + 1), at(rings - 1, s)])
        for r in range(1, rings - 1):
            faces.append([at(r, s), at(r + 1, s), at(r + 1, s + 1), at(r, s + 1)])
    center = (0.0, 0.0, 0.0)
    sums = [[0.0, 0.0, 0.0] for _ in pts]
    for i, f in enumerate(faces):
        n = newell([pts[k] for k in f])
        if dot(n, sub(centroid([pts[k] for k in f]), center)) < 0:
            f.reverse()
            n = tuple(-x for x in n)
        length = math.sqrt(dot(n, n))
        for k in f:
            for a in range(3):
                sums[k][a] += n[a] / length
    normals = [tuple(x / math.sqrt(dot(n, n)) for x in n) for n in sums]
    grey = "0.700 0.700 0.700"
    lines = [
        f"# Moose v2 test prop: a {2 * radius:g} m ball (radius {radius:g} m), origin at its center.",
        "# Smooth normals: each vertex's normal averages the normals of the faces sharing it.",
        "# Faces are counter-clockwise viewed from outside (outward facing).",
        "",
        "o ball",
    ]
    lines += [f"v {p[0]:.6f} {p[1]:.6f} {p[2]:.6f}  {grey}" for p in pts]
    lines += [f"vn {n[0]:.6f} {n[1]:.6f} {n[2]:.6f}" for n in normals]
    lines += ["f " + " ".join(f"{k + 1}//{k + 1}" for k in f) for f in faces]
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------- level
verts = []          # global vertex table
vindex = {}


def V(x, y, z):
    key = (float(x), float(y), float(z))
    if key not in vindex:
        vindex[key] = len(verts)
        verts.append(key)
    return vindex[key]


# Generic attribute declarations: (name, storage format, component count).
# The level file never knows what an attribute means; 'color' is just a name
# a shader can ask for. Values are written as stored (u8 -> integers 0-255).
ATTRIBUTES = [("color", "u8", 3)]
colors = []
cindex = {}


def C(c):
    key = tuple(int(round(x * 255)) for x in c)
    if key not in cindex:
        cindex[key] = len(colors)
        colors.append(key)
    return cindex[key]


sectors = []   # (name, first, count, center)
surfaces = []  # dict(sector, adjoin, flags, verts=[(v, c or None)], comment)
adjoins = []   # (surface, mirror, flags)


def add_surface(sector_id, center, pts, color_fn, comment, portal=False):
    pts = orient(pts, center)
    sv = []
    for p in pts:
        c = None if portal else C(color_fn(p))
        sv.append((V(*p), c))
    surfaces.append(dict(sector=sector_id, adjoin=-1, flags=0, verts=sv, comment=comment))
    return len(surfaces) - 1


def wall_color(low, high, height, shade=1.0):
    return lambda p: scale3(lerp3(low, high, p[1] / height), shade)


def room(name, zd, zf, palette):
    """Box room x in [-4,4], y in [0,4]; doorway wall at z=zd, far wall at z=zf.
    Doorway: x in [-1,1], y in [0,3]."""
    sid = len(sectors)
    first = len(surfaces)
    center = (0.0, 2.0, (zd + zf) / 2)
    floor_c, ceil_c, lo, hi = palette
    H = 4.0
    # No T-junctions: floor/ceiling include the doorway / pillar-top points on
    # the doorway-wall edge; pillars include the door-top corner on their inner edge.
    add_surface(sid, center, [(-4, 0, zd), (-1, 0, zd), (1, 0, zd), (4, 0, zd), (4, 0, zf), (-4, 0, zf)],
                lambda p: floor_c, "floor")
    add_surface(sid, center, [(-4, 4, zd), (-1, 4, zd), (1, 4, zd), (4, 4, zd), (4, 4, zf), (-4, 4, zf)],
                lambda p: ceil_c, "ceiling")
    add_surface(sid, center, [(-4, 0, zf), (4, 0, zf), (4, 4, zf), (-4, 4, zf)],
                wall_color(lo, hi, H), "far wall")
    add_surface(sid, center, [(-4, 0, zd), (-4, 0, zf), (-4, 4, zf), (-4, 4, zd)],
                wall_color(lo, hi, H, 0.85), "left wall (-X)")
    add_surface(sid, center, [(4, 0, zd), (4, 0, zf), (4, 4, zf), (4, 4, zd)],
                wall_color(lo, hi, H, 0.85), "right wall (+X)")
    add_surface(sid, center, [(-4, 0, zd), (-1, 0, zd), (-1, 3, zd), (-1, 4, zd), (-4, 4, zd)],
                wall_color(lo, hi, H), "doorway wall, -X pillar")
    add_surface(sid, center, [(1, 0, zd), (4, 0, zd), (4, 4, zd), (1, 4, zd), (1, 3, zd)],
                wall_color(lo, hi, H), "doorway wall, +X pillar")
    add_surface(sid, center, [(-1, 3, zd), (1, 3, zd), (1, 4, zd), (-1, 4, zd)],
                wall_color(lo, hi, H), "doorway wall, lintel")
    portal = add_surface(sid, center, [(-1, 0, zd), (1, 0, zd), (1, 3, zd), (-1, 3, zd)],
                         None, "portal to hallway", portal=True)
    sectors.append((name, first, len(surfaces) - first))
    return sid, portal


def hallway(name, z0, z1, palette):
    """x in [-1,1], y in [0,3], z in [z1, z0] (z1 < z0). Both ends are portals."""
    sid = len(sectors)
    first = len(surfaces)
    center = (0.0, 1.5, (z0 + z1) / 2)
    floor_c, ceil_c, lo, hi = palette
    H = 3.0
    add_surface(sid, center, [(-1, 0, z0), (1, 0, z0), (1, 0, z1), (-1, 0, z1)], lambda p: floor_c, "floor")
    add_surface(sid, center, [(-1, 3, z0), (1, 3, z0), (1, 3, z1), (-1, 3, z1)], lambda p: ceil_c, "ceiling")
    add_surface(sid, center, [(-1, 0, z0), (-1, 0, z1), (-1, 3, z1), (-1, 3, z0)], wall_color(lo, hi, H), "left wall (-X)")
    add_surface(sid, center, [(1, 0, z0), (1, 0, z1), (1, 3, z1), (1, 3, z0)], wall_color(lo, hi, H), "right wall (+X)")
    pa = add_surface(sid, center, [(-1, 0, z0), (1, 0, z0), (1, 3, z0), (-1, 3, z0)], None, "portal to room_a", portal=True)
    pb = add_surface(sid, center, [(-1, 0, z1), (1, 0, z1), (1, 3, z1), (-1, 3, z1)], None, "portal to room_b", portal=True)
    sectors.append((name, first, len(surfaces) - first))
    return sid, pa, pb


WARM = ((0.40, 0.30, 0.22), (0.85, 0.80, 0.72), (0.45, 0.28, 0.20), (0.90, 0.62, 0.42))
NEUTRAL = ((0.30, 0.33, 0.30), (0.70, 0.74, 0.70), (0.30, 0.36, 0.32), (0.62, 0.72, 0.64))
COOL = ((0.22, 0.27, 0.36), (0.72, 0.78, 0.88), (0.18, 0.26, 0.42), (0.45, 0.62, 0.90))

room_a, pa_room = room("room_a", 0.0, 8.0, WARM)
hall, pa_hall, pb_hall = hallway("hallway", 0.0, -12.0, NEUTRAL)
room_b, pb_room = room("room_b", -12.0, -20.0, COOL)


def link(s1, s2, flags=0x3):
    a1, a2 = len(adjoins), len(adjoins) + 1
    adjoins.append([s1, a2, flags])
    adjoins.append([s2, a1, flags])
    surfaces[s1]["adjoin"] = a1
    surfaces[s2]["adjoin"] = a2


link(pa_room, pa_hall)
link(pb_hall, pb_room)

entities = [
    # kind, sector, model, pos, (pitch, yaw, roll), scale, name
    ("spawn", room_a, "-", (0.0, 0.0, 6.0), (0, 0, 0), 1.0, "player_start"),
    ("prop", room_a, "crate.obj", (-2.5, 0.0, 2.0), (0, 0, 0), 1.0, "crate_a1"),
    ("prop", room_a, "crate.obj", (-2.5, 1.0, 2.0), (0, 15, 0), 1.0, "crate_a2_stacked"),
    ("prop", room_a, "crate.obj", (2.5, 0.0, 5.0), (0, 30, 0), 1.0, "crate_a3"),
    ("prop", hall, "crate.obj", (0.3, 0.0, -6.0), (0, 10, 0), 1.0, "crate_hall"),
    ("prop", room_b, "crate.obj", (0.0, 0.0, -16.0), (0, 45, 0), 1.0, "crate_b1"),
    ("prop", room_b, "crate.obj", (3.0, 0.0, -18.0), (0, 0, 0), 1.0, "crate_b2"),
]


# Texture tile size for generated UVs, in meters: one texture repeat per UV_TILE.
UV_TILE = 2.0


def planar_uv(p, normal):
    """Texture coordinates for point p on a surface with this normal: projected along the
    normal's dominant axis, one texture tile per UV_TILE meters, v growing downward on walls."""
    ax = max(range(3), key=lambda i: abs(normal[i]))
    x, y, z = p
    if ax == 1:
        u, v = x, z
    elif ax == 0:
        u, v = z, -y
    else:
        u, v = x, -y
    return (round(u / UV_TILE, 4), round(v / UV_TILE, 4))


def level_text(title, about, shiny=(), darker=(), uv=False, props=(), ambient=None, lights=(),
               options=None, sky=(), sky_color=(112, 158, 214), directional=(),
               light_options=""):
    """The level as .mmp text. Surfaces listed in `shiny` get the reflective flag (0x1).
    Surfaces listed in `darker` get their vertex colors at half brightness (as new color
    rows, so other surfaces sharing a color keep it). With `uv`, every drawn surface also
    gets texture coordinates (`uv f32 2`, see planar_uv). `props` are extra entities, after
    the shared ones. `ambient` (r, g, b) and `lights` ((sector, position, color, range) for
    a point light, plus (direction, inner, outer) in degrees for a spot light) light it;
    without them it shows its full colors. `options` maps entity names to their options
    (for example "static occluder=facing:16:0.25"). Surfaces listed in `sky` get the sky flag
    (0x2) and `sky_color` (0-255): directional lights ((direction, color, angle in degrees))
    enter through them. `light_options` follow every light's row (for example "radius=0.05")."""
    options = options or {}
    table = list(colors)
    index = dict(cindex)
    attributes = ATTRIBUTES + ([("uv", "f32", 2)] if uv else [])
    uvs, uv_index = [], {}

    def uv_row(key):
        if key not in uv_index:
            uv_index[key] = len(uvs)
            uvs.append(key)
        return uv_index[key]

    def sky_row():
        key = tuple(sky_color)
        if key not in index:
            index[key] = len(table)
            table.append(key)
        return index[key]

    def darker_row(c):
        key = tuple(int(round(x * 0.5)) for x in colors[c])
        if key not in index:
            index[key] = len(table)
            table.append(key)
        return index[key]

    o = []
    o += [
        f"# Moose v2 test level: {about}",
        "# Format reference: 'Level Format Spec.md' in the repo root.",
        "# Right-handed, +Y up, meters. Yaw 0 faces -Z.",
        "",
        "MOOSEMAP 1",
        f'name "{title}"',
        "",
        f"vertices {len(verts)}",
        "#  id       x        y        z",
    ]
    for i, v in enumerate(verts):
        o.append(f"   {i:<4} " + " ".join(f"{fmt(x):>8}" for x in v))
    o += ["", f"attributes {len(attributes)}", "#  id  name    format  count"]
    for i, (n, f, c) in enumerate(attributes):
        o.append(f"   {i:<3} {n:<7} {f:<7} {c}")
    rows = []
    for i, s in enumerate(surfaces):
        normal = newell([verts[v] for v, _ in s["verts"]])
        row = []
        for v, c in s["verts"]:
            if c is None:
                row.append((v,))  # portal: vertex index only
                continue
            refs = (sky_row() if i in sky else darker_row(c) if i in darker else c,)
            if uv:
                refs += (uv_row(planar_uv(verts[v], normal)),)
            row.append((v,) + refs)
        rows.append(row)
    o += ["", f"values color {len(table)}", "#  id   values"]
    for i, c in enumerate(table):
        o.append(f"   {i:<4} " + " ".join(f"{x:>4}" for x in c))
    if uv:
        o += ["", f"values uv {len(uvs)}", "#  id   values"]
        for i, t in enumerate(uvs):
            o.append(f"   {i:<4} " + " ".join(f"{x:>8.4f}" for x in t))
    o += ["", f"sectors {len(sectors)}", "#  id  name      first_surface  surface_count"]
    for i, (n, f, c) in enumerate(sectors):
        o.append(f"   {i:<3} {n:<9} {f:<14} {c}")
    o += ["", f"surfaces {len(surfaces)}",
          "#  id  sector  adjoin  flags  nverts  vert[:attr ...] ...   (flags: 0x1 = reflective, 0x2 = sky; attr = row in each values table, in declaration order; portals list verts only)"]
    last_sector = None
    for i, s in enumerate(surfaces):
        if s["sector"] != last_sector:
            o.append(f"   # --- sector {s['sector']}: {sectors[s['sector']][0]}")
            last_sector = s["sector"]
        vs = " ".join(":".join(str(r) for r in refs) for refs in rows[i])
        flags = s['flags'] | (0x1 if i in shiny else 0) | (0x2 if i in sky else 0)
        comment = s['comment'] + (" (reflective)" if i in shiny else "") + (" (sky)" if i in sky else "")
        o.append(f"   {i:<3} {s['sector']:<7} {s['adjoin']:<7} 0x{flags:<4x} {len(s['verts']):<7} {vs:<34} # {comment}")
    o += ["", f"adjoins {len(adjoins)}",
          "#  id  surface  mirror  flags    (0x1 = render through, 0x2 = passable)"]
    for i, (s, m, f) in enumerate(adjoins):
        o.append(f"   {i:<3} {s:<8} {m:<7} 0x{f:x}")
    all_entities = entities + list(props)
    o += ["", f"entities {len(all_entities)}",
          "#  id  kind   sector  model      x       y       z       pitch  yaw   roll  scale  name  [options]"]
    for i, (k, sec, m, p, r, sc, n) in enumerate(all_entities):
        row = (f"   {i:<3} {k:<6} {sec:<7} {m:<10} " + " ".join(f"{fmt(x):<7}" for x in p)
               + f" {r[0]:<6} {r[1]:<5} {r[2]:<5} {fmt(sc):<6} {n}")
        if n in options:
            row = f"{row:<92} {options[n]}"
        o.append(row)
    if ambient is not None:
        o += ["", "ambient " + " ".join(fmt(c) for c in ambient)]
    if lights:
        o += ["", f"lights {len(lights)}",
              "#  id  sector  x       y       z       r      g      b      range  "
              "[dx     dy     dz     inner  outer]   (spot lights: direction, cone half-angles)"]
        for i, (sec, p, c, rng, *spot) in enumerate(lights):
            row = (f"   {i:<3} {sec:<7} " + " ".join(f"{fmt(x):<7}" for x in p)
                   + " " + " ".join(f"{fmt(x):<6}" for x in c) + f" {fmt(rng):<6}")
            if spot:
                (d, inner, outer) = spot
                row += " " + " ".join(f"{fmt(x):<6}" for x in d) + f" {fmt(inner):<6} {fmt(outer)}"
            o.append((row + " " + light_options).rstrip())
    if directional:
        o += ["", f"directional {len(directional)}",
              "#  id  dx     dy     dz     r      g      b      angle   (the way the light travels; "
              "angle: the source's size in degrees)"]
        for i, (d, c, angle) in enumerate(directional):
            o.append(f"   {i:<3} " + " ".join(f"{fmt(x):<6}" for x in d)
                     + " " + " ".join(f"{fmt(x):<6}" for x in c) + f" {fmt(angle)}")
    return "\n".join(o) + "\n"


# ---------------------------------------------------------------- textures
def png_rgba(width, height, pixels):
    """A PNG file (8-bit RGBA) from rows of (r, g, b, a) tuples."""
    raw = b"".join(b"\x00" + bytes(c for px in row for c in px) for row in pixels)

    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def test_floor():
    """64x64 placeholder floor tile: a polished stone slab inside a dark grout border.
    Alpha is roughness, from 128 (fully shiny) to 255 (fully rough): the slab is shiny
    with a little variation and a few rough scratches, the grout rough."""
    n = 64
    rows = []
    for y in range(n):
        row = []
        for x in range(n):
            grout = x < 2 or y < 2
            # Smooth deterministic variation for the stone.
            s = (math.sin(x * 0.37 + y * 0.11) + math.sin(y * 0.29 - x * 0.07)
                 + 0.5 * math.sin((x + y) * 0.9)) / 2.5
            scratch = abs((x - 2 * y + 70) % 41 - 20) < 1 or abs((3 * x + y) % 53 - 26) < 1
            if grout:
                rgb, rough = (46, 42, 38), 255
            else:
                base = 150 + int(30 * s)
                rgb = (base, base - 12, base - 26)
                rough = 200 if scratch else 128 + int(12 * (s + 1))
            row.append(rgb + (rough,))
        rows.append(row)
    return png_rgba(n, n, rows)


# Wall textures: 64x64 tiles, one per sector of the test levels (the app gives sector i the
# i-th, cycling). Tileable: their noise repeats every S texels.
S = 64


def hash2(x, y, seed):
    h = (x * 374761393 + y * 668265263 + seed * 2147483647) & 0xffffffff
    h = ((h ^ (h >> 13)) * 1274126177) & 0xffffffff
    return ((h ^ (h >> 16)) & 0xffff) / 65535.0


def value_noise(x, y, cell, seed):
    """Smooth noise that tiles every S texels (cell divides S)."""
    n = S // cell
    gx, gy = x / cell, y / cell
    x0, y0 = int(gx), int(gy)
    fx, fy = gx - x0, gy - y0
    fx, fy = fx * fx * (3 - 2 * fx), fy * fy * (3 - 2 * fy)
    v = lambda i, j: hash2(i % n, j % n, seed)
    a = v(x0, y0) + (v(x0 + 1, y0) - v(x0, y0)) * fx
    b = v(x0, y0 + 1) + (v(x0 + 1, y0 + 1) - v(x0, y0 + 1)) * fx
    return a + (b - a) * fy


def fbm(x, y, seed):
    return (value_noise(x, y, 16, seed) * 0.5 + value_noise(x, y, 8, seed + 1) * 0.3
            + value_noise(x, y, 4, seed + 2) * 0.2)


def clamp(v):
    return max(0, min(255, int(round(v))))


def brick(x, y):
    # Running bond: bricks 16x8 with a 1-texel mortar line, rows offset by half a brick.
    # Alpha masks the close-up detail noise: none on the mortar, full on the bricks.
    row = y // 8
    bx = (x + (8 if row % 2 else 0)) % S
    col = bx // 16
    mortar = y % 8 == 7 or bx % 16 == 15
    grit = fbm(x, y, 7) - 0.5
    speck = hash2(x, y, 3) - 0.5
    if mortar:
        m = 150 + grit * 30 + speck * 14
        return m * 0.95, m * 0.9, m * 0.82, 0
    tone = hash2(col, row, 11)  # each brick its own shade
    r = 150 + tone * 45 + grit * 40 + speck * 18
    g = 70 + tone * 20 + grit * 22 + speck * 10
    b = 48 + tone * 12 + grit * 16 + speck * 8
    # Slightly darker lower edge and lighter upper edge give the bricks some depth.
    edge = y % 8
    k = 1.12 if edge == 0 else (0.82 if edge == 6 else 1.0)
    return r * k, g * k, b * k, 255


def panel(x, y):
    # 2x2 metal panels of 32x32: a dark seam, a bevel (light top-left, dark bottom-right),
    # rivets inset from each corner, and brushed streaks.
    px, py = x % 32, y % 32
    streak = value_noise(x, (y // 2) * 2, 4, 21) - 0.5
    grime = fbm(x, y, 23) - 0.5
    speck = hash2(x, y, 5) - 0.5
    base = 118 + streak * 22 + grime * 36 + speck * 8
    r, g, b = base * 0.86, base * 0.97, base * 0.9
    if px == 31 or py == 31:
        return 34, 40, 37
    if px == 0 or py == 0:
        r, g, b = r * 1.3, g * 1.3, b * 1.3
    elif px == 30 or py == 30:
        r, g, b = r * 0.65, g * 0.65, b * 0.65
    for cx in (4, 26):
        for cy in (4, 26):
            dx, dy = px - cx, py - cy
            d2 = dx * dx + dy * dy
            if d2 <= 2:
                lit = 1.45 if dx + dy < 0 else (1.0 if dx + dy == 0 else 0.6)
                return 150 * lit, 160 * lit, 152 * lit
    # A horizontal stripe across the middle of each panel.
    if 14 <= py <= 17:
        k = 0.8 if py in (14, 17) else 0.72
        r, g, b = r * k, g * k, b * k
    return r, g, b


# Stone courses: each 16 rows tall, blocks of varied widths that tile across 64.
COURSES = [[0, 24, 40], [0, 12, 36, 52], [0, 20, 44], [0, 8, 32, 48]]
SHIFT = [0, 6, 12, 2]


def stone(x, y):
    course = y // 16
    cy = y % 16
    sx = (x + SHIFT[course]) % S
    starts = COURSES[course]
    k = max(i for i, s in enumerate(starts) if s <= sx)
    left = starts[k]
    right = starts[k + 1] if k + 1 < len(starts) else S
    cx = sx - left
    width = right - left
    joint = cy == 15 or cx == width - 1
    grit = fbm(x, y, 41) - 0.5
    speck = hash2(x, y, 9) - 0.5
    if joint:
        m = 52 + grit * 20
        return m * 0.85, m * 0.92, m * 1.1
    tone = hash2(course * 7 + k, course, 17)
    v = 112 + tone * 34 + grit * 46 + speck * 16
    r, g, b = v * 0.78, v * 0.88, v * 1.05
    # Chiseled edges: light along the top and left, shadow along the bottom and right.
    if cy == 0 or cx == 0:
        r, g, b = r * 1.22, g * 1.22, b * 1.22
    elif cy == 14 or cx == width - 2:
        r, g, b = r * 0.7, g * 0.7, b * 0.7
    # A few cracks: dark texels where fine noise dips.
    if value_noise(x, y, 4, 43 + course) < 0.08:
        r, g, b = r * 0.7, g * 0.7, b * 0.7
    return r, g, b


def wall_texture(shade):
    """A 64x64 PNG from `shade(x, y) -> (r, g, b)` (alpha 255) or `(r, g, b, a)`."""
    def texel(x, y):
        c = tuple(clamp(c) for c in shade(x, y))
        return c if len(c) == 4 else c + (255,)
    return png_rgba(S, S, [[texel(x, y) for x in range(S)] for y in range(S)])


os.makedirs(os.path.join(ROOT, "assets/textures"), exist_ok=True)
with open(os.path.join(ROOT, "assets/textures/test_floor.png"), "wb") as f:
    f.write(test_floor())
# room_a: warm brick; hallway: grey-green metal panels; room_b: blue-grey stone blocks.
for name, shade in [("brick_wall.png", brick), ("panel_wall.png", panel),
                    ("stone_wall.png", stone)]:
    with open(os.path.join(ROOT, "assets/textures", name), "wb") as f:
        f.write(wall_texture(shade))

os.makedirs(os.path.join(ROOT, "assets/models"), exist_ok=True)
os.makedirs(os.path.join(ROOT, "assets/levels"), exist_ok=True)
with open(os.path.join(ROOT, "assets/models/crate.obj"), "w") as f:
    f.write(crate_obj())
with open(os.path.join(ROOT, "assets/models/ball.obj"), "w") as f:
    f.write(ball_obj())
with open(os.path.join(ROOT, "assets/models/walker.mmdl"), "w") as f:
    f.write(walker_model())
with open(os.path.join(ROOT, "assets/levels/two_rooms.mmp"), "w") as f:
    f.write(level_text("Two Rooms", "two box rooms joined by a hallway through two portals."))
# Same level with shiny (reflective) floors throughout (both rooms and the hallway, so
# reflections continue across adjacent floors). The two room floors are also darker, so their
# reflections stand out. A mirror ball floats in room_b (the app bakes a cube map of the room
# for it).
room_floors = {sectors[room_a][1], sectors[room_b][1]}
shiny_floors = room_floors | {sectors[hall][1]}
room_a_ceiling = sectors[room_a][1] + 1
# Lit by a dim ambient light and five point lights (warm ones in room_a, a cool one halfway
# down the hallway, a cool one and a magenta accent in room_b); the app adds the player's
# flashlight. (sector, position, color, range in meters[, direction, inner and outer
# half-angles in degrees, for a spot light].)
shiny_lights = [
    (room_a, (-2.5, 3.2, 5.5), (0.8, 0.62, 0.42), 6.0),
    (room_a, (2.8, 1.8, 1.2), (1.2, 0.7, 0.4), 5.0),
    (hall, (0.0, 2.6, -6.0), (0.7, 0.95, 1.2), 5.0),
    (room_b, (2.5, 3.2, -14.5), (0.8, 1.05, 1.6), 7.0),
    (room_b, (-2.8, 1.2, -18.5), (1.3, 0.55, 1.1), 5.0),
]
with open(os.path.join(ROOT, "assets/levels/shiny_rooms.mmp"), "w") as f:
    f.write(level_text("Shiny Rooms",
                       "two_rooms.mmp with reflective floors throughout, darker in the rooms, "
                       "lit by five point lights.",
                       shiny=shiny_floors, darker=room_floors, uv=True,
                       ambient=(0.01, 0.01, 0.013), lights=shiny_lights,
                       # Lamps a few centimeters across: soft-edged shadows.
                       light_options="radius=0.05",
                       props=[("prop", room_b, "ball.obj", (-1.5, 1.5, -15.0), (0, 0, 0), 1.0,
                               "mirror_ball")],
                       # The crates never move and cast shadows as themselves; the ball casts
                       # a polygon facing the light, on its outline.
                       options={**{n: "static" for (k, _, m, *_, n) in entities if m == "crate.obj"},
                                "mirror_ball": "static occluder=facing:16:0.25"}))
# The shiny floors with room_a's ceiling reflective too: two facing mirrors, for
# reflections of reflections.
with open(os.path.join(ROOT, "assets/levels/mirror_rooms.mmp"), "w") as f:
    f.write(level_text("Mirror Rooms",
                       "shiny floors throughout, darker in the rooms, and room_a's ceiling "
                       "reflective too.",
                       shiny=shiny_floors | {room_a_ceiling}, darker=room_floors, uv=True))
# room_b open to the sky: a courtyard. The sun comes in over its walls from the south-west,
# low enough to cast long shadows and to shine through its doorway down the hallway.
# Lit by the sun, a moving lamp in room_a (and the player's flashlight) and a dim
# sky-blue ambient.
room_b_ceiling = sectors[room_b][1] + 1
with open(os.path.join(ROOT, "assets/levels/sunny_rooms.mmp"), "w") as f:
    f.write(level_text("Sunny Rooms",
                       "two_rooms.mmp with room_b open to the sky, lit by the sun and a moving lamp.",
                       uv=True, ambient=(0.05, 0.06, 0.09), sky={room_b_ceiling},
                       directional=[((0.45, -0.8, 0.55), (2.0, 1.85, 1.6), 0.53)],
                       # A light-blue lamp in room_a's middle, swinging 2 m toward each of
                       # the two corners without crates (+x -z and -x +z): moving shadows
                       # (it's carved every frame, not baked).
                       lights=[(room_a, (0.0, 2.3, 4.0), (0.45, 0.75, 1.5), 6.0)],
                       light_options="radius=0.05 oscillate=1.4142:0:-1.4142:5",
                       options={n: "static" for (k, _, m, *_, n) in entities if m == "crate.obj"}))
# The courtyard with an animated figure walking in place in the sun: its shadow is cast by a
# box proxy per bone, carved every frame as it moves.
with open(os.path.join(ROOT, "assets/levels/walker_rooms.mmp"), "w") as f:
    f.write(level_text("Walker Rooms",
                       "sunny_rooms.mmp's courtyard with an animated figure (walker.mmdl) in it.",
                       uv=True, ambient=(0.05, 0.06, 0.09), sky={room_b_ceiling},
                       directional=[((0.45, -0.8, 0.55), (2.0, 1.85, 1.6), 0.53)],
                       props=[("actor", room_b, "walker.mmdl", (1.5, 0.0, -16.5), (0, 30, 0), 1.0,
                               "walker")],
                       options={**{n: "static" for (k, _, m, *_, n) in entities if m == "crate.obj"},
                                "walker": "anim=walk"}))
print(f"verts={len(verts)} color_values={len(colors)} surfaces={len(surfaces)} adjoins={len(adjoins)}")
