//! The material creator (Esc > Materials): a material as a draft, its rows on the menu
//! page (its name, its shader, the shader's texture slots and samplers, and each input's
//! source and value), changed row by row and saved as its own file, `NAME.mmat`, by the app
//! (`App::save_material`).

use std::path::Path;

use moose_assets::{MATERIAL_SLOTS, MaterialDef, ParamValue, Scope, TextureSource, check_material_name};
use moose_raster::shaders::filter;

use crate::materials::{InputType, PARAM_SETTINGS, SHADERS, ShaderInfo, sampler, shader_info};

/// A row of the material page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Name,
    Shader,
    Texture(u8),
    Sampler(u8),
    /// One of its shader's inputs (by index): where its value comes from (Left/Right), what
    /// it is (Enter: a number, a setting's name, a key or a light's name), and for a read,
    /// what to use where there's none.
    Source(u8),
    Value(u8),
    Fallback(u8),
    Save,
}

/// What the level loaded has, for telling a key or light name nothing in it sets.
#[derive(Clone, Debug, Default)]
pub struct Known {
    pub keys: Vec<String>,
    pub lights: Vec<String>,
}

/// A material being made or changed, not saved yet.
#[derive(Clone, Debug)]
pub struct Draft {
    /// As it would be written (inputs and textures its shader doesn't read are kept here,
    /// for switching back, and left out when saved: see [`Draft::cleaned`]). Its file is
    /// `NAME.mmat` in the materials folder.
    pub def: MaterialDef,
    /// Not saved yet: its file is new. A material renamed becomes a new one (a copy, in a
    /// file of its own), so the levels and materials that name the old one keep it.
    pub new: bool,
    /// The name it was read with, if it is from a file.
    pub original: Option<String>,
    /// The comment lines at the top of its file, kept when it is saved.
    pub comments: Vec<String>,
    /// What the last save or change said.
    pub status: Option<String>,
    /// Per input, the last numbers it had, which it gets back when stepped to a number or
    /// a read (as its fallback) after a setting.
    numbers: Vec<(String, Vec<f32>)>,
}

/// The name a new material starts with: `new_material`, or with a number, unused.
fn unused_name(names: &[String]) -> String {
    (1..)
        .map(|n| if n == 1 { "new_material".to_string() } else { format!("new_material_{n}") })
        .find(|n| !names.contains(n))
        .unwrap()
}

/// What an input of type `ty` is when it's first given a number.
fn starting_value(ty: InputType) -> Vec<f32> {
    match ty {
        InputType::Color => vec![255.0; 3],
        _ => vec![0.0],
    }
}

/// Where a source reads from, as the Source row shows it.
fn source_name(value: Option<&ParamValue>) -> &'static str {
    match value {
        None => "unset (0)",
        Some(ParamValue::Fixed(_)) => "number",
        Some(ParamValue::Setting(_)) => "player setting",
        Some(ParamValue::Read { from, .. }) => from.name(),
    }
}

impl Draft {
    /// A new material in `dir` (the materials folder): the default material's look (basic
    /// diffuse with the default texture), to change from.
    pub fn new_material(dir: &Path, names: &[String]) -> Draft {
        let name = unused_name(names);
        let mut textures: [Option<TextureSource>; MATERIAL_SLOTS] = Default::default();
        textures[0] = Some(TextureSource::File("default.png".into()));
        Draft {
            def: MaterialDef {
                file: dir.join(format!("{name}.mmat")),
                name,
                shader: "textured".into(),
                line: 0,
                textures,
                filters: Default::default(),
                params: Vec::new(),
                variants: Vec::new(),
                translucent: false,
            },
            new: true,
            original: None,
            comments: Vec::new(),
            status: None,
            numbers: Vec::new(),
        }
    }

    /// A draft of `def`, as its file has it (`comments` the ones at the file's top).
    pub fn of(def: &MaterialDef, comments: Vec<String>) -> Draft {
        Draft {
            def: def.clone(),
            new: false,
            original: Some(def.name.clone()),
            comments,
            status: None,
            numbers: Vec::new(),
        }
    }

    /// Its shader, if it is registered.
    fn shader(&self) -> Option<&'static ShaderInfo> {
        shader_info(&self.def.shader)
    }

    fn slots(&self) -> &'static [&'static str] {
        self.shader().map_or(&[], |s| s.slots)
    }

    fn input(&self, i: u8) -> (&'static str, InputType) {
        let input = self.shader().unwrap().input(i as usize).0;
        (input.name, input.ty)
    }

    /// Its rows, top to bottom.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Name, Row::Shader];
        for k in 0..self.slots().len() as u8 {
            rows.extend([Row::Texture(k), Row::Sampler(k)]);
        }
        for i in 0..self.shader().map_or(0, |s| s.all_inputs().count()) as u8 {
            rows.push(Row::Source(i));
            match self.param(self.input(i).0) {
                None => {}
                Some(ParamValue::Read { .. }) => rows.extend([Row::Value(i), Row::Fallback(i)]),
                Some(_) => rows.push(Row::Value(i)),
            }
        }
        rows.push(Row::Save);
        rows
    }

    pub fn label(&self, row: Row) -> String {
        match row {
            Row::Name => "Name".into(),
            Row::Shader => "Shader".into(),
            Row::Texture(k) => format!("Texture {k}: {}", self.slots()[k as usize]),
            Row::Sampler(k) => format!("  sampler {k}"),
            Row::Source(i) => {
                let (name, ty) = self.input(i);
                if self.shader().unwrap().input(i as usize).1 {
                    format!("Engine: {name} ({}, degrees)", ty.name())
                } else {
                    format!("Input: {name} ({})", ty.name())
                }
            }
            Row::Value(i) => match self.param(self.input(i).0) {
                Some(ParamValue::Setting(_)) => "  setting".into(),
                Some(ParamValue::Read { from: Scope::Light, .. }) => "  light's name".into(),
                Some(ParamValue::Read { .. }) => "  key".into(),
                _ => "  value".into(),
            },
            Row::Fallback(_) => "  where there's none".into(),
            Row::Save => if self.new { "Save as new material" } else { "Save" }.into(),
        }
    }

    /// What `row` shows; `known` tells a key or light name the level loaded doesn't have.
    pub fn value(&self, row: Row, known: &Known) -> Option<String> {
        let d = &self.def;
        Some(match row {
            Row::Name => format!("{}  ({}.mmat)", d.name, d.name),
            Row::Shader => d.shader.clone(),
            Row::Texture(k) => d.textures[k as usize].as_ref().map_or("none".into(), |t| t.to_text()),
            Row::Sampler(k) => d.filters[k as usize].clone().unwrap_or("trilinear (default)".into()),
            Row::Source(i) => source_name(self.param(self.input(i).0)).into(),
            Row::Value(i) => match self.param(self.input(i).0) {
                Some(ParamValue::Fixed(v)) => moose_assets::meta::numbers_text(v),
                Some(ParamValue::Setting(name)) => name.clone().unwrap_or(self.input(i).0.to_string()),
                Some(ParamValue::Read { from, key, .. }) => {
                    let found = match from {
                        Scope::Light => known.lights.contains(key),
                        _ => known.keys.contains(key),
                    };
                    if found { key.clone() } else { format!("{key}  (not in this level)") }
                }
                None => String::new(),
            },
            Row::Fallback(i) => match self.param(self.input(i).0) {
                Some(ParamValue::Read { fallback: Some(f), .. }) => moose_assets::meta::numbers_text(f),
                _ => "none (0)".into(),
            },
            Row::Save => return self.status.clone(),
        })
    }

    fn param(&self, name: &str) -> Option<&ParamValue> {
        self.def.params.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    fn set_param(&mut self, name: &str, value: Option<ParamValue>) {
        let params = &mut self.def.params;
        match (params.iter().position(|(n, _)| n == name), value) {
            (Some(k), Some(v)) => params[k].1 = v,
            (Some(k), None) => drop(params.remove(k)),
            (None, Some(v)) => params.push((name.to_string(), v)),
            (None, None) => {}
        }
    }

    /// Left/Right on `row`: steps it through its choices (`textures`, what a texture slot
    /// can be: see [`texture_choices`]).
    pub fn step(&mut self, row: Row, dir: i32, textures: &[String]) -> Result<(), String> {
        match row {
            Row::Shader => {
                let names: Vec<&str> = SHADERS.iter().map(|s| s.name).collect();
                let now = names.iter().position(|&n| n == self.def.shader);
                self.def.shader = names[cycle_index(now, names.len(), dir)].to_string();
            }
            Row::Texture(k) => {
                let k = k as usize;
                let now = self.def.textures[k].as_ref().map(|t| t.to_text());
                let mut choices: Vec<Option<&str>> = vec![None];
                choices.extend(textures.iter().map(|t| Some(t.as_str())));
                let i = cycle_index(choices.iter().position(|&c| c == now.as_deref()), choices.len(), dir);
                self.def.textures[k] = choices[i].map(TextureSource::parse).transpose()?;
            }
            Row::Sampler(k) => {
                let k = k as usize;
                // Default, then the twelve by their full names.
                let at = match self.def.filters[k].as_deref() {
                    None => Some(0),
                    Some(name) => sampler(name).and_then(|f| filter::ALL.iter().position(|&a| a == f)).map(|i| i + 1),
                };
                let i = cycle_index(at, filter::ALL.len() + 1, dir);
                self.def.filters[k] = (i > 0).then(|| filter::name(filter::ALL[i - 1]).to_string());
            }
            Row::Source(i) => {
                // Unset, a number, the player's setting (where there is one), or read from a
                // face, sector, entity, the level or a light. A read starts with the input's
                // name as its key (or keeps the one it had) and its number as its fallback.
                let (name, ty) = self.input(i);
                let now = self.param(name).cloned();
                let remembered = self.numbers.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
                let number = match &now {
                    Some(ParamValue::Fixed(v)) => v.clone(),
                    Some(ParamValue::Read { fallback: Some(f), .. }) => f.clone(),
                    _ => remembered.unwrap_or_else(|| starting_value(ty)),
                };
                self.numbers.retain(|(n, _)| n != name);
                self.numbers.push((name.to_string(), number.clone()));
                let key = match &now {
                    Some(ParamValue::Read { key, .. }) => key.clone(),
                    _ => name.to_string(),
                };
                let mut sources: Vec<Option<ParamValue>> = vec![None, Some(ParamValue::Fixed(number.clone()))];
                if PARAM_SETTINGS.contains(&name) && ty != InputType::Color {
                    sources.push(Some(ParamValue::Setting(None)));
                }
                for from in Scope::ALL {
                    sources.push(Some(ParamValue::Read { from, key: key.clone(), fallback: Some(number.clone()) }));
                }
                let kind = |v: &Option<ParamValue>| match v {
                    None => 0,
                    Some(ParamValue::Fixed(_)) => 1,
                    Some(ParamValue::Setting(_)) => 2,
                    Some(ParamValue::Read { from, .. }) => 3 + *from as usize,
                };
                let at = sources.iter().position(|s| kind(s) == kind(&now));
                let next = sources[cycle_index(at, sources.len(), dir)].clone();
                self.set_param(name, next);
            }
            _ => return Err("Enter on it".into()),
        }
        Ok(())
    }

    /// The text Enter starts typing on `row` with, if it takes typing.
    pub fn typing(&self, row: Row) -> Option<String> {
        let known = Known::default();
        match row {
            Row::Name => Some(self.def.name.clone()),
            Row::Texture(k) => Some(self.def.textures[k as usize].as_ref().map_or(String::new(), |t| t.to_text())),
            Row::Value(i) => Some(match self.param(self.input(i).0) {
                Some(ParamValue::Read { key, .. }) => key.clone(),
                _ => self.value(row, &known).unwrap_or_default(),
            }),
            Row::Fallback(i) => Some(match self.param(self.input(i).0) {
                Some(ParamValue::Read { fallback: Some(f), .. }) => moose_assets::meta::numbers_text(f),
                _ => String::new(),
            }),
            _ => None,
        }
    }

    /// Sets `row` to what was typed on it. A material from a file renamed becomes a new
    /// one: a copy, saved in a file of its own, so what names the old one keeps it.
    pub fn typed(&mut self, row: Row, text: &str, names: &[String]) -> Result<String, String> {
        let text = text.trim();
        match row {
            Row::Name => {
                check_material_name(text)?;
                if self.def.name == text {
                    return Ok(String::new());
                }
                if names.iter().any(|n| n == text) {
                    return Err(format!("there is already a material '{text}'"));
                }
                self.def.name = text.to_string();
                self.def.file = self.def.file.with_file_name(format!("{text}.mmat"));
                if !self.new {
                    self.new = true;
                    return Ok(format!("a copy now: saved as {text}.mmat, '{}' kept", self.original.as_deref().unwrap_or("")));
                }
            }
            Row::Texture(k) => {
                self.def.textures[k as usize] = if text.is_empty() { None } else { Some(TextureSource::parse(text)?) };
            }
            Row::Value(i) => {
                let (name, ty) = self.input(i);
                let value = match self.param(name).cloned() {
                    // The number itself.
                    Some(ParamValue::Fixed(_)) => {
                        let v = moose_assets::meta::parse_numbers(text)?;
                        ty.check(&v)?;
                        ParamValue::Fixed(v)
                    }
                    Some(ParamValue::Setting(_)) => {
                        if !PARAM_SETTINGS.contains(&text) {
                            return Err(format!("no player setting '{text}' (settings: {})", PARAM_SETTINGS.join(", ")));
                        }
                        ParamValue::Setting((text != name).then(|| text.to_string()))
                    }
                    // Its key, or the light's name.
                    Some(ParamValue::Read { from, fallback, .. }) => {
                        let ok = !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                        if !ok {
                            return Err("a key or light's name is letters, digits and '_'".into());
                        }
                        ParamValue::Read { from, key: text.to_string(), fallback }
                    }
                    None => return Err("pick a source first (Left/Right on the input)".into()),
                };
                self.set_param(name, Some(value));
            }
            Row::Fallback(i) => {
                let (name, ty) = self.input(i);
                let Some(ParamValue::Read { from, key, .. }) = self.param(name).cloned() else {
                    return Err("only a read has a fallback".into());
                };
                let fallback = if text.is_empty() {
                    None
                } else {
                    let v = moose_assets::meta::parse_numbers(text)?;
                    ty.check(&v)?;
                    Some(v)
                };
                self.set_param(name, Some(ParamValue::Read { from, key, fallback }));
            }
            _ => return Err("nothing to type there".into()),
        }
        Ok(String::new())
    }

    /// The material as it is saved: without the inputs and textures its shader doesn't read.
    pub fn cleaned(&self) -> MaterialDef {
        let mut def = self.def.clone();
        let inputs: Vec<&str> = self.shader().map_or(Vec::new(), |s| s.all_inputs().map(|i| i.name).collect());
        def.params.retain(|(n, _)| inputs.contains(&n.as_str()));
        for k in self.slots().len()..MATERIAL_SLOTS {
            def.textures[k] = None;
            def.filters[k] = None;
        }
        def
    }
}

/// The index `dir` steps to from `now` (or from before the first, if none) among `n`,
/// wrapping around.
fn cycle_index(now: Option<usize>, n: usize, dir: i32) -> usize {
    let start = now.map_or(if dir > 0 { -1 } else { 0 }, |k| k as i32);
    (start + dir).rem_euclid(n as i32) as usize
}

/// What a texture slot can be stepped to: the files in `dir` (`.png`, a cube map's six
/// faces, `NAME_px.png` ... `NAME_nz.png`, as one `cube:NAME`), then the textures the engine
/// makes: the entity's cube map, the ripples' heights, and each file rippled.
pub fn texture_choices(dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".png"))
        .collect();
    files.sort();
    let faces = ["px", "nx", "py", "ny", "pz", "nz"];
    let cubes: Vec<String> = files
        .iter()
        .filter_map(|f| f.strip_suffix("_px.png"))
        .filter(|name| faces.iter().all(|a| files.contains(&format!("{name}_{a}.png"))))
        .map(String::from)
        .collect();
    let face_of_cube = |f: &String| cubes.iter().any(|c| faces.iter().any(|a| *f == format!("{c}_{a}.png")));
    let plain: Vec<String> = files.iter().filter(|f| !face_of_cube(f)).cloned().collect();
    let mut out = plain.clone();
    out.extend(cubes.iter().map(|c| format!("cube:{c}")));
    out.extend(["@cube".to_string(), "@water_heights".to_string()]);
    out.extend(plain.iter().map(|f| format!("@water:{f}")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> Draft {
        Draft::new_material(Path::new("materials"), &["new_material".into()])
    }

    #[test]
    fn a_new_material_is_its_own_file_and_follows_its_shader() {
        let mut d = draft();
        assert_eq!(d.def.name, "new_material_2");
        assert_eq!(d.def.file, Path::new("materials/new_material_2.mmat"));
        d.typed(Row::Name, "road", &[]).unwrap();
        assert_eq!(d.def.file, Path::new("materials/road.mmat"));
        assert!(d.typed(Row::Name, "Road", &[]).is_err());
        assert_eq!(d.label(Row::Source(0)), "Input: detail (0-255)");
        // `basic_bumpy` has a normal map slot and two inputs.
        d.def.shader = "basic_bumpy".into();
        assert!(d.rows().contains(&Row::Texture(1)));
        assert_eq!(d.label(Row::Source(1)), "Input: far (number)");
    }

    #[test]
    fn an_inputs_source_steps_and_its_value_is_typed() {
        let known = Known { keys: vec!["wet".into()], lights: vec![] };
        let mut d = draft();
        d.def.shader = "textured_fresnel".into();
        let (source, value, fallback) = (Row::Source(0), Row::Value(0), Row::Fallback(0));
        // Unset: no value row.
        assert!(!d.rows().contains(&value));
        d.step(source, 1, &[]).unwrap();
        assert_eq!(d.value(source, &known).unwrap(), "number");
        d.typed(value, "64", &[]).unwrap();
        assert!(d.typed(value, "300", &[]).is_err());
        assert!(d.typed(value, "0.5", &[]).is_err());
        d.step(source, 1, &[]).unwrap();
        assert_eq!(d.value(source, &known).unwrap(), "player setting");
        assert_eq!(d.value(value, &known).unwrap(), "reflectance");
        assert!(d.typed(value, "shininess", &[]).is_err());
        d.step(source, 1, &[]).unwrap();
        assert_eq!(d.value(source, &known).unwrap(), "face");
        assert!(d.rows().contains(&fallback));
        // The key is free-typed; one the level doesn't have says so.
        assert_eq!(d.value(value, &known).unwrap(), "reflectance  (not in this level)");
        d.typed(value, "wet", &[]).unwrap();
        assert_eq!(d.value(value, &known).unwrap(), "wet");
        assert_eq!(d.value(fallback, &known).unwrap(), "64");
        d.typed(fallback, "200", &[]).unwrap();
        assert!(d.typed(fallback, "1,2,3", &[]).is_err());
        d.step(source, 1, &[]).unwrap();
        assert_eq!(d.param("reflectance").unwrap().to_text(), "sector:wet|200");
        // Back to a number: the fallback's.
        for _ in 0..3 {
            d.step(source, -1, &[]).unwrap();
        }
        assert_eq!(d.param("reflectance").unwrap().to_text(), "200");
        // Back to `textured`: what it doesn't read isn't saved.
        d.def.shader = "textured".into();
        assert!(d.cleaned().params.is_empty());
    }

    #[test]
    fn samplers_shaders_and_textures_wrap_around() {
        let mut d = draft();
        d.step(Row::Sampler(0), -1, &[]).unwrap();
        assert_eq!(d.def.filters[0].as_deref(), Some(filter::name(*filter::ALL.last().unwrap())));
        d.step(Row::Sampler(0), 1, &[]).unwrap();
        assert_eq!(d.def.filters[0], None);
        d.def.shader = SHADERS[0].name.into();
        d.step(Row::Shader, -1, &[]).unwrap();
        assert_eq!(d.def.shader, SHADERS[SHADERS.len() - 1].name);
        let textures = vec!["a.png".to_string(), "@water:a.png".to_string()];
        d.step(Row::Texture(0), -1, &textures).unwrap();
        assert_eq!(d.def.textures[0], Some(TextureSource::Runtime { name: "water".into(), file: Some("a.png".into()) }));
    }

    #[test]
    fn renaming_a_material_from_a_file_makes_a_copy() {
        let def = draft().def;
        let mut d = Draft::of(&MaterialDef { name: "brick".into(), line: 4, ..def }, Vec::new());
        assert!(d.typed(Row::Name, "stone", &["stone".into()]).is_err());
        assert!(d.typed(Row::Name, "has space", &[]).is_err());
        assert!(!d.new);
        d.typed(Row::Name, "brick2", &["brick".into()]).unwrap();
        assert!(d.new && d.def.name == "brick2" && d.def.file.ends_with("brick2.mmat"));
    }
}
