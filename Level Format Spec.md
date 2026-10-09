# Level Format Spec (.mmp, version 1)

## Overview

A `.mmp` ("moose map") file describes a level as convex **sectors** bounded by polygonal **surfaces**. Sectors connect through **adjoins**, which are portals. Surface vertices carry generic, named **attributes**. **Entities** are placed objects such as props, actors and spawn points. The structure follows JKL: one global vertex table, surfaces grouped in contiguous ranges per sector, and paired adjoins.

The format is plain text and meant to be edited by hand. A version number in the header allows new sections and columns, such as materials, to be added later.

Models referenced by entities are `.obj` files or `.mmdl` files (see the Model Format Spec), which add shadow proxies, a skeleton and animations.

## Conventions

| Topic | Convention |
| --- | --- |
| Handedness and axes | Right-handed, +Y up |
| Units | Meters |
| Forward | Yaw 0 faces −Z; right is +X |
| Front face | Counter-clockwise when viewed from the side the face points toward |
| Level surfaces | Face into their sector |
| Model faces | Face outward |
| Orientation | Pitch, yaw, roll in degrees. Yaw about Y, pitch about X, roll about Z, applied roll → pitch → yaw (R = Ry·Rx·Rz). Positive yaw turns counter-clockwise seen from above. |

**Sectors must be convex.** From inside a convex sector, the faces that survive backface culling never overlap on screen. Portal clipping keeps that true across sectors. The span buffer module depends on this to insert world spans with no overlap tests.

**Portals and sectors are stored already split.** A non-convex opening is authored as several convex portal surfaces, each with its own adjoin pair to the same target sector. A non-convex room is authored as several convex sectors joined by portals that are never drawn. The engine never splits geometry at load or run time; it rejects anything non-convex.

**No T-junctions.** If a vertex lies on another surface's edge within the same sector, that surface must include the vertex as an extra collinear point. Shared edges then have identical endpoints, and the top-left fill rule leaves no cracks. Collinear points are allowed, and the polygon remains convex.

## File structure

- Tokens are separated by whitespace.
- `#` starts a comment that runs to the end of the line.
- Quoted strings may contain spaces.
- The file begins with the header `MOOSEMAP 1`.
- Each remaining section begins with its keyword and a row count. Rows start with an `id` that must equal the row's position, counting from 0.
- Sections appear in this order: `name`, `vertices`, `attributes`, one `values` block per attribute (in declaration order), `sectors`, `surfaces`, `adjoins`, optionally `templates`, `entities`, then optionally `ambient`, optionally `lights` and optionally `directional`.
- Some rows take **options** after their fixed columns: words like `static`, or `key=value` pairs like `radius=0.05`. Options are optional and can come in any order.
- **Meta values** are options written `$KEY=VALUE`: numbers the level puts on itself (after its name on the `name` line), a sector, a surface, a template or an entity (over its template's), for materials to read (see the Material Format Spec's sources) and scripts to change. `KEY` is letters, digits and `_`; `VALUE` is one or more numbers separated by commas (`$wet=200`, `$team=204,51,51`), written as the material inputs that read them are typed (colors and 0-255 values as whole numbers). The engine gives them no meaning, but checks the other options strictly: a word that isn't an option is still an error, and only `$` marks a meta value.
- **Light exclusion lists:** `exclude_lights=NAME,NAME` on a sector, a solid surface, a template or a drawn entity (prop, actor, terrain) names the lights it isn't lit by: none of their light, bumps, highlights or shadows reach it, though it still casts their shadows. Lights are named with `name=` (lights and directional lights share the names), and `flashlight` is the player's flashlight. A sector's list keeps its lights off all its surfaces, along with each surface's own; entities have their own lists (a sector's doesn't reach them), and an entity's list replaces its template's (`exclude_lights=none` for none over a template's). Only the level's first 63 lights (its lights, then its directional lights) can be excluded.

```
MOOSEMAP 1
name "Two Rooms"

vertices 28
#  id   x      y      z
   0   -4.00   0.00   8.00

attributes 1
#  id  name   format  count
   0   color  u8      3

values color 18
#  id  values
   0   102  76  56

sectors 3
#  id  name     first_surface  surface_count
   0   room_a   0              9

surfaces 24
#  id  sector  adjoin  flags  nverts  vert[:attr ...] ...
   0   0       -1      0x0    6       0:0 1:0 2:0 3:0 4:0 5:0
   8   0        0      0x0    4       4 3 13 12

adjoins 4
#  id  surface  mirror  flags
   0   8        1       0x3

entities 7
#  id  kind   sector  model      x     y     z     pitch  yaw  roll  scale  name
   1   prop   0       crate.obj  -2.5  0.0   2.0   0      0    0     1.0    crate_a1

ambient 0.12 0.12 0.14

lights 5
#  id  sector  x      y     z     r     g     b     range
   0   0       -2.5   3.2   5.5   1.6   1.25  0.85  7.0
```

## Sections

**`vertices`**: the global position table. Adjacent sectors share vertices wherever they meet.

**`attributes`**: declares the per-vertex attributes (varyings) that every drawn surface vertex carries. The format assigns them no meaning. `color` is only a name that a shader asks for; the rasterizer sees three u8 values to interpolate.

| Column | Meaning |
| --- | --- |
| `name` | Identifier, unique within the file. Shaders match attributes by name. |
| `format` | Storage format: `u8`, `i8`, `i16`, `f16` or `f32` |
| `count` | Number of components, 1 or more |

The `format` column is the storage format only. The interpolation format (16.16, 8.8 or f32) belongs to the shader's layout; at draw time, validation checks that the storage format converts to it.

**`values <name> <count>`**: the value table for one declared attribute. Each row holds `count` values written exactly as stored: integers for `u8`, `i8` and `i16`, decimals for `f16` and `f32`. Any number of surface vertices can share a row.

**`sectors`**

| Column | Meaning |
| --- | --- |
| `name` | Identifier, for debugging and scripting |
| `first_surface`, `surface_count` | The sector's contiguous range in `surfaces`. Ranges appear in sector order and cover every surface. |

Then its meta values (`$KEY=VALUE`) and its light exclusion list (`exclude_lights=`), if it has them: a sector takes no other options. The loader computes each sector's bounds and center; the file does not store them.

**`surfaces`**

| Column | Meaning |
| --- | --- |
| `sector` | Owning sector; must match the sector range the surface falls in |
| `adjoin` | Index into `adjoins`, or −1 for a solid surface |
| `flags` | Hex, solid surfaces only; `0x0` otherwise. `0x1` = reflective: the surface reflects its sector like a mirror and is drawn over its reflection (typically translucent, with a Fresnel falloff). `0x2` = sky: directional lights (the sun) enter the sector through it. A surface can't be both. `0x4` = hidden: drawn not at all, not even as sky, for faces something always covers (an outdoor sector's floor under its terrain); it still bounds the sector. A hidden surface takes no other flag. |
| `nverts` | Vertex count, 3 or more |
| `vert[:attr ...]` | Vertex indices in counter-clockwise order as seen from inside the sector. On solid surfaces, each index is followed by one `:row` per declared attribute, in declaration order (for example `12:3:7` with two attributes). Attribute values belong to the surface vertex, so a point shared by two surfaces can carry different values on each. |

Solid surfaces reference every attribute on every vertex. Portal surfaces list vertex indices only.

After its vertices, a solid surface takes options (portal surfaces take none):

| Option | Meaning |
| --- | --- |
| `material=NAME` | The material it is drawn with, from `assets/materials/` (see the Material Format Spec). A surface with none is drawn with its vertex colors. |
| `filterN=SAMPLER` | How texture slot N of its material is read, over the material's own `filterN` (`nearest`, `bilinear`, `trilinear`, `dithered`, or a full sampler name). |

```
  12   1       -1      0x0    4       8:0 9:1 13:2 12:3  material=brick filter0=nearest
``` A portal surface is not drawn; the renderer draws the sector behind it, clipped to the portal polygon.

**`adjoins`**

| Column | Meaning |
| --- | --- |
| `surface` | The portal surface this adjoin belongs to |
| `mirror` | The adjoin on the other side |
| `flags` | `0x1` = render through, `0x2` = passable |

Adjoins always come in pairs. The two surfaces use the same vertex indices in reverse order, so each portal matches its partner exactly by construction.

**`templates`** (optional): named models with default options, which entities place by name instead of a model file.

```
templates 1
#  id  name    model        [options]
   0   walker  walker.mmdl  shadow=blurred anim=walk
```

| Column | Meaning |
| --- | --- |
| `name` | What entities write in their `model` column to place it |
| `model` | Model file name, resolved from `assets/models/` |

Its options are any of an entity's (below). An entity placed by a template gets the template's options, except those it sets itself, key by key: `shadow=hard` on the instance replaces the template's `shadow=`. A flag is turned off with `=off` (`static=off`), and a template's animation with `anim=none`.

**`entities`**

| Column | Meaning |
| --- | --- |
| `kind` | `spawn`, `prop`, `actor`, `terrain` or `blocker`. The kind belongs to the instance, not the model: the same mesh can be placed as a prop or an actor. `prop` and `actor` map to the span buffer module's `MeshKind`. `terrain` and `blocker` are described under Terrain below. |
| `sector` | The sector that contains the entity's origin. It is the starting point for portal traversal and culling. |
| `model` | A template's name (see `templates`), a model file name resolved from `assets/models/`, or `-` for none |
| `x y z` | Position of the model origin in world space |
| `pitch yaw roll` | Orientation in degrees (see Conventions) |
| `scale` | Uniform scale |
| `name` | Identifier, for debugging and scripting |

Options, after `name` (props and actors; terrains take only `material=` and `filterN=`; other kinds take none):

| Option | Meaning |
| --- | --- |
| `material=NAME` | The material the whole model is drawn with (see the Material Format Spec). Without one, its vertex colors. |
| `filterN=SAMPLER` | How texture slot N of its material is read, over the material's own (as for surfaces). |
| `static` | Props only. The prop never moves, so static lights' shadows from it and on it can be worked out once. `static=on` is the same, `static=off` turns off a template's. |
| `occluder=...` | What the entity casts shadows with. Without it, `mesh`. |
| `shadow=soft\|blurred\|hard` | How its shadows' edges are drawn, from a light with a size. `soft` (the default) carves their soft edges: exact, and cached for static props. `blurred` carves them hard and blurs them on screen, as wide as their soft edge would be: for figures casting shadows with simple proxies, whose blur hides how simple they are; worked out every frame. `hard` carves them from the light's center: the cheapest. |
| `anim=NAME` | It plays its model's animation `NAME` over and over (or holds its last frame, for one that doesn't loop), from when the level starts. Not with `static`. `anim=none` turns off a template's. |

Occluders are the artist's choice. Any shape works; a complex one only costs more to shadow with.

| Occluder | Meaning |
| --- | --- |
| `mesh` | Its own model (the default): its shadow proxies if it has any (see the Model Format Spec), or else its polygons |
| `none` | It casts no shadows |
| `lod:N` | Level of detail `N` of its model (its own model until models have levels of detail) |
| `model:FILE` | A separate proxy model from `assets/models/`, placed like the entity |
| `facing:SIDES:RADIUS[:X:Y:Z]` | A polygon of 3 to 64 sides that always faces the light: on the outline, seen from the light, of a ball of `RADIUS` around `X Y Z` (model space, default the origin). A round thing's shadow with no silhouette to find. |

**`ambient r g b`** (optional): light that reaches every surface, in linear RGB, where 1 shows a surface's full color. Without it, ambient light is 1 1 1, so a level with no lights looks as it always did.

**`lights`** (optional): static lights. A row with 8 fields is a point light, which shines every way. A row with 13 fields is a spot light, which adds a direction and a cone.

| Column | Meaning |
| --- | --- |
| `sector` | The sector that contains the light |
| `x y z` | Position in world space |
| `r g b` | Linear RGB at full strength (close to the light, facing it). 1 shows a surface's full color; more overbrightens. |
| `range` | Where the light ends, in meters. It fades smoothly to nothing there. |
| `dx dy dz` | Spot lights only: where it shines (any length) |
| `inner outer` | Spot lights only: the cone's half-angles in degrees. Full strength within `inner`, fading smoothly to nothing at `outer`. |

Options: `name=NAME` names it, for materials to read its color or brightness (`light:NAME`) and exclusion lists to name it (`exclude_lights=`); names are unique, and `flashlight` and `none` are taken. `radius=R` is the size of the light's source in meters, 0 or more (default 0). Shadows soften over the part of the source an occluder covers, and 0 casts hard shadows. `shadows=on|off` sets whether it casts shadows (default on). `oscillate=DX:DY:DZ:PERIOD` makes it a moving light: it swings smoothly (a sine) through its position, `DX DY DZ` either way, once every `PERIOD` seconds. Moving lights aren't baked; their shadows are worked out as they go. Both ends of the swing must lie inside the level, but the path between them isn't checked.

A light reaches its own sector, and passes into the next through any open portal (render-through adjoin) within its range and, for a spot light, its cone. It never passes through walls. See the Lighting Spec for how surfaces are lit.

**`directional`** (optional): lights from far away, like the sun, arriving along one direction everywhere. They enter the level only through sky surfaces (flag `0x2`), and from there through open portals like any light, so a courtyard's walls shadow its ground and sunlight falls through a doorway.

| Column | Meaning |
| --- | --- |
| `dx dy dz` | The way the light travels (any length): the sun's direction reversed |
| `r g b` | Linear RGB, as for lights |
| `angle` | The source's angular diameter in degrees, 0 to 45 (the sun is about 0.53): its shadows soften over the part of it an occluder covers. 0 casts hard shadows. |

Options: `shadows=on|off` (default on), and `name=NAME`, as a light's.

## Terrain

Open ground (hills, canyons, a valley) is too big and too open for a prop, and too uneven for sectors. It is authored in two layers:

1. **Blockout.** Convex sectors roughly following the land, as for any level: an outdoor sector has sky walls and a sky ceiling (flag `0x2`) and a hidden floor (`0x4`) below the ground. Neighboring outdoor sectors share their whole common edge as one portal, since terrain crosses it: a portal narrower than the ground through it would show the far side's hills cut off at its edges. A building in the open splits the open space into convex sectors around it, with one above its roof.
2. **Terrain.** A `terrain` entity places one model over all of it, usually at the origin: a heightfield or any mesh of convex polygons. Its origin needn't be in its sector.

At load, the terrain is **carved** by the sectors: each of its polygons is clipped to every sector it crosses, and what lies outside every sector is dropped. Each piece then belongs to one sector, and is drawn when that sector is seen, clipped to its portal window like the sector's own surfaces and sorted into the span buffer like a prop (pieces can overlap each other on screen). Pieces get their sector's lights, and static lights' shadows on them are baked like a static prop's. Where a cut crosses one of the model's edges, the point is worked out from the edge's own corners, and a portal's two sides cut with the same plane, so pieces meet with no gaps.

The terrain is the artist's to fit to the blockout:

- Wherever it crosses a sky wall, the cut shows, unless the wall stands just past a crest, so sight lines from inside clear the ridge and land on sky.
- Inside an indoor sector it would be drawn too: keep it below such a sector's floor (the canyon level lowers it under its house).
- The editor rebuilds the level on every change, so the terrain is carved again whenever the blockout moves.

A **`blocker`** entity places a model whose polygons are never drawn, but hide from the eye whatever lies wholly behind one of them (from either side): props, actors and terrain pieces. It is for a solid thing inside one sector, such as a hill or a mesa, that portals can't help with. A blocker must stay inside solid ground: wherever it shows, it would hide what should be seen. One quad across the inside of the hill is usually enough.

`tools/gen_canyon_level.py` generates `canyon_rooms.mmp`, the test level: a basin with a mesa (and a blocker hiding two crates behind it from the spawn point), a canyon with a bend, and a valley with a house. Its terrain (3,774 triangles) has vertex colors, texture coordinates and smooth normals: its material, `terrain` (the `terrain_detail` shader, `VertexColorDetail`), draws its colors times the detail texture `terrain_detail.png` at twice its value, lit from the interpolated normals (smooth shading from the sun and every other light). The `terrain_noise` material (`VertexColorNoise`) swaps the texture for five octaves of 3D value noise in world space (cells from 1 m down to 6 cm, each fading out where a pixel covers a cell), with no texture coordinates, no seams and no stretching on steep slopes. `canyon_rooms_dense.mmp` is the same level with a terrain of twice the triangles, for timing.

## Validation (enforced by the loader)

1. The header and version are recognized. Sections appear in order, section counts match their rows, ids are sequential, and every index reference is in range.
2. Attribute names are unique, and each declared attribute has exactly one `values` table. Every row has `count` values that are valid for the storage format (for example 0–255 for `u8`).
3. Every surface has at least 3 vertices and is planar and convex. Collinear vertices are allowed.
4. Every solid surface's normal points into its sector.
5. Every sector is convex: all of its vertices lie on or inside each of its surface planes.
6. Every sector is closed with consistent winding. Within a sector, each directed edge appears exactly once and its reverse appears exactly once, counting portal surfaces. This rule also catches T-junctions.
7. Adjoins are symmetric (`mirror.mirror == self`), each points back to its surface, the mirror surface lists the same vertices in reverse order, the two sides belong to different sectors, and flags use only the defined bits. Surface flags use only the defined bits, a hidden surface has no other, and portal surfaces have none.
8. Every solid surface vertex references exactly one row per declared attribute; portal surface vertices reference none.
9. Each entity's origin lies inside its sector: on the inner side of every one of the sector's surface planes. This is an exact test because sectors are convex. A terrain's origin is not checked.
10. Template names are unique, and each template row has a name and a model. Entity names are unique. Spawn points have no model (`-`) and no options; props, actors, terrains and blockers must have one, and it must load. Terrains take no options but `material=` and `filterN=`, and blockers none. A `material=` names a loaded material. A `filterN=` comes with a material and names slot 0 or 1 (its sampler's name is checked by the app, which owns the samplers). Scale is positive. Only props are `static`. `anim=` names an animation of the entity's model, and not on a `static` prop. An occluder is one of the forms above, a facing polygon has 3 to 64 sides and a positive radius, and a proxy model must load.
11. Ambient light and light colors are not negative. Each light's range is positive, and its position lies inside its sector (as for entities). Each light row has 8 or 13 fields before its options. A spot light's direction is not zero, and its angles satisfy 0 ≤ inner ≤ outer ≤ 180. A light's radius is 0 or more.
12. Each directional light has 7 fields before its options, a direction that is not zero, a color that is not negative, and an angle from 0 to 45 degrees.
13. Each name an `exclude_lights=` list holds is a light's (`name=`) or `flashlight`, and one of the level's first 63 lights.
14. Options are known ones: anything else is an error. An oscillation has four numbers and a positive period, and both ends of its swing lie inside the level.

## Planned extensions

- More surface flags (for example two-sided, translucent).
- Portals with a drawn surface (water, glass) for the translucent pass.
- Per-sector properties (ambient light, fog, tint).
- A binary format for fast loading, compiled from this text format.
