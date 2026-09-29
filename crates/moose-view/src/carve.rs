//! Shadows carved into polygons: each polygon a shadow-casting light reaches is split, in
//! world space, into pieces the light reaches and pieces it doesn't, and each piece records
//! the lights it is in the shadow of ([`ViewPolygon::shadowed`](crate::ViewPolygon)). A
//! shadow's edge is then an edge between polygons, drawn exactly at any resolution, and
//! lighting is sampled across each piece as usual.
//!
//! What blocks a light:
//!
//! - **Portals.** Light enters a sector beyond its own only through the openings between
//!   them: from the light, each opening seen through the ones before it is a window, and
//!   only what lies within some window into its sector is lit.
//! - **Occluders.** An entity with an [`Occluder`] shape casts a shadow volume per polygon
//!   of the shape facing the light: the region beyond it, within the planes through the
//!   light and its edges. A convex shape's polygons facing the light don't overlap as seen
//!   from it, so their volumes together are exactly the shape's shadow.
//!
//! Pieces meet without gaps: a cut's points are computed once and shared by the pieces on
//! both sides, and every edge that is part of a longer one is walked along the longer one's
//! line ([`PieceEdge::Line`]), as its neighbors walk it.

use std::ops::Range;

use glam::Vec3;
use moose_assets::{Assets, Light};
use moose_scene::{Occluder, World};

use crate::clip::Edge;

/// Floats in a record before its weights (clip x, y, w, world x, y, z).
#[cfg(test)]
const RECORD_FLOATS: usize = 6;

/// Distance within which a point counts as on a cutting plane, in meters.
const ON_PLANE: f32 = 1e-5;
/// How far past an occluder's polygon a point must be to be in its shadow, in meters, so
/// the occluder's own faces (and what touches them) aren't shadowed by it.
const CAP_BIAS: f32 = 1e-3;
/// Sides of the disk a sphere occluder casts its shadow with.
const DISK_SIDES: usize = 16;
/// Limit on portals followed from a light.
const MAX_DEPTH: u16 = 16;
/// Most shadow slots (the bits of a shadow mask).
pub const MAX_SHADOW_SLOTS: u8 = 32;

/// A half-space: points with `normal · p + offset > 0`, in world space.
#[derive(Clone, Copy, Debug)]
struct Half {
    normal: Vec3,
    offset: f32,
}

impl Half {
    fn distance(&self, p: Vec3) -> f32 {
        self.normal.dot(p) + self.offset
    }

    /// The plane through `light` and the edge `a`-`b`, facing `inside`; `None` if the edge is
    /// seen end-on from the light.
    fn through(light: Vec3, a: Vec3, b: Vec3, inside: Vec3) -> Option<Half> {
        let n = (a - light).cross(b - light);
        let len = n.length();
        if len < 1e-9 {
            return None;
        }
        let n = if n.dot(inside - light) < 0.0 { -n / len } else { n / len };
        Some(Half {
            normal: n,
            offset: -n.dot(light),
        })
    }
}

/// How the edge leaving a piece's vertex is walked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PieceEdge {
    /// As the clipper left it: one of the source polygon's edges, whole, or along a clip
    /// plane (see [`Edge`]).
    Clip(Edge),
    /// Along the line through these two clip-space points `(x, y, w)`: the whole edge it is
    /// part of, or a cut.
    Line(Vec3, Vec3),
}

/// A piece of a carved polygon: `count` records from `start` in the carver's buffer, and the
/// shadow slots of the lights it is in the shadow of.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Piece {
    start: u32,
    count: u32,
    pub shadowed: u32,
}

/// A polygon being carved: the sectors it is in (an entity's may be several), the entity
/// it belongs to (level polygons belong to none), and its plane in world space: its normal,
/// facing its front, and a point on it.
pub(crate) struct Receiver<'a> {
    pub sectors: &'a [u32],
    pub entity: Option<u32>,
    pub normal: Vec3,
    pub point: Vec3,
}

/// A shadow-casting light, for one frame.
struct Caster {
    bit: u32,
    position: Vec3,
    range: f32,
    /// Sectors it lights whole: its own.
    whole: u32,
    /// The windows it lights other sectors through: (sector, planes).
    windows: Range<u32>,
    /// Its occluders' shadow volumes: ranges of `Carver::planes`.
    volumes: Range<u32>,
}

/// Carves polygons for every shadow-casting light. Reuse one; buffers keep their capacity.
#[derive(Default)]
pub(crate) struct Carver {
    casters: Vec<Caster>,
    planes: Vec<Half>,
    windows: Vec<(u32, Range<u32>)>,
    /// Shadow volumes (ranges of `planes`) and the entity casting each.
    volumes: Vec<(Range<u32>, Option<u32>)>,
    // Per polygon.
    stride: usize,
    records: Vec<f32>,
    edges: Vec<PieceEdge>,
    pieces: Vec<Piece>,
    work: Vec<Piece>,
    rest: Vec<Piece>,
    distances: Vec<f32>,
    front: Vec<(usize, Tag)>,
    back: Vec<(usize, Tag)>,
    // Scratch.
    points: Vec<Vec3>,
    /// An occluder's faces, as ranges of `points`.
    faces: Vec<Range<usize>>,
    next_points: Vec<Vec3>,
    stack: Vec<(u32, Option<Range<u32>>, u16)>,
}

impl Carver {
    /// Gathers what blocks each shadow-casting light this frame: where portals let it
    /// through, and its occluders' shadow volumes.
    pub fn prepare(&mut self, world: &World, assets: &Assets, lights: &[Light]) {
        self.casters.clear();
        self.planes.clear();
        self.windows.clear();
        self.volumes.clear();
        let geometry = assets.mesh(world.geometry);
        for light in lights {
            let Some(slot) = light.shadow.filter(|&s| s < MAX_SHADOW_SLOTS) else {
                continue;
            };
            let l = light.position;
            // Windows: out through portals, each clipped to the window it was seen through.
            let windows_start = self.windows.len() as u32;
            let mut reached = vec![false; world.sectors.len()];
            self.stack.clear();
            self.stack.push((light.sector, None, 0));
            while let Some((sector, window, depth)) = self.stack.pop() {
                reached[sector as usize] = true;
                if let Some(w) = &window {
                    self.windows.push((sector, w.clone()));
                }
                if depth >= MAX_DEPTH {
                    continue;
                }
                for p in world.sectors[sector as usize].portals.clone() {
                    let portal = &world.portals[p as usize];
                    if !portal.flags.render_through() || portal.plane.distance(l) <= ON_PLANE {
                        continue;
                    }
                    self.points.clear();
                    self.points
                        .extend(portal.positions.iter().map(|&i| geometry.positions[i as usize]));
                    if let Some(w) = &window {
                        for k in w.clone() {
                            clip_points(&mut self.points, &mut self.next_points, self.planes[k as usize]);
                        }
                    }
                    if self.points.len() < 3 {
                        continue;
                    }
                    let center = self.points.iter().copied().sum::<Vec3>() / self.points.len() as f32;
                    let radius = self.points.iter().map(|q| q.distance(center)).fold(0.0, f32::max);
                    let nearest = self.points.iter().map(|q| q.distance(l)).fold(f32::INFINITY, f32::min);
                    if nearest - radius >= light.range || !light.cone_reaches(center, radius) {
                        continue;
                    }
                    let start = self.planes.len() as u32;
                    for i in 0..self.points.len() {
                        let (a, b) = (self.points[i], self.points[(i + 1) % self.points.len()]);
                        if let Some(h) = Half::through(l, a, b, center) {
                            self.planes.push(h);
                        }
                    }
                    let end = self.planes.len() as u32;
                    self.stack.push((portal.target, Some(start..end), depth + 1));
                }
            }
            let windows_end = self.windows.len() as u32;

            // Occluders in the sectors it reaches.
            let volumes_start = self.volumes.len() as u32;
            for (index, entity) in world.entities.iter().enumerate() {
                let owner = Some(index as u32);
                if entity.occluder == Occluder::None
                    || !entity.sectors.iter().any(|&s| reached[s as usize])
                {
                    continue;
                }
                let center = (entity.bounds.min + entity.bounds.max) * 0.5;
                let radius = (entity.bounds.max - entity.bounds.min).length() * 0.5;
                if center.distance(l) - radius >= light.range
                    || center.distance(l) <= radius
                    || !light.cone_reaches(center, radius)
                {
                    continue;
                }
                let transform = entity.transform();
                match entity.occluder {
                    Occluder::None => {}
                    Occluder::Mesh => {
                        let mesh = assets.mesh(entity.mesh);
                        self.points.clear();
                        self.faces.clear();
                        for polygon in &mesh.polygons {
                            let start = self.points.len();
                            self.points.extend(
                                mesh.polygon_points(polygon)
                                    .map(|p| transform.transform_point3(p)),
                            );
                            self.faces.push(start..self.points.len());
                        }
                        self.add_volume(l, owner);
                    }
                    Occluder::Sphere { center, radius } => {
                        // Seen from the light, a sphere's outline is the circle where rays
                        // from the light graze it.
                        let c = transform.transform_point3(center);
                        let r = radius * entity.scale;
                        let to = c - l;
                        let d = to.length();
                        if d <= r {
                            continue;
                        }
                        let axis = to / d;
                        let (along, ring) = (d - r * r / d, r * (1.0 - r * r / (d * d)).sqrt());
                        let middle = l + axis * along;
                        let helper = if axis.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
                        // Counter-clockwise seen from the light, so it faces it.
                        let u = axis.cross(helper).normalize();
                        let v = u.cross(axis);
                        self.points.clear();
                        self.points.extend((0..DISK_SIDES).map(|k| {
                            let a = k as f32 * std::f32::consts::TAU / DISK_SIDES as f32;
                            middle + (u * a.cos() + v * a.sin()) * ring
                        }));
                        self.faces.clear();
                        self.faces.push(0..DISK_SIDES);
                        self.add_volume(l, owner);
                    }
                }
            }
            let volumes_end = self.volumes.len() as u32;
            self.casters.push(Caster {
                bit: 1 << slot,
                position: l,
                range: light.range,
                whole: light.sector,
                windows: windows_start..windows_end,
                volumes: volumes_start..volumes_end,
            });
        }
    }

    /// Adds the shadow volume of the convex occluder whose faces (convex polygons,
    /// counter-clockwise from outside) are `self.faces` of `self.points`. What it shadows
    /// from `light` is one convex region: inside the planes through the light and its
    /// outline (the edges of its faces toward the light that no other such face shares),
    /// and behind every face toward the light (by [`CAP_BIAS`]). Along any ray from the
    /// light through it, the faces toward the light all lie before the point where the ray
    /// enters it, so a point behind all their planes is past the occluder.
    fn add_volume(&mut self, light: Vec3, owner: Option<u32>) {
        // Its faces toward the light, and each one's plane.
        let mut front: Vec<(Range<usize>, Vec3)> = Vec::new();
        for face in &self.faces {
            let p = &self.points[face.clone()];
            if p.len() < 3 {
                continue;
            }
            let normal = (1..p.len() - 1)
                .map(|i| (p[i] - p[0]).cross(p[i + 1] - p[0]))
                .sum::<Vec3>();
            let len = normal.length();
            if len < 1e-12 || (normal / len).dot(light - p[0]) <= ON_PLANE {
                continue; // facing away, edge-on, or degenerate
            }
            front.push((face.clone(), normal / len));
        }
        if front.is_empty() {
            return;
        }
        let points = &self.points;
        let center = front.iter().flat_map(|(f, _)| &points[f.clone()]).copied().sum::<Vec3>()
            / front.iter().map(|(f, _)| f.len()).sum::<usize>() as f32;
        let start = self.planes.len() as u32;
        // The outline's planes first: cuts outside the shadow happen there, once.
        for (face, _) in &front {
            let p = &points[face.clone()];
            for i in 0..p.len() {
                let (a, b) = (p[i], p[(i + 1) % p.len()]);
                let shared = front.iter().any(|(other, _)| {
                    let q = &points[other.clone()];
                    (0..q.len()).any(|k| q[k] == b && q[(k + 1) % q.len()] == a)
                });
                if !shared && let Some(h) = Half::through(light, a, b, center) {
                    self.planes.push(h);
                }
            }
        }
        for (face, normal) in &front {
            self.planes.push(Half {
                normal: -*normal,
                offset: normal.dot(points[face.start]) - CAP_BIAS,
            });
        }
        self.volumes.push((start..self.planes.len() as u32, owner));
    }

    /// The `i`-th piece of the last polygon carved.
    pub fn piece(&self, i: usize) -> Piece {
        self.pieces[i]
    }

    /// A piece's records.
    pub fn records(&self, piece: &Piece) -> &[f32] {
        let s = self.stride;
        &self.records[piece.start as usize * s..(piece.start + piece.count) as usize * s]
    }

    /// How the edge leaving each of a piece's vertices is walked.
    pub fn edges(&self, piece: &Piece) -> &[PieceEdge] {
        &self.edges[piece.start as usize..(piece.start + piece.count) as usize]
    }

    /// Carves a clipped polygon (`records` of `stride` floats: clip x, y, w, then world
    /// x, y, z, then weights; `edges` from the clipper) for every shadow-casting light.
    /// Clip-plane edges numbered `lined` or more have no line of their own. Returns its
    /// pieces.
    pub fn carve(
        &mut self,
        records: &[f32],
        edges: &[Edge],
        stride: usize,
        lined: usize,
        receiver: &Receiver,
    ) -> &[Piece] {
        let (sectors, normal, point) = (receiver.sectors, receiver.normal, receiver.point);
        self.stride = stride;
        self.records.clear();
        self.edges.clear();
        self.records.extend_from_slice(records);
        self.edges.extend(edges.iter().map(|&e| PieceEdge::Clip(e)));
        self.pieces.clear();
        self.pieces.push(Piece {
            start: 0,
            count: edges.len() as u32,
            shadowed: 0,
        });
        for c in 0..self.casters.len() {
            let (bit, position, range) = {
                let c = &self.casters[c];
                (c.bit, c.position, c.range)
            };
            // Facing away from the light, or out of its reach: it gets none of it anyway.
            let height = normal.dot(position - point);
            if height <= 0.0 || height >= range {
                continue;
            }
            // Portals: unless it is in the light's own sector, only what some window into
            // its sectors reaches is lit.
            if !sectors.contains(&self.casters[c].whole) {
                std::mem::swap(&mut self.work, &mut self.pieces);
                self.pieces.clear();
                for w in self.casters[c].windows.clone() {
                    let (sector, planes) = self.windows[w as usize].clone();
                    if !sectors.contains(&sector) {
                        continue;
                    }
                    self.rest.clear();
                    let mut i = 0;
                    while i < self.work.len() {
                        let piece = self.work[i];
                        if let Some(inside) = self.split_region(piece, planes.clone(), lined) {
                            self.pieces.push(inside);
                        }
                        i += 1;
                    }
                    // What no window has reached yet waits for the next.
                    std::mem::swap(&mut self.work, &mut self.rest);
                }
                for piece in &mut self.work {
                    piece.shadowed |= bit;
                }
                self.pieces.append(&mut self.work);
            }
            // Occluders: each lit piece is split by each shadow volume.
            for v in self.casters[c].volumes.clone() {
                let (planes, owner) = self.volumes[v as usize].clone();
                // An occluder's shape stands in for its model, so it doesn't shadow it.
                if owner.is_some() && owner == receiver.entity {
                    continue;
                }
                std::mem::swap(&mut self.work, &mut self.pieces);
                self.pieces.clear();
                let mut i = 0;
                while i < self.work.len() {
                    let piece = self.work[i];
                    i += 1;
                    if piece.shadowed & bit != 0 {
                        self.pieces.push(piece);
                        continue;
                    }
                    self.rest.clear();
                    if let Some(mut inside) = self.split_region(piece, planes.clone(), lined) {
                        inside.shadowed |= bit;
                        self.pieces.push(inside);
                    }
                    let rest = std::mem::take(&mut self.rest);
                    self.pieces.extend_from_slice(&rest);
                    self.rest = rest;
                }
            }
        }
        &self.pieces
    }

    /// Splits `piece` by the region inside every plane of `planes` (in `self.planes`):
    /// returns the part inside, if any, and appends the parts outside to `self.rest`.
    fn split_region(&mut self, piece: Piece, planes: Range<u32>, lined: usize) -> Option<Piece> {
        let mut current = piece;
        for k in planes {
            let plane = self.planes[k as usize];
            let (front, back) = self.split(current, plane, lined);
            if let Some(back) = back {
                self.rest.push(back);
            }
            current = front?;
        }
        Some(current)
    }

    /// Splits a piece by a plane into the parts in front of it and behind it (either may
    /// be missing), each keeping the piece's shadow mask. Points on the plane go to both.
    fn split(&mut self, piece: Piece, plane: Half, lined: usize) -> (Option<Piece>, Option<Piece>) {
        let s = self.stride;
        let n = piece.count as usize;
        let base = piece.start as usize;
        self.distances.clear();
        for i in 0..n {
            let r = &self.records[(base + i) * s..];
            let d = plane.distance(Vec3::new(r[3], r[4], r[5]));
            self.distances.push(if d.abs() <= ON_PLANE { 0.0 } else { d });
        }
        if self.distances.iter().all(|&d| d >= 0.0) {
            return (Some(piece), None);
        }
        if self.distances.iter().all(|&d| d <= 0.0) {
            return (None, Some(piece));
        }
        let mut front = std::mem::take(&mut self.front);
        let mut back = std::mem::take(&mut self.back);
        front.clear();
        back.clear();
        for i in 0..n {
            let j = (i + 1) % n;
            let (di, dj) = (self.distances[i], self.distances[j]);
            let crosses = (di > 0.0 && dj < 0.0) || (di < 0.0 && dj > 0.0);
            // The edge leaving vertex i on each side: along the cut where it leaves that
            // side from the plane, part of edge i where edge i is cut, or edge i whole.
            let edge = if crosses { self.part(base, n, i, lined) } else { self.edges[base + i] };
            if di >= 0.0 {
                front.push((base + i, if di == 0.0 && dj < 0.0 { Tag::Cut } else { Tag::Edge(edge) }));
            }
            if di <= 0.0 {
                back.push((base + i, if di == 0.0 && dj > 0.0 { Tag::Cut } else { Tag::Edge(edge) }));
            }
            if crosses {
                // The cut point, computed once for both sides.
                let t = di / (di - dj);
                let at = self.records.len() / s;
                for c in 0..s {
                    let (p, q) = (self.records[(base + i) * s + c], self.records[(base + j) * s + c]);
                    self.records.push(p + (q - p) * t);
                }
                self.edges.push(edge); // keeps edges parallel to records; not read
                // Entering a side, the edge continues along edge i; leaving it, along the cut.
                let (to_front, to_back) = if di < 0.0 {
                    (Tag::Edge(edge), Tag::Cut)
                } else {
                    (Tag::Cut, Tag::Edge(edge))
                };
                front.push((at, to_front));
                back.push((at, to_back));
            }
        }
        let shadowed = piece.shadowed;
        let f = self.append(&front).map(|p| Piece { shadowed, ..p });
        let b = self.append(&back).map(|p| Piece { shadowed, ..p });
        self.front = front;
        self.back = back;
        (f, b)
    }

    /// The clip-space point of record `v`.
    fn clip_point(&self, v: usize) -> Vec3 {
        let s = self.stride;
        Vec3::new(self.records[v * s], self.records[v * s + 1], self.records[v * s + 2])
    }

    /// How a part of edge `i` of the piece at `base` (of `n` vertices) is walked: along the
    /// whole edge.
    fn part(&self, base: usize, n: usize, i: usize, lined: usize) -> PieceEdge {
        match self.edges[base + i] {
            e @ PieceEdge::Clip(Edge::Plane(k)) if (k as usize) < lined => e,
            e @ PieceEdge::Line(..) => e,
            PieceEdge::Clip(_) => {
                PieceEdge::Line(self.clip_point(base + i), self.clip_point(base + (i + 1) % n))
            }
        }
    }

    /// Appends a piece of existing records (by index), with the edge leaving each; a cut
    /// gets the line through its two endpoints, which the piece on its other side shares.
    fn append(&mut self, vertices: &[(usize, Tag)]) -> Option<Piece> {
        let n = vertices.len();
        if n < 3 {
            return None;
        }
        let s = self.stride;
        let start = self.records.len() / s;
        for &(v, _) in vertices {
            self.records.extend_from_within(v * s..(v + 1) * s);
        }
        for (i, &(_, tag)) in vertices.iter().enumerate() {
            let edge = match tag {
                Tag::Edge(e) => e,
                Tag::Cut => PieceEdge::Line(
                    self.clip_point(start + i),
                    self.clip_point(start + (i + 1) % n),
                ),
            };
            self.edges.push(edge);
        }
        Some(Piece {
            start: start as u32,
            count: n as u32,
            shadowed: 0,
        })
    }
}

/// How the edge leaving a vertex of a new piece is walked, while it is built.
#[derive(Clone, Copy)]
enum Tag {
    Edge(PieceEdge),
    /// Along the cutting plane.
    Cut,
}

/// Clips a convex outline to the half-space of `plane` (keeping points on it).
fn clip_points(points: &mut Vec<Vec3>, next: &mut Vec<Vec3>, plane: Half) {
    next.clear();
    let n = points.len();
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        let (da, db) = (plane.distance(a), plane.distance(b));
        if da >= 0.0 {
            next.push(a);
        }
        if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
            next.push(a + (b - a) * (da / (da - db)));
        }
    }
    std::mem::swap(points, next);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A carver with one caster: a light at `light` casting the shadows of the given
    /// convex occluders (each a list of faces), lighting its own sector 0 whole.
    fn carver(light: Vec3, occluders: &[&[&[Vec3]]]) -> Carver {
        let mut c = Carver::default();
        for faces in occluders {
            c.points.clear();
            c.faces.clear();
            for face in *faces {
                let start = c.points.len();
                c.points.extend_from_slice(face);
                c.faces.push(start..c.points.len());
            }
            c.add_volume(light, None);
        }
        c.casters.push(Caster {
            bit: 1,
            position: light,
            range: 100.0,
            whole: 0,
            windows: 0..0,
            volumes: 0..c.volumes.len() as u32,
        });
        c
    }

    /// Clip records for a world-space polygon: clip coordinates are the world ones (any
    /// affine map would do), then one weight per vertex.
    fn records(points: &[Vec3]) -> Vec<f32> {
        let n = points.len();
        points
            .iter()
            .enumerate()
            .flat_map(|(i, p)| {
                let mut r = vec![p.x, p.z, p.y + 10.0, p.x, p.y, p.z];
                r.extend((0..n).map(|k| if k == i { 1.0 } else { 0.0 }));
                r
            })
            .collect()
    }

    /// A level polygon in sector 0 on the plane through `point` facing `normal`.
    fn at(normal: Vec3, point: Vec3) -> Receiver<'static> {
        Receiver { sectors: &[0], entity: None, normal, point }
    }

    fn area(records: &[f32], stride: usize) -> f32 {
        let p: Vec<Vec3> = records.chunks(stride).map(|r| Vec3::new(r[3], r[4], r[5])).collect();
        (1..p.len() - 1)
            .map(|i| (p[i] - p[0]).cross(p[i + 1] - p[0]))
            .sum::<Vec3>()
            .length()
            / 2.0
    }

    #[test]
    fn a_square_shadows_the_floor_below_it() {
        // A light 3 m above a 4 m floor, a 1 m square 2 m below it: its shadow on the floor
        // is 1.5 m square.
        let light = Vec3::new(0.0, 3.0, 0.0);
        let square = [
            Vec3::new(-0.5, 1.0, -0.5),
            Vec3::new(-0.5, 1.0, 0.5),
            Vec3::new(0.5, 1.0, 0.5),
            Vec3::new(0.5, 1.0, -0.5),
        ];
        let mut c = carver(light, &[&[&square]]);
        assert_eq!(c.volumes.len(), 1, "the square faces the light");
        let floor = [
            Vec3::new(-2.0, 0.0, -2.0),
            Vec3::new(-2.0, 0.0, 2.0),
            Vec3::new(2.0, 0.0, 2.0),
            Vec3::new(2.0, 0.0, -2.0),
        ];
        let stride = RECORD_FLOATS + 4;
        let input = records(&floor);
        let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
        let n = c.carve(&input, &edges, stride, 0, &at(Vec3::Y, floor[0])).len();
        let (mut lit, mut dark) = (0.0, 0.0);
        for i in 0..n {
            let piece = c.piece(i);
            let a = area(c.records(&piece), stride);
            if piece.shadowed & 1 != 0 {
                dark += a;
                // Inside the shadow's square.
                for r in c.records(&piece).chunks(stride) {
                    assert!(r[3].abs() <= 0.7501 && r[5].abs() <= 0.7501, "{r:?}");
                }
            } else {
                lit += a;
            }
            // Every edge made by a cut, or part of a cut edge, is walked along a line.
            let whole = c
                .edges(&piece)
                .iter()
                .filter(|e| matches!(e, PieceEdge::Clip(_)))
                .count();
            assert!(whole <= 2, "at most two of the floor's corners survive whole in a piece");
        }
        assert!((dark - 2.25).abs() < 1e-4, "shadow area {dark}");
        assert!((lit + dark - 16.0).abs() < 1e-3, "pieces cover the floor: {}", lit + dark);

        // The square itself (on its own cap) and the floor seen from below are lit.
        let n = c.carve(&records(&square), &edges, stride, 0, &at(Vec3::Y, square[0])).len();
        assert!((0..n).all(|i| c.piece(i).shadowed == 0));
    }

    #[test]
    fn cut_points_are_shared_by_both_sides() {
        let light = Vec3::new(0.0, 3.0, 0.0);
        let half = [
            Vec3::new(0.0, 1.0, -5.0),
            Vec3::new(0.0, 1.0, 5.0),
            Vec3::new(5.0, 1.0, 5.0),
            Vec3::new(5.0, 1.0, -5.0),
        ];
        let mut c = carver(light, &[&[&half]]);
        let floor = [
            Vec3::new(-2.0, 0.0, -2.0),
            Vec3::new(-2.0, 0.0, 2.0),
            Vec3::new(2.0, 0.0, 2.0),
            Vec3::new(2.0, 0.0, -2.0),
        ];
        let stride = RECORD_FLOATS + 4;
        let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
        let n = c.carve(&records(&floor), &edges, stride, 0, &at(Vec3::Y, floor[0])).len();
        assert_eq!(n, 2);
        let (a, b) = (c.piece(0), c.piece(1));
        let points = |p: &Piece| -> Vec<[u32; 3]> {
            c.records(p)
                .chunks(stride)
                .map(|r| [r[0].to_bits(), r[1].to_bits(), r[2].to_bits()])
                .collect()
        };
        let (pa, pb) = (points(&a), points(&b));
        assert_eq!(pa.iter().filter(|p| pb.contains(p)).count(), 2, "two bit-identical cut points");
    }

    /// A cube's faces, counter-clockwise from outside.
    fn cube(center: Vec3, half: f32) -> Vec<Vec<Vec3>> {
        let c = |x: f32, y: f32, z: f32| center + Vec3::new(x, y, z) * half;
        vec![
            vec![c(-1., -1., 1.), c(1., -1., 1.), c(1., 1., 1.), c(-1., 1., 1.)],
            vec![c(1., -1., -1.), c(-1., -1., -1.), c(-1., 1., -1.), c(1., 1., -1.)],
            vec![c(1., -1., 1.), c(1., -1., -1.), c(1., 1., -1.), c(1., 1., 1.)],
            vec![c(-1., -1., -1.), c(-1., -1., 1.), c(-1., 1., 1.), c(-1., 1., -1.)],
            vec![c(-1., 1., 1.), c(1., 1., 1.), c(1., 1., -1.), c(-1., 1., -1.)],
            vec![c(-1., -1., -1.), c(1., -1., -1.), c(1., -1., 1.), c(-1., -1., 1.)],
        ]
    }

    #[test]
    fn a_cube_casts_one_volume_its_outline_bounds() {
        // A light off the cube's corner sees three of its faces; its outline is six edges.
        let light = Vec3::new(2.0, 4.0, 1.0);
        let faces = cube(Vec3::new(0.0, 1.5, 0.0), 0.5);
        let refs: Vec<&[Vec3]> = faces.iter().map(|f| f.as_slice()).collect();
        let mut c = carver(light, &[&refs]);
        assert_eq!(c.volumes.len(), 1);
        assert_eq!(c.volumes[0].0.len(), 6 + 3, "six outline planes, three faces'");

        // Its shadow on the floor: the convex hull of its corners projected from the light.
        let corners: Vec<Vec3> = faces.iter().flatten().copied().collect();
        let mut shadow: Vec<(f32, f32)> = corners
            .iter()
            .map(|&p| {
                let t = light.y / (light.y - p.y);
                let q = light + (p - light) * t;
                (q.x, q.z)
            })
            .collect();
        shadow.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let cross = |o: (f32, f32), a: (f32, f32), b: (f32, f32)| {
            (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
        };
        let mut hull: Vec<(f32, f32)> = Vec::new();
        for pass in 0..2 {
            let start = hull.len();
            let points: Vec<_> = if pass == 0 { shadow.clone() } else { shadow.iter().rev().copied().collect() };
            for p in points {
                while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                    hull.pop();
                }
                hull.push(p);
            }
            hull.pop();
        }
        let expected: f32 = (0..hull.len())
            .map(|i| cross((0.0, 0.0), hull[i], hull[(i + 1) % hull.len()]))
            .sum::<f32>()
            .abs()
            / 2.0;

        let floor = [
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(-10.0, 0.0, 10.0),
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(10.0, 0.0, -10.0),
        ];
        let stride = RECORD_FLOATS + 4;
        let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
        let n = c.carve(&records(&floor), &edges, stride, 0, &at(Vec3::Y, floor[0])).len();
        let dark: f32 = (0..n)
            .map(|i| c.piece(i))
            .filter(|p| p.shadowed != 0)
            .map(|p| area(c.records(&p), stride))
            .sum();
        assert!((dark - expected).abs() < 1e-3 * expected, "shadow {dark}, expected {expected}");
        // The shadow, and a ring of pieces around it: at most one per outline plane.
        assert!(n <= 1 + 6, "{n} pieces");

        // Its own faces toward the light stay lit.
        for face in &faces {
            let normal = (face[1] - face[0]).cross(face[2] - face[0]).normalize();
            if normal.dot(light - face[0]) <= 0.0 {
                continue;
            }
            let edges: Vec<Edge> = (0..face.len() as u16).map(Edge::Input).collect();
            let n = c.carve(&records(face), &edges, RECORD_FLOATS + 4, 0, &at(normal, face[0])).len();
            assert!((0..n).all(|i| c.piece(i).shadowed == 0));
        }
    }
}
