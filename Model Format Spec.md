# Model Format Spec (.mmdl, version 1)

Sep 30, 2026 · Built: the format, its loader, skeletal posing, shadow proxies, a Python writer and a test actor

## Overview

A `.mmdl` ("moose model") file is a model: convex polygons (n-gons, not just triangles) over a shared position list, with named per-corner **attributes**. It can also have **shadow proxies**, a **skeleton** and **animations**.

It is the engine's own format because nothing standard fits: glTF and FBX store triangles only, and `.obj` has no skeleton. The layout follows the level format (`.mmp`): plain text, sections with counts, rows with ids, meant to be read and fixed by hand. Models come from Blender through an exporter (planned, see below) or from scripts.

`.obj` models still load too; they have no proxies, bones or animations.

## Conventions

The same as levels (see the Level Format Spec): right-handed, +Y up, meters, yaw 0 facing −Z. Model faces face outward, counter-clockwise seen from outside. A model's origin is where it stands: at its feet, on the floor.

## Grammar

Tokens are separated by whitespace, and `#` starts a comment. Each section header gives its row count, and each row starts with its id, counting from 0.

```
MOOSEMODEL 1
name "walker"

positions 112
#  id  x  y  z  bone
   0    -0.24 0.88 -0.12  0
   ...

attributes 1
#  id  name  format  count
   0   color u8 3

values color 84
#  id  values
   0    30 60 150
   ...

polygons 84
#  id  flags  nverts  position[:row ...] ...
   0    0x0 4  4:0 6:0 2:0 0:0
   ...
   42   0x100 4  60:0 62:0 58:0 56:0     # a shadow proxy

bones 7                                   # optional
#  id  name  parent  x  y  z  qx  qy  qz  qw
   0   hips -  0.00 0.90 0.00  0.00 0.00 0.00 1.00
   1   spine 0  0.00 0.10 0.00  0.00 0.00 0.00 1.00
   ...

animations 2                              # optional, needs bones
#  id  name  fps  frames  loop|once
   0   walk 24.00 24 loop
   1   idle 1.00 2 loop

poses walk 168                            # one per animation, in order
#  id  frame  bone  x  y  z  qx  qy  qz  qw
   0     0 0  0.00 0.925 0.00  0.00 0.00 0.00 1.00
   ...
```

### Sections

| Section | Rows |
| --- | --- |
| `positions` | `x y z bone`: a point in model space at the rest pose, and the bone that moves it (`-` in a model without bones). Polygons that use the same position share it, so it is moved once. |
| `attributes` | `name format count`, as in levels: storage format `u8`, `i8`, `i16` or `f32`, and values per corner. Shaders look attributes up by name (`color`, `uv`, `normal` …). |
| `values NAME` | One table per attribute, in the order declared: `count` numbers per row. |
| `polygons` | `flags nverts` then one corner per vertex: `position:row`, with a values row per attribute (`position:row:row` for two). Counter-clockwise from outside. |
| `bones` | `name parent x y z qx qy qz qw`: its rest pose relative to its parent (`-` for a root), as a translation and a rotation quaternion. Parents come before their children. |
| `animations` | `name fps frames loop\|once`. |
| `poses NAME` | Every bone's pose at every frame, relative to its parent: frame by frame, and within a frame bone by bone, so it has `frames × bones` rows. |

### Polygon flags

| Flag | Meaning |
| --- | --- |
| `0x100` | **Shadow proxy.** Not drawn; it casts the model's shadows instead of its drawn polygons (see below). |

## Skeletal animation

This is how Half-Life animates its models: every position belongs to exactly one bone, and a frame is a pose for each bone. Nothing is blended per position.

Per bone, each frame (`moose_assets::Skin::matrices`):

1. **Blend** the two keyframes around the time: translation by lerp, rotation by slerp.
2. **Chain** it to its parent: `world = parent's world × local`. Parents come first, so one pass does it.
3. **Skin** it: `skin = world × inverse rest`, where `inverse rest` is the inverse of the bone's rest pose in model space (worked out once at load). A bone at its rest pose gets the identity, so positions stay as written.

Per position, one matrix multiply: `p' = skin[bone] × p` (`Skin::pose_positions`).

A looping animation runs from its last frame back into its first; one that doesn't loop holds its last frame. Frames are `1 / fps` seconds apart.

**In the engine:** an entity with `anim=NAME` (see the Level Format Spec) gets its own copy of its model, which is posed every frame (`World::animate`): its positions move, each polygon's plane is refitted to its corners, and its box and the sectors it touches are worked out again. The copies are reused when the level is rebuilt.

**Polygons across bones** bend when the bones move apart, and the renderer draws them flat on their refitted plane. It works for small bends. The exporter will split such polygons into triangles, which stay flat.

## Shadow proxies

A drawn model is often too detailed to cast shadows with cheaply, and an animated one isn't convex, so it would cast face by face. Proxies are simple stand-ins.

- If a model has proxy polygons, **only they cast its shadows** (with the default `occluder=mesh`); its drawn polygons cast none. Without proxies, the drawn polygons cast them, as before.
- **One part per bone:** proxy polygons are grouped by the bone of their first corner, and each group is one occluder. A group that is convex (a box, say) casts one volume with soft edges. Any other shape casts face by face, with hard edges.
- A proxy moves with its bone, so it stays rigid and its convexity holds in every pose.
- For a model without bones, all its proxies are one group.

The test actor (`assets/models/walker.mmdl`) has a box per body part and a matching proxy box on the same bone.

## Rules

The loader refuses a model unless:

1. The header is `MOOSEMODEL 1`, and the sections come in the order above. Each section's rows are numbered from 0 and have the right number of fields.
2. Attribute names are unique, and counts are at least 1. Each attribute has its `values` table.
3. Every polygon has at least 3 corners, each naming a position and one existing values row per attribute. It is flat and convex at the rest pose. Its flags are only `0x100`.
4. In a model with bones, every position a polygon uses names an existing bone. A bone's parent comes before it.
5. Animations need bones. Each has a positive rate and at least one frame, and its `poses` table comes in order, with a row per frame per bone, in frame then bone order.
6. No rotation is zero (rotations are normalized as they load).
7. Nothing follows the last section.

## Tools

- **`tools/moose_model.py`** writes `.mmdl` text from plain Python data (positions and their bones, attributes, polygons, bones, animations). It needs nothing but Python, so it's shared by the test asset generator and the Blender exporter.
- **`tools/gen_test_assets.py`** builds the test actor with it: `walker.mmdl`, a figure of seven boxes on seven bones (hips, spine, head, legs, arms) with a proxy box each, and animations `walk` (24 frames at 24 fps, in place) and `idle`. It also writes `walker_rooms.mmp`, the sunny courtyard with the walker in it.

- **`tools/blender/moose_export.py`** is the Blender exporter. Run it from the repo, either as `blender model.blend --background --python tools/blender/moose_export.py -- out.mmdl`, or from Blender's Text Editor, which adds File > Export > Moose Model. What it writes:
  - **Meshes:** n-gons are kept. A face that isn't flat and convex, or that spans bones, becomes its triangles. Modifiers are applied, with the armature at rest.
  - **Proxies:** objects named `proxy_*` become shadow proxies.
  - **Attributes:**
    - `color` comes from the active color attribute, or else the material's viewport color.
    - `uv` has v flipped to grow downward.
  - **Skeleton:** every bone of the armature, parents first. Each vertex takes the bone of its strongest vertex group, or the bone its object is parented to.
  - **Animations:**
    - Every action on the armature's bones, sampled at every frame at the scene's rate.
    - An action loops unless its name ends in `_once`, or it has a manual frame range without Cyclic. A loop's repeated last frame is dropped.
  - **Axes:** a model facing Blender's front view (−Y) faces the engine's forward (−Z).

  **Untested in Blender** (Blender wasn't available when it was written). Its conversion logic is plain Python, and `tools/blender/test_moose_export.py` tests it. The test's model loads and poses correctly in the engine. The parts that call Blender's API (`gather`, `add_mesh`, `sample_actions`) have never run.

## Planned

- **Up to four bone weights per position**, for linear blend skinning (smoother joints) where it pays.
- **Materials** per polygon, and **levels of detail**, set in the engine's mesh editor.
- **Root motion** and blending between animations.
