#!/usr/bin/env python3
"""Writes assets/models/beach_ball.obj: a 0.6 m ball in six colored gores, so its rolling
shows. Vertex colors only (no texture): each gore has its own vertices, so the colors
meet in hard lines. Run from the repository's root."""
import math

RADIUS, SEGMENTS, RINGS = 0.3, 24, 12
COLORS = [(0.85, 0.12, 0.10), (0.93, 0.93, 0.90), (0.10, 0.28, 0.80),
          (0.95, 0.78, 0.10), (0.93, 0.93, 0.90), (0.10, 0.60, 0.25)]
PER_GORE = SEGMENTS // len(COLORS)

vertices, faces = [], []

def vertex(ring, segment, color):
    """A vertex at ring `ring` (0 the top pole, RINGS the bottom) and segment `segment`."""
    polar, around = math.pi * ring / RINGS, 2 * math.pi * segment / SEGMENTS
    n = (math.sin(polar) * math.sin(around), math.cos(polar), math.sin(polar) * math.cos(around))
    vertices.append((tuple(RADIUS * c for c in n), n, color))
    return len(vertices)  # OBJ indices count from 1

for gore, color in enumerate(COLORS):
    first = gore * PER_GORE
    # This gore's vertices: a pole each end, and a column of rings per segment edge.
    top, bottom = vertex(0, first, color), vertex(RINGS, first, color)
    columns = [[vertex(r, first + k, color) for r in range(1, RINGS)] for k in range(PER_GORE + 1)]
    for k in range(PER_GORE):
        a, b = columns[k], columns[k + 1]
        faces.append((top, a[0], b[0]))
        for r in range(RINGS - 2):
            faces.append((a[r], a[r + 1], b[r + 1], b[r]))
        faces.append((bottom, b[-1], a[-1]))

with open("assets/models/beach_ball.obj", "w") as out:
    out.write("# A 0.6 m beach ball (radius 0.3 m), origin at its center, in six colored gores:\n")
    out.write("# for physics to roll about. Made by tools/gen_ball.py.\n")
    out.write("# Smooth normals (each vertex's points straight out); faces are counter-clockwise\n")
    out.write("# viewed from outside.\n\no beach_ball\n")
    for (p, _, c) in vertices:
        out.write("v %.6f %.6f %.6f  %.3f %.3f %.3f\n" % (*p, *c))
    for (_, n, _) in vertices:
        out.write("vn %.6f %.6f %.6f\n" % n)
    for f in faces:
        out.write("f " + " ".join("%d//%d" % (i, i) for i in f) + "\n")
print(len(vertices), "vertices,", len(faces), "faces")
