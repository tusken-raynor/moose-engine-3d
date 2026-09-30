//! A `.mmp` level as editable tables: the file's sections as they are written (see `Level
//! Format Spec.md`), for an editor to change and write back. Reading one checks only its
//! layout; the level loader validates the rest when the written text is loaded.

use std::path::Path;

use glam::{EulerRot, Quat, Vec3};

use crate::error::LoadError;
use crate::level::{EntityKind, Occluder};
use crate::mmp::Cursor;
use crate::store::MeshId;
use crate::text::{Line, tokenize};

/// A level's sections as tables.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelDoc {
    pub name: String,
    pub vertices: Vec<Vec3>,
    pub attributes: Vec<AttributeDoc>,
    pub sectors: Vec<SectorDoc>,
    pub surfaces: Vec<SurfaceDoc>,
    pub adjoins: Vec<AdjoinDoc>,
    pub entities: Vec<EntityDoc>,
    pub ambient: Option<Vec3>,
    pub lights: Vec<LightDoc>,
    pub directional: Vec<DirectionalDoc>,
}

/// A declared attribute, with its values table (each row's values as written).
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeDoc {
    pub name: String,
    pub format: String,
    pub count: usize,
    pub values: Vec<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SectorDoc {
    pub name: String,
    pub first_surface: usize,
    pub surface_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceDoc {
    pub sector: usize,
    /// Its adjoin, if it is a portal.
    pub adjoin: Option<usize>,
    pub flags: u32,
    /// Its corners: a vertex index, and one values row per attribute (none on a portal).
    pub corners: Vec<(usize, Vec<usize>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdjoinDoc {
    pub surface: usize,
    pub mirror: usize,
    pub flags: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntityDoc {
    pub kind: EntityKind,
    pub sector: usize,
    /// Its model's file name; `None` for a spawn point.
    pub model: Option<String>,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: f32,
    pub name: String,
    /// Its options, as written (`static`, `occluder=...`).
    pub options: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LightDoc {
    pub sector: usize,
    pub position: Vec3,
    pub color: Vec3,
    pub range: f32,
    /// A spot light's direction and cone half-angles (degrees).
    pub spot: Option<(Vec3, f32, f32)>,
    /// Its options, as written (`radius=`, `shadows=`, `oscillate=`).
    pub options: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DirectionalDoc {
    pub direction: Vec3,
    pub color: Vec3,
    pub angle: f32,
    pub options: Vec<String>,
}

impl EntityDoc {
    /// Whether it has option `word` (like `static`).
    pub fn has_option(&self, word: &str) -> bool {
        self.options.iter().any(|o| o == word)
    }

    /// Sets or clears option `word`.
    pub fn set_option(&mut self, word: &str, on: bool) {
        self.options.retain(|o| o != word);
        if on {
            self.options.push(word.to_string());
        }
    }
}

impl LevelDoc {
    /// Reads a level's text (`path` names it in errors).
    pub fn parse(path: &Path, src: &str) -> Result<LevelDoc, LoadError> {
        let lines = tokenize(src).map_err(|(l, m)| LoadError::new(path, Some(l), m))?;
        let mut c = Cursor { path, lines, pos: 0 };
        let (line, args) = c.header("MOOSEMAP", 1)?;
        if args[0] != "1" {
            return Err(c.err(line, format!("unsupported format version '{}'", args[0])));
        }
        let (_, args) = c.header("name", 1)?;
        let name = args[0].clone();
        let vec3 = |c: &Cursor, r: &Line, i: usize| -> Result<Vec3, LoadError> {
            Ok(Vec3::new(c.float(r, i)?, c.float(r, i + 1)?, c.float(r, i + 2)?))
        };
        let section = c.section("vertices", false, Some(3))?;
        let vertices = section
            .rows
            .iter()
            .map(|r| vec3(&c, r, 0))
            .collect::<Result<Vec<_>, _>>()?;
        let section = c.section("attributes", false, Some(3))?;
        let mut attributes = Vec::new();
        for r in &section.rows {
            attributes.push(AttributeDoc {
                name: r.tokens[0].clone(),
                format: r.tokens[1].clone(),
                count: c.parse(r, 2, "component count")?,
                values: Vec::new(),
            });
        }
        for attribute in &mut attributes {
            let section = c.section("values", true, Some(attribute.count))?;
            attribute.values = section.rows.into_iter().map(|r| r.tokens).collect();
        }
        let section = c.section("sectors", false, Some(3))?;
        let mut sectors = Vec::new();
        for r in &section.rows {
            sectors.push(SectorDoc {
                name: r.tokens[0].clone(),
                first_surface: c.parse(r, 1, "surface index")?,
                surface_count: c.parse(r, 2, "surface count")?,
            });
        }
        let section = c.section("surfaces", false, None)?;
        let mut surfaces = Vec::new();
        for r in &section.rows {
            if r.tokens.len() < 4 {
                return Err(c.err(r.no, "surface row needs sector, adjoin, flags and nverts"));
            }
            let adjoin: i64 = c.parse(r, 1, "adjoin index")?;
            let n: usize = c.parse(r, 3, "vertex count")?;
            if r.tokens.len() != 4 + n {
                return Err(c.err(r.no, format!("surface lists {} vertices, not {n}", r.tokens.len() - 4)));
            }
            let corners = r.tokens[4..]
                .iter()
                .map(|t| {
                    let mut parts = t.split(':').map(str::parse::<usize>);
                    let vertex = parts.next().and_then(Result::ok);
                    let rows: Result<Vec<usize>, _> = parts.collect();
                    match (vertex, rows) {
                        (Some(v), Ok(rows)) => Ok((v, rows)),
                        _ => Err(c.err(r.no, format!("'{t}' is not a vertex (like 12 or 12:3)"))),
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            surfaces.push(SurfaceDoc {
                sector: c.parse(r, 0, "sector index")?,
                adjoin: (adjoin >= 0).then_some(adjoin as usize),
                flags: c.hex(r, 2)?,
                corners,
            });
        }
        let section = c.section("adjoins", false, Some(3))?;
        let mut adjoins = Vec::new();
        for r in &section.rows {
            adjoins.push(AdjoinDoc {
                surface: c.parse(r, 0, "surface index")?,
                mirror: c.parse(r, 1, "adjoin index")?,
                flags: c.hex(r, 2)?,
            });
        }
        let section = c.section("entities", false, None)?;
        let mut entities = Vec::new();
        for r in &section.rows {
            if r.tokens.len() < 11 {
                return Err(c.err(r.no, format!("entity row needs 11 fields, found {}", r.tokens.len())));
            }
            let kind = match r.tokens[0].as_str() {
                "spawn" => EntityKind::Spawn,
                "prop" => EntityKind::Prop,
                "actor" => EntityKind::Actor,
                k => return Err(c.err(r.no, format!("unknown entity kind '{k}'"))),
            };
            let (pitch, yaw, roll) = (c.float(r, 6)?, c.float(r, 7)?, c.float(r, 8)?);
            entities.push(EntityDoc {
                kind,
                sector: c.parse(r, 1, "sector index")?,
                model: (r.tokens[2] != "-").then(|| r.tokens[2].clone()),
                position: vec3(&c, r, 3)?,
                rotation: Quat::from_euler(
                    EulerRot::YXZ,
                    yaw.to_radians(),
                    pitch.to_radians(),
                    roll.to_radians(),
                ),
                scale: c.float(r, 9)?,
                name: r.tokens[10].clone(),
                options: r.tokens[11..].to_vec(),
            });
        }
        let at = |c: &Cursor, keyword: &str| c.lines.get(c.pos).is_some_and(|l| l.tokens[0] == keyword);
        let mut ambient = None;
        if at(&c, "ambient") {
            let (line, args) = c.header("ambient", 3)?;
            ambient = Some(vec3(&c, &Line { no: line, tokens: args }, 0)?);
        }
        let mut lights = Vec::new();
        if at(&c, "lights") {
            let section = c.section("lights", false, None)?;
            for r in &section.rows {
                // 8 fields, or 13 for a spot light, then options (words with '=').
                let fields = r.tokens.iter().take_while(|t| !t.contains('=')).count();
                if fields != 8 && fields != 13 {
                    return Err(c.err(r.no, format!("light row needs 8 or 13 fields, found {fields}")));
                }
                lights.push(LightDoc {
                    sector: c.parse(r, 0, "sector index")?,
                    position: vec3(&c, r, 1)?,
                    color: vec3(&c, r, 4)?,
                    range: c.float(r, 7)?,
                    spot: if fields == 13 {
                        Some((vec3(&c, r, 8)?, c.float(r, 11)?, c.float(r, 12)?))
                    } else {
                        None
                    },
                    options: r.tokens[fields..].to_vec(),
                });
            }
        }
        let mut directional = Vec::new();
        if at(&c, "directional") {
            let section = c.section("directional", false, None)?;
            for r in &section.rows {
                if r.tokens.len() < 7 {
                    return Err(c.err(r.no, "directional row needs 7 fields"));
                }
                directional.push(DirectionalDoc {
                    direction: vec3(&c, r, 0)?,
                    color: vec3(&c, r, 3)?,
                    angle: c.float(r, 6)?,
                    options: r.tokens[7..].to_vec(),
                });
            }
        }
        if let Some(extra) = c.lines.get(c.pos) {
            return Err(c.err(extra.no, format!("unexpected '{}' after the last section", extra.tokens[0])));
        }
        Ok(LevelDoc {
            name,
            vertices,
            attributes,
            sectors,
            surfaces,
            adjoins,
            entities,
            ambient,
            lights,
            directional,
        })
    }

    /// The level as `.mmp` text.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let mut line = |s: String| {
            out.push_str(s.trim_end());
            out.push('\n');
        };
        line("MOOSEMAP 1".into());
        line(format!("name {}", quoted(&self.name)));
        line(String::new());
        line(format!("vertices {}", self.vertices.len()));
        line("#  id   x       y       z".into());
        for (i, v) in self.vertices.iter().enumerate() {
            line(format!("   {i:<4} {:<7} {:<7} {}", number(v.x), number(v.y), number(v.z)));
        }
        line(String::new());
        line(format!("attributes {}", self.attributes.len()));
        line("#  id  name  format  count".into());
        for (i, a) in self.attributes.iter().enumerate() {
            line(format!("   {i:<3} {} {} {}", a.name, a.format, a.count));
        }
        for a in &self.attributes {
            line(String::new());
            line(format!("values {} {}", a.name, a.values.len()));
            for (i, row) in a.values.iter().enumerate() {
                line(format!("   {i:<4} {}", row.join(" ")));
            }
        }
        line(String::new());
        line(format!("sectors {}", self.sectors.len()));
        line("#  id  name  first_surface  surface_count".into());
        for (i, s) in self.sectors.iter().enumerate() {
            line(format!("   {i:<3} {} {} {}", quoted(&s.name), s.first_surface, s.surface_count));
        }
        line(String::new());
        line(format!("surfaces {}", self.surfaces.len()));
        line("#  id  sector  adjoin  flags  nverts  vert[:attr ...] ...".into());
        for (i, s) in self.surfaces.iter().enumerate() {
            let corners: Vec<String> = s
                .corners
                .iter()
                .map(|(v, rows)| {
                    std::iter::once(v.to_string())
                        .chain(rows.iter().map(|r| r.to_string()))
                        .collect::<Vec<_>>()
                        .join(":")
                })
                .collect();
            let adjoin = s.adjoin.map_or("-1".to_string(), |a| a.to_string());
            line(format!(
                "   {i:<4} {:<7} {:<7} {:<6} {:<7} {}",
                s.sector,
                adjoin,
                format!("{:#x}", s.flags),
                s.corners.len(),
                corners.join(" ")
            ));
        }
        line(String::new());
        line(format!("adjoins {}", self.adjoins.len()));
        line("#  id  surface  mirror  flags".into());
        for (i, a) in self.adjoins.iter().enumerate() {
            line(format!("   {i:<3} {:<8} {:<7} {:#x}", a.surface, a.mirror, a.flags));
        }
        line(String::new());
        line(format!("entities {}", self.entities.len()));
        line(
            "#  id  kind   sector  model      x       y       z       pitch  yaw    roll   \
             scale  name  [options]"
                .into(),
        );
        for (i, e) in self.entities.iter().enumerate() {
            let kind = match e.kind {
                EntityKind::Spawn => "spawn",
                EntityKind::Prop => "prop",
                EntityKind::Actor => "actor",
            };
            let (yaw, pitch, roll) = e.rotation.to_euler(EulerRot::YXZ);
            line(format!(
                "   {i:<3} {kind:<6} {:<7} {:<10} {:<7} {:<7} {:<7} {:<6} {:<6} {:<6} {:<6} {}  {}",
                e.sector,
                e.model.as_deref().unwrap_or("-"),
                number(e.position.x),
                number(e.position.y),
                number(e.position.z),
                number(pitch.to_degrees()),
                number(yaw.to_degrees()),
                number(roll.to_degrees()),
                number(e.scale),
                quoted(&e.name),
                e.options.join(" "),
            ));
        }
        if let Some(a) = self.ambient {
            line(String::new());
            line(format!("ambient {} {} {}", number(a.x), number(a.y), number(a.z)));
        }
        if !self.lights.is_empty() {
            line(String::new());
            line(format!("lights {}", self.lights.len()));
            line(
                "#  id  sector  x       y       z       r      g      b      range  [dx  dy  dz  \
                 inner  outer]  [options]"
                    .into(),
            );
            for (i, l) in self.lights.iter().enumerate() {
                let mut row = format!(
                    "   {i:<3} {:<7} {:<7} {:<7} {:<7} {:<6} {:<6} {:<6} {:<6}",
                    l.sector,
                    number(l.position.x),
                    number(l.position.y),
                    number(l.position.z),
                    number(l.color.x),
                    number(l.color.y),
                    number(l.color.z),
                    number(l.range),
                );
                if let Some((d, inner, outer)) = l.spot {
                    row.push_str(&format!(
                        " {} {} {} {} {}",
                        number(d.x),
                        number(d.y),
                        number(d.z),
                        number(inner),
                        number(outer)
                    ));
                }
                row.push_str("  ");
                row.push_str(&l.options.join(" "));
                line(row);
            }
        }
        if !self.directional.is_empty() {
            line(String::new());
            line(format!("directional {}", self.directional.len()));
            line("#  id  dx  dy  dz  r  g  b  angle  [options]".into());
            for (i, d) in self.directional.iter().enumerate() {
                line(format!(
                    "   {i:<3} {} {} {} {} {} {} {}  {}",
                    number(d.direction.x),
                    number(d.direction.y),
                    number(d.direction.z),
                    number(d.color.x),
                    number(d.color.y),
                    number(d.color.z),
                    number(d.angle),
                    d.options.join(" ")
                ));
            }
        }
        out
    }

    /// The surfaces of sector `s`, as a range of `surfaces`.
    pub fn sector_surfaces(&self, s: usize) -> std::ops::Range<usize> {
        let sector = &self.sectors[s];
        sector.first_surface..sector.first_surface + sector.surface_count
    }
}

/// The `occluder=` option's value for `occluder`, or `None` for the default (the entity's
/// own model). `model_name` names a proxy model's file.
pub fn occluder_value(
    occluder: Occluder,
    model_name: impl Fn(MeshId) -> Option<String>,
) -> Option<String> {
    match occluder {
        Occluder::Mesh => None,
        Occluder::None => Some("none".into()),
        Occluder::Lod(n) => Some(format!("lod:{n}")),
        Occluder::Model(mesh) => Some(format!("model:{}", model_name(mesh)?)),
        Occluder::Facing { sides, radius, center } => Some(if center == Vec3::ZERO {
            format!("facing:{sides}:{}", number(radius))
        } else {
            format!(
                "facing:{sides}:{}:{}:{}:{}",
                number(radius),
                number(center.x),
                number(center.y),
                number(center.z)
            )
        }),
    }
}

/// A number as the level files write them: two decimals, or up to four if it needs them.
pub fn number(v: f32) -> String {
    let v = if v.abs() < 5e-5 { 0.0 } else { v };
    let two = format!("{v:.2}");
    if (two.parse::<f32>().unwrap_or(v) - v).abs() < 5e-5 {
        return two;
    }
    format!("{v:.4}").trim_end_matches('0').to_string()
}

/// A name as a token: quoted if it has spaces or a comment mark in it.
fn quoted(name: &str) -> String {
    if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == '#' || c == '"') {
        format!("\"{}\"", name.replace('"', ""))
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_keep_two_decimals_or_what_they_need() {
        assert_eq!(number(2.5), "2.50");
        assert_eq!(number(-0.0), "0.00");
        assert_eq!(number(0.3), "0.30");
        assert_eq!(number(1.2345), "1.2345");
        assert_eq!(number(12.125), "12.125");
    }
}
