"""Writes Moose .mmdl models (see "Model Format Spec.md") from plain data.

Used by gen_test_assets.py for the test models, and by the Blender exporter
(tools/blender/moose_export.py), which only gathers the data from Blender and hands it here.
Nothing here needs Blender.

The data, all plain Python:
- positions: [(x, y, z)], in model space at the rest pose.
- position_bones: [bone index] per position (or None throughout, for a model without bones).
- attributes: [(name, format, count, rows)], rows being lists of `count` numbers.
- polygons: [(flags, [(position, [row per attribute]), ...])], corners counter-clockwise
  seen from outside. Flag 0x100 marks a shadow proxy (not drawn).
- bones: [(name, parent index or None, (x, y, z), (qx, qy, qz, qw))]: rest poses relative
  to the parent, parents first.
- animations: [(name, fps, loop, frames)], frames being lists (one per frame) of per-bone
  poses ((x, y, z), (qx, qy, qz, qw)), relative to each bone's parent.
"""

PROXY = 0x100


def number(x, integer=False):
    if integer:
        return str(int(round(x)))
    if abs(x - round(x, 2)) < 1e-9:
        return f"{x:.2f}"
    return f"{x:.6f}".rstrip("0")


def write_model(name, positions, position_bones, attributes, polygons, bones=(), animations=(),
                about=()):
    out = ["MOOSEMODEL 1", f'name "{name}"']
    out += [f"# {line}" for line in about]
    out += ["", f"positions {len(positions)}", "#  id  x  y  z  bone"]
    for i, p in enumerate(positions):
        bone = "-" if not bones else str(position_bones[i])
        out.append(f"   {i:<4} {' '.join(number(v) for v in p)}  {bone}")
    out += ["", f"attributes {len(attributes)}", "#  id  name  format  count"]
    for i, (attr, fmt, count, _) in enumerate(attributes):
        out.append(f"   {i:<3} {attr} {fmt} {count}")
    for attr, fmt, count, rows in attributes:
        integer = fmt in ("u8", "i8", "i16")
        out += ["", f"values {attr} {len(rows)}"]
        for i, row in enumerate(rows):
            assert len(row) == count, f"{attr} row {i} has {len(row)} values, not {count}"
            out.append(f"   {i:<4} {' '.join(number(v, integer) for v in row)}")
    out += ["", f"polygons {len(polygons)}", "#  id  flags  nverts  position[:row ...] ..."]
    for i, (flags, corners) in enumerate(polygons):
        refs = " ".join(":".join([str(p)] + [str(r) for r in rows]) for p, rows in corners)
        out.append(f"   {i:<4} {flags:#x} {len(corners)}  {refs}")
    if bones:
        out += ["", f"bones {len(bones)}", "#  id  name  parent  x  y  z  qx  qy  qz  qw"]
        for i, (bone, parent, t, q) in enumerate(bones):
            assert parent is None or parent < i, f"bone {bone}: its parent must come first"
            p = "-" if parent is None else str(parent)
            out.append(f"   {i:<3} {bone} {p}  {' '.join(number(v) for v in t)}  "
                       f"{' '.join(number(v) for v in q)}")
    if animations:
        out += ["", f"animations {len(animations)}", "#  id  name  fps  frames  loop|once"]
        for i, (anim, fps, loop, frames) in enumerate(animations):
            out.append(f"   {i:<3} {anim} {number(fps)} {len(frames)} {'loop' if loop else 'once'}")
        for anim, fps, loop, frames in animations:
            rows = len(frames) * len(bones)
            out += ["", f"poses {anim} {rows}", "#  id  frame  bone  x  y  z  qx  qy  qz  qw"]
            k = 0
            for f, poses in enumerate(frames):
                assert len(poses) == len(bones), f"{anim} frame {f}: a pose per bone"
                for b, (t, q) in enumerate(poses):
                    out.append(f"   {k:<5} {f} {b}  {' '.join(number(v) for v in t)}  "
                               f"{' '.join(number(v) for v in q)}")
                    k += 1
    return "\n".join(out) + "\n"


def quat_axis(axis, angle):
    """A rotation of `angle` radians about unit `axis`, as (qx, qy, qz, qw)."""
    import math
    s = math.sin(angle / 2)
    return (axis[0] * s, axis[1] * s, axis[2] * s, math.cos(angle / 2))
