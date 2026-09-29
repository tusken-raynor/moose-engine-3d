//! Parser and validator for `.mmp` levels. See `Level Format Spec.md`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;

use glam::{EulerRot, Quat, Vec3};

use crate::error::LoadError;
use crate::geom::{Aabb, Plane, TOLERANCE, convex_polygon_plane};
use crate::level::{
    DirectionalLight, EntityKind, EntitySpawn, Level, Light, Occluder, Oscillation, Portal,
    PortalFlags, Sector,
};
use crate::mesh::{AttribData, MeshBuilder, PolyFlags, StorageFormat};
use crate::store::Assets;
use crate::text::{Line, tokenize};

struct SectorRec {
    line: usize,
    name: String,
    first: usize,
    count: usize,
}

struct SurfaceRec {
    line: usize,
    sector: usize,
    adjoin: Option<usize>,
    flags: u32,
    verts: Vec<usize>,
    /// Per vertex, one row index per declared attribute. Empty for portals.
    attr_rows: Vec<Vec<usize>>,
    plane: Plane,
}

struct AdjoinRec {
    line: usize,
    surface: usize,
    mirror: usize,
    flags: u32,
}

struct EntityRec {
    line: usize,
    name: String,
    kind: EntityKind,
    sector: usize,
    model: Option<String>,
    position: Vec3,
    rotation: Quat,
    scale: f32,
    is_static: bool,
    occluder: OccluderRec,
}

/// An entity's occluder as written: a proxy model is loaded with the entity's.
enum OccluderRec {
    Ready(Occluder),
    Model(String),
}

/// Parses an `occluder=` option's value: `none`, `mesh`, `lod:N`, `model:FILE` or
/// `facing:SIDES:RADIUS[:X:Y:Z]`.
fn occluder_option(value: &str) -> Result<OccluderRec, String> {
    let parts: Vec<&str> = value.split(':').collect();
    let number = |s: &str| {
        s.parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("'{s}' is not a number"))
    };
    Ok(match parts.as_slice() {
        ["none"] => OccluderRec::Ready(Occluder::None),
        ["mesh"] => OccluderRec::Ready(Occluder::Mesh),
        ["lod", n] => OccluderRec::Ready(Occluder::Lod(
            n.parse().map_err(|_| format!("'{n}' is not a level of detail"))?,
        )),
        ["model", file] if !file.is_empty() => OccluderRec::Model(file.to_string()),
        ["facing", sides, radius, rest @ ..] if rest.is_empty() || rest.len() == 3 => {
            let sides: u8 = sides
                .parse()
                .ok()
                .filter(|n| (3..=64).contains(n))
                .ok_or_else(|| format!("facing polygons have 3 to 64 sides, not '{sides}'"))?;
            let radius = number(radius)?;
            if radius <= 0.0 {
                return Err("a facing polygon's radius must be positive".to_string());
            }
            let center = match rest {
                [x, y, z] => Vec3::new(number(x)?, number(y)?, number(z)?),
                _ => Vec3::ZERO,
            };
            OccluderRec::Ready(Occluder::Facing { sides, radius, center })
        }
        _ => {
            return Err(format!(
                "unknown occluder '{value}' (expected none, mesh, lod:N, model:FILE or \
                 facing:SIDES:RADIUS[:X:Y:Z])"
            ));
        }
    })
}

struct Cursor<'a> {
    path: &'a Path,
    lines: Vec<Line>,
    pos: usize,
}

impl Cursor<'_> {
    fn err(&self, line: usize, msg: impl Into<String>) -> LoadError {
        LoadError::new(self.path, Some(line), msg)
    }

    /// Reads a header line `keyword arg...`, returning its line number and arguments.
    fn header(&mut self, keyword: &str, args: usize) -> Result<(usize, Vec<String>), LoadError> {
        let Some(line) = self.lines.get(self.pos) else {
            return Err(LoadError::new(
                self.path,
                None,
                format!("missing '{keyword}' section"),
            ));
        };
        if line.tokens[0] != keyword {
            return Err(self.err(
                line.no,
                format!("expected '{keyword}', found '{}'", line.tokens[0]),
            ));
        }
        if line.tokens.len() != args + 1 {
            return Err(self.err(line.no, format!("'{keyword}' takes {args} argument(s)")));
        }
        self.pos += 1;
        Ok((line.no, line.tokens[1..].to_vec()))
    }

    /// Reads a section header `keyword [name] count` and its rows. Each row must
    /// start with its sequential id; rows are returned without it.
    fn section(
        &mut self,
        keyword: &str,
        named: bool,
        fields: Option<usize>,
    ) -> Result<Section, LoadError> {
        let (line, args) = self.header(keyword, if named { 2 } else { 1 })?;
        let count_arg = args.last().unwrap();
        let count: usize = count_arg
            .parse()
            .map_err(|_| self.err(line, format!("'{count_arg}' is not a valid count")))?;
        let mut rows = Vec::with_capacity(count);
        for i in 0..count {
            let Some(row) = self.lines.get(self.pos) else {
                return Err(self.err(
                    line,
                    format!("'{keyword}' declares {count} rows, but the file ends after {i}"),
                ));
            };
            if row.tokens[0].parse::<usize>().ok() != Some(i) {
                return Err(self.err(
                    row.no,
                    format!(
                        "'{keyword}' row has id '{}', expected {i} (declared {count} rows)",
                        row.tokens[0]
                    ),
                ));
            }
            if let Some(n) = fields.filter(|&n| row.tokens.len() - 1 != n) {
                return Err(self.err(
                    row.no,
                    format!(
                        "'{keyword}' row {i} needs {n} fields, found {}",
                        row.tokens.len() - 1
                    ),
                ));
            }
            rows.push(Line {
                no: row.no,
                tokens: row.tokens[1..].to_vec(),
            });
            self.pos += 1;
        }
        Ok(Section {
            line,
            name: named.then(|| args[0].clone()),
            rows,
        })
    }

    fn parse<T: FromStr>(&self, row: &Line, i: usize, what: &str) -> Result<T, LoadError> {
        let token = &row.tokens[i];
        token
            .parse()
            .map_err(|_| self.err(row.no, format!("'{token}' is not a valid {what}")))
    }

    fn float(&self, row: &Line, i: usize) -> Result<f32, LoadError> {
        let x: f32 = self.parse(row, i, "number")?;
        if x.is_finite() {
            Ok(x)
        } else {
            Err(self.err(
                row.no,
                format!("'{}' is not a finite number", row.tokens[i]),
            ))
        }
    }

    fn hex(&self, row: &Line, i: usize) -> Result<u32, LoadError> {
        let token = &row.tokens[i];
        token
            .strip_prefix("0x")
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .ok_or_else(|| {
                self.err(
                    row.no,
                    format!("'{token}' is not a valid hex value (like 0x3)"),
                )
            })
    }
}

struct Section {
    line: usize,
    name: Option<String>,
    rows: Vec<Line>,
}

pub(crate) fn parse_mmp(assets: &mut Assets, path: &Path, src: &str) -> Result<Level, LoadError> {
    let lines = tokenize(src).map_err(|(l, m)| LoadError::new(path, Some(l), m))?;
    let mut c = Cursor {
        path,
        lines,
        pos: 0,
    };

    // ---- Header and name
    let (line, args) = c.header("MOOSEMAP", 1)?;
    if args[0] != "1" {
        return Err(c.err(line, format!("unsupported format version '{}'", args[0])));
    }
    let (_, args) = c.header("name", 1)?;
    let level_name = args[0].clone();

    // ---- Vertices
    let section = c.section("vertices", false, Some(3))?;
    let vertices = section
        .rows
        .iter()
        .map(|r| Ok(Vec3::new(c.float(r, 0)?, c.float(r, 1)?, c.float(r, 2)?)))
        .collect::<Result<Vec<_>, LoadError>>()?;

    // ---- Attribute declarations, then one values table per attribute
    let section = c.section("attributes", false, Some(3))?;
    let mut decls: Vec<(String, u8, StorageFormat)> = Vec::new();
    for r in &section.rows {
        let name = r.tokens[0].clone();
        if decls.iter().any(|d| d.0 == name) {
            return Err(c.err(r.no, format!("attribute '{name}' is declared twice")));
        }
        let format = StorageFormat::parse(&r.tokens[1]).ok_or_else(|| {
            c.err(
                r.no,
                format!(
                    "unknown storage format '{}' (expected u8, i8, i16, f16 or f32)",
                    r.tokens[1]
                ),
            )
        })?;
        let count: u8 = c.parse(r, 2, "component count (1-255)")?;
        if count == 0 {
            return Err(c.err(r.no, "component count must be at least 1"));
        }
        decls.push((name, count, format));
    }
    let mut tables: Vec<AttribData> = Vec::new();
    for (name, count, format) in &decls {
        let section = c.section("values", true, Some(*count as usize))?;
        let found = section.name.as_deref().unwrap();
        if found != name {
            return Err(c.err(
                section.line,
                format!("expected values for attribute '{name}', found '{found}'"),
            ));
        }
        let mut data = AttribData::new(*format);
        for r in &section.rows {
            for token in &r.tokens {
                data.push_token(token)
                    .map_err(|m| c.err(r.no, format!("values {name}: {m}")))?;
            }
        }
        tables.push(data);
    }
    let table_rows = |k: usize| tables[k].len() / decls[k].1 as usize;

    // ---- Sectors
    let sector_section = c.section("sectors", false, Some(3))?;
    let mut sectors = Vec::new();
    for r in &sector_section.rows {
        sectors.push(SectorRec {
            line: r.no,
            name: r.tokens[0].clone(),
            first: c.parse(r, 1, "surface index")?,
            count: c.parse(r, 2, "surface count")?,
        });
    }

    // ---- Surfaces
    let section = c.section("surfaces", false, None)?;
    let mut surfaces = Vec::new();
    for (i, r) in section.rows.iter().enumerate() {
        if r.tokens.len() < 4 {
            return Err(c.err(
                r.no,
                format!("surface {i} needs sector, adjoin, flags and nverts"),
            ));
        }
        let sector: usize = c.parse(r, 0, "sector index")?;
        if sector >= sectors.len() {
            return Err(c.err(r.no, format!("surface {i}: sector {sector} does not exist")));
        }
        let adjoin: i64 = c.parse(r, 1, "adjoin index")?;
        let adjoin = match adjoin {
            -1 => None,
            a if a >= 0 => Some(a as usize),
            _ => {
                return Err(c.err(
                    r.no,
                    format!("surface {i}: adjoin must be -1 or an adjoin index"),
                ));
            }
        };
        let flags = c.hex(r, 2)?;
        if flags & !PolyFlags::ALL != 0 {
            return Err(c.err(
                r.no,
                format!("surface {i}: unknown surface flags 0x{flags:x}"),
            ));
        }
        if flags & PolyFlags::ALL == PolyFlags::ALL {
            return Err(c.err(
                r.no,
                format!("surface {i}: a surface cannot be both reflective and sky"),
            ));
        }
        if flags != 0 && adjoin.is_some() {
            return Err(c.err(
                r.no,
                format!("surface {i}: portal surfaces take no surface flags"),
            ));
        }
        let nverts: usize = c.parse(r, 3, "vertex count")?;
        let refs = &r.tokens[4..];
        if refs.len() != nverts {
            return Err(c.err(
                r.no,
                format!(
                    "surface {i} declares {nverts} vertices but lists {}",
                    refs.len()
                ),
            ));
        }
        let mut verts = Vec::with_capacity(nverts);
        let mut attr_rows = Vec::new();
        for token in refs {
            let parts = token
                .split(':')
                .map(|p| p.parse::<usize>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| {
                    c.err(
                        r.no,
                        format!("surface {i}: '{token}' is not a valid vertex reference"),
                    )
                })?;
            let v = parts[0];
            if v >= vertices.len() {
                return Err(c.err(r.no, format!("surface {i}: vertex {v} does not exist")));
            }
            verts.push(v);
            let rows = &parts[1..];
            if adjoin.is_some() {
                if !rows.is_empty() {
                    return Err(c.err(
                        r.no,
                        format!(
                            "surface {i}: portal surfaces list vertex indices only, found '{token}'"
                        ),
                    ));
                }
                continue;
            }
            if rows.len() != decls.len() {
                return Err(c.err(
                    r.no,
                    format!("surface {i}: '{token}' references {} attribute rows, {} attributes are declared", rows.len(), decls.len()),
                ));
            }
            for (k, &row) in rows.iter().enumerate() {
                if row >= table_rows(k) {
                    return Err(c.err(
                        r.no,
                        format!("surface {i}: {} row {row} does not exist", decls[k].0),
                    ));
                }
            }
            attr_rows.push(rows.to_vec());
        }
        let points: Vec<Vec3> = verts.iter().map(|&v| vertices[v]).collect();
        let plane =
            convex_polygon_plane(&points).map_err(|m| c.err(r.no, format!("surface {i}: {m}")))?;
        surfaces.push(SurfaceRec {
            line: r.no,
            sector,
            adjoin,
            flags,
            verts,
            attr_rows,
            plane,
        });
    }

    // ---- Adjoins
    let section = c.section("adjoins", false, Some(3))?;
    let mut adjoins = Vec::new();
    for r in &section.rows {
        adjoins.push(AdjoinRec {
            line: r.no,
            surface: c.parse(r, 0, "surface index")?,
            mirror: c.parse(r, 1, "adjoin index")?,
            flags: c.hex(r, 2)?,
        });
    }

    // ---- Entities
    let section = c.section("entities", false, None)?;
    let mut entities = Vec::new();
    for r in &section.rows {
        if r.tokens.len() < 11 {
            return Err(c.err(
                r.no,
                format!("entity row needs 11 fields, found {}", r.tokens.len()),
            ));
        }
        let name = r.tokens[10].clone();
        let kind = match r.tokens[0].as_str() {
            "spawn" => EntityKind::Spawn,
            "prop" => EntityKind::Prop,
            "actor" => EntityKind::Actor,
            k => {
                return Err(c.err(
                    r.no,
                    format!("entity '{name}': unknown kind '{k}' (expected spawn, prop or actor)"),
                ));
            }
        };
        let sector: usize = c.parse(r, 1, "sector index")?;
        if sector >= sectors.len() {
            return Err(c.err(
                r.no,
                format!("entity '{name}': sector {sector} does not exist"),
            ));
        }
        let model = (r.tokens[2] != "-").then(|| r.tokens[2].clone());
        match (kind, &model) {
            (EntityKind::Spawn, Some(_)) => {
                return Err(c.err(
                    r.no,
                    format!("entity '{name}': spawn points take no model ('-')"),
                ));
            }
            (EntityKind::Prop | EntityKind::Actor, None) => {
                return Err(c.err(r.no, format!("entity '{name}': needs a model")));
            }
            _ => {}
        }
        let position = Vec3::new(c.float(r, 3)?, c.float(r, 4)?, c.float(r, 5)?);
        let (pitch, yaw, roll) = (c.float(r, 6)?, c.float(r, 7)?, c.float(r, 8)?);
        let rotation = Quat::from_euler(
            EulerRot::YXZ,
            yaw.to_radians(),
            pitch.to_radians(),
            roll.to_radians(),
        );
        let scale = c.float(r, 9)?;
        if scale <= 0.0 {
            return Err(c.err(r.no, format!("entity '{name}': scale must be positive")));
        }
        // Options after the name.
        let (mut is_static, mut occluder) = (false, OccluderRec::Ready(Occluder::Mesh));
        for option in &r.tokens[11..] {
            let fail = |m: String| c.err(r.no, format!("entity '{name}': {m}"));
            if kind == EntityKind::Spawn {
                return Err(fail(format!("spawn points take no options ('{option}')")));
            }
            match option.split_once('=') {
                None if option == "static" => {
                    if kind == EntityKind::Actor {
                        return Err(fail("only props can be static".to_string()));
                    }
                    is_static = true;
                }
                Some(("occluder", value)) => occluder = occluder_option(value).map_err(fail)?,
                _ => return Err(fail(format!("unknown option '{option}'"))),
            }
        }
        entities.push(EntityRec {
            line: r.no,
            name,
            kind,
            sector,
            model,
            position,
            rotation,
            scale,
            is_static,
            occluder,
        });
    }

    // Optional: ambient light (1 1 1 without it: surfaces show their full color), then
    // point lights.
    let mut ambient = Vec3::ONE;
    if c.lines.get(c.pos).is_some_and(|l| l.tokens[0] == "ambient") {
        let (line, args) = c.header("ambient", 3)?;
        let row = Line {
            no: line,
            tokens: args,
        };
        ambient = Vec3::new(c.float(&row, 0)?, c.float(&row, 1)?, c.float(&row, 2)?);
        if ambient.min_element() < 0.0 {
            return Err(c.err(line, "ambient light cannot be negative".to_string()));
        }
    }
    let mut lights = Vec::new();
    if c.lines.get(c.pos).is_some_and(|l| l.tokens[0] == "lights") {
        let section = c.section("lights", false, None)?;
        for (i, r) in section.rows.iter().enumerate() {
            // A point light, or a spot light with its direction and cone; then options.
            let fields = r.tokens.iter().take_while(|t| !t.contains('=')).count();
            if fields != 8 && fields != 13 {
                return Err(c.err(
                    r.no,
                    format!(
                        "light {i} needs 8 fields (a point light) or 13 (a spot light), found {fields}"
                    ),
                ));
            }
            let sector: usize = c.parse(r, 0, "sector index")?;
            if sector >= sectors.len() {
                return Err(c.err(r.no, format!("light {i}: sector {sector} does not exist")));
            }
            let position = Vec3::new(c.float(r, 1)?, c.float(r, 2)?, c.float(r, 3)?);
            let color = Vec3::new(c.float(r, 4)?, c.float(r, 5)?, c.float(r, 6)?);
            let range = c.float(r, 7)?;
            if color.min_element() < 0.0 {
                return Err(c.err(r.no, format!("light {i}: color cannot be negative")));
            }
            if range <= 0.0 {
                return Err(c.err(r.no, format!("light {i}: range must be positive")));
            }
            let mut light = if fields == 8 {
                Light::point(sector as u32, position, color, range)
            } else {
                let direction = Vec3::new(c.float(r, 8)?, c.float(r, 9)?, c.float(r, 10)?);
                let (inner, outer) = (c.float(r, 11)?, c.float(r, 12)?);
                if direction.length() < 1e-6 {
                    return Err(c.err(r.no, format!("light {i}: direction cannot be zero")));
                }
                if !(0.0 <= inner && inner <= outer && outer <= 180.0) {
                    return Err(c.err(
                        r.no,
                        format!("light {i}: cone angles need 0 <= inner <= outer <= 180"),
                    ));
                }
                Light::spot(sector as u32, position, color, range, direction, inner, outer)
            };
            light.is_static = true;
            for option in &r.tokens[fields..] {
                let fail = |m: String| c.err(r.no, format!("light {i}: {m}"));
                match option.split_once('=') {
                    Some(("radius", v)) => {
                        light.radius = v
                            .parse::<f32>()
                            .ok()
                            .filter(|r| r.is_finite() && *r >= 0.0)
                            .ok_or_else(|| fail(format!("radius '{v}' must be 0 or more")))?;
                    }
                    Some(("shadows", "on")) => light.shadows = true,
                    Some(("shadows", "off")) => light.shadows = false,
                    Some(("oscillate", v)) => {
                        let n: Vec<f32> = v
                            .split(':')
                            .map(|x| x.parse::<f32>().ok().filter(|x| x.is_finite()))
                            .collect::<Option<_>>()
                            .filter(|n: &Vec<f32>| n.len() == 4)
                            .ok_or_else(|| fail(format!("oscillate is DX:DY:DZ:PERIOD, not '{v}'")))?;
                        if n[3] <= 0.0 {
                            return Err(fail("an oscillation's period must be positive".into()));
                        }
                        light.motion = Some(Oscillation {
                            offset: Vec3::new(n[0], n[1], n[2]),
                            period: n[3],
                        });
                        // It moves: its shadows are worked out as it goes, not kept.
                        light.is_static = false;
                    }
                    _ => return Err(fail(format!("unknown option '{option}'"))),
                }
            }
            lights.push((r.no, light));
        }
    }

    // Optional: directional lights, entering through sky surfaces.
    let mut directional = Vec::new();
    if c.lines.get(c.pos).is_some_and(|l| l.tokens[0] == "directional") {
        let section = c.section("directional", false, None)?;
        for (i, r) in section.rows.iter().enumerate() {
            let fields = r.tokens.iter().take_while(|t| !t.contains('=')).count();
            if fields != 7 {
                return Err(c.err(
                    r.no,
                    format!("directional light {i} needs 7 fields, found {fields}"),
                ));
            }
            let direction = Vec3::new(c.float(r, 0)?, c.float(r, 1)?, c.float(r, 2)?);
            let color = Vec3::new(c.float(r, 3)?, c.float(r, 4)?, c.float(r, 5)?);
            let angle = c.float(r, 6)?;
            if direction.length() < 1e-6 {
                return Err(c.err(r.no, format!("directional light {i}: direction cannot be zero")));
            }
            if color.min_element() < 0.0 {
                return Err(c.err(r.no, format!("directional light {i}: color cannot be negative")));
            }
            if !(0.0..=45.0).contains(&angle) {
                return Err(c.err(
                    r.no,
                    format!("directional light {i}: angle must be from 0 to 45 degrees"),
                ));
            }
            let mut shadows = true;
            for option in &r.tokens[fields..] {
                match option.as_str() {
                    "shadows=on" => shadows = true,
                    "shadows=off" => shadows = false,
                    _ => {
                        return Err(c.err(
                            r.no,
                            format!("directional light {i}: unknown option '{option}'"),
                        ));
                    }
                }
            }
            directional.push(DirectionalLight {
                direction: direction.normalize(),
                color,
                angle,
                shadows,
            });
        }
    }

    if let Some(extra) = c.lines.get(c.pos) {
        return Err(c.err(
            extra.no,
            format!("unexpected '{}' after the last section", extra.tokens[0]),
        ));
    }

    // ---- Validation: sector ranges
    let mut expect = 0;
    for s in &sectors {
        if s.first != expect {
            return Err(c.err(
                s.line,
                format!("sector '{}': surfaces start at {}, expected {expect} (ranges must be contiguous and in order)", s.name, s.first),
            ));
        }
        if s.count == 0 {
            return Err(c.err(s.line, format!("sector '{}' has no surfaces", s.name)));
        }
        expect += s.count;
    }
    if expect != surfaces.len() {
        return Err(c.err(
            sector_section.line,
            format!(
                "sector ranges cover {expect} surfaces, but there are {}",
                surfaces.len()
            ),
        ));
    }
    for (si, s) in sectors.iter().enumerate() {
        for (i, surf) in surfaces.iter().enumerate().skip(s.first).take(s.count) {
            if surf.sector != si {
                return Err(c.err(
                    surf.line,
                    format!(
                        "surface {i} is in sector '{}' 's range but names sector {}",
                        s.name, surf.sector
                    ),
                ));
            }
        }
    }

    // ---- Validation: each sector faces inward, is convex, and is closed
    let mut centers = Vec::with_capacity(sectors.len());
    let mut bounds = Vec::with_capacity(sectors.len());
    for s in &sectors {
        let range = s.first..s.first + s.count;
        let mut ids: Vec<usize> = range
            .clone()
            .flat_map(|i| surfaces[i].verts.iter().copied())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let points: Vec<Vec3> = ids.iter().map(|&v| vertices[v]).collect();
        let center = points.iter().copied().sum::<Vec3>() / points.len() as f32;
        let size = points
            .iter()
            .map(|p| p.distance(center))
            .fold(0.0, f32::max);
        let tol = TOLERANCE * size.max(1.0);

        for i in range.clone() {
            let surf = &surfaces[i];
            if surf.plane.distance(center) <= tol {
                return Err(c.err(
                    surf.line,
                    format!(
                        "surface {i} does not face into sector '{}' (check its winding)",
                        s.name
                    ),
                ));
            }
            if points.iter().any(|&p| surf.plane.distance(p) < -tol) {
                return Err(c.err(
                    surf.line,
                    format!(
                        "sector '{}' is not convex: a vertex lies behind surface {i}",
                        s.name
                    ),
                ));
            }
        }

        let mut edges: HashMap<(usize, usize), usize> = HashMap::new();
        for i in range.clone() {
            let v = &surfaces[i].verts;
            for k in 0..v.len() {
                let edge = (v[k], v[(k + 1) % v.len()]);
                if let Some(other) = edges.insert(edge, i) {
                    return Err(c.err(
                        surfaces[i].line,
                        format!("sector '{}': edge {}->{} is used by surfaces {other} and {i} (inconsistent winding)", s.name, edge.0, edge.1),
                    ));
                }
            }
        }
        for i in range {
            let v = &surfaces[i].verts;
            for k in 0..v.len() {
                let (a, b) = (v[k], v[(k + 1) % v.len()]);
                if !edges.contains_key(&(b, a)) {
                    return Err(c.err(
                        surfaces[i].line,
                        format!("sector '{}' is not closed: edge {a}->{b} of surface {i} has no matching edge (a hole or T-junction)", s.name),
                    ));
                }
            }
        }
        centers.push(center);
        bounds.push(Aabb::from_points(points));
    }

    // ---- Validation: adjoins
    for (ai, a) in adjoins.iter().enumerate() {
        let err = |msg: String| Err(c.err(a.line, format!("adjoin {ai}: {msg}")));
        if a.surface >= surfaces.len() {
            return err(format!("surface {} does not exist", a.surface));
        }
        if surfaces[a.surface].adjoin != Some(ai) {
            return err(format!("surface {} does not name this adjoin", a.surface));
        }
        if a.mirror >= adjoins.len() || a.mirror == ai || adjoins[a.mirror].mirror != ai {
            return err(format!(
                "mirror {} does not point back to this adjoin",
                a.mirror
            ));
        }
        if a.flags & !PortalFlags::ALL != 0 {
            return err(format!("unknown flags 0x{:x}", a.flags));
        }
        let ms = adjoins[a.mirror].surface;
        let (verts, mut reversed) = (&surfaces[a.surface].verts, surfaces[ms].verts.clone());
        reversed.reverse();
        let rotation = reversed.iter().position(|&v| v == verts[0]);
        if rotation.is_none_or(|k| {
            reversed.rotate_left(k);
            &reversed != verts
        }) {
            return err(format!(
                "surfaces {} and {ms} must list the same vertices in reverse order",
                a.surface
            ));
        }
        if surfaces[a.surface].sector == surfaces[ms].sector {
            return err("both sides are in the same sector".into());
        }
    }
    for (i, surf) in surfaces.iter().enumerate() {
        if let Some(a) = surf.adjoin
            && (a >= adjoins.len() || adjoins[a].surface != i)
        {
            return Err(c.err(
                surf.line,
                format!("surface {i}: adjoin {a} does not name this surface"),
            ));
        }
    }

    // ---- Validation: entities sit inside their sector
    for e in &entities {
        let s = &sectors[e.sector];
        let tol = TOLERANCE
            * (bounds[e.sector].max - bounds[e.sector].min)
                .length()
                .max(1.0);
        if (s.first..s.first + s.count).any(|i| surfaces[i].plane.distance(e.position) < -tol) {
            return Err(c.err(
                e.line,
                format!("entity '{}': origin is outside sector '{}'", e.name, s.name),
            ));
        }
    }

    // ---- Validation: moving lights stay inside the level (both ends of their swing)
    let inside_level = |p: Vec3| {
        sectors.iter().enumerate().any(|(si, s)| {
            let tol = TOLERANCE * (bounds[si].max - bounds[si].min).length().max(1.0);
            (s.first..s.first + s.count).all(|k| surfaces[k].plane.distance(p) >= -tol)
        })
    };
    for (i, (line, light)) in lights.iter().enumerate() {
        if let Some(m) = light.motion
            && !(inside_level(light.position + m.offset) && inside_level(light.position - m.offset))
        {
            return Err(c.err(*line, format!("light {i} swings outside the level")));
        }
    }

    // ---- Validation: lights sit inside their sector
    for (i, (line, light)) in lights.iter().enumerate() {
        let sector = light.sector as usize;
        let s = &sectors[sector];
        let tol = TOLERANCE * (bounds[sector].max - bounds[sector].min).length().max(1.0);
        if (s.first..s.first + s.count).any(|k| surfaces[k].plane.distance(light.position) < -tol)
        {
            return Err(c.err(*line, format!("light {i} is outside sector '{}'", s.name)));
        }
    }

    // ---- Build: geometry mesh (solid surfaces) and portals, in sector order
    let mut portal_ids = vec![0u32; adjoins.len()];
    let mut next = 0;
    for surf in &surfaces {
        if let Some(a) = surf.adjoin {
            portal_ids[a] = next;
            next += 1;
        }
    }
    // The vertex table becomes the mesh's positions in order; unused entries are dropped at finish.
    let mut builder = MeshBuilder::new(&level_name, vertices, decls.iter().cloned());
    let mut sectors_out = Vec::with_capacity(sectors.len());
    let mut portals = Vec::with_capacity(adjoins.len());
    for (si, s) in sectors.iter().enumerate() {
        let (first_polygon, first_portal) = (builder.polygon_count(), portals.len() as u32);
        for surf in &surfaces[s.first..s.first + s.count] {
            let indices: Vec<u32> = surf.verts.iter().map(|&v| v as u32).collect();
            match surf.adjoin {
                None => {
                    builder
                        .push_polygon(&indices, PolyFlags(surf.flags))
                        .map_err(|m| c.err(surf.line, m))?;
                    for rows in &surf.attr_rows {
                        for (k, &row) in rows.iter().enumerate() {
                            let n = decls[k].1 as usize;
                            builder
                                .attrib_data(k)
                                .extend_from(&tables[k], row * n..(row + 1) * n);
                        }
                    }
                }
                Some(a) => {
                    let mirror = &adjoins[adjoins[a].mirror];
                    portals.push(Portal {
                        sector: si as u32,
                        target: surfaces[mirror.surface].sector as u32,
                        mirror: portal_ids[adjoins[a].mirror],
                        positions: indices,
                        plane: surf.plane,
                        flags: PortalFlags(adjoins[a].flags),
                    });
                }
            }
        }
        sectors_out.push(Sector {
            name: s.name.clone(),
            polygons: first_polygon..builder.polygon_count(),
            portals: first_portal..portals.len() as u32,
            bounds: bounds[si],
            center: centers[si],
        });
    }

    // ---- Models, then register the geometry
    let mut spawns = Vec::with_capacity(entities.len());
    let mut used_names = HashSet::new();
    for e in entities {
        if !used_names.insert(e.name.clone()) {
            return Err(c.err(e.line, format!("entity name '{}' is used twice", e.name)));
        }
        let mesh = match &e.model {
            Some(model) => Some(
                assets
                    .load_mesh(model)
                    .map_err(|err| c.err(e.line, format!("entity '{}': {err}", e.name)))?,
            ),
            None => None,
        };
        let occluder = match e.occluder {
            OccluderRec::Ready(o) => o,
            OccluderRec::Model(file) => Occluder::Model(
                assets
                    .load_mesh(&file)
                    .map_err(|err| c.err(e.line, format!("entity '{}': occluder: {err}", e.name)))?,
            ),
        };
        spawns.push(EntitySpawn {
            name: e.name,
            kind: e.kind,
            sector: e.sector as u32,
            mesh,
            position: e.position,
            rotation: e.rotation,
            scale: e.scale,
            is_static: e.is_static,
            occluder,
        });
    }
    let mut outlines: Vec<&mut Vec<u32>> = portals.iter_mut().map(|p| &mut p.positions).collect();
    let geometry = assets.add_mesh(builder.finish(&mut outlines));

    Ok(Level {
        name: level_name,
        geometry,
        sectors: sectors_out,
        portals,
        spawns,
        ambient,
        lights: lights.into_iter().map(|(_, l)| l).collect(),
        directional,
    })
}
