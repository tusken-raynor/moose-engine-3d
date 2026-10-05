"""Names the materials a level's surfaces and entities were drawn with by the app's old
rules (before materials were data), as `material=NAME` options: for migrating levels and
for the level generators, so their levels look as they always did.

Usage (from repo root): python3 tools/materials_rule.py assets/levels/*.mmp
    Rewrites each level in place. Surfaces and entities that already name a material keep it.

The old rules, by surface (normal facing into its sector):
- sky surfaces (flag 0x2): `sky`;
- in levels with texture coordinates: shiny (0x1) floors `metal_floor_shiny`, other shiny
  surfaces `metal_shiny`; walls (|normal.y| < 0.5) brick, panel and stone by sector, in
  turn; floors (normal.y > 0.9) `metal_floor`; the rest `vertex_color`;
- without: shiny surfaces `color_shiny`, the rest `vertex_color`.
Entities: crate.obj `crate`, ball.obj `mirror_ball`, terrains `terrain` (by their template
if they have one).
"""
import math, sys

WALLS = ["brick", "panel", "stone"]
MODELS = {"crate.obj": "crate", "ball.obj": "mirror_ball"}


def newell(pts):
    n = [0.0, 0.0, 0.0]
    for i, p in enumerate(pts):
        q = pts[(i + 1) % len(pts)]
        n[0] += (p[1] - q[1]) * (p[2] + q[2])
        n[1] += (p[2] - q[2]) * (p[0] + q[0])
        n[2] += (p[0] - q[0]) * (p[1] + q[1])
    length = math.sqrt(sum(c * c for c in n)) or 1.0
    return [c / length for c in n]


def strip(line):
    """A line's tokens, without its comment."""
    return line.split("#", 1)[0].split()


def surface_material(sector, flags, normal, has_uv):
    if flags & 0x2:
        return "sky"
    shiny = flags & 0x1
    if not has_uv:
        return "color_shiny" if shiny else "vertex_color"
    if shiny:
        return "metal_floor_shiny" if normal[1] > 0.9 else "metal_shiny"
    if abs(normal[1]) < 0.5:
        return WALLS[sector % len(WALLS)]
    if normal[1] > 0.9:
        return "metal_floor"
    return "vertex_color"


def assign(text):
    """The level `text` with a material named on every drawn surface and on crates, mirror
    balls and terrains."""
    lines = text.split("\n")
    vertices, has_uv, templates = [], False, {}
    section, left = None, 0
    out = []
    for line in lines:
        tokens = strip(line)
        if section is None and tokens:
            head = tokens[0]
            if head in ("vertices", "attributes", "sectors", "surfaces", "adjoins", "templates", "entities", "lights", "directional") \
                    or (head == "values" and len(tokens) == 3):
                section, left = head, int(tokens[-1])
                out.append(line)
                if left == 0:
                    section = None
                continue
            out.append(line)
            continue
        if section is not None and tokens:
            left -= 1
            if section == "vertices":
                vertices.append(tuple(float(t) for t in tokens[1:4]))
            elif section == "attributes" and tokens[1] == "uv":
                has_uv = True
            elif section == "templates":
                templates[tokens[1]] = (tokens[2], tokens[3:])
                if tokens[2] in MODELS and not any(t.startswith("material=") for t in tokens[3:]):
                    line = line.rstrip() + f" material={MODELS[tokens[2]]}"
            elif section == "surfaces":
                sector, adjoin, flags, n = int(tokens[1]), int(tokens[2]), int(tokens[3], 16), int(tokens[4])
                refs = tokens[5:5 + n]
                options = tokens[5 + n:]
                if adjoin < 0 and not any(t.startswith("material=") for t in options):
                    pts = [vertices[int(r.split(":")[0])] for r in refs]
                    name = surface_material(sector, flags, newell(pts), has_uv)
                    code, comment = (line.split("#", 1) + [None])[:2]
                    line = code.rstrip() + f"  material={name}" + (f"  #{comment}" if comment is not None else "")
            elif section == "entities":
                kind, model, options = tokens[1], tokens[3], tokens[12:]
                template = templates.get(model)
                named = any(t.startswith("material=") for t in options) or (
                    template and any(t.startswith("material=") for t in template[1]))
                file = template[0] if template else model
                name = "terrain" if kind == "terrain" else MODELS.get(file)
                if name and not named and not template:
                    code, comment = (line.split("#", 1) + [None])[:2]
                    line = code.rstrip() + f" material={name}" + (f"  #{comment}" if comment is not None else "")
            out.append(line)
            if left == 0:
                section = None
            continue
        out.append(line)
    return "\n".join(out)


if __name__ == "__main__":
    for path in sys.argv[1:]:
        with open(path) as f:
            text = f.read()
        new = assign(text)
        if new != text:
            with open(path, "w") as f:
                f.write(new)
            print(f"named materials in {path}")
