# Material Format Spec (.mmat, version 2)

Oct 1, 2026 · Built Oct 5 (branch `materials`); inputs, sources and mirrored copies Oct 6; one material a file (version 2) Oct 6. Where the build differs from the proposal, the spec says what was built; see As built at the end.

## Overview

A **material** says how a surface looks: which shader draws it, with which textures, read how, and with which numbers. Before this, that was decided in Rust, by rules in the app (`App::draw`): floors got the floor texture, walls their sector's texture, shiny floors Fresnel or water, brick walls their bump shader, crates the crate texture. Changing a look meant changing code.

With this spec, looks are data. Shaders are code (`moose_raster::shaders`), registered with the engine under names; materials are text files that pick a shader and fill in what it reads; levels, templates and entities name the materials their surfaces use. The engine binds what the data says, and the player's settings choose between what the material allows.

What stays code: the shaders themselves, and the engine's special cases (shadows, the flashlight's beam, where lighting is sampled).

## Concepts

- **Shader (program):** a registered material program (see the Material Pipeline Spec) under a name, with what it reads: its texture slots, and its **inputs**, each named and typed. One name can stand for several compiled copies: `lit` is `TexturedLit<BUMP, SPECULAR, MIRRORED>`; which copy draws a polygon is the program's business (what the material gives it, the player's settings, whether the polygon is seen in a mirror), never the material's.
- **Material:** a name, a shader, textures for its slots, a sampler per slot, a **source** for each input, and variants (other materials to use in particular scenarios).
- **Binding:** a level surface, a template or an entity names the material it is drawn with.
- **Resolution:** each frame, for each polygon, the engine turns its material, the scenario (is its reflection drawn under it, is it seen in a mirror), the player's settings and its inputs' values into a `Surface`: one compiled copy, its textures, samplers and params.

## Files

A shader program is the reusable part; a **material** is one package of the data a program needs (or where to get it), which faces, templates and entities are then drawn with. Each material is its own file in `assets/materials/`, `NAME.mmat`, named by its file: lowercase letters, digits and `_` (macOS doesn't tell `Brick.mmat` from `brick.mmat`). All are read at startup. A level can also have its own materials (planned, not built).

```
MOOSEMATERIAL 2
# Brick: bumps from its normal map, a highlight dulled on the mortar.
shader             lit
texture0           brick_wall.png
texture1           brick_wall_normal.png
filter1            bilinear
specular           face:wet|64
shininess          32
detail             26
```

`MOOSEMATERIAL 2`, then a setting a line (a `#` starts a comment), each set once:

| Setting | Meaning |
| --- | --- |
| `shader NAME` | The program that draws it (required). |
| `textureN SOURCE` | Slot N's texture (0 to 7; a program reads the slots it declares): a file in `assets/textures/`, `cube:NAME` for a cube map (`NAME_px.png` to `NAME_nz.png`), or `@...`, a texture the engine makes (below). |
| `filterN SAMPLER` | How slot N is read: `nearest` (`nearest_mipmap_nearest`), `bilinear` (`bilinear_mipmap_nearest`), `trilinear` (`bilinear_mipmap_linear`, the default), `dithered` (`dithered_mipmap_nearest`), or any full sampler name such as `nearest_mipmap_none`. The artist's choice: a surface can override it (see Binding), and no player setting changes it. Picked at run time, per polygon: no shader copies. |
| `INPUT SOURCE` | An input the program reads by name, and where its value comes from (below). |
| `variant.SCENARIO MATERIAL` | Another material to draw with in a scenario (below). |
| `translucent` | Drawn translucent, after opaque geometry (for programs that can be both). Read, but no registered program can be both yet: translucency comes with the program (`*_translucent`). |

There are no feature words (the old `bump=normal`): a material supplies textures and values, and the program decides what to do with them. `lit` bumps where its normal map slot is filled and shines where its `specular` is above 0.

Version 1 (one shared file of rows, `materials N`) is refused; `tools/split_materials.py FILE` splits one into version 2 files, each row's comments at the top of its own file (`standard.mmat` became the 20 test materials' files that way).

## Inputs and their sources

Each input has a **type**, which says how its values are written (in the material, in a fallback, and in the meta values it reads) and what the shader gets:

| Type | Written | The shader gets |
| --- | --- | --- |
| color | three whole numbers 0-255 (`204,51,51`), or one for gray | each over 255 |
| unit (0-255) | one whole number 0-255: a strength, an opacity | it over 255 |
| number | one number as it is: meters, a power, a scale | it |

An input's **source**, and the rate it is read at (no value is read more often than it can change):

| Source | Written | Read |
| --- | --- | --- |
| The material's | `64`, `204,51,51`, `2.5` | once |
| A player setting | `setting` (the input's own name) or `setting:NAME`: `reflectance`, `fade` | once a frame |
| The level's meta value | `level:KEY` | once a frame |
| A named light | `light:NAME`: its color now for a color input, how bright that is (its brightest channel) otherwise; black or 0 while it's off | once a frame |
| A face's, sector's or entity's meta value | `face:KEY`, `sector:KEY`, `entity:KEY` (the entity's own, else its template's) | per polygon drawn |

A read can end in `|VALUE`, written in the input's type: what to use where there is no such value (on a polygon without a face, say, or with a key nothing sets). Without one, 0. Meta values are `$KEY=VALUE` options in the level (see the Level Format Spec); scripts will change them, and materials see the change the next frame. Keys are interned when the level loads, so a polygon's read is an index, not a string.

With brick's `specular face:wet|64`, a surface row with `$wet=200` makes that wall shinier; every other is 64.

### Engine inputs

Inputs the engine reads, not the program: the same name and meaning on every program that takes one, with the same sources. A program opts in to those that mean something for it (the registry's `engine`), so other materials don't carry them; on a program that doesn't take one, it is an error saying which do. They follow the program's own inputs, and the shader never sees them (they aren't in its params).

| Input | Type | Meaning |
| --- | --- | --- |
| `back_light` (`lit`, `basic_bumpy`, `random_sections`, `reflective_bumpy`) | number, degrees 0 to 90 (a fixed value past 90 is refused; a read is clamped) | How far behind a face its lights may be and still light it, for a normal map whose bumps turn past the face's plane: a curb's rounded corner catches a light just behind its face. 0 (the default): none. The engine keeps those lights for the polygon and eases them into its bumps (see the Lighting Spec); a flat face looks the same. It goes to the polygon's `Surface::back_light` as the angle's sine. |

`plain_curb back_light 20` keeps the corner lit as the flashlight passes over the curb's plane; `back_light face:corner|0` sets it face by face.

Things every program of a kind needs and that have one source (the eye, the object's transform, the lights reaching the polygon and their shadows, the pixel's place, time) stay in the stages' contexts, not inputs.

## Seen in a mirror

A program is compiled in a copy for polygons seen in a mirror (`Material::MIRRORED`), which can do less: its `material_io!` marks values `if !MIRRORED`, which that copy's layout drops (never computed between stages, stored or interpolated), and its stages' branches on it compile out. `basic_bumpy`'s (a road's: diffuse and bumps only) draws no bumps; `lit`'s mirrored copy draws neither bumps nor a highlight and carries 6 interpolated values instead of 27 (about 5% less raster time with brick in the shiny floors at 1080p). Programs without one draw the same in a mirror. This replaced the material's Coarse slot (Oct 6), which would have needed a second program per material.

Distance dithering between quality levels is shelved (Oct 6): two programs don't share their earlier stages, so a pixel can't cheaply switch between them. If it comes back, it will be one program with two pixel shaders (high and low quality).

## Textures the engine makes

Slots can name them like files, as `@NAME` (or `@NAME:FILE` for one made from a file): `@water:FILE`, `FILE` rippled (rebuilt as the ripples step, so a water pixel reads one texel, as Half-Life's software renderer did; a program could instead read `FILE` at UVs it bends itself, two reads and some math a pixel); `@water_heights`, the ripples' heights; `@cube`, the entity's own cube map, baked at load. Render targets (monitors) and decals would be more.

## Scenarios (variants)

A material can name others to use in particular cases. This replaces the app's special rules:

| Scenario | When | Replaces |
| --- | --- | --- |
| `reflected` | A reflective surface whose reflection was drawn under it: it is drawn over it. | The app's Fresnel-or-plain choice for shiny floors. |
| `water` | The Water floors setting is on. | The app's water rule. |
| `translucent` | The Translucent crates setting is on. | The app's translucent crates rule. |
| `simple_sky` | The Sky box setting is off. | The app's flat-sky rule. |

Settings that swap looks (Water floors, Translucent crates) are scenarios, so a material decides what they mean for it. A material without the variant ignores the setting. Scenarios apply in the order `water`, `translucent`, `simple_sky` (the frame's settings), then `reflected` (the polygon's), each to the material the one before chose: `metal_floor_shiny`'s water variant is `water_floor`, whose reflected variant is `water`. (Seen in a mirror was one, `in_mirror`; it is the program's mirrored copy now.)

## Settings

- **Inputs:** one written `setting` takes the player's setting of its name (reflectance, reflection fade).
- **Samplers** are the material's and the surface's, never a setting: the engine owns no texture filtering choice.
- **Quality:** the player's Bump mapping (`--bump off|normal`) and Specular highlights (`--specular on|off`) settings choose among a program's compiled copies, as the program says (`lit`: no bumps, or no highlight); materials don't name them.
- **Presets** (planned): Low, Medium and High set the individual settings; fine settings stay available.

## Binding

- **Level surfaces:** a `material=NAME` option at the end of a surface row (after its vertices). Portal surfaces take none.
- **Templates and entities:** a `material=NAME` option: the whole model drawn with it. Models keep their vertex colors when they name none (the `vertex_color` material).
- **Sampler overrides:** a surface, template or entity can also take `filterN=SAMPLER`, over its material's for that slot: one face of a wall pixel-sharp, say, without a material of its own. Resolution: the surface's, else the material's, else trilinear.
- **Models** (planned): per polygon, in `.mmdl`, for models with several materials.
- **Sky surfaces:** a `sky` material (the unlit vertex colors), named like any other.
- **No default rules.** Existing levels are rewritten once with the materials their surfaces get today, so nothing changes on screen and the app's rules can go.

## The shader registry

Each program is registered under a name with what it reads, so materials are checked when they load (an unknown shader, slot or input, or a value that doesn't fit its type, is an error with the file and line):

| Name | Program | Textures | Inputs |
| --- | --- | --- | --- |
| `vertex_color` | `VertexColor` | none | |
| `sky` | `UnlitColor` | none | |
| `vertex_color_translucent` | `VertexColorTranslucent` | none | `opacity` (unit) |
| `vertex_color_fresnel` | `VertexColorFresnel` | none | `reflectance` (unit) |
| `textured` | `Textured` | color | `detail` (unit) |
| `textured_translucent` | `TexturedTranslucent` | color | `opacity` (unit) |
| `textured_fresnel` | `TexturedFresnel` | color | `reflectance` (unit), `fade` (number, m) |
| `water` | `Water` | color, heights | `reflectance` (unit), `fade` (number, m), `shift` (number) |
| `lit` | `TexturedLit` | color, normal map | `specular` (unit), `shininess` (number), `detail` (unit) |
| `basic_bumpy` | `BasicBumpy` | color, normal map | `short`, `far` (numbers, m: bumps whole out to `short`, flat by `far`) |
| `random_sections` | `RandomSections` | color, normal map (each four sections side by side, 4:1) | `short`, `far` (as `basic_bumpy`'s), `seed` (a whole number) |
| `reflective_bumpy` | `ReflectiveBumpy` | color (alpha: where it's wet, 255 water), normal map, turbulence (grey; `@water_heights` animates) | `short`, `far` (as `basic_bumpy`'s), `reflectance` (unit), `fade` (number, m), `shift` (number), `ripple`, `ripple_size` (numbers, repeats), `ripple_speed` (number, waves a second), `drift` (number, repeats a second) |
| `cube_reflection` | `CubeReflection` | cube (`@cube`) | (the engine's: `radius`, `size`) |
| `terrain_detail` | `VertexColorDetail` | detail | |
| `terrain_noise` | `VertexColorNoise` | none | `strength` (unit) |
| `sky_box` | `SkyBox` | cube (RGBM) | (the engine's: the sun) |

`random_sections` is `basic_bumpy` for a texture of four sections side by side that tile with each other in any order (a sidewalk's slabs), mapped one section to a whole repeat of `uv` (1:1). Per pixel, the repeat it is in (its cell: the whole parts of `uv`, in 16.16 `u >> 16`, `v >> 16`) picks a section, 0 to 3, by an integer hash of the cell and `seed`; the texture (and normal map, laid out the same way) is read at `u = (section << 16 | fraction) >> 2`. Its level of detail is measured from `u / 4`, so it is as sharp as a plain texture. The pick depends only on the cell, so it never changes from frame to frame, and stitched surfaces carry the layout on across the seam; a different `seed` lays the same surfaces out anew. Filtering can reach a few texels into the next section at a section's edge, which is unseen when sections tile with each other; nearest filtering, or padding, takes it away.

`reflective_bumpy` is `basic_bumpy` with puddles, on one material with no variants: its color's alpha says where it's wet (255 water, 0 dry, between them the shore). On a reflective surface (flag `0x1`) with reflections on, the engine draws the reflection under it, and it is drawn over that: where it's wet the reflection shows by Fresnel (`reflectance`, F0; `fade`, as `textured_fresnel`'s), shifted along the screen's rows by the turbulence (its blue byte from 128, `shift` as `water`'s, slid by `drift` repeats a second). Without a reflection under it (reflections off, a surface not marked reflective) it is drawn opaque: the same puddles, no reflection; seen in a mirror, textured and lit only. Where it's wet, as much as it is, the ground under the water wobbles (its texture read again at a `uv` moved by two crossing smooth waves, `ripple` repeats at most, `ripple_size` repeats long, `ripple_speed` a second: no texture or copy of its own) and the bumps flatten; the wetness is read where the pixel really is, so the shore stays still. It reads its color where the pixel is and, where it's wet, again where the ripples move it, the normal map, and the turbulence where it's wet and over a reflection. A color texture with an opaque alpha (255 everywhere) is all water.

Inputs are passed to the shader in its order (`Params::values`, a color taking three). The registry also says which mesh attributes each program reads (`color`, `uv`, `normal`); a mesh without them is drawn with its vertex colors, and any attribute a mesh lacks (its vertex colors too) reads 0: a material never stops a frame, it draws black at worst. `lit` with neither bumps nor a highlight is drawn by `textured` with its `detail` (the plain bricks' detail noise); with only the highlight, its normal map isn't read. Its `shininess` goes to the shader as a number of squarings (log2, rounded). The registry is `SHADERS` in `crates/moose-app/src/materials.rs`.

## Editor

The surface panel has a Material field (cycling none and the loaded materials), and the entity panel one too, for props, actors and terrains (an entity with none takes its template's). Material files hot-reload like textures: a change is read, checked, and the level rebuilt with it; if it is refused, the materials stay as they were and the editor says why.

## The material creator

Esc > Materials lists the materials (and New material); Enter opens one on the Material page, which saves it as its own file:

- **Name** (Enter types it): also its file's, `NAME.mmat`. A material from a file, renamed, is saved as a new one in a file of its own (a copy), so what names the old one keeps it. A new one can't take a name that's used.
- **Shader**: Left/Right step through the registry.
- **Texture N** (one per slot the shader reads) and its **sampler**: Left/Right step through the files in `assets/textures/` (a cube map's six faces as one `cube:NAME`), then the textures the engine makes (`@cube`, `@water_heights`, `@water:FILE` for each file); and the samplers (the default, trilinear, or one of the twelve). Enter types a texture.
- **Input: NAME (type)**, one per input, then **Engine: NAME** for the engine inputs its shader takes (`back_light` on the bump programs), which take sources the same way. Left/Right step its source: unset (0), a number, the player's setting (where there is one), or a read from a face, sector, entity, the level or a light. Under it, Enter types its **value** (the number, the setting's name, or a read's key or light's name, free: a key the level loaded has nowhere shows "not in this level", which isn't an error, as a script may set it), and for a read, **where there's none** (its fallback). A read starts with the input's name as its key and its number as its fallback; stepping back to a number gets the number back. Each is checked against the input's type.
- **Save**: the material is checked with all the others, as they would be with it, before its file is written; refused, the files stay as they were and the page says why. Saved, it is drawn with at once. The comment lines at the top of its file are kept; the rest is written anew.

What its shader doesn't read (after switching shaders) is kept on the page but not saved. Variants and `translucent` are kept as they were; the page doesn't change them yet. There is no Delete: a material some level still uses would break it.

## The default material

`default` (`default.mmat`): `textured` with `default.png`, Moose v1's default texture (`get_dflt_bitmap` in v1's `src/textures.rs`; `tools/gen_default_texture.py` writes it): 16x16, gray-green with an orange top row and left column, a grid where it repeats. New levels (Esc > Levels > New level) are drawn with it, and new materials start from it.

## As built

- **Loader** (`moose_assets::MaterialLibrary`, `assets/materials/*.mmat`, read before the first level): one material a file, named by it, its settings parsed. Levels keep bindings (`Level::bindings`, a material and its sampler overrides), and each polygon the index of its own (`Polygon::material`).
- **Registry and resolution** (`crates/moose-app/src/materials.rs`): materials are checked against the registry when the app starts, with the file and line of what's wrong, and their textures loaded. Each frame a table of resolved `Surface`s is made per material, reflected or not and in a mirror or not, from the frame's settings; polygons look theirs up.
- **Migration:** `tools/materials_rule.py` wrote the old rules' materials into the levels, and the level generators run their levels through it. Checked pixel-identical against the rules on every test level, with water, translucent crates, the flat sky and plain bricks.
- **Removed:** the app's rules, its texture tables, and the settings that chose textures and samplers: Texture filter (`--filter`), Floor texture, Bump sampler (`--bump-sampler`), Shininess (`--shininess`), Terrain detail (`--terrain-detail`, now the `terrain_noise` material). Specular is on/off (`--specular on|off`); its strength is the material's.
- **Changed:** with Translucent crates on, only materials with a `translucent` variant turn translucent (the crate); other props used to as well.
- **Inputs and sources (Oct 6):** typed inputs (`InputType`), sources checked at load; the level's, settings' and lights' read once a frame (`Materials::table`), faces', sectors' and entities' per polygon (`Table::surface`, from the polygon's `Place`). The values live in the world (`World::meta`, `faces`, `Sector::meta`, `Entity::meta`), for scripts to change. Unit values were 0-1 numbers before: `standard.mmat`'s became 0-255 (0.25 to 64), at most one level off on screen.
- **Mirrored copies (Oct 6):** `lit`'s; the Coarse slot, feature words (`bump=`) and the `in_mirror` scenario are gone. Texture slots went from 2 to 8.
- **One a file (Oct 6):** version 2, the creator writing a material's own file; `standard.mmat` split by `tools/split_materials.py`, pixel-identical on every test level.
- **Engine inputs (Oct 7):** `back_light` (`EngineInput` in materials.rs), taken by the programs that bump (`ShaderInfo::engine`), resolved with the material's values and split off before its params.
- **Not built:** per-level materials, per-polygon materials in models, presets, translucency as an option, vertex-rate inputs (a mesh attribute an input names, carried by the program as a varying).

## Decisions

- One library of materials for all levels (`assets/materials/`), not per level, for now.
- A material is one file, named by its file; the program is what is reused.
- Settings that swap looks become scenarios (`variant.water`, `variant.translucent`), so materials own their meaning.
- A material supplies textures and values; what a program does with them (bumps, highlights, which channel holds a mask) is the program's. No feature words.
- Inputs by name, typed; colors and unit values written as u8 (a GPU loader would convert them).
- Sources at their rates: the material's, a setting, the level's or a light's once a frame; a face's, sector's or entity's per polygon. Engine truths (eye, lights, time) stay in the contexts.
- One program per material, compiled in a copy for polygons seen in a mirror, whose layout drops what it doesn't use.
- Samplers set by materials and overridden by surfaces; no engine or player setting for them. The sampler code goes with the shader library (into its own crate, when that is split from the rasterizer).
- Texture paths in materials; runtime textures as `@name`.
