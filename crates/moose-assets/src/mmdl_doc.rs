//! A `.mmdl` model as editable tables (see `Model Format Spec.md`), for the engine's mesh
//! editor: what Blender doesn't know (shadow proxies, polygon colors) is set here and
//! written back. Reading one checks only its layout; the model loader validates the rest
//! when the written text is loaded.

use std::path::Path;

use glam::Vec3;

use crate::error::LoadError;
use crate::mesh::PolyFlags;
use crate::mmp::Cursor;
use crate::mmp_doc::AttributeDoc;
use crate::text::tokenize;

/// A model's sections as tables.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelDoc {
    pub name: String,
    /// Each position and its bone (`None` in a model without bones).
    pub positions: Vec<(Vec3, Option<usize>)>,
    pub attributes: Vec<AttributeDoc>,
    pub polygons: Vec<ModelPolygonDoc>,
    pub bones: Vec<BoneDoc>,
    pub animations: Vec<AnimationDoc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelPolygonDoc {
    pub flags: u32,
    /// Its corners: a position, and one values row per attribute.
    pub corners: Vec<(usize, Vec<usize>)>,
}

impl ModelPolygonDoc {
    pub fn proxy(&self) -> bool {
        self.flags & PolyFlags::PROXY != 0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoneDoc {
    pub name: String,
    pub parent: Option<usize>,
    /// Its rest pose as written: x y z qx qy qz qw.
    pub rest: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnimationDoc {
    pub name: String,
    pub fps: String,
    pub frames: usize,
    pub looping: bool,
    /// Its poses as written (x y z qx qy qz qw), frame by frame, bone by bone.
    pub poses: Vec<Vec<String>>,
}

impl ModelDoc {
    /// Reads a model's text (`path` names it in errors).
    pub fn parse(path: &Path, src: &str) -> Result<ModelDoc, LoadError> {
        let lines = tokenize(src).map_err(|(l, m)| LoadError::new(path, Some(l), m))?;
        let mut c = Cursor { path, lines, pos: 0 };
        let (line, args) = c.header("MOOSEMODEL", 1)?;
        if args[0] != "1" {
            return Err(c.err(line, format!("unsupported format version '{}'", args[0])));
        }
        let (_, args) = c.header("name", 1)?;
        let name = args[0].clone();

        let mut positions = Vec::new();
        for r in &c.section("positions", false, Some(4))?.rows {
            let bone = match r.tokens[3].as_str() {
                "-" => None,
                _ => Some(c.parse::<usize>(r, 3, "bone index")?),
            };
            positions.push((Vec3::new(c.float(r, 0)?, c.float(r, 1)?, c.float(r, 2)?), bone));
        }

        let mut attributes = Vec::new();
        for r in &c.section("attributes", false, Some(3))?.rows {
            attributes.push(AttributeDoc {
                name: r.tokens[0].clone(),
                format: r.tokens[1].clone(),
                count: c.parse(r, 2, "component count")?,
                values: Vec::new(),
            });
        }
        for a in &mut attributes {
            let section = c.section("values", true, Some(a.count))?;
            if section.name.as_deref() != Some(a.name.as_str()) {
                return Err(c.err(section.line, format!("expected values for '{}'", a.name)));
            }
            a.values = section.rows.into_iter().map(|r| r.tokens).collect();
        }

        let mut polygons = Vec::new();
        for r in &c.section("polygons", false, None)?.rows {
            if r.tokens.len() < 2 {
                return Err(c.err(r.no, "polygon row needs flags and a vertex count"));
            }
            let flags = c.hex(r, 0)?;
            let n: usize = c.parse(r, 1, "vertex count")?;
            if r.tokens.len() != 2 + n {
                return Err(c.err(r.no, format!("polygon needs {n} vertices listed")));
            }
            let mut corners = Vec::with_capacity(n);
            for t in &r.tokens[2..] {
                let refs: Vec<usize> = t
                    .split(':')
                    .map(|s| s.parse().map_err(|_| c.err(r.no, format!("'{t}': bad corner"))))
                    .collect::<Result<_, _>>()?;
                corners.push((refs[0], refs[1..].to_vec()));
            }
            polygons.push(ModelPolygonDoc { flags, corners });
        }

        let at = |c: &Cursor, keyword: &str| c.lines.get(c.pos).is_some_and(|l| l.tokens[0] == keyword);
        let mut bones = Vec::new();
        if at(&c, "bones") {
            for r in &c.section("bones", false, Some(9))?.rows {
                let parent = match r.tokens[1].as_str() {
                    "-" => None,
                    _ => Some(c.parse::<usize>(r, 1, "parent bone")?),
                };
                bones.push(BoneDoc { name: r.tokens[0].clone(), parent, rest: r.tokens[2..].to_vec() });
            }
        }
        let mut animations = Vec::new();
        if at(&c, "animations") {
            for r in &c.section("animations", false, Some(4))?.rows {
                animations.push(AnimationDoc {
                    name: r.tokens[0].clone(),
                    fps: r.tokens[1].clone(),
                    frames: c.parse(r, 2, "frame count")?,
                    looping: r.tokens[3] == "loop",
                    poses: Vec::new(),
                });
            }
            for a in &mut animations {
                let section = c.section("poses", true, Some(9))?;
                if section.name.as_deref() != Some(a.name.as_str()) {
                    return Err(c.err(section.line, format!("expected poses for '{}'", a.name)));
                }
                a.poses = section.rows.into_iter().map(|r| r.tokens[2..].to_vec()).collect();
            }
        }
        Ok(ModelDoc { name, positions, attributes, polygons, bones, animations })
    }

    /// The tables as a `.mmdl` file.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let mut line = |s: String| {
            out.push_str(s.trim_end());
            out.push('\n');
        };
        line("MOOSEMODEL 1".into());
        line(format!("name \"{}\"", self.name.replace('"', "")));
        line(String::new());
        line(format!("positions {}", self.positions.len()));
        line("#  id  x  y  z  bone".into());
        for (i, (p, bone)) in self.positions.iter().enumerate() {
            let bone = bone.map_or("-".to_string(), |b| b.to_string());
            line(format!("   {i:<4} {} {} {}  {bone}", fine(p.x), fine(p.y), fine(p.z)));
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
        line(format!("polygons {}", self.polygons.len()));
        line("#  id  flags  nverts  position[:row ...] ...".into());
        for (i, p) in self.polygons.iter().enumerate() {
            let corners: Vec<String> = p
                .corners
                .iter()
                .map(|(v, rows)| {
                    std::iter::once(v.to_string())
                        .chain(rows.iter().map(|r| r.to_string()))
                        .collect::<Vec<_>>()
                        .join(":")
                })
                .collect();
            line(format!("   {i:<4} {:#x} {}  {}", p.flags, p.corners.len(), corners.join(" ")));
        }
        if !self.bones.is_empty() {
            line(String::new());
            line(format!("bones {}", self.bones.len()));
            line("#  id  name  parent  x  y  z  qx  qy  qz  qw".into());
            for (i, b) in self.bones.iter().enumerate() {
                let parent = b.parent.map_or("-".to_string(), |p| p.to_string());
                line(format!("   {i:<3} {} {parent}  {}", b.name, b.rest.join(" ")));
            }
        }
        if !self.animations.is_empty() {
            line(String::new());
            line(format!("animations {}", self.animations.len()));
            line("#  id  name  fps  frames  loop|once".into());
            for (i, a) in self.animations.iter().enumerate() {
                let looping = if a.looping { "loop" } else { "once" };
                line(format!("   {i:<3} {} {} {} {looping}", a.name, a.fps, a.frames));
            }
            let bones = self.bones.len().max(1);
            for a in &self.animations {
                line(String::new());
                line(format!("poses {} {}", a.name, a.poses.len()));
                line("#  id  frame  bone  x  y  z  qx  qy  qz  qw".into());
                for (k, pose) in a.poses.iter().enumerate() {
                    line(format!("   {k:<5} {} {}  {}", k / bones, k % bones, pose.join(" ")));
                }
            }
        }
        out
    }

    /// The bone polygon `polygon` moves with: its first corner's.
    pub fn polygon_bone(&self, polygon: usize) -> Option<usize> {
        let &(p, _) = self.polygons[polygon].corners.first()?;
        self.positions.get(p).and_then(|&(_, bone)| bone)
    }

    /// Polygon `polygon`'s color (0 to 1), from its first corner's `color` values, if the
    /// model has colors.
    pub fn polygon_color(&self, polygon: usize) -> Option<Vec3> {
        let k = self.attributes.iter().position(|a| a.name == "color")?;
        let (_, rows) = self.polygons[polygon].corners.first()?;
        let row = &self.attributes[k].values[*rows.get(k)?];
        let v = |i: usize| row.get(i).and_then(|t| t.parse::<f32>().ok()).unwrap_or(0.0) / 255.0;
        Some(Vec3::new(v(0), v(1), v(2)))
    }

    /// Paints polygon `polygon` one color (0 to 1): a new `color` row, which all its
    /// corners take (so other polygons sharing rows keep theirs).
    pub fn set_polygon_color(&mut self, polygon: usize, color: Vec3) -> Result<(), String> {
        let k = self
            .attributes
            .iter()
            .position(|a| a.name == "color" && a.count == 3)
            .ok_or("the model has no color attribute")?;
        let c = (color.clamp(Vec3::ZERO, Vec3::ONE) * 255.0).round();
        let row = vec![c.x.to_string(), c.y.to_string(), c.z.to_string()];
        let values = &mut self.attributes[k].values;
        let index = match values.iter().position(|r| *r == row) {
            Some(i) => i,
            None => {
                values.push(row);
                values.len() - 1
            }
        };
        for (_, rows) in &mut self.polygons[polygon].corners {
            rows[k] = index;
        }
        Ok(())
    }

    /// A box shadow proxy around the drawn polygons that move with bone `bone` (all of
    /// them, for `None` or a model without bones), with that bone. Returns its first
    /// polygon.
    pub fn add_proxy_box(&mut self, bone: Option<usize>) -> Result<usize, String> {
        let points: Vec<Vec3> = self
            .polygons
            .iter()
            .enumerate()
            .filter(|(i, p)| !p.proxy() && (bone.is_none() || self.polygon_bone(*i) == bone))
            .flat_map(|(_, p)| p.corners.iter().map(|&(v, _)| self.positions[v].0))
            .collect();
        if points.is_empty() {
            return Err("no drawn polygons move with that bone".into());
        }
        let lo = points.iter().copied().fold(Vec3::INFINITY, Vec3::min);
        let hi = points.iter().copied().fold(Vec3::NEG_INFINITY, Vec3::max);
        if (hi - lo).min_element() < 1e-3 {
            return Err("its polygons are flat: no box around them".into());
        }
        let bone = if self.bones.is_empty() { None } else { Some(bone.unwrap_or(0)) };
        let first = self.positions.len();
        for k in 0..8 {
            let pick = |bit: usize, a: f32, b: f32| if k & bit == 0 { a } else { b };
            self.positions.push((Vec3::new(pick(1, lo.x, hi.x), pick(2, lo.y, hi.y), pick(4, lo.z, hi.z)), bone));
        }
        // Each attribute's first row (the proxy isn't drawn), made if there is none.
        let rows: Vec<usize> = self
            .attributes
            .iter_mut()
            .map(|a| {
                if a.values.is_empty() {
                    a.values.push(vec!["0".to_string(); a.count]);
                }
                0
            })
            .collect();
        let polygon = self.polygons.len();
        // Corners by bits (x 1, y 2, z 4), counter-clockwise from outside.
        for face in [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]] {
            self.polygons.push(ModelPolygonDoc {
                flags: PolyFlags::PROXY,
                corners: face.iter().map(|&c| (first + c, rows.clone())).collect(),
            });
        }
        Ok(polygon)
    }

    /// Removes polygon `polygon`, and positions no polygon uses any more.
    pub fn remove_polygon(&mut self, polygon: usize) {
        self.polygons.remove(polygon);
        self.drop_unused_positions();
    }

    /// Removes the shadow proxies that move with bone `bone` (all, for `None`). Returns how
    /// many polygons went.
    pub fn remove_proxies(&mut self, bone: Option<usize>) -> usize {
        let before = self.polygons.len();
        let keep: Vec<bool> = (0..before)
            .map(|i| !self.polygons[i].proxy() || (bone.is_some() && self.polygon_bone(i) != bone))
            .collect();
        let mut k = 0;
        self.polygons.retain(|_| {
            k += 1;
            keep[k - 1]
        });
        self.drop_unused_positions();
        before - self.polygons.len()
    }

    fn drop_unused_positions(&mut self) {
        let mut used = vec![false; self.positions.len()];
        for p in &self.polygons {
            for &(v, _) in &p.corners {
                used[v] = true;
            }
        }
        let mut remap = vec![usize::MAX; self.positions.len()];
        let mut kept = Vec::new();
        for (i, &position) in self.positions.iter().enumerate() {
            if used[i] {
                remap[i] = kept.len();
                kept.push(position);
            }
        }
        self.positions = kept;
        for p in &mut self.polygons {
            for (v, _) in &mut p.corners {
                *v = remap[*v];
            }
        }
    }
}

/// A coordinate for the file: two decimals when that's exact, otherwise up to six.
fn fine(v: f32) -> String {
    let v = if v.abs() < 5e-7 { 0.0 } else { v };
    let two = format!("{v:.2}");
    if two.parse::<f32>().is_ok_and(|t| (t - v).abs() < 5e-7) {
        return two;
    }
    let six = format!("{v:.6}");
    let six = six.trim_end_matches('0');
    six.strip_suffix('.').unwrap_or(six).to_string()
}
