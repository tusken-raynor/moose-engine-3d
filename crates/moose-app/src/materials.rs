//! Materials as data, resolved for drawing (see `Material Format Spec.md`): the registry of
//! shader programs materials can name (what each reads: texture slots, and typed inputs),
//! materials checked against it and their textures loaded, and each frame, for each
//! material and scenario, the `Surface` it is drawn with under the player's settings.
//!
//! An input's value comes from its source at the rate it can change: the material's own
//! numbers, a player setting, the level's meta values or a named light (once a frame, for
//! every polygon), or a face's, sector's or entity's meta values (per polygon drawn).

use std::collections::HashMap;

use moose_assets::{
    Assets, Binding, MATERIAL_SLOTS, MaterialLibrary, MetaValues, ParamValue, Ripples, Scope, Texture, TextureId,
    TextureSource,
};
use moose_raster::shaders::{
    BasicBumpy, BasicBumpyMirrored, CubeReflection, RandomSections, RandomSectionsMirrored, ReflectiveBumpy,
    ReflectiveBumpyMirrored, ReflectiveBumpyReflected, SkyBox, Textured, TexturedFresnel, TexturedLitMirrored, TexturedNormal, TexturedNormalSpecular,
    TexturedSpecular, TexturedTranslucent, UnlitColor, VertexColor, VertexColorDetail, VertexColorFresnel,
    VertexColorNoise, VertexColorTranslucent, Water, filter,
};
use moose_raster::{MAX_TEXTURES, MaterialId, Params, Renderer, Surface};
use moose_scene::World;

/// What a registered shader program is, for resolving it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    VertexColor,
    Sky,
    VertexColorTranslucent,
    VertexColorFresnel,
    Textured,
    TexturedTranslucent,
    TexturedFresnel,
    Water,
    /// `TexturedLit`: bumps where the material gives a normal map, and a highlight where
    /// its specular is above 0, as the player's settings allow; neither in a mirror.
    Lit,
    /// `BasicBumpy`: diffuse, bumped where the material gives a normal map and the player's
    /// setting allows; flat in a mirror.
    BasicBumpy,
    /// `RandomSections`: `BasicBumpy` with each repeat one of four sections, picked at random.
    RandomSections,
    /// `ReflectiveBumpy`: `BasicBumpy` with puddles where its color's alpha says, reflecting
    /// over the reflection drawn under it.
    ReflectiveBumpy,
    CubeReflection,
    TerrainDetail,
    TerrainNoise,
    SkyBox,
}

/// What an input holds, which says how it is written (in a material, a fallback or a
/// meta value) and what the shader gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputType {
    /// Three u8 (`204,51,51`, or one for gray); the shader gets them over 255.
    Color,
    /// One u8, 0 to 255 for 0 to 1 (a strength, an opacity); the shader gets it over 255.
    Unit,
    /// A number as it is (meters, a power, a scale).
    Number,
}

impl InputType {
    /// How many floats the shader gets.
    pub fn len(self) -> usize {
        match self {
            InputType::Color => 3,
            _ => 1,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            InputType::Color => "color",
            InputType::Unit => "0-255",
            InputType::Number => "number",
        }
    }

    /// Numbers as written, as the shader gets them (missing ones 0; one for a color is gray).
    fn convert(self, written: &[f32], out: &mut [f32]) {
        match self {
            InputType::Color => {
                for (k, o) in out.iter_mut().enumerate().take(3) {
                    let v = if written.len() == 1 { written[0] } else { written.get(k).copied().unwrap_or(0.0) };
                    *o = v / 255.0;
                }
            }
            InputType::Unit => out[0] = written.first().copied().unwrap_or(0.0) / 255.0,
            InputType::Number => out[0] = written.first().copied().unwrap_or(0.0),
        }
    }

    /// Whether numbers as written fit the type.
    pub fn check(self, written: &[f32]) -> Result<(), String> {
        let u8s = |n: &[f32]| n.iter().all(|&x| (0.0..=255.0).contains(&x) && x.fract() == 0.0);
        match self {
            InputType::Color if (written.len() == 1 || written.len() == 3) && u8s(written) => Ok(()),
            InputType::Color => Err("a color is three whole numbers 0-255 (204,51,51), or one for gray".into()),
            InputType::Unit if written.len() == 1 && u8s(written) => Ok(()),
            InputType::Unit => Err("this is one whole number, 0-255".into()),
            InputType::Number if written.len() == 1 => Ok(()),
            InputType::Number => Err("this is one number".into()),
        }
    }
}

/// An input a program reads, by name.
pub struct Input {
    pub name: &'static str,
    pub ty: InputType,
}

const fn input(name: &'static str, ty: InputType) -> Input {
    Input { name, ty }
}

/// A program materials can name: its texture slots, its inputs (in the order it reads
/// them; `cube_reflection`'s and `sky_box`'s the engine fills in), the engine inputs it
/// takes, and the mesh attributes it reads.
pub struct ShaderInfo {
    pub name: &'static str,
    pub kind: Kind,
    pub slots: &'static [&'static str],
    pub inputs: &'static [Input],
    pub engine: &'static [EngineInput],
    pub attribs: &'static [&'static str],
}

/// An input the engine reads, not the program: the same name and meaning on every
/// program that takes it (those it means something for, which opt in), after the
/// program's own inputs (as input numbers and among a material's values). The shader
/// never sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineInput {
    /// How far behind a face its lights may be and still light it, in degrees (0, none, to
    /// 90): for a normal map whose bumps turn past the face's plane, a curb's rounded
    /// corner say, which a light just behind the face lights. For programs that bump. See
    /// `Surface::back_light`.
    BackLight,
}

static BACK_LIGHT: Input = input("back_light", Number);

impl EngineInput {
    pub fn input(self) -> &'static Input {
        match self {
            EngineInput::BackLight => &BACK_LIGHT,
        }
    }
}

impl ShaderInfo {
    /// Its inputs, then the engine inputs it takes.
    pub fn all_inputs(&self) -> impl Iterator<Item = &'static Input> {
        self.inputs.iter().chain(self.engine.iter().map(|e| e.input()))
    }

    /// Input `k` among [`ShaderInfo::all_inputs`], and whether it is the engine's.
    pub fn input(&self, k: usize) -> (&'static Input, bool) {
        match self.inputs.get(k) {
            Some(input) => (input, false),
            None => (self.engine[k - self.inputs.len()].input(), true),
        }
    }

    /// Where input `k`'s floats start among the material's values (the shader's params,
    /// then the engine's).
    fn offset(&self, k: usize) -> usize {
        self.all_inputs().take(k).map(|i| i.ty.len()).sum()
    }

    fn params_len(&self) -> usize {
        self.offset(self.inputs.len())
    }

    fn values_len(&self) -> usize {
        self.offset(self.inputs.len() + self.engine.len())
    }
}

use EngineInput::BackLight;
use InputType::{Number, Unit};

/// The registry (see the Material Format Spec).
pub const SHADERS: [ShaderInfo; 16] = [
    ShaderInfo { name: "vertex_color", kind: Kind::VertexColor, slots: &[], inputs: &[], engine: &[], attribs: &["color"] },
    ShaderInfo { name: "sky", kind: Kind::Sky, slots: &[], inputs: &[], engine: &[], attribs: &["color"] },
    ShaderInfo { name: "vertex_color_translucent", kind: Kind::VertexColorTranslucent, slots: &[], inputs: &[input("opacity", Unit)], engine: &[], attribs: &["color"] },
    ShaderInfo { name: "vertex_color_fresnel", kind: Kind::VertexColorFresnel, slots: &[], inputs: &[input("reflectance", Unit)], engine: &[], attribs: &["color"] },
    ShaderInfo { name: "textured", kind: Kind::Textured, slots: &["color"], inputs: &[input("detail", Unit)], engine: &[], attribs: &["uv"] },
    ShaderInfo { name: "textured_translucent", kind: Kind::TexturedTranslucent, slots: &["color"], inputs: &[input("opacity", Unit)], engine: &[], attribs: &["uv"] },
    ShaderInfo { name: "textured_fresnel", kind: Kind::TexturedFresnel, slots: &["color"], inputs: &[input("reflectance", Unit), input("fade", Number)], engine: &[], attribs: &["uv"] },
    ShaderInfo { name: "water", kind: Kind::Water, slots: &["color", "heights"], inputs: &[input("reflectance", Unit), input("fade", Number), input("shift", Number)], engine: &[], attribs: &["uv"] },
    // Bumps where the normal map slot is filled; a highlight where specular is above 0.
    ShaderInfo { name: "lit", kind: Kind::Lit, slots: &["color", "normal map"], inputs: &[input("specular", Unit), input("shininess", Number), input("detail", Unit)], engine: &[BackLight], attribs: &["uv"] },
    // A road: diffuse with bumps, whole out to `short` meters and gone by `far`.
    ShaderInfo { name: "basic_bumpy", kind: Kind::BasicBumpy, slots: &["color", "normal map"], inputs: &[input("short", Number), input("far", Number)], engine: &[BackLight], attribs: &["uv"] },
    // A sidewalk: `basic_bumpy` with each repeat (1:1) one of the 4:1 texture's four
    // sections, picked by a hash of the repeat and `seed` (a whole number).
    ShaderInfo { name: "random_sections", kind: Kind::RandomSections, slots: &["color", "normal map"], inputs: &[input("short", Number), input("far", Number), input("seed", Number)], engine: &[BackLight], attribs: &["uv"] },
    // `basic_bumpy` with puddles where the color's alpha is (255 wet): over a reflection
    // (a reflective surface's), they reflect it by Fresnel, shifted by the turbulence; the
    // ground under them wobbles and the bumps flatten.
    ShaderInfo { name: "reflective_bumpy", kind: Kind::ReflectiveBumpy, slots: &["color (alpha: wet)", "normal map", "turbulence"], inputs: &[input("short", Number), input("far", Number), input("reflectance", Unit), input("fade", Number), input("shift", Number), input("ripple", Number), input("ripple_size", Number), input("ripple_speed", Number), input("drift", Number)], engine: &[BackLight], attribs: &["uv"] },
    ShaderInfo { name: "cube_reflection", kind: Kind::CubeReflection, slots: &["cube"], inputs: &[], engine: &[], attribs: &["normal"] },
    ShaderInfo { name: "terrain_detail", kind: Kind::TerrainDetail, slots: &["detail"], inputs: &[], engine: &[], attribs: &["color", "uv", "normal"] },
    ShaderInfo { name: "terrain_noise", kind: Kind::TerrainNoise, slots: &[], inputs: &[input("strength", Unit)], engine: &[], attribs: &["color", "normal"] },
    ShaderInfo { name: "sky_box", kind: Kind::SkyBox, slots: &["cube"], inputs: &[], engine: &[], attribs: &[] },
];

/// The registered program named `name`.
pub fn shader_info(name: &str) -> Option<&'static ShaderInfo> {
    SHADERS.iter().find(|s| s.name == name)
}

/// The player settings an input can take (`setting`, or `setting:NAME`).
pub const PARAM_SETTINGS: [&str; 2] = ["reflectance", "fade"];

/// The scenarios a material can have variants for, in the order they apply: the settings'
/// first (they hold for the whole frame), then each polygon's. (Seen in a mirror isn't one:
/// programs are compiled in a copy for it, see `ShaderIds`.)
pub const SCENARIOS: [&str; 4] = ["water", "translucent", "simple_sky", "reflected"];
const WATER: usize = 0;
const TRANSLUCENT: usize = 1;
const SIMPLE_SKY: usize = 2;
const REFLECTED: usize = 3;

/// The compiled programs behind the registry's names.
#[derive(Clone, Copy)]
pub struct ShaderIds {
    vertex_color: MaterialId,
    sky: MaterialId,
    vertex_color_translucent: MaterialId,
    vertex_color_fresnel: MaterialId,
    textured: MaterialId,
    textured_translucent: MaterialId,
    textured_fresnel: MaterialId,
    water: MaterialId,
    /// `TexturedLit` with bumps; with a highlight; with both; and its copy seen in a mirror.
    lit: [MaterialId; 3],
    lit_mirrored: MaterialId,
    /// `BasicBumpy`, and its copy seen in a mirror (or with bumps off).
    basic_bumpy: [MaterialId; 2],
    /// `RandomSections`, and its copy seen in a mirror (or with bumps off).
    random_sections: [MaterialId; 2],
    /// `ReflectiveBumpy`: opaque, over its reflection, and seen in a mirror.
    reflective_bumpy: [MaterialId; 3],
    cube_reflection: MaterialId,
    terrain_detail: MaterialId,
    terrain_noise: MaterialId,
    sky_box: MaterialId,
}

impl ShaderIds {
    pub fn register(r: &mut Renderer) -> Self {
        Self {
            vertex_color: r.register_material::<VertexColor>(),
            sky: r.register_material::<UnlitColor>(),
            vertex_color_translucent: r.register_material::<VertexColorTranslucent>(),
            vertex_color_fresnel: r.register_material::<VertexColorFresnel>(),
            textured: r.register_material::<Textured>(),
            textured_translucent: r.register_material::<TexturedTranslucent>(),
            textured_fresnel: r.register_material::<TexturedFresnel>(),
            water: r.register_material::<Water>(),
            lit: [
                r.register_material::<TexturedNormal>(),
                r.register_material::<TexturedSpecular>(),
                r.register_material::<TexturedNormalSpecular>(),
            ],
            lit_mirrored: r.register_material::<TexturedLitMirrored>(),
            basic_bumpy: [r.register_material::<BasicBumpy>(), r.register_material::<BasicBumpyMirrored>()],
            random_sections: [r.register_material::<RandomSections>(), r.register_material::<RandomSectionsMirrored>()],
            reflective_bumpy: [
                r.register_material::<ReflectiveBumpy>(),
                r.register_material::<ReflectiveBumpyReflected>(),
                r.register_material::<ReflectiveBumpyMirrored>(),
            ],
            cube_reflection: r.register_material::<CubeReflection>(),
            terrain_detail: r.register_material::<VertexColorDetail>(),
            terrain_noise: r.register_material::<VertexColorNoise>(),
            sky_box: r.register_material::<SkyBox>(),
        }
    }

    /// Vertex colors, lit: what polygons that name no material are drawn with.
    pub fn plain(&self) -> Surface {
        Surface::new(self.vertex_color)
    }
}

/// A sampler by name: one of the full names in `filter::ALL`, or `nearest`, `bilinear`,
/// `trilinear` or `dithered`.
pub fn sampler(name: &str) -> Option<u8> {
    match name {
        "nearest" => Some(filter::NEAREST_MIPMAP_NEAREST),
        "bilinear" => Some(filter::BILINEAR_MIPMAP_NEAREST),
        "trilinear" => Some(filter::BILINEAR_MIPMAP_LINEAR),
        "dithered" => Some(filter::DITHERED_MIPMAP_NEAREST),
        _ => filter::named(name),
    }
}

/// A binding's sampler overrides, by slot.
pub fn binding_filters(binding: &Binding) -> Result<[Option<u8>; MATERIAL_SLOTS], String> {
    let mut out = [None; MATERIAL_SLOTS];
    for (slot, name) in binding.filters.iter().enumerate() {
        if let Some(name) = name {
            out[slot] = Some(sampler(name).ok_or_else(|| format!("unknown sampler '{name}'"))?);
        }
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tex {
    None,
    Id(TextureId),
    /// The entity's own cube map (`@cube`).
    EntityCube,
}

/// Player settings an input can take.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Setting {
    Reflectance,
    Fade,
}

/// Where a checked input's value comes from.
#[derive(Clone, Debug, PartialEq)]
enum Source {
    /// The material's numbers, as the shader gets them.
    Fixed(Vec<f32>),
    Setting(Setting),
    /// A meta value or a light (by key or name), and what to use without one (as written).
    Read { from: Scope, key: String, fallback: Vec<f32> },
}

struct Compiled {
    info: &'static ShaderInfo,
    textures: [Tex; MATERIAL_SLOTS],
    filters: [u8; MATERIAL_SLOTS],
    /// Per input (the program's order), where its value comes from; unset ones are 0.
    inputs: Vec<Option<Source>>,
    variants: [Option<u32>; SCENARIOS.len()],
}

/// The library checked against the registry, with its textures loaded.
pub struct Materials {
    names: Vec<String>,
    compiled: Vec<Compiled>,
    /// Rippling textures (`@water:FILE`): each and the file it ripples, and the ripples'
    /// heights (`@water_heights`).
    pub waters: Vec<(TextureId, TextureId)>,
    pub heights: Option<TextureId>,
}

impl Materials {
    /// Checks every material `assets` has read against the registry (an unknown shader,
    /// slot, input, value, sampler, scenario or variant is an error naming its file and
    /// line), and loads the textures they name.
    pub fn compile(assets: &mut Assets, ripples: &Ripples) -> Result<Self, String> {
        let library: MaterialLibrary = assets.materials().clone();
        let mut compiled = Vec::with_capacity(library.len());
        let mut waters: Vec<(TextureId, TextureId)> = Vec::new();
        let mut heights = None;
        let mut cubes: HashMap<String, TextureId> = HashMap::new();
        for def in &library.materials {
            let at = |m: String| format!("{}:{}: material '{}': {m}", def.file.display(), def.line, def.name);
            let info = shader_info(&def.shader).ok_or_else(|| {
                at(format!("unknown shader '{}' (known: {})", def.shader, SHADERS.map(|s| s.name).join(", ")))
            })?;
            let mut textures = [Tex::None; MATERIAL_SLOTS];
            for (slot, source) in def.textures.iter().enumerate() {
                let Some(source) = source else { continue };
                if slot >= info.slots.len() {
                    return Err(at(format!("shader '{}' has {} texture slot(s)", info.name, info.slots.len())));
                }
                textures[slot] = match source {
                    TextureSource::File(file) => Tex::Id(assets.load_texture(file).map_err(|e| at(e.to_string()))?),
                    TextureSource::Cube(name) => Tex::Id(match cubes.get(name) {
                        Some(&id) => id,
                        None => {
                            let id = load_cube(assets, name).map_err(at)?;
                            cubes.insert(name.clone(), id);
                            id
                        }
                    }),
                    TextureSource::Runtime { name, file } => match (name.as_str(), file) {
                        ("water", Some(file)) => {
                            let base = assets.load_texture(file).map_err(|e| at(e.to_string()))?;
                            let water = match waters.iter().find(|&&(_, b)| b == base) {
                                Some(&(w, _)) => w,
                                None => {
                                    let w = assets.add_texture(ripples.texture("water", assets.texture(base).base()));
                                    waters.push((w, base));
                                    w
                                }
                            };
                            Tex::Id(water)
                        }
                        ("water_heights", None) => {
                            Tex::Id(*heights.get_or_insert_with(|| assets.add_texture(ripples.heights("water heights"))))
                        }
                        ("cube", None) => Tex::EntityCube,
                        _ => return Err(at(format!("no run-time texture '@{name}' (known: @water:FILE, @water_heights, @cube)"))),
                    },
                };
            }
            let mut filters = [filter::BILINEAR_MIPMAP_LINEAR; MATERIAL_SLOTS];
            for (slot, name) in def.filters.iter().enumerate() {
                if let Some(name) = name {
                    filters[slot] = sampler(name).ok_or_else(|| at(format!("unknown sampler '{name}'")))?;
                }
            }
            let mut inputs = vec![None; info.inputs.len() + info.engine.len()];
            for (name, value) in &def.params {
                let k = info.all_inputs().position(|i| i.name == name).ok_or_else(|| {
                    // An engine input another program takes says which.
                    let takers: Vec<&str> = SHADERS
                        .iter()
                        .filter(|s| s.engine.iter().any(|e| e.input().name == name))
                        .map(|s| s.name)
                        .collect();
                    match takers.is_empty() {
                        true => at(format!("shader '{}' has no input '{name}'", info.name)),
                        false => at(format!("shader '{}' doesn't take '{name}' (only {})", info.name, takers.join(", "))),
                    }
                })?;
                let source = check_source(info.input(k).0.ty, name, value).map_err(|m| at(format!("{name}: {m}")))?;
                if let (true, Source::Fixed(v)) = (name == BACK_LIGHT.name, &source)
                    && !(0.0..=90.0).contains(&v[0])
                {
                    return Err(at(format!("{name}: an angle, 0 to 90 degrees")));
                }
                inputs[k] = Some(source);
            }
            if def.translucent {
                return Err(at(format!("shader '{}' can't be drawn translucent", info.name)));
            }
            compiled.push(Compiled { info, textures, filters, inputs, variants: [None; SCENARIOS.len()] });
        }
        // Variants, now every material has an id.
        for (i, def) in library.materials.iter().enumerate() {
            for (scenario, name) in &def.variants {
                let at = |m: String| format!("{}:{}: material '{}': {m}", def.file.display(), def.line, def.name);
                let k = SCENARIOS
                    .iter()
                    .position(|s| s == scenario)
                    .ok_or_else(|| at(format!("unknown scenario '{scenario}' (known: {})", SCENARIOS.join(", "))))?;
                compiled[i].variants[k] = Some(library.id(name).ok_or_else(|| at(format!("unknown material '{name}'")))?);
            }
        }
        Ok(Self {
            names: library.materials.iter().map(|d| d.name.clone()).collect(),
            compiled,
            waters,
            heights,
        })
    }

    /// The materials' names, by id.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Whether material `id` draws with the entity's own cube map (so it needs one baked).
    pub fn uses_entity_cube(&self, id: u32) -> bool {
        self.compiled[id as usize].textures.contains(&Tex::EntityCube)
    }

    /// Each material's surface this frame, in each per-polygon scenario (its reflection
    /// drawn under it, seen in a mirror): its inputs read from their sources once a frame,
    /// except those a polygon's place gives (face, sector, entity), which [`Table::surface`]
    /// reads for each polygon.
    pub fn table(&self, ids: &ShaderIds, s: &FrameSettings, world: &World) -> Table {
        let mut table = Table { entries: Vec::new(), ids: *ids, bump: s.bump, specular: s.specular, sun: s.sun };
        let entries = (0..self.compiled.len())
            .map(|m| {
                std::array::from_fn(|k| {
                    let (reflected, mirrored) = (k & 1 != 0, k & 2 != 0);
                    let mut id = m;
                    for (scenario, on) in [
                        (WATER, s.water),
                        (TRANSLUCENT, s.translucent),
                        (SIMPLE_SKY, s.simple_sky),
                        (REFLECTED, reflected),
                    ] {
                        if on && let Some(v) = self.compiled[id].variants[scenario] {
                            id = v as usize;
                        }
                    }
                    self.frame_entry(&table, id, (reflected, mirrored), s, world)
                })
            })
            .collect();
        table.entries = entries;
        table
    }

    /// Material `id`'s entry this frame, over its reflection or not, and seen in a mirror
    /// or not (the program's copy for each).
    fn frame_entry(&self, table: &Table, id: usize, (reflected, mirrored): (bool, bool), s: &FrameSettings, world: &World) -> Resolved {
        let c = &self.compiled[id];
        let mut values = vec![0.0; c.info.values_len()];
        let mut per_polygon = Vec::new();
        for (k, source) in c.inputs.iter().enumerate() {
            let Some(source) = source else { continue };
            let (ty, offset) = (c.info.input(k).0.ty, c.info.offset(k));
            let out = &mut values[offset..offset + ty.len()];
            match source {
                Source::Fixed(v) => out.copy_from_slice(v),
                Source::Setting(Setting::Reflectance) => out[0] = s.reflectance,
                Source::Setting(Setting::Fade) => out[0] = s.fade,
                Source::Read { from: Scope::Level, key, fallback } => {
                    let found = world.meta_keys.id(key).and_then(|key| world.meta.get(key));
                    ty.convert(found.unwrap_or(fallback), out);
                }
                Source::Read { from: Scope::Light, key, fallback } => {
                    // Its color now (black while it's off), or how bright that is.
                    let found = world.light_names.iter().position(|n| n.as_deref() == Some(key.as_str()));
                    match found.and_then(|i| s.lights.get(i)) {
                        Some(&color) if ty == InputType::Color => {
                            out.copy_from_slice(&color.min(glam::Vec3::ONE).to_array())
                        }
                        Some(&color) => out[0] = color.max_element(),
                        None => ty.convert(fallback, out),
                    }
                }
                Source::Read { from, key, fallback } => {
                    // Read per polygon; its fallback until then.
                    ty.convert(fallback, out);
                    per_polygon.push(PolygonRead {
                        offset,
                        ty,
                        from: *from,
                        key: world.meta_keys.id(key),
                        fallback: fallback.clone(),
                    });
                }
            }
        }
        let build = Build {
            kind: c.info.kind,
            textures: c.textures,
            filters: c.filters,
            mirrored,
            reflected,
            params: c.info.params_len(),
            engine: c.info.engine,
            values,
        };
        Resolved {
            surface: table.build(&build),
            entity_cube: c.textures.contains(&Tex::EntityCube),
            attribs: c.info.attribs,
            per_polygon: (!per_polygon.is_empty()).then(|| Box::new((build, per_polygon))),
        }
    }
}

/// An input's source checked against its type.
fn check_source(ty: InputType, name: &str, value: &ParamValue) -> Result<Source, String> {
    Ok(match value {
        ParamValue::Fixed(v) => {
            ty.check(v)?;
            let mut out = vec![0.0; ty.len()];
            ty.convert(v, &mut out);
            Source::Fixed(out)
        }
        ParamValue::Setting(setting) => {
            let setting = setting.as_deref().unwrap_or(name);
            if ty == InputType::Color {
                return Err("no player setting is a color".into());
            }
            Source::Setting(match setting {
                "reflectance" => Setting::Reflectance,
                "fade" => Setting::Fade,
                _ => return Err(format!("no setting '{setting}' (settings: {})", PARAM_SETTINGS.join(", "))),
            })
        }
        ParamValue::Read { from, key, fallback } => {
            if let Some(f) = fallback {
                ty.check(f).map_err(|m| format!("its fallback: {m}"))?;
            }
            Source::Read { from: *from, key: key.clone(), fallback: fallback.clone().unwrap_or_default() }
        }
    })
}

/// The cube map whose faces are `NAME_px.png` ... `NAME_nz.png` (in `CUBE_FACES` order).
fn load_cube(assets: &mut Assets, name: &str) -> Result<TextureId, String> {
    let mut faces: [Vec<u32>; 6] = Default::default();
    let mut size = 0;
    for (face, axis) in faces.iter_mut().zip(["px", "nx", "py", "ny", "pz", "nz"]) {
        let id = assets.load_texture(&format!("{name}_{axis}.png")).map_err(|e| e.to_string())?;
        let base = assets.texture(id).base();
        if size != 0 && base.width != size {
            return Err(format!("cube map '{name}': its faces differ in size"));
        }
        size = base.width;
        *face = base.texels.clone();
    }
    let cube = Texture::cube(name, size, faces)?;
    Ok(assets.add_texture(cube))
}

/// What the player's settings say this frame, for resolving materials.
pub struct FrameSettings {
    pub water: bool,
    pub translucent: bool,
    pub simple_sky: bool,
    pub bump: bool,
    pub specular: bool,
    pub reflectance: f32,
    pub fade: f32,
    /// The sky box's params (see `moose_raster::shaders::sky_box`): the sun's, filled in by
    /// the engine.
    pub sun: Params,
    /// Per level light (in the level's order), its color now: black while it's off.
    pub lights: Vec<glam::Vec3>,
}

/// What a material's surface is made from.
#[derive(Clone)]
struct Build {
    kind: Kind,
    textures: [Tex; MATERIAL_SLOTS],
    filters: [u8; MATERIAL_SLOTS],
    mirrored: bool,
    /// Whether its reflection is drawn under it (the polygon is a reflective surface's,
    /// with reflections on).
    reflected: bool,
    /// How many of `values` are the shader's (the rest are the engine inputs', `engine`'s).
    params: usize,
    engine: &'static [EngineInput],
    /// Its inputs' values, as the shader gets them (the program's order), then the engine
    /// inputs'.
    values: Vec<f32>,
}

/// An input read for each polygon drawn: where its value goes, and from where.
struct PolygonRead {
    offset: usize,
    ty: InputType,
    from: Scope,
    /// Its key's id, if the level has it anywhere.
    key: Option<u16>,
    /// As written.
    fallback: Vec<f32>,
}

/// Where a polygon is, for the inputs it gives: its level surface (a geometry polygon),
/// its sector and its entity, as it has them.
#[derive(Clone, Copy, Default)]
pub struct Place {
    pub face: Option<u32>,
    pub sector: Option<u32>,
    pub entity: Option<u32>,
}

/// A material's surface in one scenario, and what else it needs: the entity's cube map,
/// and the mesh attributes its shader reads (a mesh without them is drawn plain).
pub struct Resolved {
    /// With every input read once a frame (and those read per polygon at their fallbacks).
    pub surface: Surface,
    pub entity_cube: bool,
    pub attribs: &'static [&'static str],
    /// What its surface is made from, and its inputs read per polygon, if it has any.
    per_polygon: Option<Box<(Build, Vec<PolygonRead>)>>,
}

/// Every material's [`Resolved`] this frame (see [`Materials::table`]).
pub struct Table {
    entries: Vec<[Resolved; 4]>,
    ids: ShaderIds,
    bump: bool,
    specular: bool,
    sun: Params,
}

impl Table {
    pub fn get(&self, material: u32, reflected: bool, mirrored: bool) -> &Resolved {
        &self.entries[material as usize][reflected as usize | (mirrored as usize) << 1]
    }

    /// `r`'s surface for a polygon at `place`: its own, unless it has inputs read per
    /// polygon, which are read from `world` there (one it doesn't have is the input's
    /// fallback).
    pub fn surface(&self, r: &Resolved, place: Place, world: &World) -> Surface {
        let Some(per_polygon) = &r.per_polygon else {
            return r.surface;
        };
        let (build, reads) = &**per_polygon;
        let mut values = build.values.clone();
        for read in reads {
            let meta: Option<&MetaValues> = match read.from {
                Scope::Face => place.face.and_then(|f| world.faces.get(f as usize)),
                Scope::Sector => place.sector.and_then(|s| world.sectors.get(s as usize)).map(|s| &s.meta),
                Scope::Entity => place.entity.and_then(|e| world.entities.get(e as usize)).map(|e| &e.meta),
                Scope::Level | Scope::Light => None,
            };
            let found = read.key.and_then(|key| meta?.get(key));
            read.ty.convert(found.unwrap_or(&read.fallback), &mut values[read.offset..read.offset + read.ty.len()]);
        }
        self.build(&Build { values, ..build.clone() })
    }

    /// A surface from what it is made from, under this frame's settings.
    fn build(&self, b: &Build) -> Surface {
        let ids = &self.ids;
        let mut textures = [None; MAX_TEXTURES];
        for (slot, t) in b.textures.iter().enumerate() {
            if let Tex::Id(t) = t {
                textures[slot] = Some(*t);
            }
        }
        let mut filters = [filter::BILINEAR_MIPMAP_LINEAR; MAX_TEXTURES];
        filters[..MATERIAL_SLOTS].copy_from_slice(&b.filters);
        let (values, engine) = b.values.split_at(b.params);
        let (material, params) = match b.kind {
            Kind::VertexColor => (ids.vertex_color, Params::default()),
            Kind::Sky => (ids.sky, Params::default()),
            Kind::VertexColorTranslucent => (ids.vertex_color_translucent, Params::new(values)),
            Kind::VertexColorFresnel => (ids.vertex_color_fresnel, Params::new(values)),
            Kind::Textured => (ids.textured, Params::new(values)),
            Kind::TexturedTranslucent => (ids.textured_translucent, Params::new(values)),
            Kind::TexturedFresnel => (ids.textured_fresnel, Params::new(values)),
            Kind::Water => (ids.water, Params::new(values)),
            Kind::Lit => {
                // Bumps where it has a normal map, a highlight where its specular is above
                // 0, as the settings allow; neither, plain `textured` with its detail. Seen
                // in a mirror, its copy without either.
                let (specular, shininess, detail) = (values[0], values[1], values[2]);
                let (bump, shiny) = (b.textures[1] != Tex::None && self.bump, specular > 0.0 && self.specular);
                let lit = |k: usize| (ids.lit[k], Params::new(&[specular, shininess.max(1.0).log2().round()]));
                match (bump, shiny) {
                    _ if b.mirrored => (ids.lit_mirrored, Params::default()),
                    (false, false) => (ids.textured, Params::new(&[detail])),
                    (true, false) => lit(0),
                    (false, true) => {
                        textures[1] = None;
                        lit(1)
                    }
                    (true, true) => lit(2),
                }
            }
            Kind::BasicBumpy | Kind::RandomSections => {
                // Its flat copy in a mirror, without a normal map, or with bumps off.
                let bumpy = !b.mirrored && b.textures[1] != Tex::None && self.bump;
                let copies = if b.kind == Kind::BasicBumpy { ids.basic_bumpy } else { ids.random_sections };
                (copies[if bumpy { 0 } else { 1 }], Params::new(values))
            }
            Kind::ReflectiveBumpy => {
                // Over its reflection, or opaque without one; textured and lit in a mirror.
                // Without a normal map, or with bumps off, no bumps (no fade distance).
                let copy = if b.mirrored { 2 } else { b.reflected as usize };
                let mut values = values.to_vec();
                if b.textures[1] == Tex::None || !self.bump {
                    values[0] = 0.0;
                    values[1] = 0.0;
                }
                (ids.reflective_bumpy[copy], Params::new(&values))
            }
            Kind::CubeReflection => (ids.cube_reflection, Params::default()),
            Kind::TerrainDetail => (ids.terrain_detail, Params::default()),
            Kind::TerrainNoise => (ids.terrain_noise, Params::new(values)),
            Kind::SkyBox => (ids.sky_box, self.sun),
        };
        // Each engine input is one number.
        let mut back_light = 0.0;
        for (input, &value) in b.engine.iter().zip(engine) {
            match input {
                EngineInput::BackLight => back_light = value.clamp(0.0, 90.0).to_radians().sin(),
            }
        }
        Surface { textures, params, filters, back_light, ..Surface::new(material) }
    }
}
