"""Generate the canyon test level: assets/levels/canyon_rooms.mmp, its terrain
(assets/models/canyon_terrain.obj, with colors, texture coordinates and smooth normals),
the terrain's detail texture (assets/textures/terrain_detail.png) and the view blocker
inside its mesa (assets/models/mesa_blocker.obj). Also canyon_rooms_dense.mmp, the same
level with a terrain of twice the triangles (canyon_terrain_dense.obj), for timing.

Usage (from repo root): python3 tools/gen_canyon_level.py .

The outdoor space is blocked out as convex sectors (prisms with sky walls, a sky ceiling
and a hidden floor), joined by portals at the canyon's narrows and bend. A valley holds a
house, which splits it into four sectors around it, with a fifth above the house's roof.
One heightfield terrain covers it all; the engine carves it by the sectors at load.
A blocker inside the basin's mesa hides two crates behind it from the spawn point.

Coordinates: x east, z south (so north is -Z, where yaw 0 faces), y up, meters.
"""
import math, os, sys

ROOT = sys.argv[1] if len(sys.argv) > 1 else "."

FLOOR = -3.0     # outdoor sectors' hidden floors, under the terrain
CEIL = 30.0      # their sky ceilings
HOUSE_TOP = 5.0  # the house's roof
DOOR = (146.0, 150.0, 2.5)  # the house's door: x from, x to, height (on its south wall)


def sub(a, b): return (a[0]-b[0], a[1]-b[1], a[2]-b[2])
def dot(a, b): return a[0]*b[0] + a[1]*b[1] + a[2]*b[2]
def cross(a, b): return (a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0])


def newell(pts):
    n = [0.0, 0.0, 0.0]
    for i, p in enumerate(pts):
        q = pts[(i+1) % len(pts)]
        n[0] += (p[1]-q[1]) * (p[2]+q[2])
        n[1] += (p[2]-q[2]) * (p[0]+q[0])
        n[2] += (p[0]-q[0]) * (p[1]+q[1])
    return tuple(n)


def centroid(pts):
    return tuple(sum(p[i] for p in pts) / len(pts) for i in range(3))


def smooth(e0, e1, x):
    t = min(1.0, max(0.0, (x - e0) / (e1 - e0)))
    return t * t * (3 - 2 * t)


# ---------------------------------------------------------------- blockout
# Footprints (x, z), convex.
basin = [(12, 120), (84, 120), (94, 86), (66, 66), (18, 72)]
canyon_s = [(42, 69), (54, 67.5), (58, 34), (40, 34)]
bend = [(40, 34), (58, 34), (72, 30), (66, 14), (46, 16)]
canyon_e = [(66, 14), (72, 30), (112, 30), (115, 15)]
valley = [(120, 10), (115, 15), (112, 30), (120, 82), (164, 82), (166, 10)]


def clip2(poly, keep):
    """Clips a convex 2D polygon to where keep(p) >= 0 (keep linear)."""
    out = []
    for i, a in enumerate(poly):
        b = poly[(i + 1) % len(poly)]
        fa, fb = keep(a), keep(b)
        if fa >= 0:
            out.append(a)
        if (fa >= 0) != (fb >= 0):
            t = fa / (fa - fb)
            out.append((round(a[0] + (b[0]-a[0]) * t, 6), round(a[1] + (b[1]-a[1]) * t, 6)))
    return out


valley_w = clip2(valley, lambda p: 140 - p[0])
valley_e = clip2(valley, lambda p: p[0] - 156)
valley_n = [(140, 10), (156, 10), (156, 40), (140, 40)]
valley_s = [(140, 54), (156, 54), (156, 82), (140, 82)]
house = [(140, 40), (156, 40), (156, 54), (140, 54)]

# Sectors: name, footprint, floor y, ceiling y, floor flags, ceiling flags.
SECTORS = [
    ("basin", basin, FLOOR, CEIL),
    ("canyon_s", canyon_s, FLOOR, CEIL),
    ("bend", bend, FLOOR, CEIL),
    ("canyon_e", canyon_e, FLOOR, CEIL),
    ("valley_w", valley_w, FLOOR, CEIL),
    ("valley_n", valley_n, FLOOR, CEIL),
    ("valley_s", valley_s, FLOOR, CEIL),
    ("valley_e", valley_e, FLOOR, CEIL),
    ("roof", house, HOUSE_TOP, CEIL),
    ("house", house, 0.0, HOUSE_TOP),
]
SID = {name: i for i, (name, *_ ) in enumerate(SECTORS)}

# Portals: pairs of sectors and the vertical rectangle they share (2D ends, y from, y to).
# Between outdoor sectors, the whole edge they share: terrain crosses it, and a portal
# narrower than the terrain would show the far side's hills cut off at its edges.
PORTALS = [
    ("basin", "canyon_s", (54, 67.5), (42, 69), FLOOR, CEIL),
    ("canyon_s", "bend", (58, 34), (40, 34), FLOOR, CEIL),
    ("bend", "canyon_e", (72, 30), (66, 14), FLOOR, CEIL),
    ("canyon_e", "valley_w", (112, 30), (115, 15), FLOOR, CEIL),
    ("valley_w", "valley_n", (140, 10), (140, 40), FLOOR, CEIL),
    ("valley_w", "valley_s", (140, 54), (140, 82), FLOOR, CEIL),
    ("valley_n", "valley_e", (156, 10), (156, 40), FLOOR, CEIL),
    ("valley_s", "valley_e", (156, 54), (156, 82), FLOOR, CEIL),
    ("valley_w", "roof", (140, 40), (140, 54), HOUSE_TOP, CEIL),
    ("valley_e", "roof", (156, 40), (156, 54), HOUSE_TOP, CEIL),
    ("valley_n", "roof", (140, 40), (156, 40), HOUSE_TOP, CEIL),
    ("valley_s", "roof", (140, 54), (156, 54), HOUSE_TOP, CEIL),
    ("valley_s", "house", (DOOR[0], 54), (DOOR[1], 54), 0.0, DOOR[2]),
]

# The house's outside walls, as faces of the valley sectors around it: (sector, rectangles).
HOUSE_OUTSIDE = [
    ("valley_w", [((140, 40), (140, 54), FLOOR, HOUSE_TOP)]),
    ("valley_e", [((156, 40), (156, 54), FLOOR, HOUSE_TOP)]),
    ("valley_n", [((140, 40), (156, 40), FLOOR, HOUSE_TOP)]),
    ("valley_s", [
        ((140, 54), (DOOR[0], 54), FLOOR, HOUSE_TOP),
        ((DOOR[1], 54), (156, 54), FLOOR, HOUSE_TOP),
        ((DOOR[0], 54), (DOOR[1], 54), FLOOR, 0.0),
        ((DOOR[0], 54), (DOOR[1], 54), DOOR[2], HOUSE_TOP),
    ]),
]
# The house's inside walls (all but the door).
HOUSE_INSIDE = [
    ((140, 40), (140, 54), 0.0, HOUSE_TOP),
    ((156, 40), (156, 54), 0.0, HOUSE_TOP),
    ((140, 40), (156, 40), 0.0, HOUSE_TOP),
    ((140, 54), (DOOR[0], 54), 0.0, HOUSE_TOP),
    ((DOOR[1], 54), (156, 54), 0.0, HOUSE_TOP),
    ((DOOR[0], 54), (DOOR[1], 54), DOOR[2], HOUSE_TOP),
]

# One sky color: a gradient by height shows seams where the big sky polygons meet.
SKY = (0.55, 0.70, 0.90)
HOUSE_COLOR = (0.85, 0.80, 0.72)


def sky_color(p):
    return SKY


def on_segment(p, a, b):
    """Whether 2D point p lies on segment a-b (within a hair)."""
    ab, ap = (b[0]-a[0], b[1]-a[1]), (p[0]-a[0], p[1]-a[1])
    l2 = ab[0]**2 + ab[1]**2
    if abs(ab[0]*ap[1] - ab[1]*ap[0]) > 1e-6 * math.sqrt(l2):
        return False
    t = (ab[0]*ap[0] + ab[1]*ap[1]) / l2
    return -1e-9 <= t <= 1 + 1e-9


def rect(a, b, y0, y1):
    return [(a[0], y0, a[1]), (b[0], y0, b[1]), (b[0], y1, b[1]), (a[0], y1, a[1])]


# faces: (sector, points, kind, extra) where kind is 'sky', 'hidden', 'solid' or 'portal'.
faces = []
for name, foot, y0, y1 in SECTORS:
    s = SID[name]
    if name == "house":
        faces.append((s, [(x, y0, z) for x, z in foot], "solid", None))
        faces.append((s, [(x, y1, z) for x, z in foot], "solid", None))
        for a, b, lo, hi in HOUSE_INSIDE:
            faces.append((s, rect(a, b, lo, hi), "solid", None))
        continue
    faces.append((s, [(x, y0, z) for x, z in foot], "solid" if name == "roof" else "hidden", None))
    faces.append((s, [(x, y1, z) for x, z in foot], "sky", None))
    if name == "roof":
        continue  # its sides are all portals
    # Walls: each footprint edge, less its portals and house walls, is sky.
    taken = [(a, b, lo, hi) for (s1, s2, a, b, lo, hi) in PORTALS if name in (s1, s2)]
    taken += [r for (sec, rs) in HOUSE_OUTSIDE if sec == name for r in rs]
    for i, a in enumerate(foot):
        b = foot[(i + 1) % len(foot)]
        # Pieces of this edge that are taken, by where they start along it, as (t0, t1).
        along = lambda p: ((p[0]-a[0])*(b[0]-a[0]) + (p[1]-a[1])*(b[1]-a[1])) / ((b[0]-a[0])**2 + (b[1]-a[1])**2)
        spans = sorted({tuple(sorted((along(p), along(q)))) for p, q, lo, hi in taken
                        if on_segment(p, a, b) and on_segment(q, a, b) and p != q})
        # Full-height spans cover the whole height; partial ones are filled out below.
        at = 0.0
        pt = lambda t: (a[0] + (b[0]-a[0]) * t, a[1] + (b[1]-a[1]) * t)
        for t0, t1 in spans:
            if t0 > at + 1e-9:
                faces.append((s, rect(pt(at), pt(t0), y0, y1), "sky", None))
            at = max(at, t1)
        if at < 1 - 1e-9:
            faces.append((s, rect(pt(at), b, y0, y1), "sky", None))
for name, rects in HOUSE_OUTSIDE:
    for a, b, lo, hi in rects:
        faces.append((SID[name], rect(a, b, lo, hi), "solid", None))
for k, (s1, s2, a, b, lo, hi) in enumerate(PORTALS):
    faces.append((SID[s1], rect(a, b, lo, hi), "portal", k))
    faces.append((SID[s2], rect(a, b, lo, hi), "portal", k))

# Sector centers (inside, for winding), from their footprints and heights.
centers = []
for name, foot, y0, y1 in SECTORS:
    cx = sum(p[0] for p in foot) / len(foot)
    cz = sum(p[1] for p in foot) / len(foot)
    centers.append((cx, (y0 + y1) / 2, cz))

# Every corner, then each face gets every corner that lies on one of its edges (no
# T-junctions), and faces wind counter-clockwise from inside their sector.
corners = sorted({p for _, pts, _, _ in faces for p in pts})


def on_edge3(p, a, b):
    ab, ap = sub(b, a), sub(p, a)
    l2 = dot(ab, ab)
    c = cross(ab, ap)
    if dot(c, c) > 1e-12 * l2:
        return False
    t = dot(ab, ap) / l2
    return 1e-9 < t < 1 - 1e-9


def fill(pts):
    out = []
    for i, a in enumerate(pts):
        b = pts[(i + 1) % len(pts)]
        out.append(a)
        between = [p for p in corners if p != a and p != b and on_edge3(p, a, b)]
        between.sort(key=lambda p: dot(sub(p, a), sub(b, a)))
        out.extend(between)
    return out


def orient(pts, toward):
    if dot(newell(pts), sub(toward, centroid(pts))) < 0:
        return list(reversed(pts))
    return list(pts)


# ---------------------------------------------------------------- terrain
PATH = [(28, 112), (45, 95), (48, 70), (50, 34), (56, 26), (69, 22), (90, 22), (113.5, 22.5),
        (125, 30), (138, 47)]
RIM = 12.0
MESA = (55.8, 102.9)
MESA_TOP = 7.0


def seg_dist(p, a, b):
    ab = (b[0]-a[0], b[1]-a[1])
    t = max(0.0, min(1.0, ((p[0]-a[0])*ab[0] + (p[1]-a[1])*ab[1]) / (ab[0]**2 + ab[1]**2)))
    return math.hypot(p[0] - a[0] - ab[0]*t, p[1] - a[1] - ab[1]*t)


def inside_distance(p, poly):
    """How far p is inside a convex polygon (negative outside)."""
    best = float("inf")
    area = sum(poly[i][0]*poly[(i+1) % len(poly)][1] - poly[(i+1) % len(poly)][0]*poly[i][1]
               for i in range(len(poly)))
    s = 1 if area > 0 else -1
    for i, a in enumerate(poly):
        b = poly[(i + 1) % len(poly)]
        e = (b[0]-a[0], b[1]-a[1])
        c = e[0]*(p[1]-a[1]) - e[1]*(p[0]-a[0])
        best = min(best, s * c / math.hypot(*e))
    return best


def hash01(i, j):
    h = (i * 374761393 + j * 668265263) & 0xFFFFFFFF
    h = ((h ^ (h >> 13)) * 1274126177) & 0xFFFFFFFF
    return ((h ^ (h >> 16)) & 0xFFFF) / 65535.0


def value_noise(x, z, cell):
    """Smooth noise from 0 to 1: random values on a `cell`-meter lattice, blended between."""
    fx, fz = x / cell, z / cell
    i, j = math.floor(fx), math.floor(fz)
    u, v = smooth(0, 1, fx - i), smooth(0, 1, fz - j)
    a, b = hash01(i, j), hash01(i + 1, j)
    c, d = hash01(i, j + 1), hash01(i + 1, j + 1)
    return a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v


def height(x, z):
    # Under the house, and out to a cell's diagonal past its walls (so every triangle
    # reaching into it dips below its floor), a little below the floor: the carver leaves
    # none of it in there.
    if 140 - 4.6 < x < 156 + 4.6 and 40 - 4.6 < z < 54 + 4.6:
        return -0.15
    p = (x, z)
    d = min(seg_dist(p, PATH[i], PATH[i + 1]) for i in range(len(PATH) - 1))
    canyon = smooth(3.5, 9.0, d)
    bowl = 1 - smooth(3.0, 11.0, inside_distance(p, basin))
    vale = 1 - smooth(3.0, 10.0, inside_distance(p, valley))
    h = RIM * min(canyon, bowl, vale)
    # Rough highlands, flat floors.
    h += 1.6 * (value_noise(x, z, 3.2) - 0.5) * smooth(0.3, 0.8, h / RIM)
    mesa = MESA_TOP * (1 - smooth(6.0, 10.0, math.hypot(x - MESA[0], z - MESA[1])))
    # Level ground around the house.
    near_house = math.hypot(max(140 - x, 0, x - 156), max(40 - z, 0, z - 54))
    h *= smooth(3.0, 7.0, near_house)
    return max(h, mesa, 0.0)


class Heightfield:
    """The terrain: heights on a grid of `cell`-meter squares, each split in two
    triangles (the diagonal alternating), over the whole level."""

    X0, Z0, WIDTH, DEPTH = 6.4, 6.4, 163.2, 118.4

    def __init__(self, cell):
        self.cell = cell
        self.nx = math.ceil(self.WIDTH / cell - 1e-9)
        self.nz = math.ceil(self.DEPTH / cell - 1e-9)
        self.grid = [[height(self.X0 + i * cell, self.Z0 + j * cell) for i in range(self.nx + 1)]
                     for j in range(self.nz + 1)]

    def ground(self, x, z):
        """The mesh's height at (x, z): on its triangles, as the engine sees it."""
        g, cell = self.grid, self.cell
        fi, fj = (x - self.X0) / cell, (z - self.Z0) / cell
        i, j = int(fi), int(fj)
        u, v = fi - i, fj - j
        h00, h10, h01, h11 = g[j][i], g[j][i+1], g[j+1][i], g[j+1][i+1]
        if (i + j) % 2:   # diagonal 00-11
            return h00 + (h10 - h00) * u + (h11 - h10) * v if u >= v else h00 + (h11 - h01) * u + (h01 - h00) * v
        # diagonal 10-01
        return h00 + (h10 - h00) * u + (h01 - h00) * v if u + v <= 1 else h11 + (h01 - h11) * (1 - u) + (h10 - h11) * (1 - v)

    def obj(self):
        g, cell, nx, nz = self.grid, self.cell, self.nx, self.nz
        lines = [f"# Canyon test level terrain: a heightfield, {cell:.2f} m cells, {nx * nz * 2} triangles.",
                 "# Vertex colors by height and slope, texture coordinates for a detail texture",
                 f"# ({DETAIL_REPEAT:g} m across), and smooth normals. Generated by tools/gen_canyon_level.py.",
                 "# Placed whole at the origin; the engine carves it by the level's sectors."]
        normals = []
        for j in range(nz + 1):
            for i in range(nx + 1):
                h = g[j][i]
                dx = (g[j][min(i+1, nx)] - g[j][max(i-1, 0)]) / ((min(i+1, nx) - max(i-1, 0)) * cell)
                dz = (g[min(j+1, nz)][i] - g[max(j-1, 0)][i]) / ((min(j+1, nz) - max(j-1, 0)) * cell)
                slope = 1 - 1 / math.sqrt(1 + dx*dx + dz*dz)
                c = terrain_color(h, slope * 3)
                x, z = self.X0 + i * cell, self.Z0 + j * cell
                lines.append(f"v {x:.3f} {h:.3f} {z:.3f} {c[0]:.3f} {c[1]:.3f} {c[2]:.3f}")
                n = (-dx, 1.0, -dz)
                length = math.sqrt(sum(k * k for k in n))
                normals.append(tuple(k / length for k in n))
        for j in range(nz + 1):
            for i in range(nx + 1):
                x, z = self.X0 + i * cell, self.Z0 + j * cell
                lines.append(f"vt {x / DETAIL_REPEAT:.4f} {z / DETAIL_REPEAT:.4f}")
        lines += [f"vn {n[0]:.4f} {n[1]:.4f} {n[2]:.4f}" for n in normals]
        idx = lambda i, j: j * (nx + 1) + i + 1
        f = lambda *k: "f " + " ".join(f"{v}/{v}/{v}" for v in k)
        for j in range(nz):
            for i in range(nx):
                a, b, c, d = idx(i, j), idx(i+1, j), idx(i+1, j+1), idx(i, j+1)
                # Counter-clockwise seen from above (+y): with z south, that's a -> d -> c.
                if (i + j) % 2:
                    lines += [f(a, d, c), f(a, c, b)]
                else:
                    lines += [f(a, d, b), f(b, d, c)]
        return "\n".join(lines) + "\n"


def terrain_color(h, slope):
    grass, dirt, rock, dry = (0.36, 0.48, 0.24), (0.50, 0.40, 0.28), (0.52, 0.50, 0.46), (0.58, 0.55, 0.36)
    t = smooth(0.35, 0.8, slope)
    low = tuple(grass[i] + (dry[i] - grass[i]) * smooth(4, 11, h) for i in range(3))
    steep = tuple(dirt[i] + (rock[i] - dirt[i]) * smooth(5, 10, h) for i in range(3))
    return tuple(low[i] + (steep[i] - low[i]) * t for i in range(3))


# ---------------------------------------------------------------- detail texture
DETAIL_REPEAT = 6.0   # meters across one repeat of the detail texture
DETAIL_SIZE = 256


def detail_png():
    """Grey grain to multiply terrain colors by at twice its value: around 128 (no change),
    tileable. Clumps (soil, grass tufts) over fine speckle."""
    n = DETAIL_SIZE

    def periodic(x, y, cells, seed):
        fx, fy = x * cells / n, y * cells / n
        i, j = math.floor(fx), math.floor(fy)
        u, v = smooth(0, 1, fx - i), smooth(0, 1, fy - j)
        hv = lambda a, b: hash01((a % cells) + seed * 7919, (b % cells) + seed * 104729)
        a, b, c, d = hv(i, j), hv(i + 1, j), hv(i, j + 1), hv(i + 1, j + 1)
        return a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v

    rows = []
    for y in range(n):
        row = bytearray()
        for x in range(n):
            clumps = periodic(x, y, 8, 1) * 0.5 + periodic(x, y, 16, 2) * 0.3 + periodic(x, y, 32, 3) * 0.2
            speck = hash01(x + 3, y + 11)
            g = 128 + (clumps - 0.5) * 110 + (speck - 0.5) * 34
            g = max(0, min(255, int(round(g))))
            row += bytes((g, g, g, 255))
        rows.append(bytes(row))
    return png_rgba(n, n, rows)


def png_rgba(width, height, rows):
    import struct, zlib
    raw = b"".join(b"\x00" + r for r in rows)
    chunk = lambda kind, data: (struct.pack(">I", len(data)) + kind + data
                                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


TERRAIN = Heightfield(3.2)
ground = TERRAIN.ground


# The mesa's blocker: a quad across it, square to the way from the spawn point, well inside.
SPAWN = (27.0, 111.0)
to_mesa = (MESA[0] - SPAWN[0], MESA[1] - SPAWN[1])
dist = math.hypot(*to_mesa)
fwd = (to_mesa[0] / dist, to_mesa[1] / dist)
YAW = math.degrees(math.atan2(-fwd[0], -fwd[1]))  # forward is (-sin yaw, -cos yaw)
BLOCK_HALF, BLOCK_LOW, BLOCK_HIGH = 4.5, -1.0, 5.5


def blocker_obj():
    return "\n".join([
        "# The view blocker inside the canyon level's mesa: one quad, either side hides what's",
        "# wholly behind it. Generated by tools/gen_canyon_level.py.",
        f"v {-BLOCK_HALF} {BLOCK_LOW} 0", f"v {BLOCK_HALF} {BLOCK_LOW} 0",
        f"v {BLOCK_HALF} {BLOCK_HIGH} 0", f"v {-BLOCK_HALF} {BLOCK_HIGH} 0",
        "f 1 2 3 4", ""])


def check_blocker():
    # Its corners and edges, placed, must be under the terrain.
    right = (math.cos(math.radians(YAW)), -math.sin(math.radians(YAW)))  # local +x
    for k in range(21):
        s = -BLOCK_HALF + 2 * BLOCK_HALF * k / 20
        x, z = MESA[0] + right[0] * s, MESA[1] + right[1] * s
        g = ground(x, z)
        assert BLOCK_HIGH < g - 0.3, f"blocker pokes out at ({x:.1f}, {z:.1f}): ground {g:.2f}"


# ---------------------------------------------------------------- entities
def on_ground(x, z, pitch=0, yaw=0, name="", kind="prop", model="crate.obj", options="static"):
    s = next(i for i, (n, foot, *_ ) in enumerate(SECTORS[:8]) if inside_distance((x, z), foot) > 0.5)
    return (kind, s, model, x, ground(x, z), z, pitch, yaw, name, options)


def behind_mesa(t, side):
    return (SPAWN[0] + fwd[0] * t - fwd[1] * side, SPAWN[1] + fwd[1] * t + fwd[0] * side)


ENTITIES = [
    ("spawn", 0, "-", SPAWN[0], ground(*SPAWN), SPAWN[1], 0, round(YAW, 1), "player_start", ""),
    ("terrain", 0, "TERRAIN_MODEL", 0.0, 0.0, 0.0, 0, 0, "ground", ""),
    ("blocker", 0, "mesa_blocker.obj", MESA[0], 0.0, MESA[1], 0, round(YAW, 1), "mesa_blocker", ""),
    on_ground(*behind_mesa(46, -1.5), yaw=20, name="crate_hidden_1"),
    on_ground(*behind_mesa(49, 1.5), yaw=55, name="crate_hidden_2"),
    on_ground(30, 100, yaw=10, name="crate_basin"),
    on_ground(48.5, 52, yaw=30, name="crate_canyon"),
    on_ground(95, 21, yaw=5, name="crate_canyon_e"),
    on_ground(134, 62, yaw=40, name="crate_valley"),
]


# ---------------------------------------------------------------- write
def level(terrain_model, about):
    verts, vindex = [], {}

    def V(p):
        if p not in vindex:
            vindex[p] = len(verts)
            verts.append(p)
        return vindex[p]

    colors, cindex, uvs, uindex = [], {}, [], {}

    def C(c):
        key = tuple(int(round(x * 255)) for x in c)
        if key not in cindex:
            cindex[key] = len(colors)
            colors.append(key)
        return cindex[key]

    def U(uv):
        key = (round(uv[0], 4), round(uv[1], 4))
        if key not in uindex:
            uindex[key] = len(uvs)
            uvs.append(key)
        return uindex[key]

    def planar_uv(p, n):
        ax = max(range(3), key=lambda i: abs(n[i]))
        x, y, z = p
        u, v = (x, z) if ax == 1 else ((z, -y) if ax == 0 else (x, -y))
        return (u / 2.0, v / 2.0)

    rows = []  # (sector, adjoin key or None, flags, [(v, c, u) or v])
    for s in range(len(SECTORS)):
        for sec, pts, kind, extra in faces:
            if sec != s:
                continue
            pts = orient(fill(pts), centers[s])
            n = newell(pts)
            if kind == "portal":
                rows.append((s, extra, 0, [V(p) for p in pts]))
                continue
            flags = {"sky": 0x2, "hidden": 0x4, "solid": 0x0}[kind]
            color = sky_color if kind == "sky" else (lambda p: HOUSE_COLOR)
            rows.append((s, None, flags, [(V(p), C(color(p)), U(planar_uv(p, n))) for p in pts]))

    # Adjoins: two per portal, in surface order.
    adjoin_of, adjoins = {}, []
    for i, (s, key, flags, vs) in enumerate(rows):
        if key is not None:
            adjoin_of[i] = len(adjoins)
            adjoins.append([i, key])
    for a in adjoins:
        a.append(next(k for k, b in enumerate(adjoins) if b[1] == a[1] and b[0] != a[0]))

    out = ["# Moose test level: a canyon between a basin and a valley, with a house in the valley." + about,
           "# Generated by tools/gen_canyon_level.py. Format reference: 'Level Format Spec.md'.",
           "# Outdoor sectors: sky walls and ceilings, hidden floors under the terrain.",
           "", "MOOSEMAP 1", 'name "Canyon"', "", f"vertices {len(verts)}", "#  id       x        y        z"]
    out += [f"   {i:<6} {p[0]:8.3f} {p[1]:8.3f} {p[2]:8.3f}" for i, p in enumerate(verts)]
    out += ["", "attributes 2", "#  id  name    format  count", "   0   color   u8      3", "   1   uv      f32     2",
            "", f"values color {len(colors)}", "#  id   values"]
    out += [f"   {i:<4} {c[0]:4} {c[1]:4} {c[2]:4}" for i, c in enumerate(colors)]
    out += ["", f"values uv {len(uvs)}", "#  id   values"]
    out += [f"   {i:<4} {u[0]:.4f} {u[1]:.4f}" for i, u in enumerate(uvs)]
    out += ["", f"sectors {len(SECTORS)}", "#  id  name      first_surface  surface_count"]
    first = 0
    for s, (name, *_ ) in enumerate(SECTORS):
        count = sum(1 for r in rows if r[0] == s)
        out.append(f"   {s:<3} {name:<9} {first:<14} {count}")
        first += count
    out += ["", f"surfaces {len(rows)}", "#  id  sector  adjoin  flags  nverts  vert:color:uv ...   (flags: 0x2 sky, 0x4 hidden)"]
    for i, (s, key, flags, vs) in enumerate(rows):
        adj = adjoin_of.get(i, -1)
        refs = " ".join(str(v) if key is not None else f"{v[0]}:{v[1]}:{v[2]}" for v in vs)
        out.append(f"   {i:<4} {s:<7} {adj:<7} {flags:#x}    {len(vs):<7} {refs}")
    out += ["", f"adjoins {len(adjoins)}", "#  id  surface  mirror  flags"]
    out += [f"   {i:<4} {a[0]:<8} {a[2]:<7} 0x3" for i, a in enumerate(adjoins)]
    out += ["", f"entities {len(ENTITIES)}",
            "#  id  kind     sector  model               x        y        z        pitch  yaw     roll  scale  name  [options]"]
    for i, (kind, s, model, x, y, z, pitch, yaw, name, options) in enumerate(ENTITIES):
        model = terrain_model if model == "TERRAIN_MODEL" else model
        out.append(f"   {i:<3} {kind:<8} {s:<7} {model:<19} {x:<8.2f} {y:<8.2f} {z:<8.2f} {pitch:<6} {yaw:<7} 0     1.00   {name}  {options}".rstrip())
    # A dim sky-blue ambient and a strong sun: sunlit flat ground at about 1.67 / 1.60 / 1.49,
    # shade at a tenth of that.
    out += ["", "ambient 0.15 0.16 0.20", "",
            "directional 1",
            "#  id  dx     dy     dz     r      g      b      angle",
            "   0   0.30   -0.85  0.42   1.78   1.69   1.51   0.53", ""]
    return "\n".join(out)


check_blocker()
# The level, and the same with a terrain of twice the triangles (cells 1/sqrt(2) the size).
DENSE = Heightfield(3.2 / math.sqrt(2))
for terrain, model, level_file, about in [
    (TERRAIN, "canyon_terrain.obj", "canyon_rooms.mmp", ""),
    (DENSE, "canyon_terrain_dense.obj", "canyon_rooms_dense.mmp", " Terrain twice as dense."),
]:
    with open(os.path.join(ROOT, "assets/models", model), "w") as f:
        f.write(terrain.obj())
    with open(os.path.join(ROOT, "assets/levels", level_file), "w") as f:
        f.write(level(model, about))
    print(f"wrote {level_file} and {model} ({terrain.nx * terrain.nz * 2} triangles)")
with open(os.path.join(ROOT, "assets/models/mesa_blocker.obj"), "w") as f:
    f.write(blocker_obj())
with open(os.path.join(ROOT, "assets/textures/terrain_detail.png"), "wb") as f:
    f.write(detail_png())
print(f"wrote mesa_blocker.obj and terrain_detail.png; spawn yaw {YAW:.1f}")
