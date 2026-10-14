"""Generate the shadow stress level: assets/levels/shadow_stress.mmp, for timing shadow
carving. Two box rooms side by side, joined by a doorway in the wall they share, every
surface a grid of small squares (so there are many polygons to carve), with many
shadow-casting lights (static ones and moving ones) and many crates (static ones and
ones that may move, whose shadows are carved every frame).

Usage (from repo root): python3 tools/gen_shadow_stress_level.py . [CELL [LIGHTS [CRATES]]]

CELL is the squares' size in meters (default 1; it must divide 1), LIGHTS the lights per
room (default 10, at most 15: every light takes a shadow slot), and CRATES the crates per
room (default 28).

Coordinates: x east, z south (yaw 0 faces -Z), y up, meters.
"""
import math, os, random, sys

ROOT = sys.argv[1] if len(sys.argv) > 1 else "."
CELL = float(sys.argv[2]) if len(sys.argv) > 2 else 1.0
LIGHTS = int(sys.argv[3]) if len(sys.argv) > 3 else 10
CRATES = int(sys.argv[4]) if len(sys.argv) > 4 else 28

SIZE = 16.0    # each room, along x and z
HEIGHT = 5.0
DOOR = (5.0, 11.0, 4.0)  # the doorway in the shared wall: z from, z to, height
ROOMS = [("west", 0.0), ("east", SIZE)]  # name, x of its west wall

random.seed(7)

vertices = {}  # rounded position -> id


def vertex(x, y, z):
    key = (round(x, 4), round(y, 4), round(z, 4))
    return vertices.setdefault(key, len(vertices))


COLORS = [
    (120, 112, 100),  # floor, dark squares
    (150, 142, 128),  # floor, light squares
    (176, 170, 160),  # ceiling
    (158, 120, 96),   # walls, low
    (196, 168, 140),  # walls, high
]


def steps(length):
    n = round(length / CELL)
    return [i * CELL for i in range(n + 1)]


surfaces = []  # (sector, portal to sector or None, [(vertex, color)], comment)
sector_ranges = []
for s, (name, x0) in enumerate(ROOMS):
    first = len(surfaces)
    xs, ys, zs = [x0 + x for x in steps(SIZE)], steps(HEIGHT), steps(SIZE)
    # Floor (facing up) and ceiling (facing down), counter-clockwise seen from inside.
    for i in range(len(xs) - 1):
        for k in range(len(zs) - 1):
            a, b, c, d = (xs[i], zs[k]), (xs[i + 1], zs[k]), (xs[i + 1], zs[k + 1]), (xs[i], zs[k + 1])
            color = (i + k) % 2
            surfaces.append((s, None, [(vertex(p[0], 0.0, p[1]), color) for p in (d, c, b, a)], "floor"))
            surfaces.append((s, None, [(vertex(p[0], HEIGHT, p[1]), 2) for p in (a, b, c, d)], "ceiling"))
    # Walls: each a grid of squares from (u, y), facing into the room.
    def wall(at, flip, comment, portal=None):
        us = xs if comment in ("north wall", "south wall") else zs
        for i in range(len(us) - 1):
            for j in range(len(ys) - 1):
                u0, u1, y0, y1 = us[i], us[i + 1], ys[j], ys[j + 1]
                corners = [(u0, y0), (u1, y0), (u1, y1), (u0, y1)]
                if flip:
                    corners.reverse()
                to = None
                if portal is not None and DOOR[0] <= u0 and u1 <= DOOR[1] and y1 <= DOOR[2]:
                    to = portal
                surfaces.append((
                    s, to,
                    [(vertex(*at(u, y)), 3 if y < HEIGHT / 2 else 4) for u, y in corners],
                    comment,
                ))
    wall(lambda u, y: (u, y, 0.0), False, "north wall")
    wall(lambda u, y: (u, y, SIZE), True, "south wall")
    wall(lambda u, y: (x0, y, u), True, "west wall", s - 1 if s > 0 else None)
    wall(lambda u, y: (x0 + SIZE, y, u), False, "east wall", s + 1 if s + 1 < len(ROOMS) else None)
    sector_ranges.append((name, first, len(surfaces) - first))

# Portals: each doorway square pairs with the one on the other side (the same corners,
# the other way around).
by_corners = {}
for i, (s, to, corners, _) in enumerate(surfaces):
    if to is not None:
        by_corners[(s, frozenset(v for v, _ in corners))] = i
adjoins = []  # (surface, mirror adjoin)
adjoin_of = {}
for i, (s, to, corners, _) in enumerate(surfaces):
    if to is not None:
        adjoin_of[i] = len(adjoins)
        adjoins.append(i)
mirrors = [adjoin_of[by_corners[(surfaces[i][1], frozenset(v for v, _ in surfaces[i][2]))]] for i in adjoins]

# Crates: on a jittered lattice, some stacked, half of them static.
entities = [("spawn", 0, "-", (2.0, 0.0, 14.0), 315, "player_start", "")]
for s, (name, x0) in enumerate(ROOMS):
    side = math.ceil(math.sqrt(CRATES))
    pitch = (SIZE - 4.0) / side
    spots = [(i, k) for i in range(side) for k in range(side)]
    random.shuffle(spots)
    for n, (i, k) in enumerate(spots[:CRATES]):
        x = x0 + 2.0 + (i + 0.5) * pitch + random.uniform(-0.3, 0.3)
        z = 2.0 + (k + 0.5) * pitch + random.uniform(-0.3, 0.3)
        yaw = random.randrange(0, 90, 5)
        static = " static" if n % 2 == 0 else ""
        entities.append(("prop", s, "crate.obj", (x, 0.0, z), yaw, f"crate_{name}_{n}", static))
        if n % 4 == 0:
            entities.append(("prop", s, "crate.obj", (x, 1.0, z), yaw + 20, f"crate_{name}_{n}_top", static))

# Lights: spread over each room at two heights, every third one moving.
lights = []
for s, (name, x0) in enumerate(ROOMS):
    side = math.ceil(math.sqrt(LIGHTS))
    pitch = (SIZE - 3.0) / side
    spots = [(i, k) for i in range(side) for k in range(side)]
    random.shuffle(spots)
    for n, (i, k) in enumerate(spots[:LIGHTS]):
        x = x0 + 1.5 + (i + 0.5) * pitch
        z = 1.5 + (k + 0.5) * pitch
        y = 4.2 if n % 2 == 0 else 2.4
        hue = n / LIGHTS * math.tau
        color = tuple(0.55 + 0.35 * math.cos(hue + shift) for shift in (0.0, 2.1, 4.2))
        options = "radius=0.05"
        if n % 3 == 2:
            options += " oscillate=0.60:0.00:0.60:%.1f" % (4.0 + n)
        lights.append((s, (x, y, z), color, 9.0, options))

out = []
w = out.append
w("# Moose v2 test level: two rooms of small squares, many lights and many crates, for timing")
w("# shadow carving. Generated by tools/gen_shadow_stress_level.py; do not edit by hand.")
w("# Format reference: 'Level Format Spec.md' in the repo root.")
w("# Right-handed, +Y up, meters. Yaw 0 faces -Z.")
w("")
w("MOOSEMAP 1")
w('name "Shadow Stress"')
w("")
w(f"vertices {len(vertices)}")
w("#  id       x        y        z")
for (x, y, z), i in sorted(vertices.items(), key=lambda kv: kv[1]):
    w(f"   {i:<6} {x:8.2f} {y:8.2f} {z:8.2f}")
w("")
w("attributes 1")
w("#  id  name    format  count")
w("   0   color   u8      3")
w("")
w(f"values color {len(COLORS)}")
w("#  id   values")
for i, c in enumerate(COLORS):
    w(f"   {i:<4} {c[0]:4d} {c[1]:4d} {c[2]:4d}")
w("")
w(f"sectors {len(sector_ranges)}")
w("#  id  name      first_surface  surface_count")
for i, (name, first, count) in enumerate(sector_ranges):
    w(f"   {i:<3} {name:<9} {first:<14} {count}")
w("")
w(f"surfaces {len(surfaces)}")
w("#  id  sector  adjoin  flags  nverts  vert[:attr ...] ...")
for i, (s, to, corners, comment) in enumerate(surfaces):
    if to is None:
        verts = " ".join(f"{v}:{c}" for v, c in corners)
        w(f"   {i:<5} {s:<7} -1      0x0    {len(corners)}       {verts}  # {comment}")
    else:
        verts = " ".join(str(v) for v, _ in corners)
        w(f"   {i:<5} {s:<7} {adjoin_of[i]:<7} 0x0    {len(corners)}       {verts}  # doorway")
w("")
w(f"adjoins {len(adjoins)}")
w("#  id  surface  mirror  flags    (0x1 = render through, 0x2 = passable)")
for i, surface in enumerate(adjoins):
    w(f"   {i:<3} {surface:<8} {mirrors[i]:<7} 0x3")
w("")
w(f"entities {len(entities)}")
w("#  id  kind   sector  model      x       y       z       pitch  yaw   roll  scale  name  [options]")
for i, (kind, s, model, (x, y, z), yaw, name, options) in enumerate(entities):
    w(f"   {i:<3} {kind:<6} {s:<7} {model:<10} {x:<7.2f} {y:<7.2f} {z:<7.2f} 0      {yaw:<5} 0     1.00   {name}{options}")
w("")
w("ambient 0.04 0.04 0.05")
w("")
w(f"lights {len(lights)}")
w("#  id  sector  x       y       z       r      g      b      range  [options]")
for i, (s, (x, y, z), (r, g, b), reach, options) in enumerate(lights):
    w(f"   {i:<3} {s:<7} {x:<7.2f} {y:<7.2f} {z:<7.2f} {r:<6.2f} {g:<6.2f} {b:<6.2f} {reach:<6.2f} {options}")
w("")

path = os.path.join(ROOT, "assets", "levels", "shadow_stress.mmp")
with open(path, "w") as f:
    f.write("\n".join(out))
print(f"wrote {path}: {len(surfaces)} surfaces, {len(entities) - 1} crates, {len(lights)} lights")
