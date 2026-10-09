//! Materials as data: `.mmat` files (see `Material Format Spec.md`). A material names a
//! shader and fills in what it reads: textures per slot, how each is read, numbers by name,
//! what it can do (features), and other materials to use in particular scenarios. Which
//! shaders exist and what they read is the renderer's (its registry checks these against
//! it); this module only reads the files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::LoadError;
use crate::mmp::Cursor;
use crate::text::tokenize;

/// Texture slots a material can fill (as `Surface` has): only a number for fixed-size
/// arrays; a shader reads the slots it declares.
pub const MATERIAL_SLOTS: usize = 8;

/// Where a slot's texture comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum TextureSource {
    /// A file in `assets/textures/`.
    File(String),
    /// A cube map from six files in `assets/textures/`: `NAME_px.png`, `NAME_nx.png`,
    /// `NAME_py.png`, `NAME_ny.png`, `NAME_pz.png` and `NAME_nz.png`.
    Cube(String),
    /// A texture the engine makes at run time (`@NAME`, or `@NAME:FILE` for one made from a
    /// file): `@water:FILE` (FILE rippling), `@water_heights` (the ripples' heights), `@cube`
    /// (the entity's own cube map, baked at load).
    Runtime { name: String, file: Option<String> },
}

impl TextureSource {
    /// Reads a slot's texture as a material file writes it (`textureN=` values).
    pub fn parse(value: &str) -> Result<Self, String> {
        if let Some(rest) = value.strip_prefix('@') {
            let (name, file) = match rest.split_once(':') {
                Some((name, file)) => (name, Some(file.to_string())),
                None => (rest, None),
            };
            if name.is_empty() {
                return Err(format!("'{value}' names no run-time texture"));
            }
            return Ok(Self::Runtime { name: name.to_string(), file });
        }
        if let Some(name) = value.strip_prefix("cube:") {
            if name.is_empty() {
                return Err(format!("'{value}' names no cube map"));
            }
            return Ok(Self::Cube(name.to_string()));
        }
        if value.is_empty() {
            return Err("a texture needs a file".into());
        }
        Ok(Self::File(value.to_string()))
    }

    /// As a material file writes it (what [`parse`](Self::parse) reads).
    pub fn to_text(&self) -> String {
        match self {
            Self::File(file) => file.clone(),
            Self::Cube(name) => format!("cube:{name}"),
            Self::Runtime { name, file: Some(file) } => format!("@{name}:{file}"),
            Self::Runtime { name, file: None } => format!("@{name}"),
        }
    }
}

/// Where a param's value comes from (its source).
#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    /// The material's own numbers, as written: one (`0.5`, `64`) or several (`204,51,51`).
    /// What they mean is the shader's (the param's type).
    Fixed(Vec<f32>),
    /// The player's setting: of the param's own name (`setting`), or another (`setting:NAME`).
    Setting(Option<String>),
    /// A meta value read for each polygon drawn (`face:KEY`, `sector:KEY`, `entity:KEY`,
    /// `level:KEY`), or a named light's color or strength (`light:NAME`), with what to use
    /// where there's none (`|VALUE`; 0 without).
    Read { from: Scope, key: String, fallback: Option<Vec<f32>> },
}

/// What a [`ParamValue::Read`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The polygon's surface (level geometry), or none (models).
    Face,
    /// The sector the polygon is in.
    Sector,
    /// The polygon's entity, or its template; none for level geometry.
    Entity,
    /// The level.
    Level,
    /// A light, by name.
    Light,
}

impl Scope {
    pub const ALL: [Scope; 5] = [Scope::Face, Scope::Sector, Scope::Entity, Scope::Level, Scope::Light];

    pub fn name(self) -> &'static str {
        match self {
            Scope::Face => "face",
            Scope::Sector => "sector",
            Scope::Entity => "entity",
            Scope::Level => "level",
            Scope::Light => "light",
        }
    }
}

impl ParamValue {
    /// Reads a param's value as a material file writes it.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text == "setting" {
            return Ok(Self::Setting(None));
        }
        if let Some(name) = text.strip_prefix("setting:") {
            if name.is_empty() {
                return Err("'setting:' needs a setting's name".into());
            }
            return Ok(Self::Setting(Some(name.to_string())));
        }
        if let Some((from, rest)) = text.split_once(':')
            && let Some(&from) = Scope::ALL.iter().find(|s| s.name() == from)
        {
            let (key, fallback) = match rest.split_once('|') {
                Some((key, fallback)) => (key, Some(crate::meta::parse_numbers(fallback)?)),
                None => (rest, None),
            };
            if key.is_empty() {
                return Err(format!("'{text}' needs a key ({}:KEY)", from.name()));
            }
            return Ok(Self::Read { from, key: key.to_string(), fallback });
        }
        crate::meta::parse_numbers(text)
            .map(Self::Fixed)
            .map_err(|_| format!("'{text}' is not numbers, a setting or a source (face:, sector:, entity:, level:, light:)"))
    }

    /// As a material file writes it.
    pub fn to_text(&self) -> String {
        match self {
            Self::Fixed(values) => crate::meta::numbers_text(values),
            Self::Setting(None) => "setting".into(),
            Self::Setting(Some(name)) => format!("setting:{name}"),
            Self::Read { from, key, fallback: None } => format!("{}:{key}", from.name()),
            Self::Read { from, key, fallback: Some(f) } => {
                format!("{}:{key}|{}", from.name(), crate::meta::numbers_text(f))
            }
        }
    }
}

/// One material, as its file says (unchecked against the shaders).
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialDef {
    pub name: String,
    /// What draws it (its program).
    pub shader: String,
    /// Where it is defined, for errors.
    pub file: PathBuf,
    pub line: usize,
    pub textures: [Option<TextureSource>; MATERIAL_SLOTS],
    /// Per slot, its sampler's name, if it sets one.
    pub filters: [Option<String>; MATERIAL_SLOTS],
    /// Params by name, in file order.
    pub params: Vec<(String, ParamValue)>,
    /// Other materials by scenario (`variant.SCENARIO=MATERIAL`), in file order.
    pub variants: Vec<(String, String)>,
    pub translucent: bool,
}

/// Every material in `assets/materials/`, by name: one a file, `NAME.mmat`.
#[derive(Clone, Debug, Default)]
pub struct MaterialLibrary {
    pub materials: Vec<MaterialDef>,
    by_name: HashMap<String, u32>,
}

/// Whether `name` can be a material's name (and so its file's): lowercase letters, digits
/// and `_`, so no two files differ only by case (macOS doesn't tell them apart).
pub fn check_material_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a material needs a name".into());
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return Err(format!("'{name}': material names are lowercase letters, digits and '_'"));
    }
    Ok(())
}

impl MaterialLibrary {
    /// The materials in `dir`, one a `.mmat` file (in name order). No directory is an empty
    /// library.
    pub fn load(dir: &Path) -> Result<Self, LoadError> {
        let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "mmat"))
                .collect(),
            Err(_) => return Ok(Self::default()),
        };
        files.sort();
        let mut library = Self::default();
        for path in files {
            let src = std::fs::read_to_string(&path)
                .map_err(|e| LoadError::new(&path, None, format!("cannot read file: {e}")))?;
            library.add_file(&path, &src)?;
        }
        Ok(library)
    }

    /// Reads one material file (`NAME.mmat`, the material `NAME`) into the library.
    pub fn add_file(&mut self, path: &Path, src: &str) -> Result<(), LoadError> {
        let def = parse_material(path, src)?;
        if let Some(&other) = self.by_name.get(&def.name) {
            let o = &self.materials[other as usize];
            return Err(LoadError::new(path, None, format!("material '{}' is already read from {}", def.name, o.file.display())));
        }
        self.by_name.insert(def.name.clone(), self.materials.len() as u32);
        self.materials.push(def);
        Ok(())
    }

    pub fn id(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).copied()
    }

    pub fn get(&self, id: u32) -> &MaterialDef {
        &self.materials[id as usize]
    }

    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }
}

/// One material file: `MOOSEMATERIAL 2`, then a setting a line (`shader`, `textureN`,
/// `filterN`, `variant.SCENARIO`, `translucent`, or an input's name), each once. Its name
/// is its file's.
fn parse_material(path: &Path, src: &str) -> Result<MaterialDef, LoadError> {
    let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    check_material_name(&name).map_err(|m| LoadError::new(path, None, m))?;
    let lines = tokenize(src).map_err(|(l, m)| LoadError::new(path, Some(l), m))?;
    let mut c = Cursor { path, lines, pos: 0 };
    let (line, args) = c.header("MOOSEMATERIAL", 1)?;
    if args[0] != "2" {
        return Err(c.err(line, format!("unsupported format version '{}' (2 is one material a file)", args[0])));
    }
    let mut def = MaterialDef {
        name,
        shader: String::new(),
        file: path.to_path_buf(),
        line,
        textures: Default::default(),
        filters: Default::default(),
        params: Vec::new(),
        variants: Vec::new(),
        translucent: false,
    };
    let mut seen: Vec<String> = Vec::new();
    for l in &c.lines[c.pos..] {
        let fail = |m: String| c.err(l.no, m);
        let key = l.tokens[0].as_str();
        if seen.iter().any(|k| k == key) {
            return Err(fail(format!("'{key}' is set twice")));
        }
        seen.push(key.to_string());
        if key == "translucent" {
            if l.tokens.len() != 1 {
                return Err(fail("'translucent' takes no value".into()));
            }
            def.translucent = true;
            continue;
        }
        let [_, value] = &l.tokens[..] else {
            return Err(fail(format!("'{key}' takes one value")));
        };
        if key == "shader" {
            (def.shader, def.line) = (value.clone(), l.no);
        } else if let Some(slot) = key.strip_prefix("texture") {
            let slot = parse_slot(slot).ok_or_else(|| fail(format!("no texture slot '{slot}'")))?;
            def.textures[slot] = Some(TextureSource::parse(value).map_err(fail)?);
        } else if let Some(slot) = key.strip_prefix("filter") {
            let slot = parse_slot(slot).ok_or_else(|| fail(format!("no texture slot '{slot}'")))?;
            def.filters[slot] = Some(value.clone());
        } else if let Some(scenario) = key.strip_prefix("variant.") {
            if scenario.is_empty() {
                return Err(fail("'variant.' needs a scenario".into()));
            }
            def.variants.push((scenario.to_string(), value.clone()));
        } else {
            let value = ParamValue::parse(value).map_err(|m| fail(format!("{key}: {m}")))?;
            def.params.push((key.to_string(), value));
        }
    }
    if def.shader.is_empty() {
        return Err(LoadError::new(path, None, "a material needs a shader ('shader NAME')"));
    }
    Ok(def)
}

impl MaterialDef {
    /// Its file's text (what [`MaterialLibrary::add_file`] reads back as it), under
    /// `comments` (lines starting `#`, kept at the top).
    pub fn to_text(&self, comments: &[String]) -> String {
        let mut lines: Vec<(String, String)> = vec![("shader".into(), self.shader.clone())];
        for (slot, texture) in self.textures.iter().enumerate() {
            if let Some(texture) = texture {
                lines.push((format!("texture{slot}"), texture.to_text()));
            }
            if let Some(filter) = &self.filters[slot] {
                lines.push((format!("filter{slot}"), filter.clone()));
            }
        }
        for (name, value) in &self.params {
            lines.push((name.clone(), value.to_text()));
        }
        for (scenario, material) in &self.variants {
            lines.push((format!("variant.{scenario}"), material.clone()));
        }
        let mut out = String::from("MOOSEMATERIAL 2\n");
        for comment in comments {
            out.push_str(comment);
            out.push('\n');
        }
        for (key, value) in lines {
            out.push_str(&format!("{key:<18} {value}\n"));
        }
        if self.translucent {
            out.push_str("translucent\n");
        }
        out
    }
}

/// The comment lines at the top of a material file (before its first setting), to keep
/// when it is written anew.
pub fn material_comments(src: &str) -> Vec<String> {
    src.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("MOOSEMATERIAL"))
        .take_while(|l| l.starts_with('#'))
        .map(String::from)
        .collect()
}

fn parse_slot(s: &str) -> Option<usize> {
    s.parse::<usize>().ok().filter(|&n| n < MATERIAL_SLOTS)
}

/// What a level surface, a template or an entity is drawn with: a material (an id in the
/// library), and samplers over the material's for some slots (`filterN=`).
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub material: u32,
    pub filters: [Option<String>; MATERIAL_SLOTS],
}

/// Reads `material=` and `filterN=` from options (a surface's, or an entity's merged with
/// its template's), returning the binding and the options it didn't use. Without
/// `material=`, sampler overrides are an error.
pub(crate) fn binding_of<'a>(
    library: &MaterialLibrary,
    options: impl IntoIterator<Item = &'a String>,
) -> Result<(Option<Binding>, Vec<&'a String>), String> {
    let (mut material, mut filters, mut rest) = (None, <[Option<String>; MATERIAL_SLOTS]>::default(), Vec::new());
    for option in options {
        match option.split_once('=') {
            Some(("material", name)) => {
                material = Some(library.id(name).ok_or_else(|| format!("unknown material '{name}'"))?);
            }
            Some((key, value)) if key.starts_with("filter") => {
                let slot = parse_slot(&key["filter".len()..]).ok_or_else(|| format!("no texture slot in '{key}'"))?;
                filters[slot] = Some(value.to_string());
            }
            _ => rest.push(option),
        }
    }
    match material {
        Some(material) => Ok((Some(Binding { material, filters }), rest)),
        None if filters.iter().any(Option::is_some) => Err("sampler overrides need a material".into()),
        None => Ok((None, rest)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(name: &str, src: &str) -> Result<MaterialDef, String> {
        let mut lib = MaterialLibrary::default();
        lib.add_file(Path::new(&format!("{name}.mmat")), src).map_err(|e| e.to_string())?;
        Ok(lib.get(0).clone())
    }

    #[test]
    fn reads_a_material_from_its_file() {
        let brick = read(
            "brick",
            "MOOSEMATERIAL 2\n# Brick.\nshader lit\ntexture0 brick_wall.png\ntexture1 brick_wall_normal.png\n\
             filter1 bilinear\ndetail 26\nspecular face:wet|64\nshininess 32\nvariant.reflected brick_shiny\n",
        )
        .unwrap();
        assert_eq!((brick.name.as_str(), brick.shader.as_str(), brick.line), ("brick", "lit", 3));
        assert_eq!(brick.textures[1], Some(TextureSource::File("brick_wall_normal.png".into())));
        assert_eq!(brick.filters[1].as_deref(), Some("bilinear"));
        assert!(brick.params.contains(&("detail".to_string(), ParamValue::Fixed(vec![26.0]))));
        let wet = ParamValue::Read { from: Scope::Face, key: "wet".into(), fallback: Some(vec![64.0]) };
        assert!(brick.params.contains(&("specular".to_string(), wet)));
        assert_eq!(brick.variants, [("reflected".to_string(), "brick_shiny".to_string())]);
        let glass = read(
            "glass",
            "MOOSEMATERIAL 2\nshader textured_fresnel\ntexture0 @water:metal_tile.png\nreflectance setting\n\
             tint entity:team|204,51,51\nglow light:hall_lamp\ntranslucent\n",
        )
        .unwrap();
        assert!(glass.translucent);
        assert_eq!(glass.textures[0], Some(TextureSource::Runtime { name: "water".into(), file: Some("metal_tile.png".into()) }));
        assert_eq!(glass.params[0], ("reflectance".to_string(), ParamValue::Setting(None)));
        assert_eq!(
            glass.params[1].1,
            ParamValue::Read { from: Scope::Entity, key: "team".into(), fallback: Some(vec![204.0, 51.0, 51.0]) }
        );
        assert_eq!(glass.params[2].1, ParamValue::Read { from: Scope::Light, key: "hall_lamp".into(), fallback: None });
        // Each value writes back as it reads.
        for (_, v) in brick.params.iter().chain(&glass.params) {
            assert_eq!(&ParamValue::parse(&v.to_text()).unwrap(), v);
        }
    }

    #[test]
    fn rejects_bad_files() {
        let reject = |name: &str, body: &str, want: &str| {
            let err = read(name, &format!("MOOSEMATERIAL 2\n{body}\n")).unwrap_err();
            assert!(err.contains(want), "{err}");
        };
        reject("m", "texture0 a.png", "needs a shader");
        reject("Brick", "shader textured", "lowercase");
        reject("m", "shader textured\ntexture8 a.png", "no texture slot");
        reject("m", "shader textured\ndetail lots", "not numbers");
        reject("m", "shader lit\nbump normal", "not numbers");
        reject("m", "shader textured\ndetail face:", "needs a key");
        reject("m", "shader textured\ndetail face:wet|x", "not a number");
        reject("m", "shader textured\ndetail", "takes one value");
        reject("m", "shader textured\ndetail 1\ndetail 2", "set twice");
        reject("m", "shader textured\ntexture0 @", "names no run-time texture");
        let old = read("m", "MOOSEMATERIAL 1\nmaterials 0\n").unwrap_err();
        assert!(old.contains("one material a file"), "{old}");
        let mut lib = MaterialLibrary::default();
        lib.add_file(Path::new("a/m.mmat"), "MOOSEMATERIAL 2\nshader sky\n").unwrap();
        let twice = lib.add_file(Path::new("b/m.mmat"), "MOOSEMATERIAL 2\nshader sky\n").unwrap_err().to_string();
        assert!(twice.contains("already read"), "{twice}");
    }

    #[test]
    fn a_material_written_reads_back_the_same_under_its_comments() {
        let src = "MOOSEMATERIAL 2\n# Shiny tiles.\n# Over their reflection.\nshader textured  # inline\ntexture0 a.png\n";
        let mut def = read("tiles", src).unwrap();
        def.filters[0] = Some("nearest".into());
        def.textures[5] = Some(TextureSource::Cube("sky".into()));
        def.params.push(("detail".into(), ParamValue::Fixed(vec![26.0])));
        def.params.push(("tint".into(), ParamValue::Read { from: Scope::Sector, key: "tint".into(), fallback: None }));
        def.params.push(("reflectance".into(), ParamValue::Setting(Some("fade".into()))));
        def.variants.push(("reflected".into(), "tiles_fresnel".into()));
        def.translucent = true;
        let comments = material_comments(src);
        assert_eq!(comments, ["# Shiny tiles.", "# Over their reflection."]);
        let text = def.to_text(&comments);
        assert!(text.starts_with("MOOSEMATERIAL 2\n# Shiny tiles.\n# Over their reflection.\nshader"), "{text}");
        let back = read("tiles", &text).unwrap();
        assert_eq!(MaterialDef { line: 0, ..back }, MaterialDef { line: 0, ..def });
    }

    #[test]
    fn binds_materials_and_sampler_overrides() {
        let mut lib = MaterialLibrary::default();
        lib.add_file(Path::new("stone.mmat"), "MOOSEMATERIAL 2\nshader textured\ntexture0 stone_wall.png\n").unwrap();
        let opts: Vec<String> = ["material=stone", "filter0=nearest", "static"].map(String::from).to_vec();
        let (binding, rest) = binding_of(&lib, &opts).unwrap();
        let binding = binding.unwrap();
        assert_eq!(binding.material, 0);
        assert_eq!(binding.filters[0].as_deref(), Some("nearest"));
        assert_eq!(rest, [&opts[2]]);
        assert!(binding_of(&lib, &["material=marble".to_string()]).unwrap_err().contains("unknown material"));
        assert!(binding_of(&lib, &["filter0=nearest".to_string()]).unwrap_err().contains("need a material"));
    }
}
