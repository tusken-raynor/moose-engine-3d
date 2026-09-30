"""Tests of the Blender exporter's plain functions, without Blender.

Usage (from repo root): python3 tools/blender/test_moose_export.py [OUT.mmdl]
With OUT, also writes the test model there, for the engine's loader to check.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import moose_export as mx  # noqa: E402

SQUARE = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)]


def test_flat_and_convex():
    assert mx.flat_and_convex(SQUARE)
    assert not mx.flat_and_convex(SQUARE[::-1][:2] + [(0.5, 0.5, 0.4)] + SQUARE[:1]), "bent"
    # An L: a reflex corner.
    ell = [(0, 0, 0), (2, 0, 0), (2, 1, 0), (1, 1, 0), (1, 2, 0), (0, 2, 0)]
    assert not mx.flat_and_convex(ell)
    assert not mx.flat_and_convex([(0, 0, 0), (1, 0, 0), (2, 0, 0)]), "no area"
    # Collinear corners are fine, as in the engine.
    assert mx.flat_and_convex([(0, 0, 0), (0.5, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)])


def test_face_outlines():
    # Flat, convex, one bone: kept whole.
    assert mx.face_outlines([0, 1, 2, 3], SQUARE, [0, 0, 0, 0], [(0, 1, 2), (0, 2, 3)]) == [[0, 1, 2, 3]]
    # Across two bones: its triangles.
    assert mx.face_outlines([0, 1, 2, 3], SQUARE, [0, 1, 1, 0], [(0, 1, 2), (0, 2, 3)]) == [[0, 1, 2], [0, 2, 3]]
    # Triangles without area are dropped.
    line = [(0, 0, 0), (1, 0, 0), (2, 0, 0), (1, 1, 0)]
    assert mx.face_outlines([0, 1, 2, 3], line, [0, 1, 0, 0], [(0, 1, 2), (0, 2, 3)]) == [[0, 2, 3]]


def test_bones_and_actions():
    assert mx.dominant_bone([(2, 0.3), (5, 0.7)]) == 5
    assert mx.dominant_bone([], fallback=3) == 3
    assert mx.dominant_bone([(1, 0.0)], fallback=3) == 3
    # Child listed before its parent: moved after it.
    assert mx.parents_first(["hand", "arm", "root"], [1, 2, None]) == [2, 1, 0]
    assert mx.token("Walk Cycle.001") == "Walk_Cycle.001"
    assert mx.loops("walk") and not mx.loops("die_once")
    assert not mx.loops("wave", manual_range=True) and mx.loops("wave", True, cyclic=True)
    rest = ((0, 0, 0), (0, 0, 0, 1))
    turned = ((0, 0, 0), (0, 0, 0.7071, 0.7071))
    flipped = ((0, 0, 0), (0, 0, 0, -1))  # the same turn as rest
    assert mx.drop_repeated_end([[rest], [turned], [flipped]]) == [[rest], [turned]]
    assert len(mx.drop_repeated_end([[rest], [turned]])) == 2


def two_bone_bar():
    """A bar of two boxes on two bones, with a face across them, an L-shaped face on its
    end, and a proxy: as gather() would hand them over."""
    model = mx.Model("bar", uv=True)
    model.bones = [("root", None, (0, 0, 0), (0, 0, 0, 1)), ("tip", 0, (0, 1, 0), (0, 0, 0, 1))]
    # Corners: x, y, z each 0 or 1 for the lower box (bone 0), y up to 2 for the upper
    # (bone 1). Points 0-7 the lower box, 8-11 the upper box's top.
    points = [(x, y, z) for y in (0, 1) for z in (0, 1) for x in (0, 1)] + \
             [(x, 2, z) for z in (0, 1) for x in (0, 1)]
    bones = [0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1]

    def face(corners):
        n = len(corners)
        colors = [(0.5, 0.25, 1.0)] * n
        uvs = [(k / n, 1.0 - k / n) for k in range(n)]
        triangles = [(0, k, k + 1) for k in range(1, n - 1)]
        return (corners, colors, uvs, triangles)

    faces = [
        face([0, 1, 3, 2]),              # bottom (y 0), facing down
        face([0, 2, 10, 8]),             # x 0 side, across both bones
        face([1, 9, 11, 3]),             # x 1 side
        face([0, 8, 9, 1]),              # z 0 side
        face([2, 3, 11, 10]),            # z 1 side
        face([8, 10, 11, 9]),            # top (y 2), facing up
    ]
    model.add_part(points, bones, faces)
    # A proxy box for the root bone.
    proxy = [(x * 0.9, y * 0.9, z * 0.9) for y in (0, 1) for z in (0, 1) for x in (0, 1)]
    model.add_part(proxy, [0] * 8, [face([0, 1, 3, 2]), face([4, 6, 7, 5]), face([0, 4, 5, 1]),
                                     face([2, 3, 7, 6]), face([0, 2, 6, 4]), face([1, 5, 7, 3])], proxy=True)
    turn = (0, 0, 0.3827, 0.9239)
    frames = [[((0, 0, 0), (0, 0, 0, 1)), ((0, 1, 0), (0, 0, 0, 1))],
              [((0, 0, 0), (0, 0, 0, 1)), ((0, 1, 0), turn)],
              [((0, 0, 0), (0, 0, 0, 1)), ((0, 1, 0), (0, 0, 0, 1))]]
    model.animations = [("bend", 24.0, True, mx.drop_repeated_end(frames))]
    return model


def test_model():
    model = two_bone_bar()
    # The side faces across the bones are split in two; the rest are whole.
    drawn = [refs for flags, refs in model.polygons if not flags & mx.PROXY]
    assert [len(r) for r in drawn] == [4, 3, 3, 3, 3, 3, 3, 3, 3, 4], [len(r) for r in drawn]
    assert len(model.colors.rows) == 1, "one color, written once"
    assert model.animations[0][3][-1] != model.animations[0][3][0] and len(model.animations[0][3]) == 2
    text = model.text()
    assert text.startswith("MOOSEMODEL 1") and "poses bend 4" in text and "0x100" in text
    return text


if __name__ == "__main__":
    test_flat_and_convex()
    test_face_outlines()
    test_bones_and_actions()
    text = test_model()
    if len(sys.argv) > 1:
        with open(sys.argv[1], "w") as f:
            f.write(text)
    print("ok")
