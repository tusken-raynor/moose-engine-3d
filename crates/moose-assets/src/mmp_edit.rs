//! Editing a level's geometry, in its tables ([`LevelDoc`]): moving vertices and surfaces,
//! extruding a surface into a new sector, cleaving a sector in two and merging two back
//! into one, joining and parting sectors with portals, adding a box room, deleting a
//! sector.
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
        old.options.clear();
        old.corners = corners.iter().map(|&(v, _)| (v, Vec::new())).collect();
        // The new sector: the opening back (facing into it: reversed), the far end (facing
        // back toward the opening, like the surface), and a side per edge.
        let mut list = vec![SurfaceDoc {
            sector: new_sector,
            adjoin: Some(b_side),
            flags: 0,
            corners: corners.iter().rev().map(|&(v, _)| (v, Vec::new())).collect(),
            options: Vec::new(),
        }];
        list.push(SurfaceDoc {
            sector: new_sector,
            adjoin: None,
            flags: s.flags & 0x2,
            corners: far.iter().zip(&corners).map(|(&f, (_, rows))| (f, rows.clone())).collect(),
            options: s.options.clone(),
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
            // The sides are drawn as the surface was.
            list.push(SurfaceDoc { options: s.options.clone(), ..self.facing_in(side, center) });
        }
        lists.push(list);
        self.sectors.push(SectorDoc {
            name: self.unique_sector_name(&self.sectors[s.sector].name.clone()),
            first_surface: 0,
            surface_count: 0,
            options: Vec::new(),
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
        SurfaceDoc { sector: 0, adjoin: None, flags: 0, corners, options: Vec::new() }
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
            options: Vec::new(),
        });
        back_list.push(SurfaceDoc {
            sector: new_sector,
            adjoin: Some(cap_back),
            flags: 0,
            corners: on.iter().rev().map(|&v| (v, Vec::new())).collect(),
            options: Vec::new(),
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
        // Both halves keep the sector's meta values.
        self.sectors.push(SectorDoc {
            name: self.unique_sector_name(&self.sectors[sector].name.clone()),
            first_surface: 0,
            surface_count: 0,
            options: self.sectors[sector].options.clone(),
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
            t.options.clear();
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
            options: Vec::new(),
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

    /// Cuts solid surface `surface` in two along the plane through `point` square to
    /// `normal`, and nothing else: no new sector, no opening. Its part in front of the
    /// plane stays `surface`; the part behind follows it, `surface + 1`. Both keep its
    /// options and flags, and their corners' attribute values (texture coordinates) run on
    /// from its own, so it looks the same. Where the cut crosses its edges, the new corners
    /// go into every surface sharing those edges too (on both sides of an opening), so
    /// there are no T-junctions. Returns the part behind.
    pub fn cut_surface(&mut self, surface: usize, normal: Vec3, point: Vec3) -> Result<usize, String> {
        let s = self.surfaces[surface].clone();
        if s.adjoin.is_some() {
            return Err("cut the wall around an opening, not the opening".into());
        }
        let normal = normal.normalize();
        let distance = |doc: &LevelDoc, v: usize| {
            let d = normal.dot(doc.vertices[v] - point);
            if d.abs() < EPS { 0.0 } else { d }
        };
        let ds: Vec<f32> = s.corners.iter().map(|&(v, _)| distance(self, v)).collect();
        if !(ds.iter().any(|&d| d > 0.0) && ds.iter().any(|&d| d < 0.0)) {
            return Err("the cut doesn't cross that surface".into());
        }
        let k = s.corners.len();
        let (mut front, mut back, mut on_cut) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..k {
            let (a, b) = (&s.corners[i], &s.corners[(i + 1) % k]);
            let (da, db) = (ds[i], ds[(i + 1) % k]);
            if da >= 0.0 {
                front.push(a.clone());
            }
            if da <= 0.0 {
                back.push(a.clone());
            }
            if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
                let t = da / (da - db);
                let p = self.vertices[a.0].lerp(self.vertices[b.0], t);
                // A vertex already there (on a neighbor's corner) is the one.
                let v = match self.vertices.iter().position(|q| q.distance(p) < EPS) {
                    Some(v) => v,
                    None => {
                        self.vertices.push(p);
                        self.vertices.len() - 1
                    }
                };
                let rows = self.lerp_rows(&a.1, &b.1, t);
                front.push((v, rows.clone()));
                back.push((v, rows));
                on_cut.push(v);
            }
        }
        let mut lists = self.sector_lists();
        let at = surface - self.sector_surfaces(s.sector).start;
        lists[s.sector][at] = SurfaceDoc { corners: front, ..s.clone() };
        lists[s.sector].insert(at + 1, SurfaceDoc { corners: back, ..s.clone() });
        self.set_sector_lists(lists);
        self.add_to_edges(&on_cut);
        Ok(surface + 1)
    }

    /// Merges solid surface `other` into solid surface `target`, which must be in the same
    /// sector, on one plane facing the same way, and touch along one run of edges, their
    /// union convex: `target` becomes the union (its options and flags; with texture
    /// coordinates, its mapping carried across the whole of it, so the texture runs on with
    /// no seam), and `other` goes. Corners left in a straight line along the union's edges
    /// stay where another surface uses them (no T-junction), and go otherwise. Returns the
    /// merged surface's index (one less than `target`'s if `other` was before it).
    pub fn merge_surfaces(&mut self, target: usize, other: usize) -> Result<usize, String> {
        if target == other {
            return Err("pick another surface to merge with".into());
        }
        let a = self.surfaces[target].clone();
        if a.adjoin.is_some() || self.surfaces[other].adjoin.is_some() {
            return Err("openings can't be merged".into());
        }
        let corners = self.union_corners(target, other)?;
        let normal = self.surface_normal(target).normalize_or_zero();
        let origin = self.vertices[a.corners[0].0];
        // The texture: a's mapping (fitted to its corners) over all of it.
        let uv = self.attributes.iter().position(|x| x.name == "uv");
        let mapping = uv.and_then(|k| {
            let axis = (self.vertices[a.corners[1].0] - origin).normalize_or_zero();
            let across = normal.cross(axis);
            let samples: Vec<([f32; 3], [f32; 2])> = a
                .corners
                .iter()
                .filter_map(|(v, rows)| {
                    let d = self.vertices[*v] - origin;
                    let t = &self.attributes[k].values[*rows.get(k)?];
                    let value = |c: usize| t.get(c).and_then(|x| x.parse().ok()).unwrap_or(0.0);
                    Some(([1.0, d.dot(axis), d.dot(across)], [value(0), value(1)]))
                })
                .collect();
            fit_affine(&samples).map(|fit| (axis, across, fit))
        });
        // Put it together: `target` the union, `other` gone.
        self.surfaces[target].corners = corners;
        self.remove_surfaces(&[other]);
        let merged = if other < target { target - 1 } else { target };
        if let Some((axis, across, fit)) = mapping {
            self.set_corner_values(merged, "uv", |_, p, _| {
                let d = p - origin;
                let (s, t) = (d.dot(axis), d.dot(across));
                (0..2).map(|c| fit[0][c] + fit[1][c] * s + fit[2][c] * t).collect()
            });
        }
        Ok(merged)
    }

    /// The corners of `target` and `other` as one polygon (`target`'s values at each), if
    /// they are in the same sector, on one plane facing the same way, touch along one run
    /// of edges, and their union is convex. Corners left in a straight line along its
    /// edges stay where another surface uses them (no T-junction), and go otherwise.
    fn union_corners(&self, target: usize, other: usize) -> Result<Vec<(usize, Vec<usize>)>, String> {
        let (a, b) = (&self.surfaces[target], &self.surfaces[other]);
        if a.sector != b.sector {
            return Err("those surfaces are in different sectors".into());
        }
        // One plane, facing the same way.
        let normal = self.surface_normal(target).normalize_or_zero();
        let origin = self.vertices[a.corners[0].0];
        let flat = self.surface_normal(other).normalize_or_zero().dot(normal) > 1.0 - 1e-4
            && self.surface_points(other).iter().all(|p| normal.dot(*p - origin).abs() < 1e-3);
        if !flat {
            return Err("those surfaces aren't on one plane".into());
        }
        // The edges they share (a's, reversed in b's): one run of them, around a.
        let (n, m) = (a.corners.len(), b.corners.len());
        let in_b = |u: usize, v: usize| (0..m).any(|k| b.corners[k].0 == v && b.corners[(k + 1) % m].0 == u);
        let shared: Vec<bool> = (0..n).map(|i| in_b(a.corners[i].0, a.corners[(i + 1) % n].0)).collect();
        let runs = (0..n).filter(|&i| shared[i] && !shared[(i + n - 1) % n]).count();
        if runs == 0 {
            return Err("those surfaces don't share an edge".into());
        }
        if runs > 1 || shared.iter().all(|&x| x) {
            return Err("those surfaces touch along more than one stretch".into());
        }
        // a's run of shared edges: from corner `s` to corner `e` (both on it).
        let s0 = (0..n).find(|&i| shared[i] && !shared[(i + n - 1) % n]).unwrap();
        let mut e = s0;
        while shared[e % n] {
            e += 1;
        }
        let e = e % n;
        // The union: a from e round to s0, then b from after s0 round to before e.
        let mut corners: Vec<(usize, Vec<usize>)> = Vec::new();
        let mut i = e;
        loop {
            corners.push(a.corners[i].clone());
            if i == s0 {
                break;
            }
            i = (i + 1) % n;
        }
        let (bs, be) = (
            (0..m).find(|&k| b.corners[k].0 == a.corners[s0].0).unwrap(),
            (0..m).find(|&k| b.corners[k].0 == a.corners[e].0).unwrap(),
        );
        let mut k = (bs + 1) % m;
        while k != be {
            corners.push(b.corners[k].clone());
            k = (k + 1) % m;
        }
        // Convex (corners in a straight line allowed), and still a polygon.
        let at = |c: &(usize, Vec<usize>)| self.vertices[c.0];
        let len = corners.len();
        let turns = (0..len).map(|i| {
            let (p, q, r) = (at(&corners[(i + len - 1) % len]), at(&corners[i]), at(&corners[(i + 1) % len]));
            (q - p).cross(r - q).dot(normal)
        });
        if len < 3 || turns.clone().any(|t| t < -1e-5) {
            return Err("together they wouldn't be convex".into());
        }
        // Corners in a straight line no other surface uses go.
        let others: Vec<usize> = self
            .surfaces
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != target && i != other)
            .flat_map(|(_, s)| s.corners.iter().map(|c| c.0))
            .collect();
        let mut i = 0;
        while i < corners.len() && corners.len() > 3 {
            let l = corners.len();
            let (p, q, r) = (at(&corners[(i + l - 1) % l]), at(&corners[i]), at(&corners[(i + 1) % l]));
            let straight = (q - p).cross(r - q).length() < 1e-5 * (q - p).length().max(1e-6) * (r - q).length().max(1e-6) + 1e-9;
            if straight && !others.contains(&corners[i].0) {
                corners.remove(i);
            } else {
                i += 1;
            }
        }
        Ok(corners)
    }

    /// Merges sector `other` into sector `target`, which it opens onto: the openings
    /// between them go, and `other`'s surfaces become `target`'s (whose name and meta
    /// values are kept). Entities and lights in `other` are in `target`, and the sectors
    /// after `other` move down one. Together they must be convex. Where the openings were,
    /// two surfaces on one plane that look alike (the same flags and options, the texture
    /// running on unchanged) become one, as do two openings onto the same sector, and
    /// corners left in a straight line there go where every surface using them allows: a
    /// cleave merged back is as it was. Returns `target`'s index after.
    pub fn merge_sectors(&mut self, target: usize, other: usize) -> Result<usize, String> {
        if target == other {
            return Err("pick another sector to merge with".into());
        }
        // The openings between them, both sides.
        let (mut gone, mut gone_adjoins) = (Vec::new(), Vec::new());
        for i in self.sector_surfaces(target) {
            if let Some(a) = self.surfaces[i].adjoin {
                let mirror = self.adjoins[a].surface_of_mirror(&self.adjoins);
                if self.surfaces[mirror].sector == other {
                    gone.extend([i, mirror]);
                    gone_adjoins.extend([a, self.adjoins[a].mirror]);
                }
            }
        }
        if gone.is_empty() {
            return Err("those sectors don't open onto each other".into());
        }
        // Convex together: every corner on or in front of every surface left (as the
        // loader checks a sector).
        let kept: Vec<usize> =
            self.sector_surfaces(target).chain(self.sector_surfaces(other)).filter(|i| !gone.contains(i)).collect();
        let mut used: Vec<usize> = kept.iter().flat_map(|&i| self.surfaces[i].corners.iter().map(|c| c.0)).collect();
        used.sort_unstable();
        used.dedup();
        let points: Vec<Vec3> = used.iter().map(|&v| self.vertices[v]).collect();
        let center = points.iter().copied().sum::<Vec3>() / points.len() as f32;
        let size = points.iter().map(|p| p.distance(center)).fold(0.0, f32::max);
        let tol = crate::geom::TOLERANCE * size.max(1.0);
        for &i in &kept {
            let normal = self.surface_normal(i).normalize_or_zero();
            let origin = self.vertices[self.surfaces[i].corners[0].0];
            if points.iter().any(|&p| normal.dot(p - origin) < -tol) {
                return Err("together they wouldn't be convex".into());
            }
        }
        let seam: Vec<usize> = gone.iter().flat_map(|&i| self.surfaces[i].corners.iter().map(|c| c.0)).collect();
        // `other`'s surfaces in `target`, the openings gone, then `other`.
        let mut lists = self.sector_lists();
        lists[target] = kept.iter().map(|&i| self.surfaces[i].clone()).collect();
        lists[other].clear();
        self.set_sector_lists(lists);
        self.remove_adjoins(&gone_adjoins);
        let mut lists = self.sector_lists();
        lists.remove(other);
        self.sectors.remove(other);
        self.set_sector_lists(lists);
        let moved = |s: usize| match s {
            _ if s == other => target - (other < target) as usize,
            _ if s > other => s - 1,
            _ => s,
        };
        let target = moved(target);
        self.entities.iter_mut().for_each(|e| e.sector = moved(e.sector));
        self.lights.iter_mut().for_each(|l| l.sector = moved(l.sector));
        let all: Vec<usize> = (0..self.sectors.len()).collect();
        self.fix_t_junctions(&all);
        self.tidy_seam(target, &seam);
        Ok(target)
    }

    /// After a merge of sectors into `target`: joins its surfaces that meet along the
    /// `seam` (the corners of the openings that went) and look alike, and its openings
    /// there onto the same sector, then drops seam corners in a straight line.
    fn tidy_seam(&mut self, target: usize, seam: &[usize]) {
        let uv = self.attributes.iter().position(|a| a.name == "uv");
        let uv_at = |doc: &LevelDoc, s: usize, v: usize| -> Option<Vec<f32>> {
            let (_, rows) = doc.surfaces[s].corners.iter().find(|c| c.0 == v)?;
            let t = &doc.attributes[uv?].values[*rows.get(uv?)?];
            Some(t.iter().map(|x| x.parse().unwrap_or(0.0)).collect())
        };
        'pairs: loop {
            let range = self.sector_surfaces(target);
            for i in range.clone() {
                for j in range.clone().filter(|&j| j > i) {
                    let (a, b) = (&self.surfaces[i], &self.surfaces[j]);
                    let n = a.corners.len();
                    let on_seam = (0..n).any(|k| {
                        let (u, v) = (a.corners[k].0, a.corners[(k + 1) % n].0);
                        seam.contains(&u) && seam.contains(&v) && {
                            let m = b.corners.len();
                            (0..m).any(|q| b.corners[q].0 == v && b.corners[(q + 1) % m].0 == u)
                        }
                    });
                    if !on_seam {
                        continue;
                    }
                    let mut trial = self.clone();
                    let joined = match (a.adjoin, b.adjoin) {
                        (None, None) if a.flags == b.flags && a.options == b.options => {
                            // Only if the texture runs on unchanged: each corner as it was.
                            trial.merge_surfaces(i, j).ok().filter(|&m| {
                                uv.is_none()
                                    || trial.surfaces[m].corners.iter().all(|&(v, _)| {
                                        let was = uv_at(self, i, v).or_else(|| uv_at(self, j, v));
                                        match (was, uv_at(&trial, m, v)) {
                                            (Some(x), Some(y)) => x.iter().zip(&y).all(|(p, q)| (p - q).abs() < 1e-3),
                                            _ => false,
                                        }
                                    })
                            })
                        }
                        (Some(_), Some(_)) => trial.merge_openings(i, j).ok(),
                        _ => None,
                    };
                    if joined.is_some() {
                        *self = trial;
                        continue 'pairs;
                    }
                }
            }
            break;
        }
        // Seam corners in a straight line in every surface using them (of any sector).
        for &v in seam {
            let at = |s: &SurfaceDoc| s.corners.iter().position(|c| c.0 == v);
            let straight = |s: &SurfaceDoc, k: usize| {
                let n = s.corners.len();
                let (p, q, r) =
                    (self.vertices[s.corners[(k + n - 1) % n].0], self.vertices[v], self.vertices[s.corners[(k + 1) % n].0]);
                n > 3 && (q - p).cross(r - q).length() < 1e-5 * (q - p).length() * (r - q).length() + 1e-9
            };
            let users: Vec<(usize, usize)> =
                self.surfaces.iter().enumerate().filter_map(|(i, s)| Some((i, at(s)?))).collect();
            if !users.is_empty() && users.iter().all(|&(i, k)| straight(&self.surfaces[i], k)) {
                for (i, k) in users {
                    self.surfaces[i].corners.remove(k);
                }
            }
        }
    }

    /// Joins openings `target` and `other` (in one sector, onto one other, alike) into
    /// `target`, on both sides, as [`LevelDoc::merge_surfaces`] joins solid surfaces.
    fn merge_openings(&mut self, target: usize, other: usize) -> Result<usize, String> {
        let (Some(a), Some(b)) = (self.surfaces[target].adjoin, self.surfaces[other].adjoin) else {
            return Err("those aren't both openings".into());
        };
        let (ma, mb) = (self.adjoins[a].surface_of_mirror(&self.adjoins), self.adjoins[b].surface_of_mirror(&self.adjoins));
        if self.surfaces[ma].sector != self.surfaces[mb].sector || self.adjoins[a].flags != self.adjoins[b].flags {
            return Err("those openings aren't alike".into());
        }
        let (ours, theirs) = (self.union_corners(target, other)?, self.union_corners(ma, mb)?);
        self.surfaces[target].corners = ours;
        self.surfaces[ma].corners = theirs;
        self.remove_surfaces(&[other, mb]);
        self.remove_adjoins(&[b, self.adjoins[b].mirror]);
        Ok(target - (other < target) as usize)
    }

    /// Removes surfaces `gone` (their sectors' ranges close up; openings among them leave
    /// their adjoins to be removed).
    fn remove_surfaces(&mut self, gone: &[usize]) {
        let lists = (0..self.sectors.len())
            .map(|s| self.sector_surfaces(s).filter(|i| !gone.contains(i)).map(|i| self.surfaces[i].clone()).collect())
            .collect();
        self.set_sector_lists(lists);
    }

    /// Puts each of `vertices` into every surface (of any sector, openings too) with an
    /// edge it lies strictly inside, as a corner there (with attribute values between the
    /// edge's ends), so a new corner on a shared edge leaves no T-junction.
    fn add_to_edges(&mut self, vertices: &[usize]) {
        for i in 0..self.surfaces.len() {
            let mut k = 0;
            while k < self.surfaces[i].corners.len() {
                let n = self.surfaces[i].corners.len();
                let (a, b) = (self.surfaces[i].corners[k].clone(), self.surfaces[i].corners[(k + 1) % n].clone());
                let (pa, pb) = (self.vertices[a.0], self.vertices[b.0]);
                let ab = pb - pa;
                let inside = vertices
                    .iter()
                    .filter(|&&v| v != a.0 && v != b.0 && self.surfaces[i].corners.iter().all(|c| c.0 != v))
                    .filter_map(|&v| {
                        let t = (self.vertices[v] - pa).dot(ab) / ab.length_squared();
                        let off = (pa + ab * t).distance(self.vertices[v]);
                        (t > EPS && t < 1.0 - EPS && off < EPS).then_some((t, v))
                    })
                    .min_by(|x, y| x.0.total_cmp(&y.0));
                match inside {
                    Some((t, v)) => {
                        let rows = if self.surfaces[i].adjoin.is_some() { Vec::new() } else { self.lerp_rows(&a.1, &b.1, t) };
                        self.surfaces[i].corners.insert(k + 1, (v, rows));
                    }
                    None => k += 1,
                }
            }
        }
    }

    /// Continues solid surface `source`'s material and texture onto solid surface
    /// `target`, which touches it along an edge (two or more corners in the same places):
    /// `target` takes `source`'s `material=` and `filterN=` options (keeping its own others,
    /// its meta values), and, if the level has texture coordinates (`uv`), `source`'s
    /// mapping unfolded about the edge, so the texture runs on across the fold as if the
    /// two were one flat surface: no seam on the edge, and no stretching. `mirrored`
    /// reflects it across the edge instead: folded back onto the source's side, so a point
    /// some way past the edge takes the texture as far back from it on the source (still no
    /// seam, the texture mirrored about the edge). Returns whether the texture was continued
    /// (false without `uv`).
    pub fn stitch(&mut self, target: usize, source: usize, mirrored: bool) -> Result<bool, String> {
        if target == source {
            return Err("pick another surface to continue from".into());
        }
        if self.surfaces[target].adjoin.is_some() || self.surfaces[source].adjoin.is_some() {
            return Err("openings have no material".into());
        }
        // The edge: the two farthest apart of the places both have (where the planes meet).
        let (pt, ps) = (self.surface_points(target), self.surface_points(source));
        let shared: Vec<Vec3> = pt.iter().copied().filter(|p| ps.iter().any(|q| p.distance(*q) < EPS)).collect();
        let mut edge = None;
        for (k, &a) in shared.iter().enumerate() {
            for &b in &shared[k + 1..] {
                if edge.is_none_or(|(p, q): (Vec3, Vec3)| a.distance(b) > p.distance(q)) {
                    edge = Some((a, b));
                }
            }
        }
        let Some((p0, p1)) = edge.filter(|(a, b)| a.distance(*b) > EPS) else {
            return Err("those surfaces don't share an edge".into());
        };
        // Its material.
        let carried = |o: &String| o.starts_with("material=") || o.starts_with("filter");
        let from: Vec<String> = self.surfaces[source].options.iter().filter(|o| carried(o)).cloned().collect();
        let options = &mut self.surfaces[target].options;
        options.retain(|o| !carried(o));
        options.splice(0..0, from);
        let Some(k) = self.attributes.iter().position(|a| a.name == "uv") else {
            return Ok(false);
        };
        // The source's mapping: uv = c + s a + t b, s along the edge and t across it in its
        // plane, fitted to its corners (least squares: exact for a flat mapping).
        let e = (p1 - p0).normalize();
        let across = |surface: usize| {
            let w = self.surface_normal(surface).normalize_or_zero().cross(e);
            // Pointing into the surface, from the edge.
            let points = self.surface_points(surface);
            let middle = points.iter().copied().sum::<Vec3>() / points.len() as f32;
            if (middle - p0).dot(w) < 0.0 { -w } else { w }
        };
        let (ws, wt) = (across(source), across(target));
        let uv_of = |row: usize| -> [f32; 2] {
            let v = &self.attributes[k].values[row];
            [0, 1].map(|c| v.get(c).and_then(|t| t.parse().ok()).unwrap_or(0.0))
        };
        let samples: Vec<([f32; 3], [f32; 2])> = self.surfaces[source]
            .corners
            .iter()
            .filter_map(|(v, rows)| {
                let d = self.vertices[*v] - p0;
                Some(([1.0, d.dot(e), d.dot(ws)], uv_of(*rows.get(k)?)))
            })
            .collect();
        let fit = fit_affine(&samples).ok_or("that surface's texture can't be read (its corners are in a line)")?;
        // The target's corners, unfolded about the edge to the far side of it from the source
        // (or, mirrored, folded onto the source's side).
        self.set_corner_values(target, "uv", |_, q, _| {
            let d = q - p0;
            let across = d.dot(wt);
            let (s, t) = (d.dot(e), if mirrored { across } else { -across });
            (0..2).map(|c| fit[0][c] + fit[1][c] * s + fit[2][c] * t).collect()
        });
        Ok(true)
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

/// The affine map (three rows: a constant, then the coefficients of `s` and `t`) that best
/// gives each sample's two values from its `[1, s, t]`, `value = x[0] + x[1] s + x[2] t`, by
/// least squares; `None` if the samples don't fix it (fewer than three, or in a line).
pub fn fit_affine(samples: &[([f32; 3], [f32; 2])]) -> Option<[[f32; 2]; 3]> {
    let (mut m, mut r) = (glam::DMat3::ZERO, [glam::DVec3::ZERO; 2]);
    for (x, y) in samples {
        let x = glam::DVec3::new(x[0] as f64, x[1] as f64, x[2] as f64);
        m += glam::DMat3::from_cols(x * x.x, x * x.y, x * x.z);
        for c in 0..2 {
            r[c] += x * y[c] as f64;
        }
    }
    if m.determinant().abs() < 1e-9 {
        return None;
    }
    let inv = m.inverse();
    let (a, b) = (inv * r[0], inv * r[1]);
    Some([[a.x as f32, b.x as f32], [a.y as f32, b.y as f32], [a.z as f32, b.z as f32]])
}
