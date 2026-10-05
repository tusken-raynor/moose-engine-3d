# Material Format Spec (.mmat, version 1)

Oct 1, 2026 · Built Oct 5 (branch `materials`). Where the build differs from the proposal, the spec says what was built; see As built at the end.

## Overview

A **material** says how a surface looks: which shader draws it, with which textures, read how, and with which numbers. Before this, that was decided in Rust, by rules in the app (`App::draw`): floors got the floor texture, walls their sector's texture, shiny floors Fresnel or water, brick walls their bump shader, crates the crate texture. Changing a look meant changing code.

With this spec, looks are data. Shaders are code (`moose_raster::shaders`), registered with the engine under names; materials are text files that pick a shader and fill in what it reads; levels, templates and entities name the materials their surfaces use. The engine binds what the data says, and the player's settings choose between what the material allows.

What stays code: the shaders themselves, and the engine's special cases (shadows, the flashlight's beam, where lighting is sampled).

## Concepts

- **Shader:** a registered material program (see the Material Pipeline Spec) under a name, with what it reads: its texture slots, its params by name, and its features, the options compiled into it (see Settings). One shader name can stand for several compiled configurations: `lit` is `TexturedLit<BUMP, SPECULAR>` in each combination.
- **Material:** a name, a shader, textures for its slots, a sampler policy per slot, param values, features, and variants (other materials to use in particular scenarios).
- **Binding:** a level surface, a template or an entity names the material it is drawn with.
- **Resolution:** each frame, for each polygon, the engine turns its material, the scenario (is its reflection drawn under it, is it seen in a mirror) and the player's settings into a `Surface`: one compiled shader configuration, its textures, samplers and params. That is what the app's rules do today, but read from the data.

## Files

Materials live in `assets/materials/`, one or more `.mmat` files, all read at startup; names are unique across them. A level can also have its own `materials` section, same rows, for materials only it uses (planned, not in version 1). The materials the test levels use are in `assets/materials/standard.mmat`.

```
MOOSEMATERIAL 1

materials 4
#  id  name          shader    [options]
   0   metal_floor   textured  texture0=metal_tile.png variant.reflected=metal_fresnel
   1   metal_fresnel textured_fresnel  texture0=metal_tile.png reflectance=setting fade=setting
   2   brick         lit       texture0=brick_wall.png texture1=brick_wall_normal.png filter1=bilinear detail=0.1 bump=normal specular=0.25 shininess=32
   3   sky           sky_box   texture0=cube:sky filter0=bilinear_mipmap_none variant.simple_sky=sky_flat
```

Rows are `id name shader`, then options, in the level format's style (tokens, `#` comments, counts and ids):

| Option | Meaning |
| --- | --- |
| `textureN=FILE` | Slot N's texture, from `assets/textures/`. `cube:NAME` is a cube map, `NAME_px.png` to `NAME_nz.png`. `@...` is a texture the engine makes at run time: `@water:FILE` (`FILE` under the rippling water), `@water_heights` (the ripples' heights), `@cube` (the entity's own cube map, baked at load). |
| `filterN=SAMPLER` | How slot N is read: `nearest` (`nearest_mipmap_nearest`), `bilinear` (`bilinear_mipmap_nearest`), `trilinear` (`bilinear_mipmap_linear`, the default), `dithered` (`dithered_mipmap_nearest`), or any full sampler name such as `nearest_mipmap_none`. The artist's choice: a surface can override it (see Binding), and no player setting changes it. Picked at run time, per polygon: no shader copies. |
| `NAME=VALUE` | A param the shader reads by name (`detail`, `specular`, `shininess`, `reflectance`, `fade`, ...): a number, or `setting` for the player's setting of that name. |
| `bump=normal` / `specular=S` | Features (below): what the material can do. Off by default. `specular` is a param of `lit`: above 0 is the feature. |
| `variant.SCENARIO=MATERIAL` | Another material to draw with in a scenario (below). |
| `translucent` | Drawn translucent, after opaque geometry (for shaders that can be both). Read, but no registered shader can be both yet: translucency comes with the shader (`*_translucent`). |

## Scenarios (variants)

A material can name others to use in particular cases. This replaces the app's special rules, and is the Material Pipeline Spec's planned variants per scenario:

| Scenario | When | Replaces |
| --- | --- | --- |
| `reflected` | A reflective surface whose reflection was drawn under it: it is drawn over it. | The app's Fresnel-or-plain choice for shiny floors. |
| `in_mirror` | The polygon is seen in a mirror: a cheaper material. | The planned reflection variants. |
| `water` | The Water floors setting is on. | The app's water rule. |
| `translucent` | The Translucent crates setting is on. | The app's translucent crates rule. |
| `simple_sky` | The Sky box setting is off. | The app's flat-sky rule. |

Settings that swap looks (Water floors, Translucent crates) are scenarios, so a material decides what they mean for it. A material without the variant ignores the setting. Scenarios apply in the order `water`, `translucent`, `simple_sky` (the frame's settings), then `reflected`, `in_mirror` (the polygon's), each to the material the one before chose: `metal_floor_shiny`'s water variant is `water_floor`, whose reflected variant is `water`.

## Settings

- **Numbers:** a param written `setting` takes the player's setting of that name (reflectance, reflection fade). Any other value is the artist's.
- **Samplers** are the material's and the surface's, never a setting: the engine owns no texture filtering choice. The global Texture filter setting (and `--filter`) goes.
- **Features:** a material declares what it can do (`bump=normal`, `specular=S`); the player's settings say what's on (Bump mapping, `--bump off|normal`, and Specular highlights, `--specular on|off`; both on by default). The configuration drawn is what both allow: the shader's compiled copy for that combination. A plain wall declares nothing and is never bumped. Features are the only thing compiled into copies, so they should be few, and on/off.
- **Presets** (planned): Low, Medium and High set the individual settings; fine settings stay available.

## Binding

- **Level surfaces:** a `material=NAME` option at the end of a surface row (after its vertices). Portal surfaces take none.
- **Templates and entities:** a `material=NAME` option: the whole model drawn with it. Models keep their vertex colors when they name none (the `vertex_color` material).
- **Sampler overrides:** a surface, template or entity can also take `filterN=SAMPLER`, over its material's for that slot: one face of a wall pixel-sharp, say, without a material of its own. Resolution: the surface's, else the material's, else trilinear.
- **Models** (planned): per polygon, in `.mmdl`, for models with several materials.
- **Sky surfaces:** a `sky` material (the unlit vertex colors), named like any other.
- **No default rules.** Existing levels are rewritten once with the materials their surfaces get today, so nothing changes on screen and the app's rules can go.

## The shader registry

Each shader is registered under a name with what it reads, so materials are checked when they load (an unknown shader, slot, param or feature is an error with the file and line):

| Name | Shader | Textures | Params | Features |
| --- | --- | --- | --- | --- |
| `vertex_color` | `VertexColor` | none | | |
| `sky` | `UnlitColor` | none | | |
| `vertex_color_translucent` | `VertexColorTranslucent` | none | `opacity` | |
| `vertex_color_fresnel` | `VertexColorFresnel` | none | `reflectance`, `fade` | |
| `textured` | `Textured` | color | `detail` | |
| `textured_translucent` | `TexturedTranslucent` | color | `opacity` | |
| `textured_fresnel` | `TexturedFresnel` | color | `reflectance`, `fade` | |
| `water` | `Water` | color, heights | `reflectance`, `fade`, `shift` | |
| `lit` | `TexturedLit` | color, normal map | `specular`, `shininess`, `detail` | `bump=normal`, `specular` |
| `cube_reflection` | `CubeReflection` | cube (`@cube`) | (the engine's: `radius`, `size`) | |
| `terrain_detail` | `VertexColorDetail` | detail | | |
| `terrain_noise` | `VertexColorNoise` | none | `strength` | |
| `sky_box` | `SkyBox` | cube (RGBM) | (the engine's: the sun) | |

Params are named in the registry and passed to the shader in its order (`Params::values`), so shaders don't change. The registry also says which mesh attributes each shader reads (`color`, `uv`, `normal`); a mesh without them is drawn with its vertex colors. `lit` with neither feature on is drawn by `textured` with its `detail` (the plain bricks' detail noise); with only the highlight, its normal map isn't read. Its `shininess` goes to the shader as a number of squarings (log2, rounded). The registry is `SHADERS` in `crates/moose-app/src/materials.rs`.

## Editor

The surface panel has a Material field (cycling none and the loaded materials), and the entity panel one too, for props, actors and terrains (an entity with none takes its template's). Material files hot-reload like textures: a change is read, checked, and the level rebuilt with it; if it is refused, the materials stay as they were and the editor says why.

## As built

- **Loader** (`moose_assets::MaterialLibrary`, `assets/materials/*.mmat`, read before the first level): rows and options parsed, names unique. Levels keep bindings (`Level::bindings`, a material and its sampler overrides), and each polygon the index of its own (`Polygon::material`).
- **Registry and resolution** (`crates/moose-app/src/materials.rs`): materials are checked against the registry when the app starts, with the file and line of what's wrong, and their textures loaded. Each frame a table of resolved `Surface`s is made per material, reflected or not and in a mirror or not, from the frame's settings; polygons look theirs up.
- **Migration:** `tools/materials_rule.py` wrote the old rules' materials into the levels, and the level generators run their levels through it. Checked pixel-identical against the rules on every test level, with water, translucent crates, the flat sky and plain bricks.
- **Removed:** the app's rules, its texture tables, and the settings that chose textures and samplers: Texture filter (`--filter`), Floor texture, Bump sampler (`--bump-sampler`), Shininess (`--shininess`), Terrain detail (`--terrain-detail`, now the `terrain_noise` material). Specular is on/off (`--specular on|off`); its strength is the material's.
- **Changed:** with Translucent crates on, only materials with a `translucent` variant turn translucent (the crate); other props used to as well.
- **Not built:** per-level materials, per-polygon materials in models, presets, translucency as an option.

## Decisions

- One library of materials for all levels (`assets/materials/`), not per level, in version 1.
- Settings that swap looks become scenarios (`variant.water`, `variant.translucent`), so materials own their meaning.
- Features are on/off per material and per setting; their combinations are the only compiled copies.
- Params by name, `setting` for the player's value.
- Samplers set by materials and overridden by surfaces; no engine or player setting for them. The sampler code goes with the shader library (into its own crate, when that is split from the rasterizer).
- Texture paths in materials; runtime textures as `@name`.
