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

/// Texture slots a material can fill (as `Surface` has).
pub const MATERIAL_SLOTS: usize = 2;

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
    fn parse(value: &str) -> Result<Self, String> {
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
}

/// A param's value: the artist's number, or the player's setting of that name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamValue {
    Number(f32),
    Setting,
}

/// One material, as its file says (unchecked against the shaders).
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialDef {
    pub name: String,
    pub shader: String,
    /// Where it is defined, for errors.
    pub file: PathBuf,
    pub line: usize,
    pub textures: [Option<TextureSource>; MATERIAL_SLOTS],
    /// Per slot, its sampler's name, if it sets one.
    pub filters: [Option<String>; MATERIAL_SLOTS],
    /// Params by name, in file order.
    pub params: Vec<(String, ParamValue)>,
    /// Features with a word for a value (`bump=normal`), in file order.
    pub features: Vec<(String, String)>,
    /// Other materials by scenario (`variant.SCENARIO=MATERIAL`), in file order.
    pub variants: Vec<(String, String)>,
    pub translucent: bool,
}

/// Features whose values are words, not numbers.
const WORD_FEATURES: [&str; 1] = ["bump"];

/// Every material in `assets/materials/`, by name.
#[derive(Clone, Debug, Default)]
pub struct MaterialLibrary {
    pub materials: Vec<MaterialDef>,
    by_name: HashMap<String, u32>,
}

impl MaterialLibrary {
    /// The materials in every `.mmat` file in `dir` (in name order), whose names must be
    /// unique across them. No directory is an empty library.
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

    /// Reads one file's materials into the library.
    pub fn add_file(&mut self, path: &Path, src: &str) -> Result<(), LoadError> {
        let lines = tokenize(src).map_err(|(l, m)| LoadError::new(path, Some(l), m))?;
        let mut c = Cursor { path, lines, pos: 0 };
        let (line, args) = c.header("MOOSEMATERIAL", 1)?;
        if args[0] != "1" {
            return Err(c.err(line, format!("unsupported format version '{}'", args[0])));
        }
        let section = c.section("materials", false, None)?;
        for r in &section.rows {
            if r.tokens.len() < 2 {
                return Err(c.err(r.no, "material row needs a name and a shader"));
            }
            let name = r.tokens[0].clone();
            if let Some(&other) = self.by_name.get(&name) {
                let o = &self.materials[other as usize];
                return Err(c.err(
                    r.no,
                    format!("material '{name}' is already defined ({}:{})", o.file.display(), o.line),
                ));
            }
            let mut def = MaterialDef {
                name: name.clone(),
                shader: r.tokens[1].clone(),
                file: path.to_path_buf(),
                line: r.no,
                textures: Default::default(),
                filters: Default::default(),
                params: Vec::new(),
                features: Vec::new(),
                variants: Vec::new(),
                translucent: false,
            };
            for option in &r.tokens[2..] {
                let fail = |m: String| c.err(r.no, format!("material '{name}': {m}"));
                if option == "translucent" {
                    def.translucent = true;
                    continue;
                }
                let Some((key, value)) = option.split_once('=') else {
                    return Err(fail(format!("'{option}' is not an option (KEY=VALUE, or 'translucent')")));
                };
                if let Some(slot) = key.strip_prefix("texture") {
                    let slot = parse_slot(slot).ok_or_else(|| fail(format!("no texture slot '{slot}'")))?;
                    def.textures[slot] = Some(TextureSource::parse(value).map_err(fail)?);
                } else if let Some(slot) = key.strip_prefix("filter") {
                    let slot = parse_slot(slot).ok_or_else(|| fail(format!("no texture slot '{slot}'")))?;
                    def.filters[slot] = Some(value.to_string());
                } else if let Some(scenario) = key.strip_prefix("variant.") {
                    if scenario.is_empty() || value.is_empty() {
                        return Err(fail(format!("'{option}' needs a scenario and a material")));
                    }
                    def.variants.push((scenario.to_string(), value.to_string()));
                } else if WORD_FEATURES.contains(&key) {
                    def.features.push((key.to_string(), value.to_string()));
                } else {
                    let value = match value {
                        "setting" => ParamValue::Setting,
                        v => ParamValue::Number(
                            v.parse::<f32>()
                                .ok()
                                .filter(|x| x.is_finite())
                                .ok_or_else(|| fail(format!("'{v}' is not a number or 'setting' (for {key})")))?,
                        ),
                    };
                    def.params.push((key.to_string(), value));
                }
            }
            self.by_name.insert(name, self.materials.len() as u32);
            self.materials.push(def);
        }
        if let Some(extra) = c.lines.get(c.pos) {
            return Err(c.err(extra.no, format!("unexpected '{}' after the materials", extra.tokens[0])));
        }
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

    #[test]
    fn reads_materials_and_their_options() {
        let src = "MOOSEMATERIAL 1\n\
                   materials 3\n\
                   0 brick lit texture0=brick_wall.png texture1=brick_wall_normal.png filter1=bilinear detail=0.1 bump=normal specular=0.25 shininess=32\n\
                   1 shiny textured texture0=metal_tile.png variant.reflected=shiny_fresnel\n\
                   2 shiny_fresnel textured_fresnel texture0=@water:metal_tile.png reflectance=setting translucent\n";
        let mut lib = MaterialLibrary::default();
        lib.add_file(Path::new("t.mmat"), src).unwrap();
        let brick = lib.get(lib.id("brick").unwrap());
        assert_eq!(brick.shader, "lit");
        assert_eq!(brick.textures[1], Some(TextureSource::File("brick_wall_normal.png".into())));
        assert_eq!(brick.filters[1].as_deref(), Some("bilinear"));
        assert_eq!(brick.features, [("bump".to_string(), "normal".to_string())]);
        assert!(brick.params.contains(&("specular".to_string(), ParamValue::Number(0.25))));
        let fresnel = lib.get(lib.id("shiny_fresnel").unwrap());
        assert!(fresnel.translucent);
        assert_eq!(
            fresnel.textures[0],
            Some(TextureSource::Runtime { name: "water".into(), file: Some("metal_tile.png".into()) })
        );
        assert_eq!(fresnel.params, [("reflectance".to_string(), ParamValue::Setting)]);
        assert_eq!(lib.get(1).variants, [("reflected".to_string(), "shiny_fresnel".to_string())]);
    }

    #[test]
    fn rejects_bad_rows() {
        let reject = |row: &str, want: &str| {
            let src = format!("MOOSEMATERIAL 1\nmaterials 1\n0 {row}\n");
            let err = MaterialLibrary::default().add_file(Path::new("t.mmat"), &src).unwrap_err().to_string();
            assert!(err.contains(want), "{err}");
        };
        reject("lonely", "needs a name and a shader");
        reject("m textured texture2=a.png", "no texture slot");
        reject("m textured detail=lots", "not a number");
        reject("m textured shiny", "not an option");
        reject("m textured texture0=@", "names no run-time texture");
    }

    #[test]
    fn binds_materials_and_sampler_overrides() {
        let mut lib = MaterialLibrary::default();
        lib.add_file(Path::new("t.mmat"), "MOOSEMATERIAL 1\nmaterials 1\n0 stone textured texture0=stone_wall.png\n").unwrap();
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
