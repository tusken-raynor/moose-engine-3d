# Lighting Spec

Sep 28, 2026 · Built: static point and spot lights, diffuse only, and a shadow-mapped flashlight

## Overview

Lights are evaluated only at sample points, by each material's `shade_sample`, eight points at a time. The light reaching a point is interpolated to the pixels between sample points like any other value. This is where the sample lattice pays off: a lit surface's lighting costs one evaluation per lattice point, not per pixel.

**Built:**

- static point and spot lights with a range, and an ambient light, read from the level
- light lists narrowed from sector to polygon
- diffuse (Lambert) lighting in every standard material except the mirror ball's

**Next (in rough order):**

- directional lights (with authored sector scopes), if wanted
- dynamic lights
- row-level light lists
- specular
- lattice spacing that follows light, not only perspective
- shadows beyond the flashlight's (see Flashlight shadows), which get a spec of their own

## Lights and where they reach

- **Source:** the level's `ambient` line and `lights` section (see the Level Format Spec). A level without them has ambient 1 and no lights, so it looks as before.
- **Sector lists (at load):** each light floods out from its sector. It crosses an open portal (a render-through adjoin) when the portal polygon is within its range, then continues from the sector beyond. `World::sector_lights(sector)` lists the lights reaching each sector. Light never passes through walls.
  - It does pass through a whole opening, not only the part the light can see through the openings before it. Clipping floods to portal windows, as the view does from the eye, is a later refinement.
- **Changing lights:** `World::set_lights` replaces them and floods again. This is how the app's K key switches lighting off and on.
- **The flashlight:** the app gives the player a spot light mounted on the right shoulder (0.25 m right of and 0.2 m below the eye), aimed where they look. It uses the fixed spot light's old settings (color 3.4/3.5/3.9, range 16 m, cone 6°→20°) and replaces that light in shiny_rooms. The app rebuilds the lights and floods them again every frame, which costs next to nothing for a handful of lights and sectors. The mount is traced from the eye, so a wall stops it inside the level. H toggles it, and mirror ball cube maps are baked without it.
- **Per frame (view):** `ViewGeometry` copies the lights, the ambient light and the sector lists. It also lists each drawn entity's candidates: the lights of every sector it touches. `ViewGeometry::polygon_lights(p)` gives a polygon's candidates.
- **Per polygon (raster setup):** a candidate is kept if it is in front of the polygon's plane, closer to that plane than its range, within range of the polygon's bounding box, and (for a spot light) its cone reaches the polygon's bounding sphere. The survivors go into the polygon's `SampleContext::lights`.
- **Spot lights in the flood:** a spot light's cone must also reach a portal's bounding sphere to pass through it. A spot aimed away from a doorway stays in its room.
- **Mirrors:** polygons seen in a mirror are lit where they really are, since their sample positions are real surface points, so reflections show correct lighting with no reflected lights. The mirror ball's cube maps are baked from the lit level.

## The light at a sample point

```
light = ambient + sum over the polygon's lights of
        color * (1 - d²/range²)² * max(0, n·L/|L|) * smoothstep(cos_outer, cos_inner, -L·dir/|L|)
```

- `n` is the unit face normal; `L` points from the point to the light, and `d = |L|`.
- **Cones:** every light has one. A spot light's is its direction and two half-angles, and the smoothstep eases from nothing at the outer angle to full at the inner. A point light's cone is whole, its factor exactly 1. So both kinds run the same code with no branch; the cone costs a dot product and a few multiplies per light per 8 points.
- The falloff reaches exactly zero at the range with zero slope, so culled lights never leave a seam.
- `n·L/|L|` needs one reciprocal square root per light per 8 points and no division.
- The result is linear RGB, where 1 shows a surface's full color. It can exceed 1 (overbright).

## Gamma

- **Colors are gamma-encoded:** textures, vertex colors and the framebuffer hold `linear^(1/2.2)`.
- **Encoding at sample points:** each sample point encodes its light the same way, `light^(1/2.2)`. It uses a 1025-entry LUT indexed by the light's square root, which puts entries where the curve is steep (the darks), so it is within a color level everywhere. The LUT covers linear light up to 16.
- **Why that is enough:** a power law commutes with products, so an encoded color times the encoded light is exactly the encoded lit color. Pixels need no conversion.
- **Perceptual interpolation:** the light is interpolated between sample points in the display's own terms, where its steps look even. Before this, light multiplied gamma-encoded colors directly, which made its falloff too steep and bunched its steps in the darks.
- **Ambient is linear too:** 0.01 displays at about 0.12. shiny_rooms' ambient went from 0.12 to 0.01 to keep its mood.
- **Precision:** the light output is 16.16 fixed point, where 65536 is 1. In 8.8, its per-pixel step along a row could only change in 1/256ths of full light. Across a 32 px cell that left steps of up to 1/16 at each grid column, visible as vertical bands (and hue shifts, per channel).

## In the materials

- **Vertex color** (`VertexColor`, `VertexColorTranslucent`, both Fresnel ones): color (8.8) and light (16.16) are separate outputs, multiplied per pixel and clamped to 255.
  - Multiplying at sample points would need a clamp there, and a clamp breaks the lattice's edge behavior. Sample points past a polygon's edge can carry colors beyond 255, which the wrapping 8.8 stepping brings back into range inside the polygon, but only if nothing clamps them.
- **Textured** (`Textured`, `TexturedTranslucent`, `TexturedFresnel`, `Water`): the light is a 16.16 output that multiplies the texel's color per pixel. The texel's alpha (roughness, detail mask) is kept.
- **Exactness:** a light of exactly 1 leaves every color bit-for-bit as before.
- **Not lit:** `CubeReflection` (the mirror ball), which shows its baked lit surroundings.

## Sample spacing

- A polygon that any light reaches uses a lattice spacing of at most `RasterConfig::light_spacing` (32 px by default since Sep 28, when 32 px on direct faces looked the same as 8 px to the eye) in both directions. The app changes it with `-` and `=`, or `--light-spacing`.
- Materials and perspective can ask for less. Unlit polygons keep their material's spacing.
- Spacing is chosen per 32×32 tile from the polygon's nearest depth there (see the Material Pipeline Spec), so a steep wall is sampled finely only toward its far end. Views pressed against walls render 28–36% faster than with one spacing per polygon (for example 2.59 → 1.72 ms), within 7 levels of the old image.
- On steep surfaces, perspective would ask for a point every pixel. `RasterConfig::min_step` (4 by default) caps that, which is where lighting cost concentrated.
  - The cost: a light's bright spot that is only a few pixels across on a distant steep surface can fall between sample points. For example, a light 0.4 m below a ceiling seen from 11 m away: its spot dims, and can flicker as the view moves.
  - `min_step` 1 removes the cap (`[` and `]` in the app, or `--min-step`).

## Spot light penumbra

Where a spot light's cone fades (its penumbra), the light can change faster than 32 px cells follow. Sep 28, four approaches were built and compared.

| Approach | Spot-only pixels off by >2 levels | Worst | Cost | Outcome |
| --- | --- | --- | --- | --- |
| Cone at sample points only | 1.31% | 103 | cheapest | kept, with authored soft penumbras |
| Denser sample points where the penumbra crosses | 0.17% | 70 | a few % | kept, as the rule below |
| Spots lit per pixel, gamma-2 sum | 4.4% | 18 | +20% | removed |
| Spots lit per pixel, LUT sum | 0.018% | 14 | about 2× | removed |

The per-pixel versions had two further costs:
- **Position tax:** they needed the world position carried to every pixel, which cost about 20% in every lit material, even where no spot light shone.
- **Shaders coupled to light types:** a flashlight's edge could only be sharp if every surface's shader handled a special kind of light.

**Decision:** sample points stay agnostic of light types. The engine tells them which lights reach them and evaluates them the same way. The engine alone decides where sample points go, for perspective and now for spot light penumbras. Point lights get no extra density: judged unnecessary.

**The penumbra rule:**
- **Per tile:** each tile measures how far each spot light's cone factor (1 inside, 0 outside, before easing) changes across it, at the corners and middle of the tile padded by `penumbra_padding` (16 px) on every side.
- **Why the padding:** the rule treats the measured change as spread evenly across the tile. When a sharp edge clips only a tile's corner, the unpadded corners saw a small change, so the tile kept 16 px cells, and one lit point smeared light across a whole cell. The padded rectangle reaches into the penumbra beside the tile, so such tiles go dense. On 40 random views near the spot, pixels off by more than 8 levels dropped from 0.52% to 0.38%, and cost was within timing noise of the unpadded rule. A per-tile fade-rate bound was equally accurate but cost about 7% more.
- **Spacing:** the tile then spaces its points so the cone fades by at most `RasterConfig::penumbra_threshold` (1/8) per cell, and no closer than `penumbra_spacing` (4 px).
- **Result:** a narrow penumbra, or one seen close up, gets close points, while a wide soft one keeps wide cells.
- **In the app:** J cycles the threshold (1/4, 1/8, 1/16, off), and 9 and 0 shrink and grow every spot light's penumbra angle for art direction.

**Measurements:**
- **Accuracy:** on the lighting test with a narrow spot (10°→16°), pixels off by more than 2 levels drop from 2.5% to 1.8%, and that is the default now.
- **Cost:** on shiny_rooms with its soft spot (6°→20°), about +7% over random views (4.3–4.6 ms → 4.7–5.0 ms, 1 thread).

## Flashlight shadows

Sep 28, a first test of shadow mapping, on the flashlight only. Shadow volumes were set aside: they work per pixel with a stencil pass per light, which costs CPU fill rate and doesn't fit lighting evaluated at sample points.

- **The map:** `ShadowMap` (moose-raster) holds, per texel, 1 / the distance along the light's direction to the nearest surface, over a square covering the cone (512 texels by default, `--shadow-size`). `Renderer::shadow_maps` holds the maps, and a light names its map with `Light::shadow`.
- **Rendering it:** before the frame, the level's polygons in the sectors the light reaches (the portal flood) and the entities touching them are drawn from the light, depth only.
  - Polygons are clipped to a near plane, and those facing away from the light are skipped: sectors and models are closed.
  - 1 / depth is affine in the map, so each texel is a single max with no division.
  - The map is redrawn only when the light moves.
- **Looking it up:** inside `diffuse()`, for each light with a map, wherever the light reaches any of the 8 points.
  - Each point moves off its surface by 1.5 texels along its normal (normal offset) and gets a bias of 1 texel, both scaled to texel size at its depth. No acne showed in the test views.
  - The four nearest texels' tests blend bilinearly (PCF), giving a one-texel soft edge.
  - Shaders are unchanged: the shadow is one more factor on the light, like the cone.
- **In the app:**
  - Z toggles shadows (`--no-shadows`).
  - U locks the flashlight where it is so its shadows can be seen from elsewhere, and U again puts it back on the shoulder (`--lock-flashlight X,Y,Z,YAW,PITCH` locks it for screenshots).
  - The title shows the map's render time.
- **Cost (one thread):** 0.13–0.26 ms per redraw at 512 texels, 0.06–0.09 ms at 256, over four views of shiny_rooms. The first version, which divided per texel and drew back faces, took 0.3–0.8 ms. Lookups barely show in raster time.
- **Quality:** rendering at every pixel gives crisp edges. At the normal sample spacing, edges show small stair-steps along diagonals, as cells blend a hard edge. Where the spot's penumbra rule already tightens tiles to 4 px, 32, 16 and 8 px spacing look alike. Sample placement doesn't yet account for shadow edges.

## Accuracy and cost

**Test (`lit_surfaces_match_exact_lighting`):** 200 random views of two_rooms with the five shiny_rooms lights, against exact per-pixel lighting.

| Light spacing | Pixels off by more than 2 levels | Worst |
| --- | --- | --- |
| 1 px | 0 | 2 |
| 2 px | 0.005% | 30 |
| 8 px, min step 1, before gamma | 0.8% | 32 |
| 8 px, min step 4, before gamma | 1.0% | 124 (the distant ceiling spot above) |
| 8 px, min step 4, gamma-encoded | 0.14% | 62 |
| 32 px, min step 4, gamma-encoded (default) | 1.6% | 77 |

- The larger errors sit on ceilings just above a light (0.4–0.8 m) seen from afar, where the light peaks within a few pixels.
- Spacing that follows how fast the light changes, not only perspective, would fix that.

**Timing:** shiny_rooms at 1280×720, 2 reflection bounces, five lights.

| | 1 thread | 10 threads |
| --- | --- | --- |
| Unlit | 3.9 ms | 0.74 ms |
| Lit | 4.66 ms | 0.79 ms |
| Unlit, water on | 5.1 ms | 0.84 ms |
| Lit, water on | 5.9 ms | 0.99 ms |
