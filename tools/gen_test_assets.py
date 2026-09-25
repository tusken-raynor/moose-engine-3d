"""Generate assets/models/crate.obj and assets/levels/two_rooms.mmp.

Usage (from repo root): python3 tools/gen_test_assets.py .

Polygons are listed in perimeter order, then auto-oriented so the Newell
normal points toward a reference point (level: into the sector; crate: out
of the cube). CCW-front convention.
"""
import os, sys

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
        "# Moose v2 test prop: 1 m wooden crate.",
        "# Origin at bottom-center (y = 0 rests on the floor). Units: meters, +Y up.",
        "# Vertex colors use the 'v x y z r g b' extension (0-1 floats).",
        "# 4 vertices per face so each face carries its own tint. Faces are",
        "# counter-clockwise viewed from outside (outward facing).",
        "",
        "o crate",
    ]
    vlines, flines = [], []
    idx = 1
    for name, pts, color in faces:
        pts = orient_away(pts, center)
        ids = []
        for p in pts:
            c = color(p)
            vlines.append("v " + " ".join(fmt(x) for x in p) + "  " + " ".join(f"{x:.3f}" for x in c))
            ids.append(str(idx))
            idx += 1
        flines.append(f"f {' '.join(ids)}  # {name}")
    return "\n".join(lines + vlines + [""] + flines) + "\n"


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


def level_text(title, about, shiny=(), darker=()):
    """The level as .mmp text. Surfaces listed in `shiny` get the reflective flag (0x1).
    Surfaces listed in `darker` get their vertex colors at half brightness (as new color
    rows, so other surfaces sharing a color keep it)."""
    table = list(colors)
    index = dict(cindex)

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
    o += ["", f"attributes {len(ATTRIBUTES)}", "#  id  name    format  count"]
    for i, (n, f, c) in enumerate(ATTRIBUTES):
        o.append(f"   {i:<3} {n:<7} {f:<7} {c}")
    rows = [
        [(v, darker_row(c) if i in darker and c is not None else c) for v, c in s["verts"]]
        for i, s in enumerate(surfaces)
    ]
    o += ["", f"values color {len(table)}", "#  id   values"]
    for i, c in enumerate(table):
        o.append(f"   {i:<4} " + " ".join(f"{x:>4}" for x in c))
    o += ["", f"sectors {len(sectors)}", "#  id  name      first_surface  surface_count"]
    for i, (n, f, c) in enumerate(sectors):
        o.append(f"   {i:<3} {n:<9} {f:<14} {c}")
    o += ["", f"surfaces {len(surfaces)}",
          "#  id  sector  adjoin  flags  nverts  vert[:attr ...] ...   (flags: 0x1 = reflective; attr = row in each values table, in declaration order; portals list verts only)"]
    last_sector = None
    for i, s in enumerate(surfaces):
        if s["sector"] != last_sector:
            o.append(f"   # --- sector {s['sector']}: {sectors[s['sector']][0]}")
            last_sector = s["sector"]
        vs = " ".join(f"{v}:{c}" if c is not None else f"{v}" for v, c in rows[i])
        flags = s['flags'] | (0x1 if i in shiny else 0)
        comment = s['comment'] + (" (reflective)" if i in shiny else "")
        o.append(f"   {i:<3} {s['sector']:<7} {s['adjoin']:<7} 0x{flags:<4x} {len(s['verts']):<7} {vs:<34} # {comment}")
    o += ["", f"adjoins {len(adjoins)}",
          "#  id  surface  mirror  flags    (0x1 = render through, 0x2 = passable)"]
    for i, (s, m, f) in enumerate(adjoins):
        o.append(f"   {i:<3} {s:<8} {m:<7} 0x{f:x}")
    o += ["", f"entities {len(entities)}",
          "#  id  kind   sector  model      x       y       z       pitch  yaw   roll  scale  name"]
    for i, (k, sec, m, p, r, sc, n) in enumerate(entities):
        o.append(f"   {i:<3} {k:<6} {sec:<7} {m:<10} " + " ".join(f"{fmt(x):<7}" for x in p)
                 + f" {r[0]:<6} {r[1]:<5} {r[2]:<5} {fmt(sc):<6} {n}")
    return "\n".join(o) + "\n"


os.makedirs(os.path.join(ROOT, "assets/models"), exist_ok=True)
os.makedirs(os.path.join(ROOT, "assets/levels"), exist_ok=True)
with open(os.path.join(ROOT, "assets/models/crate.obj"), "w") as f:
    f.write(crate_obj())
with open(os.path.join(ROOT, "assets/levels/two_rooms.mmp"), "w") as f:
    f.write(level_text("Two Rooms", "two box rooms joined by a hallway through two portals."))
# Same level with shiny (reflective) floors throughout (both rooms and the hallway, so
# reflections continue across adjacent floors), and room_a's ceiling shiny too. The two room
# floors and room_a's ceiling are also darker, so their reflections stand out.
room_floors = {sectors[room_a][1], sectors[room_b][1]}
shiny_floors = room_floors | {sectors[hall][1]}
room_a_ceiling = sectors[room_a][1] + 1
with open(os.path.join(ROOT, "assets/levels/shiny_rooms.mmp"), "w") as f:
    f.write(level_text("Shiny Rooms",
                       "two_rooms.mmp with reflective floors throughout and a reflective ceiling "
                       "in room_a, darker in the rooms.",
                       shiny=shiny_floors | {room_a_ceiling},
                       darker=room_floors | {room_a_ceiling}))
# The shiny floors with room_a's ceiling reflective too: two facing mirrors, for
# reflections of reflections.
with open(os.path.join(ROOT, "assets/levels/mirror_rooms.mmp"), "w") as f:
    f.write(level_text("Mirror Rooms",
                       "shiny floors throughout, darker in the rooms, and room_a's ceiling "
                       "reflective too.",
                       shiny=shiny_floors | {room_a_ceiling}, darker=room_floors))
print(f"verts={len(verts)} color_values={len(colors)} surfaces={len(surfaces)} adjoins={len(adjoins)}")
