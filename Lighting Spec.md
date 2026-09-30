# Lighting Spec

Sep 28, 2026 · Built: static point and spot lights, diffuse only, and a flashlight whose shadows are carved into polygons

## Overview

Lights are evaluated only at sample points, by each material's `shade_sample`, eight points at a time. The light reaching a point is interpolated to the pixels between sample points like any other value. This is where the sample lattice pays off: a lit surface's lighting costs one evaluation per lattice point, not per pixel.

**Built:**

- static point and spot lights with a range, and an ambient light, read from the level
- directional lights (a sun), entering through sky surfaces, with carved soft shadows
- light lists narrowed from sector to polygon
- diffuse (Lambert) lighting in every standard material except the mirror ball's

**Next (in rough order):**

- dynamic lights
- row-level light lists
- specular
- lattice spacing that follows light, not only perspective
- shadows beyond the flashlight's (see Flashlight shadows), which get a spec of their own

## Lights and where they reach

- **Source:** the level's `ambient` line and `lights` section (see the Level Format Spec). A level without them has ambient 1 and no lights, so it looks as before.
- **Sector lists (when lights are set):** each light floods out from its sector (a directional light from every sector with a sky surface it shines in through). `World::sector_lights(sector)` lists the lights reaching each sector. Light never passes through walls.
  - It crosses an open portal (a render-through adjoin) that it shines out through, and that is within its range and, for a spot light, its cone.
  - Each opening is clipped to the window the light sees it through: planes through the light (along a directional light's direction) and the edges of the openings before it. The light stops where that is empty. Until Sep 29 it passed whole openings. In sunny_rooms the sun then reached room_a through the hallway, though it lands on the hallway floor 2 m past the doorway. With shadows off, room_a's sun-facing walls were lit; with them on, carving had hidden it.
  - Within a sector it reaches, all of a light reaches; keeping parts of it dark is the job of shadows.
- **Changing lights:** `World::set_lights` replaces them and floods again. This is how the app switches lighting off and on (options menu, Lighting page).
- **The flashlight:** the app gives the player a spot light mounted on the right shoulder (0.25 m right of and 0.2 m below the eye), aimed where they look. It uses the fixed spot light's old settings (color 3.4/3.5/3.9, range 16 m, cone 6°→20°) and replaces that light in shiny_rooms. The app rebuilds the lights and floods them again every frame, which costs next to nothing for a handful of lights and sectors. The mount is traced from the eye, so a wall stops it inside the level. The options menu's Flashlight page switches it on and off, and mirror ball cube maps are baked without it.
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
- **In the app:** the options menu sets the threshold (Sampling page: 1/4, 1/8, 1/16, off) and every spot light's penumbra angle (Lighting page: spot cone edge), for art direction.

**Measurements:**
- **Accuracy:** on the lighting test with a narrow spot (10°→16°), pixels off by more than 2 levels drop from 2.5% to 1.8%, and that is the default now.
- **Cost:** on shiny_rooms with its soft spot (6°→20°), about +7% over random views (4.3–4.6 ms → 4.7–5.0 ms, 1 thread).

## Flashlight beam

Sep 29. The flashlight's cone is drawn pixel by pixel with its shadow, not lit at sample points (`Light::beam`; the options menu's Flashlight page, Cone: beam or sampled; `--cone`). Sampled stays as the lower-quality option. At low resolutions the penumbra rule's dense points still couldn't follow a sharp cone edge, and they cost sample points wherever the cone crossed.

**How it works:**
- **Pyramid:** the view cuts each surface the flashlight reaches by a square pyramid around its outer cone: 4 planes through the light, each touching the cone on one side (`Carver::add_beam`).
  - What is outside is dark for the flashlight, like full shadow. A surface wholly outside drops the light; occluders and windows are carved only inside.
  - What is inside is marked as in the beam (`Piece::beamed`), and its shadow pieces carry the cone (`ShadowPiece::beam`) and each vertex's ray from the light, in the beam's frame (`ShadowVertex::ray`).
  - A surface wholly within the inner cone isn't cut at all: the cone is 1 all over it.
- **Shadow buffer:** in a beam's pieces, `shadow_run` fades the light by the angle between each pixel's ray and the axis, eased like the cone at sample points, times the piece's own shadow value (so occluders' shadows multiply with it). Rays scale with w, which the angle ignores, so they interpolate across a row like any value. The circle is exact: no planes are spent on roundness.
- **Per row, where the row crosses each cone** (a cone is convex, so each row crosses it in one stretch): solved as a quadratic in the ray along the row, `ray.x² = cos² |ray|²`, for the inner and outer cones. Outside the outer cone is a plain fill of 0, inside the inner cone is the piece's own value with no cone math, and only the fade between takes the cone.
- **In the fade, every 8 pixels** (`BEAM_STEP`): blended between the ends of a stretch where the cone can change by at most 1/8 across it (`BEAM_BLEND`; blending strays by at most about 1%), otherwise pixel by pixel (a sharp fade, as at low resolution).
  - The first version checked stretches of 8 pixels everywhere, and at first blended every one from the pyramid's edge, which pinned a sharp fade to the pyramid's straight side: a square spot at 320×180 with a narrow penumbra.
- **Bulk fills:** a piece whose value is the same at every vertex (all dark, all lit) lowers the buffer in bulk, and not at all where it's all lit (the buffer starts there). This helps every shadow, not just beams: the per-pixel loop over whole dark and lit pieces was most of what the beam cost (about 0.45 ms at 1280×720, 1 thread).
- **Samples:** a split beam is lit at sample points as a point light (`Light::sample_cone` is whole), which varies smoothly, so the penumbra rule leaves it out. Where the renderer can't split it off (no shadow slot, or too many split lights or values for the material), its cone is lit at sample points as before.
- **Pixels:** outside the cone the buffer is 0, and `PixelContext::light` returns the other lights as they are (the "fully shadowed" fast path, new here); inside the inner cone, the fully lit one. Where each of 8 pixels is all lit or all dark (a hard or dithered beam's edge), it picks the one or the other per pixel, with no tables. Only a smooth fade pays the full combine.
- **With dynamic shadows off,** only beams are carved per frame (`Parts::Beams`): the cone still draws, without the flashlight's shadows.
- **Anchoring to sample points isn't needed:** the buffer is per pixel and doesn't depend on the lattice, and the samples see only a smooth point light.

**Split lights carry one value** (not three): a light's color is fixed, so its sample points carry only its strength (Lambert, falloff, and cone where lit there), and pixels multiply by the color (`PolygonSetup::split_colors`). `diffuse` sums the split lights' light into the total itself. This helps every split light, not just beams: about 0.15–0.18 ms off the beam at 1280×720 (1 thread), the same image within rounding (0.00% of pixels off by more than 6 levels).

**Reflections light beams at sample points:** the view carves no beams for polygons seen in a mirror (`Receiver::beams`), and the renderer lights the cone there as before. Reflections are half-rate and usually small on screen, where the sampled cone's softness doesn't show.

**Tested and removed: a screen-space beam.** No pyramid: each polygon the beam reaches is one beam piece, and the renderer's per-row solve finds the cone on it, which is the cone worked out from each pixel's screen position and depth (the ray to the light scales with w). The image is identical, but it's slower (about +0.23 ms smooth and hard at 1280×720, 1 thread): without the pyramid, more polygons split the flashlight off across their whole area. The pyramid's scoping pays for its carving.

**Tried and removed: a polygon beam.** Inside the pyramid, a 16-sided window around the cone with soft edges from the inner cone to the outer (like a doorway's), drawn by plain Gouraud with no cone math per pixel. It saved the cone math (about 0.2 ms at 1280×720, 1 thread) but added carving (about +0.1 ms of view time) and more pieces, while the combine across the fade, the larger cost, stayed. Net 0.1–0.2 ms cheaper than the exact beam, with a faint 16-sided outline and a dimmer, linear fade (values exact only at the ring's edges: at the middle of a 6°→20° fade, about 0.5 against 0.69). Not worth keeping.

**Fade setting:** the flashlight's cone fades inside its 20° edge over 0° (hard), 2°, 4°, 8° (the default, from 14° before), 14° or 20° (the Flashlight page's Fade; `--flashlight-fade`). A narrower fade leaves less of the spot paying the full combine.

**Cheap looks, for low settings:**
- **Hard (0° fade):** a crisp theatrical disc. No fade at all: every pixel takes a fast path.
- **Dithered** (`RasterConfig::beam_dither`; the Flashlight page's Dither; `--dither-beam`): the fade as a 1-bit stipple, each pixel lit or not by a 4×4 ordered (Bayer) dither (`BEAM_DITHER`), retro like early Mac or Game Boy shading. It's applied where a pixel may be partly lit: the fade, and inside the inner cone when a soft shadow crosses it. The fade is still worked out, but the combine is the per-pixel pick.

**Results:** at full resolution the beam matches the sampled cone (0.04% of pixels differ by more than 6 levels, with crate shadows in it). At 320×180 with a narrow penumbra the spot is round and crisp, where the sampled one is blurred into cells.

**Cost** (timing example, `FLASHLIGHT=beam|sampled`, random views of shiny_rooms, a noisy machine; a 6°→20° fade unless noted):
- **480×270, all threads:** the same within noise (about 0.85 ms).
- **1920×1080, all threads:** beam about 4.5–4.8 ms, sampled 3.8–4.3, none 3.1: the beam costs about 0.3–0.7 ms more.
- **1280×720, 1 thread:** about the same to +0.6 ms over sampled, across runs.
- **With the 8° fade** (1280×720, 1 thread, medians of 4 runs): none 7.8 ms, sampled 8.7, beam 9.5.
- **After the bulk fills and per-row cone stretches** (1280×720, 1 thread, 8° fade, medians of 3 runs): none 6.0 ms, sampled 7.2, hard 7.3, dithered 7.5, smooth 7.7. Before them: smooth 8.1, hard 7.8, dithered 7.9.
- **With one value per split light** (same, 3 runs): none 6.1 ms, sampled 7.07, hard 7.14, smooth 7.56; the screen-space beam 7.37 hard, 7.79 smooth, 7.55 dithered.
- **Where the beam's cost went, before them** (1280×720, 1 thread, 6°→20° fade): the split light's extra values and the pieces' fills about 0.3 ms; the cone in the fill about 0.25 ms; the full combine across the fade about 0.37 ms. Without the pieces at all, the split flashlight cost less than the sampled one: the dense sampling cost more than carrying the split light.
- **Not adopted:** a combine that blends between the two exact ends (the rest, and the total) in gamma-2 terms, with no tables, saved the 0.37 ms but changed the fade by up to 12 levels on about 2% of pixels.
- **Not adopted: powers in lanes instead of tables.** The full combine's decode and encode as `2^(p log2 x)` worked out in SIMD lanes (log2 from the exponent bits and an atanh series, exp2 by rounding and a series; within 1e-4) was slower than the tables: the smooth beam 8.1 ms against about 7.6, level-light shadows 7.1 against 6.8 (1280×720, 1 thread). The tables fit in cache, and six log2/exp2 pairs per 8 pixels, each log2 with a division, cost more. With the combine made free (a wrong image, as a bound), the most there was to save was about 0.35 ms with the smooth beam and 0.16 ms with level-light shadows.

## Directional lights

Sep 29. Light from far away, like the sun, arriving along one direction everywhere (the level's `directional` section; see the Level Format Spec).

- **Lighting:** a directional light is a point light 10 km away with a range of 1000 km and a whole cone (`Light::directional`, `Light::directional == true`). The standard lighting lights with it unchanged: its light arrives along one direction anywhere in a level, and it doesn't fade. Shaders don't know it's different.
- **Where it reaches:** it enters the level only through **sky surfaces** (surface flag `0x2`), which it shines in through. From there it passes on through open portals it shines out through, each clipped to the window it is seen through (see Lights and where they reach). The world's flood starts in every sector with such a sky surface.
- **Carving:** the carver sees every light as a `Source`, a point or a direction. For a direction, the planes through an edge run along the light's direction, not through a point, so nothing 10 km away enters the math and precision stays exact.
  - A sky surface is a window into its sector, like an opening seen from a point light. A courtyard's floor is lit only where rays through its sky reach it, so its walls cast shadows on it.
  - The window flood continues through portals. Sunlight falls through a doorway as a patch cut to the doorway's shape. The world's flood clips the same way, so the sun is listed only for sectors it can really reach.
  - Occluders cast parallel shadows. A facing polygon from a directional light is a great circle of its ball.
- **Softness:** the source's angular size (`angle`, in degrees; the sun is about 0.53). Wedge planes through an edge are turned by the source's angular radius either way.
- **Shadow slots:** the flashlight has slot 0, and the level's directional lights that cast shadows take slots from 1.
- **In the app:**
  - The options menu's Lighting page switches the level's directional lights on and off and sets their size (0, 0.53, 2, 5, 10 degrees; `--sun-angle`, `--no-sun`).
  - Sky surfaces are drawn unlit in their vertex color (`UnlitColor`).
  - Non-reflective floors get the floor texture too.
- **Test level:** `sunny_rooms.mmp` is two_rooms with room_b's ceiling open to the sky, a sun from the south-west, and a dim sky-blue ambient.
- **Cost:** in a courtyard view, the view takes about 0.07 ms against 0.02 ms without the sun, and the raster about 0.25 ms more.

## Flashlight shadows

Shadows are drawn per pixel from a **shadow buffer**. The view works out where each shadow falls on each polygon as convex shadow pieces. The rasterizer fills them in, row by row, as it shades the polygon, and the polygon's pixels add back the light the shadow lets through. A shadow's edge is exact at any resolution, whatever the spacing of the polygon's sample points, and polygons are never split. Only the flashlight casts shadows so far.

**How it evolved (Sep 28–29):** each earlier version is kept in a commit on branch flashlight-shadow-map.
- **06d4997, shadow maps:** rejected on quality. Lookups at sample points left stair-steps along edges.
- **1e24f4c, carving:** each polygon split into pieces wholly lit or wholly shadowed. The edges were exact, but polygon counts doubled or tripled, and a crate's faces cut each other's pieces.
- **918933f, soft carving:** light-sized penumbras, with per-vertex light values interpolated at sample points. It needed the ring carved in sectors and a density rule, and it split polygons even more.
- **Shadow buffer (now):** the shadow pieces are drawn into a per-row buffer instead of splitting the polygon.

**What blocks a light** (`moose-view` carve module; `Light::shadow` gives a light a shadow slot, 0–31):
- **Portals.** Light reaches a sector beyond its own only through the openings between them. From the light, each opening seen through the ones before it is a window, and only what lies within some window into a polygon's sectors is lit.
- **Occluders.** Each entity's occluder comes from the level (`occluder=` in the Level Format Spec), and by default it's the entity's own model. The artist can instead give it a proxy model, a level of detail (its model until levels of detail exist), a polygon that always faces the light, or none.
  - Convex shapes are found once per model and cast one volume (below). Crates use this.
  - Any other shape casts one volume per face toward the light. Together they are exactly its shadow, so complex proxies work and only cost more. Their shadows are hard-edged for now; soft edges need wedges on the silhouette's edges only.
  - A facing polygon sits on the outline, seen from the light, of a ball of its radius, so a round shape's shadow needs no silhouette search. shiny_rooms' mirror ball uses a 16-sided one.
  - A shape never shadows its own model.
- **One volume per occluder.** A convex shape casts one convex shadow volume: inside the planes through the light and its outline (the edges of its faces toward the light that no other such face shares), and behind every one of those faces, by 1 mm.
- **Soft shadows.** A light with a size (`Light::radius`, in meters) casts a core and a soft edge.
  - For each outline edge, two planes through the edge graze the light's sphere on opposite sides. The outer one bounds the shadow's region.
  - The soft ring is carved sector by sector, between planes through the light and the outline's corners, and each sector is split by its edge's inner plane. Past it, within every inner plane, lies the core.
  - Each soft piece's vertices get how much of the light reaches them. Each wedge covers part of the light, from 0 on its outer plane to 1 on its inner, eased with a smoothstep. An occluder covers the product of its wedges' parts.
  - **Occluders' parts add up**, capped at all of the light. This is exact for occluders side by side as seen from the light, and too dark where one is behind another. Multiplying what each leaves uncovered (the first version) left a lit line where stacked crates meet: each covers half of the light at their seam, and 0.5 × 0.5 let a quarter through. Smoothstep is symmetric, so the two edges' parts sum to exactly 1.
  - **Full shadow wins.** A piece in one occluder's core and another's soft edge is dark. Before this fix, the shadow buffer used the soft value there, which drew the thin fully lit line along the seam.
  - **A value along each edge at a contact corner.** Where an occluder's edge touches the surface its shadow falls on (a doorway jamb or a crate's corner on the floor), a soft piece has a corner on the edge's line. There both of the wedge's planes meet, and how much of the light reaches has no one value: it is the same all along each ray out from the line, from none to all. Plain Gouraud gave that corner an arbitrary 0 or 1 and spread it over the whole soft edge, which looked hard: shiny_rooms' doorway lit by the flashlight near the jamb, and a crate's shadow from a 10° sun. So each vertex has two values, along the edge arriving and along the one leaving, and a corner whose two differ is drawn as two vertices in the same place (a zero-length edge between them). Only the wedge whose line the corner is on is taken along the edge (at the edge's other end, or the piece's center if that is on the line too); every other soft edge the piece is in keeps its value at the corner. A corner counts as on a line when the wedge's width there is under 1% of its width at the piece's center: carved pieces stop 1 mm short of an occluder (`CAP_BIAS`), so the corner is near the line, not on it.
    - The first fix weighted each vertex by the wedge's width (0 on the line) and interpolated projectively. That was exact inside one soft edge, but a corner of weight 0 also dropped the other soft edges' values there: in a crate's soft edge within a wall's, the wall's penumbra vanished inside the crate's.
- **Windows are soft too.** A portal window seen from a light with a size is the same outline in reverse. Its region reaches the plane where it starts to let the light through, its core is where it lets all of it through, and its ring is carved in sectors like an occluder's. Windows' parts multiply with each other and with what occluders leave. Level geometry's shadow edges (doorways, a room seen through a hallway) are soft for the same reason. One routine, `add_outline`, builds both kinds.

**The shadow buffer:**
- **Surfaces out of a light's reach aren't carved:** those facing away from it, or with their bounding sphere wholly beyond its range or outside a spot light's cone. These are the rasterizer's tests; it would drop the light from them anyway. With this, shiny_rooms bakes 24 real shadows out of its 1,720 (light, surface) pairs.
- **View:** every polygon is emitted whole.
  - It is carved (in world space, after window clipping) only to find its shadow pieces: the parts in a light's full or soft shadow, each for one shadow slot, with screen vertices, the lines their edges walk (shared with the polygon's, so they meet it exactly), and a light value per vertex (0 in full shadow; two at a contact corner, below). These are `ViewPolygon::split`, `first_shadow` and `shadow_count`, over `ViewGeometry::shadow_pieces` and `shadow_vertices`.
  - A light whose shadow covers the whole polygon is simply dropped from it (`ViewPolygon::shadowed`).
- **Sample points:** for each light whose shadow covers part of a polygon (up to `MAX_SPLIT`, 2), `diffuse()` leaves that light out of the total and hands its light to the engine (`SampleContext::light_split`, `split`). The engine encodes it like the `light` output and carries it as 3 more outputs of each sample point, interpolated with the material's.
- **Row pass:** when a run of the polygon's row is shaded, `shadow_run` fills how much of each split light reaches each pixel. The buffer starts at 1, and each shadow piece crossing the row writes its values Gouraud-style: along its edges to the row, then across it, perspective-correct, keeping the smaller value where pieces meet.
- **Pixels:** lit materials read their light through `PixelContext::light`, which adds each split light back as much as the buffer lets through. It adds in linear terms: the encoding is close to a square root, so it takes the root of the sum of squares. Shaders only pass their `light` through it, and know nothing of shadows.

**In the app:**
- Crates cast as themselves, and the mirror ball as a sphere.
- The options menu's Lighting page switches shadows on and off (`--no-shadows`).
- The options menu's Flashlight page sets its radius: 0, 2, 5 (default), 10 and 20 cm (`--light-radius`).
- Its Mount setting locks the flashlight in place, or puts it back on the shoulder (`--lock-flashlight X,Y,Z,YAW,PITCH`, `--flashlight-at X,Y,Z,DX,DY,DZ`).

**Cost** (Sep 29, averages over 200 frames, on a noisy machine):

| View | Carving (hard / 20 cm) | Shadow buffer | Shadow pieces (hard / 20 cm) |
|---|---|---|---|
| Crate view | 140 / 304 polygons | 47 polygons | 18 / 175 |
| Ball view | 453 / 576 | 301 | 7 / 123 |
| Doorway view | 379 / 423 | 284 | 42 / 89 |

- **Raster time:** about the same as carving, within noise, and a little lower in some soft views.
- **View time:** 0.06–0.17 ms against 0.03–0.05 ms without shadows.
- **Rendered images:** the same as carving's, within 0.02% of pixels.

**Next:**
- RGB visibility for colored shadows from translucent occluders.
- Shadows from other lights.
- Occluder shapes read from assets.

## Cached shadows

Sep 29. Static lights' shadows on static surfaces are carved once and kept.

- **What's static:**
  - Lights from the level are static (`Light::is_static`), as are the level's directional lights; the flashlight isn't.
  - Level surfaces are static, and so are props marked `static`.
  - Occluders are static when their entity is.
- **The cache** (`Carver::cache`):
  - The first time a static surface is seen, each static light's shadow on it from the parts that never change (its windows and static occluders) is carved in world space, over the whole unclipped surface.
  - The result is kept as world-space pieces: fully shadowed ones, and soft ones with the soft edges they're in (their wedges).
  - A surface wholly in the full shadow is marked so, and simply drops the light.
- **Each frame:**
  - Only moving occluders are carved for a static surface (`Parts::Cached`). Everything is carved for dynamic lights and moving surfaces, as before.
  - The cached pieces are clipped to the planes the surface was clipped to, and projected.
  - How much of the light reaches each clipped corner is worked out from the cached soft edges at that point, as carving does. Interpolating stored values along long pieces instead made visible differences at the screen edges.
- **Edges still meet exactly:**
  - A piece's edge along the surface's own edge walks the line the clipped surface walks.
  - Along a clip plane, it walks that plane's line.
  - Along a cut, it walks the line through the cut's world points, trimmed to the view frustum and projected without clamping. Projecting those points with the clamp to the viewport bent lines whose ends were off screen.
- **When it's rebuilt:** a light's cache is dropped when the light changes (position, direction, size, range, cone), for example when Y changes the sun's angle. Toggling a light or shadows off and on keeps it.
- **Checking it:** `ViewConfig::cache_shadows` (the app's `--no-shadow-cache`) carves every frame instead.
  - Over 12 random sunny_rooms views, cached and uncached match within a few levels.
  - The exceptions are 1-pixel shifts along hard edges (the same line computed from different points) and soft bands at large sun angles. There the per-frame carve's pieces, cut at the screen's edges, blend the smoothstep fade differently. The cached pieces don't depend on the view, so their soft bands don't change as the view moves.
- **Cost** (sunny_rooms, 200-frame averages, sun at 2°):
  - The view takes 0.026–0.041 ms cached against 0.035–0.079 ms carving every frame, and about 0.02 ms without the sun.
  - A surface's first frame costs about 0.1 ms more while its cache is built.
  - The saving grows with the number of static lights and surfaces.
- **Not yet:** moving occluders' shadows combine with cached ones by taking the darker value per pixel, not by adding coverage.

## Baked level light shadows

Sep 29. Every level light that casts shadows (point, spot and directional; `shadows=off` opts out) has one, baked at load.

- **Shadow slots:** the flashlight has slot 0, and level light `i` has slot `i + 1`. Slots are fixed, so switching lights on and off (N, I) never hands a light another's cached shadows.
- **Baking:** `ViewGeometry::bake_shadows` carves every static light's shadow on every static surface (level polygons and static props) when the level loads, instead of when each surface is first seen. shiny_rooms bakes 1,720 (light, surface) shadows in about 1 ms.
- **Up to 4 partly shadowing lights per surface** (`MAX_SPLIT`, raised from 2). Each carries its light to pixels on its own, as 3 extra values per sample point. Past the limit (or past 32 values in all), a light is lit unshadowed on that surface.
- **Adding split lights back at pixels:**
  - Split lights are carried linear. The rest of the light is decoded from its gamma encoding with a 1,025-entry table (blended), each split light is added as its shadow lets it through, and the sum is encoded with the sample points' table.
  - Squaring encoded values instead (treating gamma as 2.0 rather than 2.2) left lit areas about 3% brighter with shadows on than off.
  - Vector `ln` and `exp` per pixel were exact but more than doubled raster time.
  - A fast path uses the total light, all split lights included, worked out at the sample points, for any block of 8 pixels all of every split light reaches. Only blocks in shadow or penumbra combine. Most of a partly shadowed surface is fully lit.
- **shiny_rooms' lamps** are 5 cm across (`radius=0.05`), for soft-edged shadows.
- **In the app:** the options menu's Lighting page sets a multiple of every level point and spot light's size (level light size: ×0 for hard shadows, ×0.5, ×1 as authored, ×2, ×4; `--light-scale`). The sun and the flashlight have their own size settings. A change rebuilds those lights' cached shadows.
- **Cost** (shiny_rooms, all 5 lamps with shadows, random views, one thread): 7.1–7.6 ms against 6.1 ms without shadows (+17–25%). The view part is 0.085 ms against 0.016 ms.


## Moving lights

Sep 29. A level light can move: `oscillate=DX:DY:DZ:PERIOD` (Level Format Spec) swings it through its position, once every period.

- **Each frame:** the app places it where it is at that moment (`Light::at_time`), in the sector it's then in.
- **Shadows:** a moving light isn't static (`Light::is_static` is false), so it isn't baked. Its shadows (windows and occluders) are carved every frame, like the flashlight's, while static lights keep their baked shadows.
- **sunny_rooms:** a light-blue lamp 2.3 m up in the middle of room_a swings 2 m toward each of the two corners without crates, every 5 seconds, with soft shadows (`radius=0.05`). The crates' shadows swing around them.
- **Cost there:** the view takes about 0.09 ms with the lamp and the sun, against about 0.03 ms for the sun alone.

## Dynamic shadows setting

Sep 29. `ViewConfig::dynamic_shadows` (the options menu's Lighting page, and `--no-dynamic-shadows`), a performance option.

- **Off:** only baked (cached) shadows are drawn, and nothing is carved per frame.
  - Dynamic lights (the flashlight, moving lamps) cast no shadows, and their windows and volumes aren't even gathered.
  - Moving occluders cast none, and moving surfaces receive none.
- **Cost saved:** in a sunny_rooms view with the flashlight, the moving lamp and the sun, the view takes about 0.054 ms off against 0.33 ms on.
- **Lighting stays correct:** a light's reach is still clipped to the openings it passes through (see Lights and where they reach); only occlusion inside the sectors it reaches is lost.

## Future optimization options

Noted Sep 29. A long surface that a static light reaches only in part pays for its shadow everywhere. For example, the hallway floor in sunny_rooms is one 12 m polygon, and the sun lights only a patch of it through the doorway. Every pixel of such a surface gets the shadow buffer fill and carries the light as a split light, though most of it is simply in full shadow.

- **Fully shadowed pixel blocks** (*done Sep 29, for the flashlight beam*): where every split light is fully blocked across a block of 8 pixels, `PixelContext::light` returns the other lights as they are, with no decode, add and encode. This mirrors the fully lit fast path, and makes the dark majority of such surfaces nearly free per pixel (the buffer fill remains).
- **Authors cut faces** (*worth it, as an authoring guideline; no engine work*): a level author splits long faces just past where a static light's penumbra ends. The far parts are then wholly in shadow and simply drop the light: no shadow pieces, no per-pixel cost. This works today.
- **The baker cuts faces** (*maybe; measure after the fully shadowed fast path*): what it would still save is the shadow buffer fill and the split light's extra values. New vertices on edges shared with neighboring faces need the whole edge's line, as carving does, to stay crack-free. the same split done automatically for static geometry. The bake already knows, in world space, where each static light's full shadow falls on each surface. When the full-shadow part is a large share of a surface, split it off as its own polygon at load, without the light. It costs a few extra polygons, only where that's worth it, and authors can still cut by hand for control.
- **Hard edges where the penumbra is too thin to see** (*probably not worth it*): a thin penumbra's per-pixel cost is small because it covers few pixels. Splitting a wedge along its length adds carving, and a view-dependent switch would pop as the view moves and couldn't use baked shadows. Revisit only for many dynamic soft lights on huge distant surfaces. if a soft edge would be under about 2 pixels wide on screen, draw only the full shadow (the umbra) there, with a hard edge between the wedge's two planes, and skip that wedge's soft pieces.
  - **Per edge, along its length:** a wedge widens with distance from its occluder and narrows with distance from the eye. A long shadow can be thin on screen at its far end and wide close up, so a wedge can be split along its length, by a plane through the light where its projected width crosses the threshold. The far part goes hard; the near part keeps its fade.
  - **It depends on the view:** it applies where shadows are carved every frame, as a view-time choice. Baked (cached) shadows are carved without a view, so their soft pieces could instead be split on screen, or snapped to hard where their projected width is under the threshold, when they are clipped and projected.
  - **Saves** soft pieces, the shadow buffer fill, and split lights on distant surfaces, where the fade isn't visible anyway.

## Planned: articulated shadow proxies

Noted Sep 29, for when actors animate. An animated actor casts its shadow with several simple proxies, one per articulating segment, instead of one occluder.

- **Limbs:** capsules (a bone segment and a radius). Seen from a light, a capsule's outline is two half-circles joined by straight sides. It is built like the facing polygon's tangent circle, at each end, so it faces the light, turns with its bone, and needs no silhouette search. That's about 12 to 16 edges each.
- **Head, torso, feet:** small convex hulls attached to bones, carved on the convex fast path.
- **Data:** an entity's occluder becomes a list. Each entry is bound to a bone, with a local offset, and is a capsule or a convex hull, placed each frame by the bone's transform.
- **Soft shadows hide the approximation:** at a few centimeters of light radius, seams between parts and the simplified shapes fall inside the penumbra.
- **Overlaps:** occluders' coverage adds up, so joints where segments meet close up, like the stacked crates' seam. Parts overlapping as seen from the light (an arm in front of the torso) make their shared penumbra a little too dark; cores are unaffected, since full shadow wins.
- **Self-shadowing, later:** a proxy doesn't shadow its own model today. For an arm to shadow the torso, ownership would go per bone: a part skips only the surfaces it stands in for.
- **Cost:** actors move, so their shadows are carved every frame. That's about 15 small volumes per actor per shadow-casting light reaching it, on nearby surfaces only, on top of the level's baked shadows.

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
