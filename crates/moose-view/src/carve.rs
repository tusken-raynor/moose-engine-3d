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

/// A piece of a carved polygon: `count` records from `start` in the carver's buffer, the
/// shadow slots of the lights it is in the full shadow of, and the soft shadows it is in
/// (`volume_count` volume indices from `first_volume` in the carver's list): there a light
/// is partly covered.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Piece {
    start: u32,
    count: u32,
    pub shadowed: u32,
    first_volume: u32,
    volume_count: u32,
}

/// The soft edge of a shadow along one outline edge of an occluder, from a light with a
/// size: the region between two planes through the edge that graze the light's sphere on
/// opposite sides. On the outer one (distance 0) the edge starts to cover the light; on the
/// inner one it covers all of it. Points past the inner plane, on the shadow's side, have
/// positive distances to both.
#[derive(Clone, Copy, Debug)]
struct Wedge {
    outer: Half,
    inner: Half,
}

impl Wedge {
    /// How much of the light the edge covers at `p`: none on the outer plane (and outside
    /// it), all on the inner one (and past it), eased (smoothstep) between, by where `p` is
    /// between them.
    fn covers(&self, p: Vec3) -> f32 {
        let (outer, inner) = (self.outer.distance(p), self.inner.distance(p));
        let span = outer - inner;
        let c = if span > 1e-6 {
            (outer / span).clamp(0.0, 1.0)
        } else if outer > 0.0 {
            1.0
        } else {
            0.0
        };
        c * c * (3.0 - 2.0 * c)
    }
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
    /// Its occluders' shadow volumes (ranges of `Carver::volumes`).
    volumes: Range<u32>,
}

/// An occluder's shadow from one light: the region where it covers the light (all of it,
/// for a light with no size; some of it, otherwise), its soft edges, and the entity
/// casting it.
struct Volume {
    /// Ranges of `Carver::planes`.
    planes: Range<u32>,
    /// Ranges of `Carver::planes`: the wedges' inner planes, which bound the full shadow
    /// within the region; empty for a light with no size.
    inner: Range<u32>,
    /// Ranges of `Carver::planes`: two per wedge (in its order), bounding the sector of the
    /// ring around the full shadow it is carved in; empty if there are none.
    sectors: Range<u32>,
    /// Ranges of `Carver::wedges`; empty for a light with no size.
    wedges: Range<u32>,
    /// The light's shadow slot.
    slot: u8,
    owner: Option<u32>,
}

/// Carves polygons for every shadow-casting light. Reuse one; buffers keep their capacity.
#[derive(Default)]
pub(crate) struct Carver {
    casters: Vec<Caster>,
    planes: Vec<Half>,
    windows: Vec<(u32, Range<u32>)>,
    volumes: Vec<Volume>,
    wedges: Vec<Wedge>,
    // Per polygon: each piece's soft shadows, as indices into `volumes`.
    volume_lists: Vec<u32>,
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
        self.wedges.clear();
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
                        self.add_volume(light, slot, owner);
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
                        self.add_volume(light, slot, owner);
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
    ///
    /// For a light with a size (`Light::radius`), the region is where the occluder covers
    /// any of the light: each outline plane is turned about its edge to graze the light's
    /// sphere on the far side, and each edge gets a wedge (see [`Wedge`]) out to the
    /// plane grazing the near side, past which the edge covers all of it. The outline and
    /// faces seen from the light's center stand for those seen from all of it.
    fn add_volume(&mut self, light: &Light, slot: u8, owner: Option<u32>) {
        let (radius, light) = (light.radius, light.position);
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
        let first_wedge = self.wedges.len() as u32;
        // The outline's planes first: cuts outside the shadow happen there, once.
        let mut outline: Vec<(Vec3, Vec3)> = Vec::new();
        for (face, _) in &front {
            let p = &points[face.clone()];
            for i in 0..p.len() {
                let (a, b) = (p[i], p[(i + 1) % p.len()]);
                let shared = front.iter().any(|(other, _)| {
                    let q = &points[other.clone()];
                    (0..q.len()).any(|k| q[k] == b && q[(k + 1) % q.len()] == a)
                });
                if !shared {
                    outline.push((a, b));
                }
            }
        }
        // In order around, each edge starting where the last ends, if they close a loop.
        let mut ordered = outline.len() >= 3;
        for i in 1..outline.len() {
            match (i..outline.len()).find(|&j| outline[j].0 == outline[i - 1].1) {
                Some(j) => outline.swap(i, j),
                None => ordered = false,
            }
        }
        ordered &= outline.last().zip(outline.first()).is_some_and(|(l, f)| l.1 == f.0);
        let mut hards: Vec<Half> = Vec::with_capacity(outline.len());
        for &(a, b) in &outline {
            let Some(hard) = Half::through(light, a, b, center) else {
                ordered = false;
                continue;
            };
            match (radius > 0.0).then(|| grazing(light, radius, a, b, hard)).flatten() {
                Some((outer, inner)) => {
                    self.planes.push(outer);
                    self.wedges.push(Wedge { outer, inner });
                }
                None => {
                    self.planes.push(hard);
                    ordered = false;
                }
            }
            hards.push(hard);
        }
        for (face, normal) in &front {
            self.planes.push(Half {
                normal: -*normal,
                offset: normal.dot(points[face.start]) - CAP_BIAS,
            });
        }
        let end = self.planes.len() as u32;
        // The inner planes, after the region's.
        for w in first_wedge..self.wedges.len() as u32 {
            let inner = self.wedges[w as usize].inner;
            self.planes.push(inner);
        }
        let inner_end = self.planes.len() as u32;
        // Each wedge's sector: between the planes through the light and its edge's ends
        // that halve the angle to the neighboring edges, so the ring around the full
        // shadow is carved edge by edge. Two per edge: at its start, then at its end, each
        // facing the edge.
        let soft = self.wedges.len() as u32 > first_wedge;
        if soft && ordered && hards.len() == outline.len() {
            let n = outline.len();
            let halving = |at: Vec3, before: Half, after: Half, toward: Vec3| {
                let out = -(before.normal + after.normal);
                let m = (at - light).cross(out);
                let len = m.length();
                (len > 1e-9).then(|| {
                    let m = if m.dot(toward - at) < 0.0 { -m / len } else { m / len };
                    Half { normal: m, offset: -m.dot(at) }
                })
            };
            let mut sectors = Vec::with_capacity(2 * n);
            for k in 0..n {
                let (a, b) = outline[k];
                let (prev, next) = (hards[(k + n - 1) % n], hards[(k + 1) % n]);
                sectors.push(halving(a, prev, hards[k], b));
                sectors.push(halving(b, hards[k], next, a));
            }
            if sectors.iter().all(Option::is_some) {
                self.planes.extend(sectors.into_iter().flatten());
            }
        }
        self.volumes.push(Volume {
            planes: start..end,
            inner: end..inner_end,
            sectors: inner_end..self.planes.len() as u32,
            wedges: first_wedge..self.wedges.len() as u32,
            slot,
            owner,
        });
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
        self.volume_lists.clear();
        self.pieces.push(Piece {
            start: 0,
            count: edges.len() as u32,
            shadowed: 0,
            first_volume: 0,
            volume_count: 0,
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
                let volume = &self.volumes[v as usize];
                let (planes, inner, owner) = (volume.planes.clone(), volume.inner.clone(), volume.owner);
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
                    let inside = self.split_region(piece, planes.clone(), lined);
                    let rest = std::mem::take(&mut self.rest);
                    self.pieces.extend_from_slice(&rest);
                    self.rest = rest;
                    let Some(mut inside) = inside else {
                        continue;
                    };
                    if inner.is_empty() {
                        inside.shadowed |= bit;
                        self.pieces.push(inside);
                        continue;
                    }
                    // A soft shadow: carved sector by sector, each split by its wedge's inner
                    // plane (see `soft_sector`); what no sector holds, around the core.
                    let sectors = self.volumes[v as usize].sectors.clone();
                    let mut work = vec![inside];
                    for k in 0..sectors.len() / 2 {
                        let planes = sectors.start + 2 * k as u32..sectors.start + 2 * k as u32 + 2;
                        let mut next = Vec::new();
                        for part in work {
                            self.rest.clear();
                            let held = self.split_region(part, planes.clone(), lined);
                            next.append(&mut self.rest);
                            if let Some(held) = held {
                                let inner_k = self.planes[(inner.start + k as u32) as usize];
                                let (past, before) = self.split(held, inner_k, lined);
                                if let Some(before) = before {
                                    let before = self.with_volume(before, v);
                                    self.pieces.push(before);
                                }
                                if let Some(past) = past {
                                    self.soft_core(past, inner.clone(), v, bit, lined);
                                }
                            }
                        }
                        work = next;
                    }
                    for part in work {
                        self.soft_core(part, inner.clone(), v, bit, lined);
                    }
                }
            }
        }
        &self.pieces
    }

    /// Carves the full shadow of soft shadow volume `v` (within its `inner` planes) out of
    /// `piece`, marking it with `bit`; the rest of `piece` is in the soft shadow.
    fn soft_core(&mut self, piece: Piece, inner: Range<u32>, v: u32, bit: u32, lined: usize) {
        let saved = std::mem::take(&mut self.rest);
        if let Some(mut core) = self.split_region(piece, inner, lined) {
            core.shadowed |= bit;
            self.pieces.push(core);
        }
        let ring = std::mem::replace(&mut self.rest, saved);
        for part in ring {
            let part = self.with_volume(part, v);
            self.pieces.push(part);
        }
    }

    /// `piece` with soft shadow volume `v` added to its list.
    fn with_volume(&mut self, piece: Piece, v: u32) -> Piece {
        let first = self.volume_lists.len() as u32;
        self.volume_lists.extend_from_within(
            piece.first_volume as usize..(piece.first_volume + piece.volume_count) as usize,
        );
        self.volume_lists.push(v);
        Piece {
            first_volume: first,
            volume_count: piece.volume_count + 1,
            ..piece
        }
    }

    /// How much of each light whose soft shadows a piece is in reaches each of its
    /// vertices: appends, vertex by vertex, one value per such light (in shadow slot
    /// order), and returns their shadow slots as bits. Interpolated across the piece, the
    /// values are exact on its edges: 1 where a shadow's soft edge starts, 0 where its full
    /// shadow does. An occluder covers the product of what its wedges cover (near its
    /// corners, two at once), and what reaches a point is the product over occluders of
    /// what each leaves uncovered.
    pub fn soft_values(&self, piece: &Piece, out: &mut Vec<f32>) -> u32 {
        let volumes = &self.volume_lists
            [piece.first_volume as usize..(piece.first_volume + piece.volume_count) as usize];
        let slots = volumes
            .iter()
            .fold(0u32, |bits, &v| bits | 1 << self.volumes[v as usize].slot);
        if slots == 0 {
            return 0;
        }
        let s = self.stride;
        for r in self.records(piece).chunks_exact(s) {
            let p = Vec3::new(r[3], r[4], r[5]);
            let mut bits = slots;
            while bits != 0 {
                let slot = bits.trailing_zeros() as u8;
                bits &= bits - 1;
                let mut reaches = 1.0;
                for &v in volumes {
                    let volume = &self.volumes[v as usize];
                    if volume.slot == slot {
                        let covered: f32 = self.wedges[volume.wedges.start as usize..volume.wedges.end as usize]
                            .iter()
                            .map(|w| w.covers(p))
                            .product();
                        reaches *= 1.0 - covered;
                    }
                }
                out.push(reaches);
            }
        }
        slots
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
        let keep = |p: Piece| Piece {
            shadowed: piece.shadowed,
            first_volume: piece.first_volume,
            volume_count: piece.volume_count,
            ..p
        };
        let f = self.append(&front).map(keep);
        let b = self.append(&back).map(keep);
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
            first_volume: 0,
            volume_count: 0,
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

/// The planes through the edge `a`-`b` that graze a light's sphere (at `light`, of
/// `radius`) on either side, turned about the edge from `hard` (the plane through the edge
/// and the light's center, facing into the shadow): the outer one with the light's center
/// `radius` outside it, the inner one with it `radius` inside, both facing into the shadow.
/// `None` if the edge's line passes through the light.
fn grazing(light: Vec3, radius: f32, a: Vec3, b: Vec3, hard: Half) -> Option<(Half, Half)> {
    let u = (b - a).normalize();
    // From the edge's line to the light, square to it.
    let c = (light - a) - u * u.dot(light - a);
    let d = c.length();
    if d <= radius * 1.001 {
        return None;
    }
    let w = u.cross(hard.normal);
    let side = w.dot(c).signum();
    // Turned by the angle whose sine is `t / d`: the light's center is then `t` from it.
    let plane = |t: f32| {
        let sin = t / (side * d);
        let m = hard.normal * (1.0 - sin * sin).sqrt() + w * sin;
        Half {
            normal: m,
            offset: -m.dot(a),
        }
    };
    Some((plane(-radius), plane(radius)))
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
            c.add_volume(&Light::point(0, light, Vec3::ONE, 100.0), 0, None);
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
        assert_eq!(c.volumes[0].planes.len(), 6 + 3, "six outline planes, three faces'");

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

    #[test]
    fn a_light_with_a_size_casts_a_core_and_a_soft_edge() {
        // A 1 m square 2 m below a light of radius 0.2, over a floor 1 m below it. Seen in
        // section, the core's edge runs from the light's far side past the square's edge
        // (x = 0.5 + 0.3 / 2 = 0.65 on the floor) and the soft edge's outer side from its
        // near side (0.5 + 0.7 / 2 = 0.85); tangent planes differ from those a little.
        let center = Vec3::new(0.0, 3.0, 0.0);
        let mut light = Light::point(0, center, Vec3::ONE, 100.0);
        light.radius = 0.2;
        let square = [
            Vec3::new(-0.5, 1.0, -0.5),
            Vec3::new(-0.5, 1.0, 0.5),
            Vec3::new(0.5, 1.0, 0.5),
            Vec3::new(0.5, 1.0, -0.5),
        ];
        let mut c = Carver::default();
        c.points.extend_from_slice(&square);
        c.faces.push(0..4);
        c.add_volume(&light, 0, None);
        c.casters.push(Caster {
            bit: 1,
            position: center,
            range: 100.0,
            whole: 0,
            windows: 0..0,
            volumes: 0..1,
        });
        assert_eq!(c.wedges.len(), 4, "a wedge per outline edge");
        let floor = [
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(-3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, -3.0),
        ];
        let stride = RECORD_FLOATS + 4;
        let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
        let n = c.carve(&records(&floor), &edges, stride, 0, &at(Vec3::Y, floor[0])).len();
        let (mut core, mut soft, mut total) = (0.0, 0.0, 0.0);
        let mut reach: f32 = 0.0;
        for i in 0..n {
            let p = c.piece(i);
            let a = area(c.records(&p), stride);
            total += a;
            if p.shadowed != 0 {
                core += a;
            } else if p.volume_count > 0 {
                soft += a;
                for r in c.records(&p).chunks(stride) {
                    reach = reach.max(r[3].abs()).max(r[5].abs());
                }
            }
        }
        assert!((total - 36.0).abs() < 1e-3, "pieces cover the floor: {total}");
        assert!((core - 1.3 * 1.3).abs() < 0.1, "core {core}");
        assert!((core + soft - 1.7 * 1.7).abs() < 0.15, "core and soft edge {}", core + soft);
        assert!((reach - 0.85).abs() < 0.02, "soft edge reaches {reach}");

        // The light reaching the soft pieces' vertices: all of it where the soft edge
        // starts, none where the core does, and between on the way.
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for i in 0..n {
            let p = c.piece(i);
            let mut values = Vec::new();
            let slots = c.soft_values(&p, &mut values);
            if p.volume_count == 0 {
                assert_eq!(slots, 0);
                continue;
            }
            assert_eq!(slots, 1);
            for (r, &v) in c.records(&p).chunks(stride).zip(&values) {
                assert!((0.0..=1.0).contains(&v), "{v}");
                let (x, z) = (r[3].abs(), r[5].abs());
                if x.max(z) > 0.84 && x.min(z) < 0.5 {
                    assert!(v > 0.99, "outer edge {v} at {x}, {z}");
                }
                if x.max(z) < 0.66 && x.min(z) < 0.5 && x.max(z) > 0.6 {
                    assert!(v < 0.01, "core's edge {v} at {x}, {z}");
                }
                (lo, hi) = (lo.min(v), hi.max(v));
            }
        }
        assert!(lo < 0.01 && hi > 0.99, "{lo}..{hi}");
    }
}
