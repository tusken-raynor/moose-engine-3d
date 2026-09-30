//! Editing a level's geometry, in its tables ([`LevelDoc`]): moving vertices and surfaces,
//! extruding a surface into a new sector, cleaving a sector in two, joining and parting
//! sectors with portals, adding a box room, deleting a sector.
//!
//! Each keeps what the level format needs by construction where it can: sectors' surfaces
//! in contiguous ranges, portals in matching pairs, no T-junctions within a sector (a
//! vertex made on an edge goes into every surface of the sector along that edge). What it
//! can't promise (a moved vertex keeping surfaces flat and sectors convex) the loader
//! checks when the level is rebuilt from the tables.

use std::collections::HashMap;

use glam::Vec3;

use crate::mmp_doc::{AdjoinDoc, LevelDoc, SectorDoc, SurfaceDoc};

/// How close to a plane or a line a point is to count as on it, in meters.
const EPS: f32 = 1e-4;
/// Render through and passable: a plain opening.
const OPEN: u32 = 0x3;

impl LevelDoc {
    /// The surfaces of each sector, in order.
    fn sector_lists(&self) -> Vec<Vec<SurfaceDoc>> {
        (0..self.sectors.len())
            .map(|s| self.surfaces[self.sector_surfaces(s)].to_vec())
            .collect()
    }

    /// Puts per-sector surface lists back: each sector's range, each surface's sector,
    /// and each adjoin's surface (found from the surface that names it).
    fn set_sector_lists(&mut self, lists: Vec<Vec<SurfaceDoc>>) {
        self.surfaces.clear();
        for (s, list) in lists.into_iter().enumerate() {
            let sector = &mut self.sectors[s];
            sector.first_surface = self.surfaces.len();
            sector.surface_count = list.len();
            for mut surface in list {
                surface.sector = s;
                self.surfaces.push(surface);
            }
        }
        for (i, surface) in self.surfaces.iter().enumerate() {
            if let Some(a) = surface.adjoin {
                self.adjoins[a].surface = i;
            }
        }
    }

    /// A surface's corner positions.
    pub fn surface_points(&self, surface: usize) -> Vec<Vec3> {
        self.surfaces[surface]
            .corners
            .iter()
            .map(|&(v, _)| self.vertices[v])
            .collect()
    }

    /// A surface's normal (facing into its sector), from its corners (Newell's method).
    pub fn surface_normal(&self, surface: usize) -> Vec3 {
        newell(&self.surface_points(surface))
    }

    /// Moves vertex `v` by `delta`: every surface using it moves with it.
    pub fn move_vertex(&mut self, v: usize, delta: Vec3) {
        self.vertices[v] += delta;
    }

    /// Moves a surface's corners `distance` along its normal (into its sector for a
    /// positive one): the surfaces sharing them stretch to follow.
    pub fn push_surface(&mut self, surface: usize, distance: f32) {
        let n = self.surface_normal(surface);
        let mut moved: Vec<usize> = self.surfaces[surface].corners.iter().map(|&(v, _)| v).collect();
        moved.sort_unstable();
        moved.dedup();
        for v in moved {
            self.vertices[v] += n * distance;
        }
    }

    /// Extrudes solid surface `surface` `distance` out of its sector (against its normal)
    /// into a new sector: a prism behind it, joined to the old sector by a portal where the
    /// surface was. The new sector's far end and sides take the surface's attribute values.
    /// Returns the new sector and its far end's surface.
    pub fn extrude(&mut self, surface: usize, distance: f32) -> Result<(usize, usize), String> {
        let s = self.surfaces[surface].clone();
        if s.adjoin.is_some() {
            return Err("that's already an opening".into());
        }
        if distance <= 0.0 {
            return Err("extrude by a positive distance".into());
        }
        let n = self.surface_normal(surface);
        let corners = s.corners.clone();
        // The far corners, out behind the surface.
        let far: Vec<usize> = corners
            .iter()
            .map(|&(v, _)| {
                self.vertices.push(self.vertices[v] - n * distance);
                self.vertices.len() - 1
            })
            .collect();
        let new_sector = self.sectors.len();
        let (a_side, b_side) = (self.adjoins.len(), self.adjoins.len() + 1);
        self.adjoins.push(AdjoinDoc { surface: 0, mirror: b_side, flags: OPEN });
        self.adjoins.push(AdjoinDoc { surface: 0, mirror: a_side, flags: OPEN });
        let mut lists = self.sector_lists();
        // The surface becomes the opening.
        let old = &mut lists[s.sector][surface - self.sectors[s.sector].first_surface];
        old.adjoin = Some(a_side);
        old.flags = 0;
        old.corners = corners.iter().map(|&(v, _)| (v, Vec::new())).collect();
        // The new sector: the opening back (facing into it: reversed), the far end (facing
        // back toward the opening, like the surface), and a side per edge.
        let mut list = vec![SurfaceDoc {
            sector: new_sector,
            adjoin: Some(b_side),
            flags: 0,
            corners: corners.iter().rev().map(|&(v, _)| (v, Vec::new())).collect(),
        }];
        list.push(SurfaceDoc {
            sector: new_sector,
            adjoin: None,
            flags: s.flags & 0x2,
            corners: far.iter().zip(&corners).map(|(&f, (_, rows))| (f, rows.clone())).collect(),
        });
        let k = corners.len();
        let center = {
            let near: Vec3 = corners.iter().map(|&(v, _)| self.vertices[v]).sum::<Vec3>() / k as f32;
            near - n * (distance / 2.0)
        };
        for i in 0..k {
            let j = (i + 1) % k;
            let (a, b) = (&corners[i], &corners[j]);
            let side = vec![
                (a.0, a.1.clone()),
                (b.0, b.1.clone()),
                (far[j], b.1.clone()),
                (far[i], a.1.clone()),
            ];
            list.push(self.facing_in(side, center));
        }
        lists.push(list);
        self.sectors.push(SectorDoc {
            name: self.unique_sector_name(&self.sectors[s.sector].name.clone()),
            first_surface: 0,
            surface_count: 0,
        });
        self.set_sector_lists(lists);
        let far_end = self.sectors[new_sector].first_surface + 1;
        Ok((new_sector, far_end))
    }

    /// A surface of `corners`, turned to face `center` (into its sector).
    fn facing_in(&self, mut corners: Vec<(usize, Vec<usize>)>, center: Vec3) -> SurfaceDoc {
        let points: Vec<Vec3> = corners.iter().map(|&(v, _)| self.vertices[v]).collect();
        let n = newell(&points);
        if n.dot(center - points[0]) < 0.0 {
            corners.reverse();
        }
        SurfaceDoc { sector: 0, adjoin: None, flags: 0, corners }
    }

    /// Splits sector `sector` by the plane through `point` facing `normal` into two, joined
    /// by an opening where the plane cuts it: the part in front of the plane keeps the
    /// sector, the part behind becomes a new one. Openings to other sectors that the plane
    /// cuts are split on both sides. Returns the new sector.
    pub fn cleave(&mut self, sector: usize, normal: Vec3, point: Vec3) -> Result<usize, String> {
        let normal = normal.normalize();
        let range = self.sector_surfaces(sector);
        let distance = |doc: &LevelDoc, v: usize| {
            let d = normal.dot(doc.vertices[v] - point);
            if d.abs() < EPS { 0.0 } else { d }
        };
        let used: Vec<usize> = self.surfaces[range.clone()]
            .iter()
            .flat_map(|s| s.corners.iter().map(|&(v, _)| v))
            .collect();
        if used.iter().all(|&v| distance(self, v) >= 0.0) || used.iter().all(|&v| distance(self, v) <= 0.0) {
            return Err("the cut doesn't cross that sector".into());
        }
        // New vertices where edges cross the plane, one per edge whichever surface asks.
        let mut made: HashMap<(usize, usize), usize> = HashMap::new();
        let mut split = |doc: &mut LevelDoc, s: &SurfaceDoc| -> (Option<SurfaceDoc>, Option<SurfaceDoc>) {
            let k = s.corners.len();
            let (mut front, mut back) = (Vec::new(), Vec::new());
            for i in 0..k {
                let (a, b) = (&s.corners[i], &s.corners[(i + 1) % k]);
                let (da, db) = (distance(doc, a.0), distance(doc, b.0));
                if da >= 0.0 {
                    front.push(a.clone());
                }
                if da <= 0.0 {
                    back.push(a.clone());
                }
                if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
                    let t = da / (da - db);
                    let key = (a.0.min(b.0), a.0.max(b.0));
                    let v = *made.entry(key).or_insert_with(|| {
                        doc.vertices.push(doc.vertices[a.0].lerp(doc.vertices[b.0], t));
                        doc.vertices.len() - 1
                    });
                    let rows = doc.lerp_rows(&a.1, &b.1, t);
                    front.push((v, rows.clone()));
                    back.push((v, rows));
                }
            }
            let part = |corners: Vec<(usize, Vec<usize>)>| {
                (corners.len() >= 3).then(|| SurfaceDoc { corners, ..s.clone() })
            };
            (part(front), part(back))
        };
        let mut lists = self.sector_lists();
        let new_sector = self.sectors.len();
        let (mut front_list, mut back_list) = (Vec::new(), Vec::new());
        // Openings this cut splits: (adjoin, the part behind's new adjoin).
        let mut split_openings = Vec::new();
        for s in std::mem::take(&mut lists[sector]) {
            match split(self, &s) {
                (Some(f), Some(mut b)) => {
                    if let Some(a) = s.adjoin {
                        // The part behind gets a new opening; its mirror is split below.
                        let (mine, theirs) = (self.adjoins.len(), self.adjoins.len() + 1);
                        self.adjoins.push(AdjoinDoc { surface: 0, mirror: theirs, flags: self.adjoins[a].flags });
                        self.adjoins.push(AdjoinDoc { surface: 0, mirror: mine, flags: self.adjoins[a].flags });
                        b.adjoin = Some(mine);
                        split_openings.push((a, theirs));
                    }
                    front_list.push(f);
                    back_list.push(b);
                }
                (Some(f), None) => front_list.push(f),
                (None, Some(b)) => back_list.push(b),
                (None, None) => {}
            }
        }
        // Where the plane cuts the sector: its points on the plane, in order around it.
        let mut on: Vec<usize> = front_list
            .iter()
            .flat_map(|s| s.corners.iter().map(|&(v, _)| v))
            .filter(|&v| distance(self, v) == 0.0)
            .collect();
        on.sort_unstable();
        on.dedup();
        if on.len() < 3 {
            return Err("the cut only grazes that sector".into());
        }
        let center = on.iter().map(|&v| self.vertices[v]).sum::<Vec3>() / on.len() as f32;
        let u = (self.vertices[on[0]] - center).normalize();
        let w = normal.cross(u);
        on.sort_by(|&a, &b| {
            let angle = |v: usize| {
                let d = self.vertices[v] - center;
                d.dot(w).atan2(d.dot(u))
            };
            angle(a).total_cmp(&angle(b))
        });
        let (cap_front, cap_back) = (self.adjoins.len(), self.adjoins.len() + 1);
        self.adjoins.push(AdjoinDoc { surface: 0, mirror: cap_back, flags: OPEN });
        self.adjoins.push(AdjoinDoc { surface: 0, mirror: cap_front, flags: OPEN });
        // Facing into the front part (along the normal): counter-clockwise seen from it.
        front_list.push(SurfaceDoc {
            sector,
            adjoin: Some(cap_front),
            flags: 0,
            corners: on.iter().map(|&v| (v, Vec::new())).collect(),
        });
        back_list.push(SurfaceDoc {
            sector: new_sector,
            adjoin: Some(cap_back),
            flags: 0,
            corners: on.iter().rev().map(|&v| (v, Vec::new())).collect(),
        });
        lists[sector] = front_list;
        lists.push(back_list);
        // Openings to other sectors that the cut split: split their mirrors the same way.
        for (a, behind) in split_openings {
            let mirror = self.adjoins[a].mirror;
            let Some((other, k)) = find_adjoin(&lists, mirror) else { continue };
            let m = lists[other][k].clone();
            let (f, b) = split(self, &m);
            let (Some(mut f), Some(mut b)) = (f, b) else {
                return Err("an opening the cut crosses doesn't split cleanly".into());
            };
            // The mirror's part in front faces the front part's opening (adjoin `a`); its
            // part behind faces the part behind's.
            f.adjoin = Some(mirror);
            b.adjoin = Some(behind);
            lists[other][k] = f;
            lists[other].push(b);
        }
        self.sectors.push(SectorDoc {
            name: self.unique_sector_name(&self.sectors[sector].name.clone()),
            first_surface: 0,
            surface_count: 0,
        });
        self.set_sector_lists(lists);
        // What's in the part behind is in the new sector.
        let behind = |p: Vec3| normal.dot(p - point) < -EPS;
        for e in &mut self.entities {
            if e.sector == sector && behind(e.position) {
                e.sector = new_sector;
            }
        }
        for l in &mut self.lights {
            if l.sector == sector && behind(l.position) {
                l.sector = new_sector;
            }
        }
        // New vertices on edges shared with surfaces that weren't cut (in the sectors on
        // the other side of split openings) go into those surfaces too.
        let touched: Vec<usize> = (0..self.sectors.len()).collect();
        self.fix_t_junctions(&touched);
        Ok(new_sector)
    }

    /// Makes solid surface `surface` and the surface of another sector with the same corners
    /// (reversed, or in the same places) an opening between their sectors. Corners in the
    /// same places become the same vertices.
    pub fn adjoin(&mut self, surface: usize) -> Result<usize, String> {
        let s = self.surfaces[surface].clone();
        if s.adjoin.is_some() {
            return Err("that's already an opening".into());
        }
        let points = self.surface_points(surface);
        let near = |a: Vec3, b: Vec3| a.distance(b) < 1e-3;
        let other = (0..self.surfaces.len()).find(|&o| {
            let t = &self.surfaces[o];
            t.sector != s.sector
                && t.adjoin.is_none()
                && t.corners.len() == s.corners.len()
                && {
                    let theirs = self.surface_points(o);
                    points.iter().all(|&p| theirs.iter().any(|&q| near(p, q)))
                }
        });
        let Some(other) = other else {
            return Err("no surface of another sector matches it".into());
        };
        // Weld their corners.
        let theirs: Vec<usize> = self.surfaces[other].corners.iter().map(|&(v, _)| v).collect();
        for &(v, _) in &s.corners {
            if let Some(&w) = theirs.iter().find(|&&w| near(self.vertices[w], self.vertices[v])) {
                self.replace_vertex(w, v);
            }
        }
        let (mine, their_side) = (self.adjoins.len(), self.adjoins.len() + 1);
        self.adjoins.push(AdjoinDoc { surface, mirror: their_side, flags: OPEN });
        self.adjoins.push(AdjoinDoc { surface: other, mirror: mine, flags: OPEN });
        for (i, a) in [(surface, mine), (other, their_side)] {
            let t = &mut self.surfaces[i];
            t.adjoin = Some(a);
            t.flags = 0;
            for corner in &mut t.corners {
                corner.1.clear();
            }
        }
        Ok(other)
    }

    /// Makes opening `surface` and its mirror solid again. Their corners take attribute
    /// values from other surfaces of their sectors at the same vertices.
    pub fn unadjoin(&mut self, surface: usize) -> Result<(), String> {
        let Some(a) = self.surfaces[surface].adjoin else {
            return Err("that's not an opening".into());
        };
        let mirror = self.adjoins[a].surface_of_mirror(&self.adjoins);
        for i in [surface, mirror] {
            let rows: Vec<Vec<usize>> = self.surfaces[i]
                .corners
                .iter()
                .map(|&(v, _)| self.rows_at(self.surfaces[i].sector, v))
                .collect();
            let t = &mut self.surfaces[i];
            t.adjoin = None;
            for (corner, rows) in t.corners.iter_mut().zip(rows) {
                corner.1 = rows;
            }
        }
        self.remove_adjoins(&[a, self.adjoins[a].mirror]);
        Ok(())
    }

    /// Adds a box sector (a room) from `min` to `max`, on its own: extrude or adjoin to
    /// join it to the level. Its surfaces take each attribute's first values row.
    pub fn add_box(&mut self, min: Vec3, max: Vec3, name: &str) -> Result<usize, String> {
        if (max - min).min_element() <= EPS {
            return Err("a room needs some size in every direction".into());
        }
        let first = self.vertices.len();
        for k in 0..8 {
            let pick = |bit: usize, lo: f32, hi: f32| if k & bit == 0 { lo } else { hi };
            self.vertices.push(Vec3::new(pick(1, min.x, max.x), pick(2, min.y, max.y), pick(4, min.z, max.z)));
        }
        let rows = vec![0; self.attributes.len()];
        if self.attributes.iter().any(|a| a.values.is_empty()) {
            return Err("an attribute has no values to give the room".into());
        }
        let center = (min + max) / 2.0;
        // Faces by the corners they keep: x low/high, y low/high, z low/high.
        let faces = [[0, 2, 6, 4], [1, 3, 7, 5], [0, 1, 5, 4], [2, 3, 7, 6], [0, 1, 3, 2], [4, 5, 7, 6]];
        let sector = self.sectors.len();
        let list: Vec<SurfaceDoc> = faces
            .iter()
            .map(|f| {
                let corners = f.iter().map(|&k| (first + k, rows.clone())).collect();
                SurfaceDoc { sector, ..self.facing_in(corners, center) }
            })
            .collect();
        let mut lists = self.sector_lists();
        lists.push(list);
        self.sectors.push(SectorDoc {
            name: self.unique_sector_name(name),
            first_surface: 0,
            surface_count: 0,
        });
        self.set_sector_lists(lists);
        Ok(sector)
    }

    /// Deletes sector `sector`: openings into it become solid walls, and the sectors after
    /// it move down one. Refused while an entity or light is in it.
    pub fn delete_sector(&mut self, sector: usize) -> Result<(), String> {
        if self.sectors.len() == 1 {
            return Err("a level needs a sector".into());
        }
        if let Some(e) = self.entities.iter().find(|e| e.sector == sector) {
            return Err(format!("{} is in that sector", e.name));
        }
        if self.lights.iter().any(|l| l.sector == sector) {
            return Err("a light is in that sector".into());
        }
        for i in self.sector_surfaces(sector) {
            if self.surfaces[i].adjoin.is_some() {
                self.unadjoin(i)?;
            }
        }
        let mut lists = self.sector_lists();
        lists.remove(sector);
        self.sectors.remove(sector);
        self.set_sector_lists(lists);
        let shift = |s: &mut usize| {
            if *s > sector {
                *s -= 1;
            }
        };
        self.entities.iter_mut().for_each(|e| shift(&mut e.sector));
        self.lights.iter_mut().for_each(|l| shift(&mut l.sector));
        Ok(())
    }

    /// Where a vertex lies on another surface's edge within a sector of `sectors`, puts it
    /// into that surface as an extra corner (with attribute values between the edge's
    /// ends): no T-junctions.
    pub fn fix_t_junctions(&mut self, sectors: &[usize]) {
        for &sector in sectors {
            let range = self.sector_surfaces(sector);
            let mut used: Vec<usize> = self.surfaces[range.clone()]
                .iter()
                .flat_map(|s| s.corners.iter().map(|&(v, _)| v))
                .collect();
            used.sort_unstable();
            used.dedup();
            for i in range {
                let mut k = 0;
                while k < self.surfaces[i].corners.len() {
                    let n = self.surfaces[i].corners.len();
                    let (a, b) = (self.surfaces[i].corners[k].clone(), self.surfaces[i].corners[(k + 1) % n].clone());
                    let (pa, pb) = (self.vertices[a.0], self.vertices[b.0]);
                    let ab = pb - pa;
                    // The nearest vertex strictly inside the edge, if any.
                    let inside = used
                        .iter()
                        .filter(|&&v| v != a.0 && v != b.0)
                        .filter_map(|&v| {
                            let t = (self.vertices[v] - pa).dot(ab) / ab.length_squared();
                            let off = (pa + ab * t).distance(self.vertices[v]);
                            (t > EPS && t < 1.0 - EPS && off < EPS).then_some((t, v))
                        })
                        .min_by(|x, y| x.0.total_cmp(&y.0));
                    if let Some((t, v)) = inside {
                        let rows = if self.surfaces[i].adjoin.is_some() { Vec::new() } else { self.lerp_rows(&a.1, &b.1, t) };
                        self.surfaces[i].corners.insert(k + 1, (v, rows));
                    } else {
                        k += 1;
                    }
                }
            }
        }
    }

    /// Attribute values rows `t` of the way from `a` to `b` (new rows, one per attribute).
    fn lerp_rows(&mut self, a: &[usize], b: &[usize], t: f32) -> Vec<usize> {
        a.iter()
            .zip(b)
            .enumerate()
            .map(|(k, (&ra, &rb))| {
                if ra == rb {
                    return ra;
                }
                let attribute = &mut self.attributes[k];
                let integer = matches!(attribute.format.as_str(), "u8" | "i8" | "i16");
                let row: Vec<String> = attribute.values[ra]
                    .iter()
                    .zip(&attribute.values[rb])
                    .map(|(x, y)| {
                        let (x, y) = (x.parse::<f32>().unwrap_or(0.0), y.parse::<f32>().unwrap_or(0.0));
                        let v = x + (y - x) * t;
                        if integer { format!("{}", v.round() as i64) } else { crate::mmp_doc::number(v) }
                    })
                    .collect();
                attribute.values.push(row);
                attribute.values.len() - 1
            })
            .collect()
    }

    /// Attribute values rows a solid surface of `sector` has at vertex `v` (the first
    /// values rows if none has it).
    fn rows_at(&self, sector: usize, v: usize) -> Vec<usize> {
        self.surfaces[self.sector_surfaces(sector)]
            .iter()
            .filter(|s| s.adjoin.is_none())
            .flat_map(|s| s.corners.iter())
            .find(|c| c.0 == v)
            .map(|c| c.1.clone())
            .unwrap_or_else(|| vec![0; self.attributes.len()])
    }

    /// Replaces vertex `old` with `new` in every surface.
    fn replace_vertex(&mut self, old: usize, new: usize) {
        for s in &mut self.surfaces {
            for corner in &mut s.corners {
                if corner.0 == old {
                    corner.0 = new;
                }
            }
        }
    }

    /// Removes adjoins (by index), renumbering the rest in surfaces and mirrors.
    fn remove_adjoins(&mut self, gone: &[usize]) {
        let mut map = Vec::with_capacity(self.adjoins.len());
        let mut next = 0;
        for i in 0..self.adjoins.len() {
            map.push((!gone.contains(&i)).then(|| {
                next += 1;
                next - 1
            }));
        }
        let old = std::mem::take(&mut self.adjoins);
        for (i, a) in old.into_iter().enumerate() {
            if map[i].is_some() {
                self.adjoins.push(AdjoinDoc { mirror: map[a.mirror].unwrap_or(0), ..a });
            }
        }
        for s in &mut self.surfaces {
            s.adjoin = s.adjoin.and_then(|a| map[a]);
        }
    }

    /// `name`, or with a number after it, so no sector has it.
    fn unique_sector_name(&self, name: &str) -> String {
        if self.sectors.iter().all(|s| s.name != name) {
            return name.to_string();
        }
        let base = name.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('_');
        (2..)
            .map(|k| format!("{base}_{k}"))
            .find(|n| self.sectors.iter().all(|s| &s.name != n))
            .unwrap()
    }
}

impl AdjoinDoc {
    /// The surface of this adjoin's mirror.
    fn surface_of_mirror(&self, adjoins: &[AdjoinDoc]) -> usize {
        adjoins[self.mirror].surface
    }
}

/// The sector and place in its list of the surface with adjoin `a`.
fn find_adjoin(lists: &[Vec<SurfaceDoc>], a: usize) -> Option<(usize, usize)> {
    lists.iter().enumerate().find_map(|(s, list)| {
        list.iter().position(|surface| surface.adjoin == Some(a)).map(|k| (s, k))
    })
}

/// A polygon's normal by Newell's method (unit length; counter-clockwise faces it).
fn newell(points: &[Vec3]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n.normalize_or_zero()
}
