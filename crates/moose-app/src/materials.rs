//! Materials as data, resolved for drawing (see `Material Format Spec.md`): the registry of
//! shaders materials can name (what each reads), materials checked against it and their
//! textures loaded, and each frame, for each material and scenario, the `Surface` it is
//! drawn with under the player's settings.

use std::collections::HashMap;

use moose_assets::{
    Assets, Binding, MATERIAL_SLOTS, MaterialLibrary, ParamValue, Ripples, Texture, TextureId, TextureSource,
};
use moose_raster::shaders::{
    CubeReflection, SkyBox, Textured, TexturedFresnel, TexturedNormal, TexturedNormalSpecular, TexturedSpecular,
    TexturedTranslucent, UnlitColor, VertexColor, VertexColorDetail, VertexColorFresnel, VertexColorNoise,
    VertexColorTranslucent, Water, filter,
};
use moose_raster::{MAX_TEXTURES, MaterialId, Params, Renderer, Surface};

/// What a registered shader is, for resolving it.
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
    /// `TexturedLit`: bumps from a normal map and a highlight, each a feature.
    Lit,
    CubeReflection,
    TerrainDetail,
    TerrainNoise,
    SkyBox,
}

/// A shader materials can name: its texture slots, params (in the order it reads them;
/// `cube_reflection`'s and `sky_box`'s the engine fills in), features, and the mesh
/// attributes it reads.
pub struct ShaderInfo {
    pub name: &'static str,
    pub kind: Kind,
    pub slots: &'static [&'static str],
    pub params: &'static [&'static str],
    pub features: &'static [&'static str],
    pub attribs: &'static [&'static str],
}

/// The registry (see the Material Format Spec).
pub const SHADERS: [ShaderInfo; 13] = [
    ShaderInfo { name: "vertex_color", kind: Kind::VertexColor, slots: &[], params: &[], features: &[], attribs: &["color"] },
    ShaderInfo { name: "sky", kind: Kind::Sky, slots: &[], params: &[], features: &[], attribs: &["color"] },
    ShaderInfo { name: "vertex_color_translucent", kind: Kind::VertexColorTranslucent, slots: &[], params: &["opacity"], features: &[], attribs: &["color"] },
    ShaderInfo { name: "vertex_color_fresnel", kind: Kind::VertexColorFresnel, slots: &[], params: &["reflectance"], features: &[], attribs: &["color"] },
    ShaderInfo { name: "textured", kind: Kind::Textured, slots: &["color"], params: &["detail"], features: &[], attribs: &["uv"] },
    ShaderInfo { name: "textured_translucent", kind: Kind::TexturedTranslucent, slots: &["color"], params: &["opacity"], features: &[], attribs: &["uv"] },
    ShaderInfo { name: "textured_fresnel", kind: Kind::TexturedFresnel, slots: &["color"], params: &["reflectance", "fade"], features: &[], attribs: &["uv"] },
    ShaderInfo { name: "water", kind: Kind::Water, slots: &["color", "heights"], params: &["reflectance", "fade", "shift"], features: &[], attribs: &["uv"] },
    // Without either feature on, drawn with `textured` (its `detail`).
    ShaderInfo { name: "lit", kind: Kind::Lit, slots: &["color", "normal map"], params: &["specular", "shininess", "detail"], features: &["bump"], attribs: &["uv"] },
    ShaderInfo { name: "cube_reflection", kind: Kind::CubeReflection, slots: &["cube"], params: &[], features: &[], attribs: &["normal"] },
    ShaderInfo { name: "terrain_detail", kind: Kind::TerrainDetail, slots: &["detail"], params: &[], features: &[], attribs: &["color", "uv", "normal"] },
    ShaderInfo { name: "terrain_noise", kind: Kind::TerrainNoise, slots: &[], params: &["strength"], features: &[], attribs: &["color", "normal"] },
    ShaderInfo { name: "sky_box", kind: Kind::SkyBox, slots: &["cube"], params: &[], features: &[], attribs: &[] },
];

/// The scenarios a material can have variants for, in the order they apply: the settings'
/// first (they hold for the whole frame), then each polygon's.
pub const SCENARIOS: [&str; 5] = ["water", "translucent", "simple_sky", "reflected", "in_mirror"];
const WATER: usize = 0;
const TRANSLUCENT: usize = 1;
const SIMPLE_SKY: usize = 2;
const REFLECTED: usize = 3;
const IN_MIRROR: usize = 4;

/// The compiled shaders behind the registry's names.
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
    /// `TexturedLit` with bumps; with a highlight; with both.
    lit: [MaterialId; 3],
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

#[derive(Clone, Copy, Debug, PartialEq)]
enum Param {
    Number(f32),
    Setting(Setting),
}

/// Player settings a param can take (`NAME=setting`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Setting {
    Reflectance,
    Fade,
}

struct Compiled {
    kind: Kind,
    textures: [Tex; MATERIAL_SLOTS],
    filters: [u8; MATERIAL_SLOTS],
    /// In the shader's order (see `ShaderInfo::params`); unset ones are 0.
    params: Vec<Param>,
    bump: bool,
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
    /// slot, param, feature, sampler, scenario or variant is an error naming its file and
    /// line), and loads the textures they name.
    pub fn compile(assets: &mut Assets, ripples: &Ripples) -> Result<Self, String> {
        let library: MaterialLibrary = assets.materials().clone();
        let mut compiled = Vec::with_capacity(library.len());
        let mut waters: Vec<(TextureId, TextureId)> = Vec::new();
        let mut heights = None;
        let mut cubes: HashMap<String, TextureId> = HashMap::new();
        for def in &library.materials {
            let at = |m: String| format!("{}:{}: material '{}': {m}", def.file.display(), def.line, def.name);
            let info = SHADERS
                .iter()
                .find(|s| s.name == def.shader)
                .ok_or_else(|| at(format!("unknown shader '{}' (known: {})", def.shader, SHADERS.map(|s| s.name).join(", "))))?;
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
            let mut params = vec![Param::Number(0.0); info.params.len()];
            for (name, value) in &def.params {
                let k = info
                    .params
                    .iter()
                    .position(|p| p == name)
                    .ok_or_else(|| at(format!("shader '{}' has no param '{name}'", info.name)))?;
                params[k] = match value {
                    ParamValue::Number(x) => Param::Number(*x),
                    ParamValue::Setting => Param::Setting(match name.as_str() {
                        "reflectance" => Setting::Reflectance,
                        "fade" => Setting::Fade,
                        _ => return Err(at(format!("no setting for '{name}' (settings: reflectance, fade)"))),
                    }),
                };
            }
            let mut bump = false;
            for (feature, value) in &def.features {
                if !info.features.contains(&feature.as_str()) {
                    return Err(at(format!("shader '{}' has no feature '{feature}'", info.name)));
                }
                match (feature.as_str(), value.as_str()) {
                    ("bump", "normal") => bump = true,
                    ("bump", "none") => bump = false,
                    _ => return Err(at(format!("'{feature}={value}' isn't a feature (bump=normal or none)"))),
                }
            }
            if def.translucent {
                return Err(at(format!("shader '{}' can't be drawn translucent", info.name)));
            }
            compiled.push(Compiled { kind: info.kind, textures, filters, params, bump, variants: [None; SCENARIOS.len()] });
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

    /// Each material's surface this frame, in each per-polygon scenario.
    pub fn table(&self, ids: &ShaderIds, s: &FrameSettings) -> Table {
        let n = self.compiled.len();
        let mut entries = Vec::with_capacity(n);
        for m in 0..n {
            entries.push(std::array::from_fn(|k| {
                let (reflected, in_mirror) = (k & 1 != 0, k & 2 != 0);
                let mut id = m;
                for (scenario, on) in [
                    (WATER, s.water),
                    (TRANSLUCENT, s.translucent),
                    (SIMPLE_SKY, s.simple_sky),
                    (REFLECTED, reflected),
                    (IN_MIRROR, in_mirror),
                ] {
                    if on && let Some(v) = self.compiled[id].variants[scenario] {
                        id = v as usize;
                    }
                }
                self.resolve(id, ids, s)
            }));
        }
        Table { entries }
    }

    fn resolve(&self, id: usize, ids: &ShaderIds, s: &FrameSettings) -> Resolved {
        let c = &self.compiled[id];
        let value = |p: &Param| match *p {
            Param::Number(x) => x,
            Param::Setting(Setting::Reflectance) => s.reflectance,
            Param::Setting(Setting::Fade) => s.fade,
        };
        let values: Vec<f32> = c.params.iter().map(value).collect();
        let mut textures = [None; MAX_TEXTURES];
        for (slot, t) in c.textures.iter().enumerate() {
            if let Tex::Id(t) = t {
                textures[slot] = Some(*t);
            }
        }
        let mut filters = [filter::BILINEAR_MIPMAP_LINEAR; MAX_TEXTURES];
        filters[..MATERIAL_SLOTS].copy_from_slice(&c.filters);
        let (material, params) = match c.kind {
            Kind::VertexColor => (ids.vertex_color, Params::default()),
            Kind::Sky => (ids.sky, Params::default()),
            Kind::VertexColorTranslucent => (ids.vertex_color_translucent, Params::new(&values)),
            Kind::VertexColorFresnel => (ids.vertex_color_fresnel, Params::new(&values)),
            Kind::Textured => (ids.textured, Params::new(&values)),
            Kind::TexturedTranslucent => (ids.textured_translucent, Params::new(&values)),
            Kind::TexturedFresnel => (ids.textured_fresnel, Params::new(&values)),
            Kind::Water => (ids.water, Params::new(&values)),
            Kind::Lit => {
                // Its features as the settings allow: a normal map, a highlight (a strength
                // above 0); neither, plain `textured` with its detail.
                let (specular, shininess, detail) = (values[0], values[1], values[2]);
                let (bump, shiny) = (c.bump && s.bump, specular > 0.0 && s.specular);
                let lit = |k: usize| (ids.lit[k], Params::new(&[specular, shininess.max(1.0).log2().round()]));
                match (bump, shiny) {
                    (false, false) => (ids.textured, Params::new(&[detail])),
                    (true, false) => lit(0),
                    (false, true) => {
                        textures[1] = None;
                        lit(1)
                    }
                    (true, true) => lit(2),
                }
            }
            Kind::CubeReflection => (ids.cube_reflection, Params::default()),
            Kind::TerrainDetail => (ids.terrain_detail, Params::default()),
            Kind::TerrainNoise => (ids.terrain_noise, Params::new(&values)),
            Kind::SkyBox => (ids.sky_box, s.sun),
        };
        let info = SHADERS.iter().find(|i| i.kind == c.kind).unwrap();
        Resolved {
            surface: Surface { textures, params, filters, ..Surface::new(material) },
            entity_cube: c.textures.contains(&Tex::EntityCube),
            attribs: info.attribs,
        }
    }
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
}

/// A material's surface in one scenario, and what else it needs: the entity's cube map,
/// and the mesh attributes its shader reads (a mesh without them is drawn plain).
#[derive(Clone, Copy)]
pub struct Resolved {
    pub surface: Surface,
    pub entity_cube: bool,
    pub attribs: &'static [&'static str],
}

/// Every material's [`Resolved`] this frame (see [`Materials::table`]).
pub struct Table {
    entries: Vec<[Resolved; 4]>,
}

impl Table {
    pub fn get(&self, material: u32, reflected: bool, in_mirror: bool) -> &Resolved {
        &self.entries[material as usize][reflected as usize | (in_mirror as usize) << 1]
    }
}
