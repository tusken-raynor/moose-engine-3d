//! Shadows carved out of polygons: each polygon a shadow-casting light reaches is split, in
//! world space, into pieces the light reaches, pieces in its full shadow, and pieces in its
//! soft shadow (with how much of it reaches their vertices). The view keeps the polygon
//! whole and hands the shadowed pieces to the rasterizer as its shadow pieces
//! ([`ShadowPiece`](crate::ShadowPiece)), which it draws into a shadow buffer pixel by pixel
//! as it shades the polygon: a shadow's edge is exact at any resolution, whatever the
//! spacing of the polygon's sample points.
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

use std::collections::HashMap;
use std::ops::Range;

use glam::Vec3;

use moose_assets::{Assets, Light, Mesh, MeshId};
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
/// Limit on portals followed from a light.
const MAX_DEPTH: u16 = 16;
/// Most shadow slots (the bits of a shadow mask).
pub const MAX_SHADOW_SLOTS: u8 = 32;
/// How close a piece's corner is to a soft edge's line to count as on it: the distance
/// between the wedge's planes there, against that at the piece's center. (Carved pieces
/// stop short of an occluder by `CAP_BIAS`, so a corner where it touches a surface is
/// near the line, not on it.)
const NEAR_LINE: f32 = 0.01;
/// How far a cached soft piece's value may be, halfway along an edge, from halfway between
/// its ends' values before a corner is added there (see [`Carver::cache`]); and how many
/// times an edge is halved at most.
const REFINE_OFF: f32 = 0.125;
const REFINE_DEPTH: u32 = 5;

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

    /// The other side.
    fn flip(self) -> Half {
        Half {
            normal: -self.normal,
            offset: -self.offset,
        }
    }
}

/// Where a light comes from, as carving sees it: a point (a light's center, with the
/// radius of its source), or a direction (a directional light: the way its light travels,
/// and the sine of its source's angular radius). Everything carving asks of a light is
/// asked of this, so point lights, spot lights and directional lights carve alike.
#[derive(Clone, Copy, Debug)]
enum Source {
    Point { at: Vec3, radius: f32 },
    Distant { toward: Vec3, spread: f32 },
}

impl Source {
    fn of(light: &Light) -> Source {
        if light.directional {
            Source::Distant {
                toward: light.direction,
                spread: light.radius,
            }
        } else {
            Source::Point {
                at: light.position,
                radius: light.radius,
            }
        }
    }

    /// Whether its source has a size (casts soft shadows).
    fn soft(&self) -> bool {
        match *self {
            Source::Point { radius, .. } => radius > 0.0,
            Source::Distant { spread, .. } => spread > 0.0,
        }
    }

    /// The way light travels from it to `p` (not unit length for a point).
    fn ray(&self, p: Vec3) -> Vec3 {
        match *self {
            Source::Point { at, .. } => p - at,
            Source::Distant { toward, .. } => toward,
        }
    }

    /// Whether the plane through `p` facing `normal` faces the light (clearly).
    fn faces(&self, normal: Vec3, p: Vec3) -> bool {
        match *self {
            Source::Point { at, .. } => normal.dot(at - p) > ON_PLANE,
            Source::Distant { toward, .. } => normal.dot(toward) < -1e-6,
        }
    }

    /// The plane through the edge `a`-`b` and the light (for a directional light, along
    /// its direction), facing `inside`; `None` if the edge is seen end-on.
    fn through(&self, a: Vec3, b: Vec3, inside: Vec3) -> Option<Half> {
        let n = match *self {
            Source::Point { at, .. } => (a - at).cross(b - at),
            Source::Distant { toward, .. } => (b - a).cross(toward),
        };
        let len = n.length();
        if len < 1e-9 {
            return None;
        }
        let n = if n.dot(inside - a) < 0.0 { -n / len } else { n / len };
        Some(Half {
            normal: n,
            offset: -n.dot(a),
        })
    }

    /// The planes through the edge `a`-`b` that graze the source on either side, turned
    /// about the edge from `hard` (the plane through the edge and the source's center,
    /// facing into the shadow): the outer one with the source wholly outside it, the inner
    /// one with it wholly inside, both facing into the shadow. `None` if the source has no
    /// size, or the edge's line passes through it.
    ///
    /// The outer plane turns no further than keeps the points `keep` (the occluder's) on
    /// its shadow side. It would pass through the occluder where the source straddles the
    /// plane of a face next to the edge (a light level with a crate's top, say): from part
    /// of the source that face is turned away, and its edge is no outline. There the soft
    /// edge would be all on one side of that plane, and cut through the occluder's own
    /// shadow; stopped at the face's plane, it fades from there instead.
    fn grazing(&self, a: Vec3, b: Vec3, hard: Half, keep: &[Vec3]) -> Option<(Half, Half)> {
        let u = (b - a).normalize();
        // Toward the source from the edge's line, square to it; and the sine of the angle
        // the source's radius subtends from the line.
        let (c, sin) = match *self {
            Source::Point { at, radius } => {
                let c = (at - a) - u * u.dot(at - a);
                (c, radius / c.length())
            }
            Source::Distant { toward, spread } => {
                let c = -toward + u * u.dot(toward);
                (c, spread / c.length())
            }
        };
        if !(sin > 0.0 && sin < 0.999) {
            return None;
        }
        let w = u.cross(hard.normal);
        let side = w.dot(c).signum();
        // Turned by that angle, either way: the source's center is then its radius from it.
        let plane = |s: f32| {
            let m = hard.normal * (1.0 - s * s).sqrt() + w * s;
            Half {
                normal: m,
                offset: -m.dot(a),
            }
        };
        // The outer plane turns toward `away`: at most as far as the first point of `keep`
        // it would pass.
        let away = -w * side;
        let mut outer = sin.asin();
        for &p in keep {
            let q = p - a;
            let toward = -away.dot(q);
            if toward > 1e-6 {
                outer = outer.min(hard.normal.dot(q).max(0.0).atan2(toward));
            }
        }
        Some((plane(-outer.sin() / side), plane(sin / side)))
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
/// shadow slots of the lights it is in the full shadow of, those of the beams it is inside
/// the pyramid of (see [`Light::beam`]), and the soft shadows it is in (`volume_count`
/// volume indices from `first_volume` in the carver's list): there a light is partly
/// covered.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Piece {
    start: u32,
    count: u32,
    pub shadowed: u32,
    pub beamed: u32,
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
    /// How much of the light the edge covers at `p`, a corner of a piece centered on
    /// `center`, along the piece's edge from `p` to `along` (see [`Wedge::covers_at`]).
    ///
    /// Where an occluder's edge touches the surface its shadow falls on, the corner is on
    /// the edge's line, where both planes meet: there it has no one value. What it covers
    /// is the same all along each ray out from the line, from none to all, so it is taken
    /// along the piece's edge (at `along`, or at `center` if that is on the line too).
    fn covers(&self, p: Vec3, along: Vec3, center: Vec3) -> f32 {
        let near = NEAR_LINE * self.span(center);
        if self.span(p) > near {
            self.covers_at(p)
        } else if self.span(along) > near {
            self.covers_at(along)
        } else {
            self.covers_at(center)
        }
    }

    /// How much of the light the edge covers at `p`: none on the outer plane (and outside
    /// it), all on the inner one (and past it), eased (smoothstep) between, by where `p` is
    /// between them.
    fn covers_at(&self, p: Vec3) -> f32 {
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

    /// How far apart its planes are at `p`: 0 on the edge's line, where they meet, and
    /// growing in step with the distance from it.
    fn span(&self, p: Vec3) -> f32 {
        (self.outer.distance(p) - self.inner.distance(p)).abs()
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
    /// Which parts of the lights' shadows to carve.
    pub parts: Parts,
    /// Carve beams (see [`Light::beam`]): not in reflections, where they are lit at sample
    /// points.
    pub beams: bool,
}

/// Which parts of the lights' shadows [`Carver::carve`] carves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Parts {
    /// All of every light's.
    All,
    /// Only what never changes, of the lights with these shadow slot bits (static lights):
    /// their windows and static occluders. No other light's.
    Static(u32),
    /// All of every light's, except the lights with these shadow slot bits, whose static
    /// parts are cached: only their moving occluders.
    Cached(u32),
    /// Only beams' pyramids (see [`Light::beam`]): no shadows.
    Beams,
}

/// The shadow of a static light on a static polygon: its static parts (windows and static
/// occluders), carved once in world space and kept (see [`Carver::cached`]).
#[derive(Default)]
pub(crate) struct Cached {
    /// Every point of the polygon is in the light's full shadow.
    pub full: bool,
    /// Its shadow pieces: ranges of `vertices`, and of `piece_soft` (the soft edges each
    /// is in; none if it is in full shadow).
    pub pieces: Vec<(Range<u32>, Range<u32>)>,
    pub vertices: Vec<CachedVertex>,
    /// Indices into `soft`, per piece.
    piece_soft: Vec<u32>,
    /// Soft edges: ranges of `wedges`, and whether each is a window's.
    soft: Vec<(Range<u32>, bool)>,
    wedges: Vec<Wedge>,
}

impl Cached {
    /// How much of the light reaches `p`, a corner of a piece centered on `center` (see
    /// [`Cached::pieces`]), along its edge to `along` (see [`reaching`]), past the soft edges
    /// it is in: none if it has none (it is in full shadow).
    pub fn light(&self, soft: &Range<u32>, p: Vec3, along: Vec3, center: Vec3) -> f32 {
        if soft.is_empty() {
            return 0.0;
        }
        reaching(
            self.piece_soft[soft.start as usize..soft.end as usize]
                .iter()
                .map(|&i| {
                    let (w, window) = &self.soft[i as usize];
                    (&self.wedges[w.start as usize..w.end as usize], *window)
                }),
            (p, along, center),
        )
    }
}

/// A vertex of a cached shadow piece: where it is, and how the edge leaving it is walked.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CachedVertex {
    pub world: Vec3,
    pub edge: CachedEdge,
}

/// How a cached piece's edge is walked, whatever the polygon is clipped to on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CachedEdge {
    /// Along the polygon's edge from its vertex `n` (in its own order), as the polygon
    /// walks it.
    Polygon(u16),
    /// Along the line through these two world points: a cut, which the piece on its
    /// other side shares.
    Line(Vec3, Vec3),
}

/// What a static light's cached shadows were carved for: they are kept while it's the
/// same.
#[derive(Clone, Copy, PartialEq)]
struct CacheKey {
    position: Vec3,
    direction: Vec3,
    radius: f32,
    range: f32,
    cos_outer: f32,
}

/// A window into a sector from a light: its planes through the light (to clip windows seen
/// through it) and its volume.
type Window = (Range<u32>, u32);

/// A shadow-casting light, for one frame.
struct Caster {
    bit: u32,
    /// Its light never moves or changes.
    is_static: bool,
    source: Source,
    range: f32,
    /// Its light, for its range and cone.
    light: Light,
    /// The sector it lights whole: its own (none for a directional light, which lights
    /// only through windows).
    whole: Option<u32>,
    /// The windows it lights other sectors through: (sector, planes).
    windows: Range<u32>,
    /// Its occluders' shadow volumes (ranges of `Carver::volumes`).
    volumes: Range<u32>,
    /// Whether it casts shadows (it may be here only for its beam): its windows and
    /// occluders are carved.
    shadows: bool,
    /// A beam (see [`Light::beam`]).
    beam: Option<BeamCaster>,
}

/// A beam, for one frame: its pyramid's planes in `Carver::planes` (none if its cone is
/// too wide for one: all of it is inside), and its frame.
#[derive(Clone)]
struct BeamCaster {
    planes: Range<u32>,
    frame: BeamFrame,
}

/// Where a beam's rays are measured (see [`Light::beam`]): from its light, in a frame whose
/// x axis is the beam's axis, and its cone (see [`Light::cone`]). The renderer's shadow
/// buffer fades the light by the angle between a pixel's ray and x.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BeamFrame {
    pub origin: Vec3,
    /// Rows: the axis, then two ways square to it.
    pub axes: [Vec3; 3],
    pub cone: (f32, f32),
}

impl BeamFrame {
    fn of(light: &Light) -> BeamFrame {
        let axis = light.direction;
        let helper = if axis.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
        let u = axis.cross(helper).normalize();
        BeamFrame {
            origin: light.position,
            axes: [axis, u, u.cross(axis)],
            cone: light.cone(),
        }
    }

    /// The way from the light to `p`, in its frame.
    pub fn ray(&self, p: Vec3) -> Vec3 {
        let d = p - self.origin;
        Vec3::new(self.axes[0].dot(d), self.axes[1].dot(d), self.axes[2].dot(d))
    }
}

/// What a convex outline does to a light's reach: an occluder's shadow, or the light let
/// through a window (an opening to another sector). Its region is where the occluder covers
/// any of the light, or the window lets any through (all of it, for a light with no size);
/// its core, within that, where it covers all of it (the full shadow), or lets all of it
/// through; its soft edges between; and the entity casting it.
struct Volume {
    /// Ranges of `Carver::planes`.
    planes: Range<u32>,
    /// Ranges of `Carver::planes`: one per wedge, bounding the core within the region;
    /// empty for a light with no size.
    core: Range<u32>,
    /// Whether it is a window: its core is lit, and what it lets through multiplies with
    /// other windows' (an occluder's core is in full shadow, and the parts of the light
    /// occluders cover add up).
    window: bool,
    /// Ranges of `Carver::planes`: two per wedge (in its order), bounding the sector of the
    /// ring around the full shadow it is carved in; empty if there are none.
    sectors: Range<u32>,
    /// Ranges of `Carver::wedges`; empty for a light with no size.
    wedges: Range<u32>,
    /// The light's shadow slot.
    slot: u8,
    owner: Option<u32>,
    /// Nothing about it ever changes: a window, or a static occluder's shadow (from a
    /// static light, it can be cached).
    is_static: bool,
}

/// Carves polygons for every shadow-casting light. Reuse one; buffers keep their capacity.
#[derive(Default)]
pub(crate) struct Carver {
    casters: Vec<Caster>,
    planes: Vec<Half>,
    /// The windows into sectors: (sector, its volume).
    windows: Vec<(u32, u32)>,
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
    /// Whether each occluder model seen so far is convex.
    convex: HashMap<(MeshId, usize), bool>,
    next_points: Vec<Vec3>,
    /// Sectors to visit from a light: (sector, window in, portal depth).
    stack: Vec<(u32, Option<Window>, u16)>,
    /// While building a cache: edges along the polygon's own keep that label.
    keep_polygon_edges: bool,
    /// Cached shadows: per (shadow slot, polygon key), and per slot what they were carved for.
    cache: HashMap<(u8, (u8, u32, u32)), Cached>,
    cache_keys: [Option<CacheKey>; MAX_SHADOW_SLOTS as usize],
}

impl Carver {
    /// Gathers what blocks each shadow-casting light this frame: where portals let it
    /// through, and its occluders' shadow volumes. Without `dynamic`, only static lights'
    /// (whose shadows are cached; see `ViewConfig::dynamic_shadows`).
    pub fn prepare(&mut self, world: &World, assets: &Assets, lights: &[Light], dynamic: bool) {
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
            let shadows = light.shadows && (dynamic || light.is_static);
            let beamed = light.beam && !light.is_point() && !light.directional;
            if !shadows && !beamed {
                continue;
            }
            let source = Source::of(light);
            // Windows: out through portals, each clipped to the window it was seen through.
            // A directional light starts at the sky surfaces it shines in through: windows
            // into their sectors.
            let windows_start = self.windows.len() as u32;
            let mut reached = vec![false; world.sectors.len()];
            self.stack.clear();
            if !shadows {
                // Here for its beam only.
            } else if light.directional {
                for (s, sector) in world.sectors.iter().enumerate() {
                    for p in sector.polygons.clone() {
                        let polygon = &geometry.polygons[p as usize];
                        if !polygon.flags.sky() || polygon.plane.normal.dot(light.direction) <= 0.0
                        {
                            continue;
                        }
                        self.points.clear();
                        self.points.extend(geometry.polygon_points(polygon));
                        let window = self.add_window(light, slot);
                        self.stack.push((s as u32, Some(window), 0));
                    }
                }
            } else {
                self.stack.push((light.sector, None, 0));
            }
            while let Some((sector, window, depth)) = self.stack.pop() {
                reached[sector as usize] = true;
                if let Some((_, volume)) = &window {
                    self.windows.push((sector, *volume));
                }
                if depth >= MAX_DEPTH {
                    continue;
                }
                for p in world.sectors[sector as usize].portals.clone() {
                    let portal = &world.portals[p as usize];
                    // Out through it: the portal faces into this sector.
                    if !portal.flags.render_through()
                        || !source.faces(portal.plane.normal, portal_point(geometry, portal))
                    {
                        continue;
                    }
                    self.points.clear();
                    self.points
                        .extend(portal.positions.iter().map(|&i| geometry.positions[i as usize]));
                    if let Some((w, _)) = &window {
                        for k in w.clone() {
                            clip_points(&mut self.points, &mut self.next_points, self.planes[k as usize]);
                        }
                    }
                    if self.points.len() < 3 {
                        continue;
                    }
                    let center = self.points.iter().copied().sum::<Vec3>() / self.points.len() as f32;
                    let radius = self.points.iter().map(|q| q.distance(center)).fold(0.0, f32::max);
                    if !light.directional {
                        let l = light.position;
                        let nearest =
                            self.points.iter().map(|q| q.distance(l)).fold(f32::INFINITY, f32::min);
                        if nearest - radius >= light.range || !light.cone_reaches(center, radius) {
                            continue;
                        }
                    }
                    let window = self.add_window(light, slot);
                    self.stack.push((portal.target, Some(window), depth + 1));
                }
            }
            let windows_end = self.windows.len() as u32;

            // Occluders in the sectors it reaches.
            let volumes_start = self.volumes.len() as u32;
            for (index, entity) in world.entities.iter().enumerate() {
                let owner = Some(index as u32);
                if !shadows
                    || entity.occluder == Occluder::None
                    || !entity.sectors.iter().any(|&s| reached[s as usize])
                {
                    continue;
                }
                let center = (entity.bounds.min + entity.bounds.max) * 0.5;
                let radius = (entity.bounds.max - entity.bounds.min).length() * 0.5;
                if !light.directional
                    && (center.distance(light.position) - radius >= light.range
                        || center.distance(light.position) <= radius
                        || !light.cone_reaches(center, radius))
                {
                    continue;
                }
                let first = self.volumes.len();
                let transform = entity.transform();
                let proxy = match entity.occluder {
                    Occluder::None => continue,
                    // Levels of detail aren't loaded yet: the model itself.
                    Occluder::Mesh | Occluder::Lod(_) => Some(entity.mesh),
                    Occluder::Model(mesh) => Some(mesh),
                    Occluder::Facing { .. } => None,
                };
                match (proxy, entity.occluder) {
                    (Some(mesh_id), _) => {
                        let mesh = assets.mesh(mesh_id);
                        // Its parts: its shadow proxies bone by bone, or the model whole.
                        for (part, polygons) in occluder_parts(mesh).iter().enumerate() {
                            let convex = *self
                                .convex
                                .entry((mesh_id, part))
                                .or_insert_with(|| is_convex(mesh, polygons));
                            self.points.clear();
                            self.faces.clear();
                            for &k in polygons {
                                let start = self.points.len();
                                self.points.extend(
                                    mesh.polygon_points(&mesh.polygons[k])
                                        .map(|p| transform.transform_point3(p)),
                                );
                                self.faces.push(start..self.points.len());
                            }
                            if convex {
                                self.add_volume(light, slot, owner);
                            } else {
                                // Any other shape: each face toward the light casts its own
                                // volume, which together are exactly its shadow. Hard-edged
                                // for now: soft edges need its silhouette's edges only.
                                let hard = Light { radius: 0.0, ..*light };
                                let faces = std::mem::take(&mut self.faces);
                                for face in &faces {
                                    self.faces.clear();
                                    self.faces.push(face.clone());
                                    self.add_volume(&hard, slot, owner);
                                }
                                self.faces = faces;
                            }
                        }
                    }
                    (None, Occluder::Facing { sides, radius, center }) => {
                        // Seen from the light, a ball's outline is the circle where rays
                        // from the light graze it: the polygon is on it, facing the light.
                        // (From a directional light, its outline is a great circle.)
                        let c = transform.transform_point3(center);
                        let r = radius * entity.scale;
                        let (axis, middle, ring) = match source {
                            Source::Point { at, .. } => {
                                let to = c - at;
                                let d = to.length();
                                if d <= r {
                                    continue;
                                }
                                let axis = to / d;
                                let along = d - r * r / d;
                                (axis, at + axis * along, r * (1.0 - r * r / (d * d)).sqrt())
                            }
                            Source::Distant { toward, .. } => (toward, c, r),
                        };
                        let helper = if axis.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
                        // Counter-clockwise seen from the light, so it faces it.
                        let u = axis.cross(helper).normalize();
                        let v = u.cross(axis);
                        let sides = sides as usize;
                        self.points.clear();
                        self.points.extend((0..sides).map(|k| {
                            let a = k as f32 * std::f32::consts::TAU / sides as f32;
                            middle + (u * a.cos() + v * a.sin()) * ring
                        }));
                        self.faces.clear();
                        self.faces.push(0..sides);
                        self.add_volume(light, slot, owner);
                    }
                    _ => {}
                }
                for volume in &mut self.volumes[first..] {
                    volume.is_static = entity.is_static;
                }
            }
            let volumes_end = self.volumes.len() as u32;
            let beam = beamed.then(|| self.add_beam(light));
            // A static light's cached shadows hold while it is the same light.
            let key = CacheKey {
                position: light.position,
                direction: light.direction,
                radius: light.radius,
                range: light.range,
                cos_outer: light.cos_outer,
            };
            let kept = &mut self.cache_keys[slot as usize];
            if !light.is_static || *kept != Some(key) {
                if kept.is_some() {
                    self.cache.retain(|&(s, _), _| s != slot);
                }
                *kept = light.is_static.then_some(key);
            }
            self.casters.push(Caster {
                bit: 1 << slot,
                is_static: light.is_static,
                source,
                range: light.range,
                light: *light,
                whole: (!light.directional).then_some(light.sector),
                windows: windows_start..windows_end,
                volumes: volumes_start..volumes_end,
                shadows,
                beam,
            });
        }
    }

    /// A beam (see [`Light::beam`]): its pyramid, four planes through the light each
    /// touching its outer cone on one side (facing in); none if the cone is too wide for
    /// one (near a half-space).
    fn add_beam(&mut self, light: &Light) -> BeamCaster {
        let frame = BeamFrame::of(light);
        let [axis, u, v] = frame.axes;
        let start = self.planes.len() as u32;
        let cos = light.cos_outer;
        let sin = (1.0 - cos * cos).max(0.0).sqrt();
        if cos > 0.05 {
            for side in [u, -u, v, -v] {
                let normal = axis * sin - side * cos;
                self.planes.push(Half {
                    normal,
                    offset: -normal.dot(light.position),
                });
            }
        }
        BeamCaster {
            planes: start..self.planes.len() as u32,
            frame,
        }
    }

    /// Forgets every cached shadow (see [`Carver::cache`]): for after the world changes
    /// (a static entity moved, a surface edited), when they may no longer hold.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
        // Meshes may have been replaced (models reloaded, posed copies reused).
        self.convex.clear();
        self.cache_keys = [None; MAX_SHADOW_SLOTS as usize];
    }

    /// Whether any light this frame is a beam (see [`Light::beam`]).
    pub fn has_beams(&self) -> bool {
        self.casters.iter().any(|c| c.beam.is_some())
    }

    /// The frame of the beam in shadow slot `slot`, if it is one.
    pub fn beam(&self, slot: u8) -> Option<BeamFrame> {
        self.casters
            .iter()
            .find(|c| c.bit == 1 << slot)
            .and_then(|c| c.beam.as_ref().map(|b| b.frame))
    }

    /// Adds the window a light sees through the convex outline in `self.points` (a portal,
    /// or a sky surface for a directional light): its planes through the light, to clip
    /// windows seen through it, then its volume (see [`add_outline`](Self::add_outline)).
    fn add_window(&mut self, light: &Light, slot: u8) -> Window {
        let source = Source::of(light);
        let n = self.points.len();
        let center = self.points.iter().copied().sum::<Vec3>() / n as f32;
        let start = self.planes.len() as u32;
        for i in 0..n {
            let (a, b) = (self.points[i], self.points[(i + 1) % n]);
            if let Some(h) = source.through(a, b, center) {
                self.planes.push(h);
            }
        }
        let end = self.planes.len() as u32;
        let outline: Vec<(Vec3, Vec3)> =
            (0..n).map(|i| (self.points[i], self.points[(i + 1) % n])).collect();
        let volume = self.volumes.len() as u32;
        self.add_outline(light, slot, None, &outline, true, center, &[], true, &[]);
        (start..end, volume)
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
    fn add_volume(&mut self, light_ref: &Light, slot: u8, owner: Option<u32>) {
        let source = Source::of(light_ref);
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
            if len < 1e-12 || !source.faces(normal / len, p[0]) {
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
        // Its outline, in order around, each edge starting where the last ends (if they
        // close a loop), and its faces' planes, which the shadow is behind.
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
        let mut ordered = outline.len() >= 3;
        for i in 1..outline.len() {
            match (i..outline.len()).find(|&j| outline[j].0 == outline[i - 1].1) {
                Some(j) => outline.swap(i, j),
                None => ordered = false,
            }
        }
        ordered &= outline.last().zip(outline.first()).is_some_and(|(l, f)| l.1 == f.0);
        let caps: Vec<Half> = front
            .iter()
            .map(|(face, normal)| Half {
                normal: -*normal,
                offset: normal.dot(points[face.start]) - CAP_BIAS,
            })
            .collect();
        // The occluder's points, which its soft edge's outer side keeps (see
        // `Source::grazing`).
        let keep: Vec<Vec3> = self.faces.iter().flat_map(|f| self.points[f.clone()].iter().copied()).collect();
        self.add_outline(light_ref, slot, owner, &outline, ordered, center, &caps, false, &keep);
    }

    /// Adds the volume of a convex outline (its edges, in order around if `ordered`, about
    /// `center`) seen from a light: an occluder's shadow (behind its `caps` too), or a
    /// window's light (`window`). Its region is within the planes through the light and
    /// the outline's edges.
    ///
    /// For a light with a size (`Light::radius`), each edge gets a wedge (see [`Wedge`])
    /// between the planes through it that graze the light's sphere on either side. The
    /// region reaches out to the plane on the lit side of an occluder's edge (where it
    /// starts to cover the light), or of a window's (where it starts to let it through);
    /// the core, within it, to the plane on the other side. The ring between is carved in
    /// sectors, one per wedge: between planes through the light and its edge's ends that
    /// halve the angle to the neighboring edges.
    #[allow(clippy::too_many_arguments)]
    fn add_outline(
        &mut self,
        light: &Light,
        slot: u8,
        owner: Option<u32>,
        outline: &[(Vec3, Vec3)],
        ordered: bool,
        center: Vec3,
        caps: &[Half],
        window: bool,
        keep: &[Vec3],
    ) {
        let source = Source::of(light);
        let start = self.planes.len() as u32;
        let first_wedge = self.wedges.len() as u32;
        let mut core = Vec::with_capacity(outline.len());
        let mut hards: Vec<Half> = Vec::with_capacity(outline.len());
        let mut soft = source.soft() && ordered;
        for &(a, b) in outline {
            // Facing into the outline.
            let Some(hard) = source.through(a, b, center) else {
                soft = false;
                continue;
            };
            // Facing into the shadow: into an occluder's outline, out of a window's.
            let shadow = if window { hard.flip() } else { hard };
            match source.grazing(a, b, shadow, keep) {
                Some((outer, inner)) => {
                    self.planes.push(if window { inner.flip() } else { outer });
                    core.push(if window { outer.flip() } else { inner });
                    self.wedges.push(Wedge { outer, inner });
                }
                None => {
                    self.planes.push(hard);
                    soft = false;
                }
            }
            hards.push(hard);
        }
        self.planes.extend_from_slice(caps);
        let end = self.planes.len() as u32;
        if !soft || hards.len() != outline.len() {
            // Hard (or not wholly soft): the region only.
            self.wedges.truncate(first_wedge as usize);
            self.volumes.push(Volume {
                planes: start..end,
                core: end..end,
                window,
                sectors: end..end,
                wedges: first_wedge..first_wedge,
                slot,
                owner,
                is_static: true,
            });
            return;
        }
        self.planes.extend(core);
        let core_end = self.planes.len() as u32;
        let n = outline.len();
        let halving = |at: Vec3, before: Half, after: Half, toward: Vec3| {
            let out = -(before.normal + after.normal);
            let m = source.ray(at).cross(out);
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
        self.volumes.push(Volume {
            planes: start..end,
            core: end..core_end,
            window,
            sectors: core_end..self.planes.len() as u32,
            wedges: first_wedge..self.wedges.len() as u32,
            slot,
            owner,
            is_static: true,
        });
    }

    /// The shadow slot bits of static lights, for a static polygon (world space, in its own
    /// order: `polygon`, facing `receiver.normal`): what their shadows' cached static parts
    /// would cover it with. Carves them the first time (see [`Parts::Static`]).
    pub fn cache(&mut self, key: (u8, u32, u32), polygon: &[Vec3], receiver: &Receiver) -> u32 {
        let mut bits = 0;
        for c in 0..self.casters.len() {
            let caster = &self.casters[c];
            if !caster.is_static {
                continue;
            }
            let (bit, slot) = (caster.bit, caster.bit.trailing_zeros() as u8);
            bits |= bit;
            if self.cache.contains_key(&(slot, key)) {
                continue;
            }
            // Carve it in world space: world positions in place of clip coordinates, so
            // cuts carry world points, and edges along the polygon keep saying so.
            let n = polygon.len();
            let stride = 6 + n;
            let mut records = Vec::with_capacity(n * stride);
            for (i, p) in polygon.iter().enumerate() {
                records.extend_from_slice(&[p.x, p.y, p.z, p.x, p.y, p.z]);
                records.extend((0..n).map(|k| if k == i { 1.0 } else { 0.0 }));
            }
            let edges: Vec<Edge> = (0..n as u16).map(Edge::Input).collect();
            self.keep_polygon_edges = true;
            let only = Receiver {
                parts: Parts::Static(bit),
                ..*receiver
            };
            let count = self.carve(&records, &edges, stride, 0, &only).len();
            self.keep_polygon_edges = false;
            let mut cached = Cached {
                full: count > 0,
                ..Cached::default()
            };
            let mut values = Vec::new();
            // Soft edges already copied: volume index to index in `cached.soft`.
            let mut copied: HashMap<u32, u32> = HashMap::new();
            for i in 0..count {
                let piece = self.pieces[i];
                values.clear();
                let dark = piece.shadowed & bit != 0;
                let soft = !dark && self.soft_values(&piece, &mut values) & bit != 0;
                cached.full &= dark;
                if !dark && !soft {
                    continue;
                }
                // The soft edges it is in, kept with it: a piece clipped on screen gets how
                // much of the light reaches its new corners worked out there, as when carved.
                let first_soft = cached.piece_soft.len() as u32;
                if soft {
                    let volumes = &self.volume_lists
                        [piece.first_volume as usize..(piece.first_volume + piece.volume_count) as usize];
                    for &v in volumes {
                        let volume = &self.volumes[v as usize];
                        if volume.slot != slot {
                            continue;
                        }
                        let index = *copied.entry(v).or_insert_with(|| {
                            let start = cached.wedges.len() as u32;
                            cached.wedges.extend_from_slice(
                                &self.wedges[volume.wedges.start as usize..volume.wedges.end as usize],
                            );
                            cached.soft.push((start..cached.wedges.len() as u32, volume.window));
                            cached.soft.len() as u32 - 1
                        });
                        cached.piece_soft.push(index);
                    }
                }
                let piece_soft = first_soft..cached.piece_soft.len() as u32;
                let start = cached.vertices.len() as u32;
                let piece_records = self.records(&piece);
                let piece_edges = self.edges(&piece);
                let corners: Vec<Vec3> =
                    piece_records.chunks_exact(stride).map(|r| Vec3::new(r[3], r[4], r[5])).collect();
                let center = corners.iter().copied().sum::<Vec3>() / corners.len() as f32;
                for (k, (&world, &edge)) in corners.iter().zip(piece_edges).enumerate() {
                    let edge = match edge {
                        PieceEdge::Clip(Edge::Input(e)) => CachedEdge::Polygon(e),
                        PieceEdge::Line(a, b) => (0..n)
                            .find(|&e| {
                                let (p, q) = (polygon[e], polygon[(e + 1) % n]);
                                (a, b) == (p, q) || (a, b) == (q, p)
                            })
                            .map_or(CachedEdge::Line(a, b), |e| CachedEdge::Polygon(e as u16)),
                        PieceEdge::Clip(Edge::Plane(_)) => unreachable!("no clip planes here"),
                    };
                    cached.vertices.push(CachedVertex { world, edge });
                    if soft {
                        // Corners along the edge where its values don't go evenly from end
                        // to end: a cut that runs close along a soft edge's outer or inner
                        // plane from near its line (where the value changes fast) crosses
                        // all of it in a short way, and the rest of the way is flat.
                        let next = corners[(k + 1) % corners.len()];
                        let value = |p: Vec3, along: Vec3| cached.light(&piece_soft, p, along, center);
                        let mut added = Vec::new();
                        refine(&value, (world, value(world, next)), (next, value(next, world)), REFINE_DEPTH, &mut added);
                        cached.vertices.extend(added.into_iter().map(|world| CachedVertex { world, edge }));
                    }
                }
                cached.pieces.push((start..cached.vertices.len() as u32, piece_soft));
            }
            self.cache.insert((slot, key), cached);
        }
        bits
    }

    /// A static polygon's cached shadow from the static light in shadow slot `slot` (see
    /// [`Carver::cache`]).
    pub fn cached(&self, slot: u8, key: (u8, u32, u32)) -> Option<&Cached> {
        self.cache.get(&(slot, key))
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
        // Its bounding sphere, for lights' range and cones.
        let (lo, hi) = records.chunks_exact(stride).fold(
            (Vec3::INFINITY, Vec3::NEG_INFINITY),
            |(lo, hi), r| {
                let p = Vec3::new(r[3], r[4], r[5]);
                (lo.min(p), hi.max(p))
            },
        );
        let (center, radius) = ((lo + hi) * 0.5, (hi - lo).length() * 0.5);
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
            beamed: 0,
            first_volume: 0,
            volume_count: 0,
        });
        for c in 0..self.casters.len() {
            let (bit, source, range, light) = {
                let c = &self.casters[c];
                (c.bit, c.source, c.range, c.light)
            };
            // Which of its parts: static ones (windows too), moving ones, or all.
            let (statics, moving) = match receiver.parts {
                Parts::All => (true, true),
                Parts::Static(mask) if mask & bit != 0 => (true, false),
                Parts::Static(_) => continue,
                Parts::Cached(mask) => (mask & bit == 0, true),
                Parts::Beams if self.casters[c].beam.is_some() => (false, true),
                Parts::Beams => continue,
            };
            // Facing away from the light, or out of its reach (all of it beyond its range, or
            // outside a spot light's cone): it gets none of it anyway.
            let out_of_reach = match source {
                Source::Point { at, .. } => {
                    normal.dot(at - point) >= range
                        || center.distance(at) - radius >= range
                        || !light.cone_reaches(center, radius)
                }
                Source::Distant { .. } => false,
            };
            if !source.faces(normal, point) || out_of_reach {
                continue;
            }
            // A beam: outside the pyramid around its cone is dark, and inside it the cone
            // fades the light (see `Light::beam`), unless all of it is within the inner cone.
            if moving && receiver.beams && let Some(beam) = self.casters[c].beam.clone() {
                let within = records.chunks_exact(stride).all(|r| {
                    let ray = beam.frame.ray(Vec3::new(r[3], r[4], r[5]));
                    ray.x >= light.cos_inner * ray.length()
                });
                if !within {
                    std::mem::swap(&mut self.work, &mut self.pieces);
                    self.pieces.clear();
                    for i in 0..self.work.len() {
                        self.rest.clear();
                        let inside = self.split_region(self.work[i], beam.planes.clone(), lined);
                        for mut outside in self.rest.drain(..) {
                            outside.shadowed |= bit;
                            self.pieces.push(outside);
                        }
                        if let Some(mut inside) = inside {
                            inside.beamed |= bit;
                            self.pieces.push(inside);
                        }
                    }
                }
            }
            if receiver.parts == Parts::Beams || !self.casters[c].shadows {
                continue;
            }
            // Portals: unless it is in the light's own sector, only what some window into
            // its sectors reaches is lit (softly at its edges, from a light with a size).
            // What is dark already stays so.
            if statics && !self.casters[c].whole.is_some_and(|w| sectors.contains(&w)) {
                std::mem::swap(&mut self.work, &mut self.pieces);
                self.pieces.clear();
                self.work.retain(|piece| {
                    let dark = piece.shadowed & bit != 0;
                    if dark {
                        self.pieces.push(*piece);
                    }
                    !dark
                });
                for w in self.casters[c].windows.clone() {
                    let (sector, v) = self.windows[w as usize];
                    if !sectors.contains(&sector) {
                        continue;
                    }
                    let planes = self.volumes[v as usize].planes.clone();
                    // What no window has reached yet waits for the next.
                    let mut next = Vec::new();
                    for piece in std::mem::take(&mut self.work) {
                        self.rest.clear();
                        let inside = self.split_region(piece, planes.clone(), lined);
                        next.append(&mut self.rest);
                        if let Some(inside) = inside {
                            self.soft_split(inside, v, bit, lined);
                        }
                    }
                    self.work = next;
                }
                for piece in &mut self.work {
                    piece.shadowed |= bit;
                }
                self.pieces.append(&mut self.work);
            }
            // Occluders: each lit piece is split by each shadow volume.
            for v in self.casters[c].volumes.clone() {
                let volume = &self.volumes[v as usize];
                let (planes, owner) = (volume.planes.clone(), volume.owner);
                // An occluder's shape stands in for its model, so it doesn't shadow it.
                if owner.is_some() && owner == receiver.entity {
                    continue;
                }
                if !(if volume.is_static { statics } else { moving }) {
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
                    if let Some(inside) = inside {
                        self.soft_split(inside, v, bit, lined);
                    }
                }
            }
        }
        &self.pieces
    }

    /// Carves a piece inside volume `v`'s region: with no soft edges, all of it is in the
    /// occluder's full shadow (marked with `bit`) or lit by the window; otherwise it is
    /// carved sector by sector, each split by its wedge's core plane, the core carved out
    /// of what is past it, and the rest of it in the soft edge. Pieces go to `self.pieces`.
    fn soft_split(&mut self, piece: Piece, v: u32, bit: u32, lined: usize) {
        let volume = &self.volumes[v as usize];
        let (core, sectors, window) = (volume.core.clone(), volume.sectors.clone(), volume.window);
        if core.is_empty() {
            let mut piece = piece;
            if !window {
                piece.shadowed |= bit;
            }
            self.pieces.push(piece);
            return;
        }
        let saved = std::mem::take(&mut self.rest);
        let mut work = vec![piece];
        for k in 0..sectors.len() / 2 {
            let planes = sectors.start + 2 * k as u32..sectors.start + 2 * k as u32 + 2;
            let mut next = Vec::new();
            for part in work {
                self.rest.clear();
                let held = self.split_region(part, planes.clone(), lined);
                next.append(&mut self.rest);
                if let Some(held) = held {
                    let core_k = self.planes[(core.start + k as u32) as usize];
                    let (past, before) = self.split(held, core_k, lined);
                    if let Some(before) = before {
                        let before = self.with_volume(before, v);
                        self.pieces.push(before);
                    }
                    if let Some(past) = past {
                        self.soft_core(past, core.clone(), v, bit, window, lined);
                    }
                }
            }
            work = next;
        }
        for part in work {
            self.soft_core(part, core.clone(), v, bit, window, lined);
        }
        self.rest = saved;
    }

    /// Carves volume `v`'s core (within its `core` planes) out of `piece`: an occluder's
    /// full shadow (marked with `bit`), or the part a window lets all of the light through;
    /// the rest of `piece` is in the soft edge.
    fn soft_core(
        &mut self,
        piece: Piece,
        core: Range<u32>,
        v: u32,
        bit: u32,
        window: bool,
        lined: usize,
    ) {
        let saved = std::mem::take(&mut self.rest);
        if let Some(mut inside) = self.split_region(piece, core, lined) {
            if !window {
                inside.shadowed |= bit;
            }
            self.pieces.push(inside);
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
    /// vertices: appends, vertex by vertex, one pair of values per such light (in shadow
    /// slot order), and returns their shadow slots as bits. Interpolated across the piece,
    /// the values are exact on its edges: 1 where a shadow's soft edge starts, 0 where its
    /// full shadow does.
    ///
    /// A pair is the value along the edge arriving at the vertex, and along the one leaving
    /// it (see [`reaching`]). They are the same but at a corner on a soft edge's line, where
    /// an occluder's edge touches the surface: there the vertex is drawn as two, one ending
    /// the edge arriving and one starting the edge leaving, each with its own value.
    ///
    /// An occluder covers the product of what its wedges cover (near its corners, two at
    /// once), and a window lets through the product of what its wedges leave uncovered.
    /// Occluders' parts add up (capped at all of the light), which is exact for occluders
    /// side by side as seen from the light (stacked crates cover the two halves of it at
    /// their seam) and too dark where one is behind another; windows' parts multiply.
    pub fn soft_values(&self, piece: &Piece, out: &mut Vec<(f32, f32)>) -> u32 {
        let volumes = &self.volume_lists
            [piece.first_volume as usize..(piece.first_volume + piece.volume_count) as usize];
        let slots = volumes
            .iter()
            .fold(0u32, |bits, &v| bits | 1 << self.volumes[v as usize].slot);
        if slots == 0 {
            return 0;
        }
        let points: Vec<Vec3> = self
            .records(piece)
            .chunks_exact(self.stride)
            .map(|r| Vec3::new(r[3], r[4], r[5]))
            .collect();
        let n = points.len();
        let center = points.iter().sum::<Vec3>() / n as f32;
        for (i, &p) in points.iter().enumerate() {
            let (prev, next) = (points[(i + n - 1) % n], points[(i + 1) % n]);
            let mut bits = slots;
            while bits != 0 {
                let slot = bits.trailing_zeros() as u8;
                bits &= bits - 1;
                let soft = || {
                    volumes
                        .iter()
                        .map(|&v| &self.volumes[v as usize])
                        .filter(move |volume| volume.slot == slot)
                        .map(|volume| {
                            let w = volume.wedges.start as usize..volume.wedges.end as usize;
                            (&self.wedges[w], volume.window)
                        })
                };
                out.push((
                    reaching(soft(), (p, prev, center)),
                    reaching(soft(), (p, next, center)),
                ));
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
            beamed: piece.beamed,
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
            e @ PieceEdge::Clip(Edge::Input(_)) if self.keep_polygon_edges => e,
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
            beamed: 0,
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

/// How much of a light reaches `p`, a corner of a piece centered on `center`, along the
/// piece's edge from `p` to `along`, past the soft edges of the windows and occluders it is
/// in (their wedges, and whether each is a window). What windows let through multiplies;
/// what occluders cover adds up (capped at all of it); see [`Carver::soft_values`].
///
/// Only a wedge whose line `p` is on is taken along the edge (see [`Wedge::covers`]): the
/// others have their value at `p`, whichever edge it is on.
fn reaching<'a>(
    soft: impl Iterator<Item = (&'a [Wedge], bool)>,
    (p, along, center): (Vec3, Vec3, Vec3),
) -> f32 {
    let (mut through, mut covered) = (1.0, 0.0);
    for (wedges, window) in soft {
        let covers = wedges.iter().map(|w| w.covers(p, along, center));
        if window {
            through *= covers.map(|c| 1.0 - c).product::<f32>();
        } else {
            covered += covers.product::<f32>();
        }
    }
    through * (1.0 - covered.min(1.0))
}

/// Appends the points to add along the edge from `a` to `b` (each with its value) where
/// the value `value(p, toward)` halfway along it is more than `REFINE_OFF` from halfway
/// between theirs, in order: halving it, and each half, up to `depth` times.
fn refine(value: &impl Fn(Vec3, Vec3) -> f32, a: (Vec3, f32), b: (Vec3, f32), depth: u32, out: &mut Vec<Vec3>) {
    if depth == 0 {
        return;
    }
    let middle = (a.0 + b.0) * 0.5;
    let m = value(middle, b.0);
    if (m - (a.1 + b.1) * 0.5).abs() <= REFINE_OFF {
        return;
    }
    refine(value, a, (middle, m), depth - 1, out);
    out.push(middle);
    refine(value, (middle, m), b, depth - 1, out);
}

/// A point on a portal (its first corner).
fn portal_point(geometry: &Mesh, portal: &moose_assets::Portal) -> Vec3 {
    geometry.positions[portal.positions[0] as usize]
}

/// Whether a mesh is convex: every position on or behind every polygon's plane (within a
/// small tolerance for its size).
/// Whether polygons `polygons` of `mesh` bound a convex shape: every one of their points
/// on or behind each of their planes.
fn is_convex(mesh: &Mesh, polygons: &[usize]) -> bool {
    let tolerance = 1e-4 * (mesh.bounds.max - mesh.bounds.min).length().max(1e-3);
    polygons.iter().all(|&k| {
        let plane = mesh.polygons[k].plane;
        polygons
            .iter()
            .flat_map(|&j| mesh.polygon_points(&mesh.polygons[j]))
            .all(|p| plane.distance(p) <= tolerance)
    })
}

/// The polygons a model casts shadows with, in parts (each convex, or cast face by face):
/// its shadow proxies, one part per bone (by their first corner's bone), if it has any;
/// otherwise all its polygons as one.
fn occluder_parts(mesh: &Mesh) -> Vec<Vec<usize>> {
    let proxies: Vec<usize> = (0..mesh.polygons.len()).filter(|&k| mesh.polygons[k].flags.proxy()).collect();
    if proxies.is_empty() {
        return vec![(0..mesh.polygons.len()).collect()];
    }
    let Some(skin) = &mesh.skin else {
        return vec![proxies];
    };
    let mut parts: Vec<(u16, Vec<usize>)> = Vec::new();
    for k in proxies {
        let first = mesh.vertex_positions[mesh.polygons[k].first_vertex as usize];
        let bone = skin.position_bones[first as usize];
        match parts.iter_mut().find(|(b, _)| *b == bone) {
            Some((_, part)) => part.push(k),
            None => parts.push((bone, vec![k])),
        }
    }
    parts.into_iter().map(|(_, part)| part).collect()
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
            is_static: false,
            source: Source::Point { at: light, radius: 0.0 },
            range: 100.0,
            light: Light::point(0, light, Vec3::ONE, 100.0),
            whole: Some(0),
            windows: 0..0,
            volumes: 0..c.volumes.len() as u32,
            shadows: true,
            beam: None,
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
        Receiver { sectors: &[0], entity: None, normal, point, parts: Parts::All, beams: true }
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
            is_static: false,
            source: Source::Point { at: center, radius: 0.0 },
            range: 100.0,
            light: Light::point(0, center, Vec3::ONE, 100.0),
            whole: Some(0),
            windows: 0..0,
            volumes: 0..1,
            shadows: true,
            beam: None,
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
            for (r, &(v, _)) in c.records(&p).chunks(stride).zip(&values) {
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

    #[test]
    fn a_light_level_with_a_face_still_softens_the_edge_past_it() {
        // A crate turned 15 degrees, its top 0.13 m below a light of radius 0.2 some 4 m
        // away, which shines past it at a wall behind: the light straddles the plane of the
        // crate's top. Its top edges' soft edges stop at that plane rather than cutting
        // through the crate, and the shadow on the wall fades at its top.
        use glam::Quat;
        let at_light = Vec3::new(-1.22, 2.13, 6.0);
        let mut light = Light::point(0, at_light, Vec3::ONE, 100.0);
        light.radius = 0.2;
        let turn = Quat::from_rotation_y(15f32.to_radians());
        let corners: Vec<Vec3> =
            cube(Vec3::ZERO, 0.5).concat().iter().map(|&p| Vec3::new(-2.5, 1.5, 2.0) + turn * p).collect();
        let mut c = Carver::default();
        for face in corners.chunks(4) {
            let start = c.points.len();
            c.points.extend_from_slice(face);
            c.faces.push(start..c.points.len());
        }
        c.add_volume(&light, 0, None);
        assert_eq!(c.wedges.len(), 6);
        for w in &c.wedges {
            let cut = corners.iter().map(|&p| w.outer.distance(p)).fold(f32::INFINITY, f32::min);
            assert!(cut > -1e-4, "an outer plane passes {cut} m into the crate");
        }
        c.casters.push(Caster {
            bit: 1,
            is_static: false,
            source: Source::Point { at: at_light, radius: 0.0 },
            range: 100.0,
            light: Light::point(0, at_light, Vec3::ONE, 100.0),
            whole: Some(0),
            windows: 0..0,
            volumes: 0..1,
            shadows: true,
            beam: None,
        });
        // The wall at z = 0, facing the light; clip x and y are the wall's.
        let wall = [
            Vec3::new(-4.0, 0.0, 0.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(-1.0, 4.0, 0.0),
            Vec3::new(-4.0, 4.0, 0.0),
        ];
        let records: Vec<f32> = wall
            .iter()
            .enumerate()
            .flat_map(|(i, p)| {
                let mut r = vec![p.x, p.y, 10.0, p.x, p.y, p.z];
                r.extend((0..4).map(|k| if k == i { 1.0 } else { 0.0 }));
                r
            })
            .collect();
        let stride = RECORD_FLOATS + 4;
        let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
        let n = c.carve(&records, &edges, stride, 0, &at(Vec3::Z, wall[0])).len();
        // Up the wall through the middle of the crate's shadow: full shadow, then its soft
        // edge (some centimeters of it), then light.
        // Each piece's outline, and what it is: full shadow, soft, or lit.
        let pieces: Vec<(Vec<(f32, f32)>, char)> = (0..n)
            .map(|i| {
                let p = c.piece(i);
                let outline = c.records(&p).chunks(stride).map(|r| (r[3], r[4])).collect();
                let kind = if p.shadowed != 0 { 'F' } else if p.volume_count > 0 { 'S' } else { 'L' };
                (outline, kind)
            })
            .collect();
        let inside = |outline: &[(f32, f32)], (x, y): (f32, f32)| {
            let signs = (0..outline.len()).map(|i| {
                let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
                (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)
            });
            let signs: Vec<f32> = signs.filter(|d| d.abs() > 1e-7).collect();
            signs.iter().all(|&d| d > 0.0) || signs.iter().all(|&d| d < 0.0)
        };
        let kinds: Vec<char> = (0..160)
            .map(|k| {
                let q = (-3.0, 1.5 + k as f32 * 0.004);
                pieces.iter().find(|(o, _)| inside(o, q)).map_or('?', |&(_, kind)| kind)
            })
            .collect();
        let line: String = kinds.iter().collect();
        let (full, lit) = (line.rfind('F').unwrap(), line.find('L').unwrap());
        assert!(full < lit && line[full + 1..lit].chars().all(|k| k == 'S'), "{line}");
        assert!(lit - full > 8, "a soft edge over 3 cm: {line}");
    }

    #[test]
    fn a_corner_touching_the_surface_has_a_value_along_each_edge() {
        // A cube resting on the floor, lit from the side by a light of radius 0.2: its
        // vertical edges' soft edges start at its bottom corners, where how much of the
        // light reaches has no one value. Along each edge from there it is the same all the
        // way: what the vertex at the edge's other end has.
        let center = Vec3::new(2.5, 2.0, 0.7);
        let mut light = Light::point(0, center, Vec3::ONE, 100.0);
        light.radius = 0.2;
        let mut c = Carver::default();
        for face in cube(Vec3::new(0.0, 0.5, 0.0), 0.5) {
            let start = c.points.len();
            c.points.extend_from_slice(&face);
            c.faces.push(start..c.points.len());
        }
        c.add_volume(&light, 0, None);
        c.casters.push(Caster {
            bit: 1,
            is_static: false,
            source: Source::Point { at: center, radius: 0.0 },
            range: 100.0,
            light: Light::point(0, center, Vec3::ONE, 100.0),
            whole: Some(0),
            windows: 0..0,
            volumes: 0..1,
            shadows: true,
            beam: None,
        });
        let floor = [
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(-3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, -3.0),
        ];
        let stride = RECORD_FLOATS + 4;
        let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
        let n = c.carve(&records(&floor), &edges, stride, 0, &at(Vec3::Y, floor[0])).len();
        let mut corners = 0;
        for i in 0..n {
            let p = c.piece(i);
            let mut values = Vec::new();
            if p.shadowed != 0 || c.soft_values(&p, &mut values) == 0 {
                continue;
            }
            let points: Vec<Vec3> =
                c.records(&p).chunks(stride).map(|r| Vec3::new(r[3], r[4], r[5])).collect();
            let m = points.len();
            for (k, q) in points.iter().enumerate() {
                // (Pieces stop just short of the cube: see `CAP_BIAS`.)
                if (q.x.abs() - 0.5).abs() > 0.002 || (q.z.abs() - 0.5).abs() > 0.002 {
                    continue;
                }
                let (arriving, leaving) = values[k];
                let (before, after) = (values[(k + m - 1) % m].1, values[(k + 1) % m].0);
                assert!((arriving - before).abs() < 0.02, "{arriving} after {before} at {q}");
                assert!((leaving - after).abs() < 0.02, "{leaving} before {after} at {q}");
                if (arriving - leaving).abs() > 0.5 {
                    corners += 1;
                }
            }
        }
        assert!(corners > 0, "soft edges start at the corners");
    }

    #[test]
    fn a_beam_darkens_all_outside_its_pyramid() {
        // A beam 3 m above the floor, pointing down, fading from 10 to 20 degrees off its
        // axis: its pyramid meets the floor in a square 2 * 3 tan 20 = 2.18 m across.
        let center = Vec3::new(0.0, 3.0, 0.0);
        let mut light = Light::spot(0, center, Vec3::ONE, 100.0, Vec3::NEG_Y, 10.0, 20.0);
        light.beam = true;
        let mut c = Carver::default();
        let beam = c.add_beam(&light);
        assert_eq!(beam.planes.len(), 4);
        c.casters.push(Caster {
            bit: 1,
            is_static: false,
            source: Source::of(&light),
            range: 100.0,
            light,
            whole: Some(0),
            windows: 0..0,
            volumes: 0..0,
            shadows: false,
            beam: Some(beam),
        });
        let stride = RECORD_FLOATS + 4;
        let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
        let floor = [
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(-3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, -3.0),
        ];
        let n = c.carve(&records(&floor), &edges, stride, 0, &at(Vec3::Y, floor[0])).len();
        let (mut inside, mut dark) = (0.0, 0.0);
        for i in 0..n {
            let p = c.piece(i);
            let a = area(c.records(&p), stride);
            assert!((p.beamed == 1) != (p.shadowed == 1), "in the beam or dark: {p:?}");
            if p.beamed == 1 {
                inside += a;
            } else {
                dark += a;
            }
        }
        let side = 2.0 * 3.0 * 20f32.to_radians().tan();
        assert!((inside - side * side).abs() < 1e-3, "inside {inside}");
        assert!((inside + dark - 36.0).abs() < 1e-3);

        // All within the inner cone: whole, and not in the beam.
        let small = floor.map(|p| p * 0.05);
        let n = c.carve(&records(&small), &edges, stride, 0, &at(Vec3::Y, small[0])).len();
        assert_eq!(n, 1);
        let p = c.piece(0);
        assert_eq!((p.beamed, p.shadowed), (0, 0));
    }

    /// Carves the floor (y = 0, 6 m square) for `c`'s caster and returns the light
    /// reaching each vertex of every piece in its shadow, with the vertex: 0 in full shadow.
    fn floor_light(c: &mut Carver) -> Vec<(Vec3, f32)> {
        let floor = [
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(-3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, -3.0),
        ];
        let stride = RECORD_FLOATS + 4;
        let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
        let n = c.carve(&records(&floor), &edges, stride, 0, &at(Vec3::Y, floor[0])).len();
        let mut out = Vec::new();
        for i in 0..n {
            let p = c.piece(i);
            let mut values = Vec::new();
            let soft = c.soft_values(&p, &mut values);
            for (k, r) in c.records(&p).chunks(stride).enumerate() {
                let v = Vec3::new(r[3], r[4], r[5]);
                if p.shadowed != 0 {
                    out.push((v, 0.0));
                } else if soft != 0 {
                    out.push((v, values[k].0));
                }
            }
        }
        out
    }

    #[test]
    fn occluders_side_by_side_leave_no_light_between_them() {
        // Two squares meeting along x = 0, 1 m below a light of radius 0.2, 3 m above the
        // floor: across their seam's soft edges (|x| < 0.4 on the floor) what one leaves
        // uncovered the other covers (smoothstep(t) + smoothstep(1 - t) = 1), so none gets
        // past.
        let center = Vec3::new(0.0, 3.0, 0.0);
        let mut light = Light::point(0, center, Vec3::ONE, 100.0);
        light.radius = 0.2;
        let mut c = Carver::default();
        for x in [-1.0f32, 0.0] {
            c.points.clear();
            c.faces.clear();
            c.points.extend_from_slice(&[
                Vec3::new(x, 2.0, -0.5),
                Vec3::new(x, 2.0, 0.5),
                Vec3::new(x + 1.0, 2.0, 0.5),
                Vec3::new(x + 1.0, 2.0, -0.5),
            ]);
            c.faces.push(0..4);
            c.add_volume(&light, 0, None);
        }
        c.casters.push(Caster {
            bit: 1,
            is_static: false,
            source: Source::Point { at: center, radius: 0.0 },
            range: 100.0,
            light: Light::point(0, center, Vec3::ONE, 100.0),
            whole: Some(0),
            windows: 0..0,
            volumes: 0..2,
            shadows: true,
            beam: None,
        });
        let values = floor_light(&mut c);
        // Around the seam's shadow, well inside both squares' shadow along z.
        let seam: Vec<f32> = values
            .iter()
            .filter(|(v, _)| v.x.abs() < 0.45 && v.z.abs() < 1.0)
            .map(|&(_, l)| l)
            .collect();
        assert!(!seam.is_empty());
        assert!(seam.iter().all(|&l| l < 1e-3), "{seam:?}");
    }

    #[test]
    fn a_window_lets_light_through_with_soft_edges() {
        // A light of radius 0.2 in sector 0, 3 m above the floor of sector 1, which it
        // lights through a 1 m square opening 1 m below it: fully in the middle of the
        // patch below, softly at its edges.
        let center = Vec3::new(0.0, 3.0, 0.0);
        let mut light = Light::point(0, center, Vec3::ONE, 100.0);
        light.radius = 0.2;
        let opening: Vec<(Vec3, Vec3)> = {
            let p = [
                Vec3::new(-0.5, 2.0, -0.5),
                Vec3::new(0.5, 2.0, -0.5),
                Vec3::new(0.5, 2.0, 0.5),
                Vec3::new(-0.5, 2.0, 0.5),
            ];
            (0..4).map(|i| (p[i], p[(i + 1) % 4])).collect()
        };
        let mut c = Carver::default();
        c.add_outline(&light, 0, None, &opening, true, Vec3::new(0.0, 2.0, 0.0), &[], true, &[]);
        c.windows.push((1, 0));
        c.casters.push(Caster {
            bit: 1,
            is_static: false,
            source: Source::Point { at: center, radius: 0.0 },
            range: 100.0,
            light: Light::point(0, center, Vec3::ONE, 100.0),
            whole: Some(0),
            windows: 0..1,
            volumes: 0..0,
            shadows: true,
            beam: None,
        });
        let floor = [
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(-3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, -3.0),
        ];
        let stride = RECORD_FLOATS + 4;
        let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
        let receiver = Receiver {
            sectors: &[1],
            entity: None,
            normal: Vec3::Y,
            point: floor[0],
            parts: Parts::All,
            beams: true,
        };
        let n = c.carve(&records(&floor), &edges, stride, 0, &receiver).len();
        // The opening is 1 m below the light and the floor 3 m. Planes through an edge of it
        // (0.5 m out) tangent to the light's sphere leave the vertical at 26.57° ± 10.30°
        // (the edge seen from the light's center, and the sphere from the edge's line), so
        // they reach the floor 2 m further down at 0.5 + 2 tan 16.26° = 1.083 and
        // 0.5 + 2 tan 36.87° = 2.0 from the middle: all of the light gets through within
        // the one, some within the other.
        let (mut lit, mut soft, mut dark) = (0.0, 0.0, 0.0);
        for i in 0..n {
            let p = c.piece(i);
            let a = area(c.records(&p), stride);
            let mut values = Vec::new();
            if p.shadowed != 0 {
                dark += a;
            } else if c.soft_values(&p, &mut values) != 0 {
                soft += a;
                assert!(values.iter().all(|(v, _)| (0.0..=1.0).contains(v)));
            } else {
                lit += a;
            }
        }
        assert!((lit - 2.1667 * 2.1667).abs() < 0.01, "fully lit {lit}");
        assert!((lit + soft - 16.0).abs() < 0.01, "lit at all {}", lit + soft);
        assert!((lit + soft + dark - 36.0).abs() < 1e-2);
    }

    #[test]
    fn the_sun_casts_parallel_shadows_softened_by_its_angle() {
        // Sunlight straight down, 2 degrees across, over a 1 m square 2 m above the floor:
        // its shadow is the square itself, its soft edge 2 m * tan 1 degree = 3.5 cm wide
        // either side of the square's edges, wherever the square is.
        let sun = Light::directional(Vec3::NEG_Y, Vec3::ONE, 2.0);
        let square = [
            Vec3::new(1.0, 2.0, -0.5),
            Vec3::new(1.0, 2.0, 0.5),
            Vec3::new(2.0, 2.0, 0.5),
            Vec3::new(2.0, 2.0, -0.5),
        ];
        let mut c = Carver::default();
        c.points.extend_from_slice(&square);
        c.faces.push(0..4);
        c.add_volume(&sun, 0, None);
        c.casters.push(Caster {
            bit: 1,
            is_static: false,
            source: Source::of(&sun),
            range: sun.range,
            light: sun,
            whole: None,
            windows: 0..0,
            volumes: 0..1,
            shadows: true,
            beam: None,
        });
        assert_eq!(c.wedges.len(), 4);
        let floor = [
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(-3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, -3.0),
        ];
        let stride = RECORD_FLOATS + 4;
        let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
        // A directional light reaches only what it shines in to: a sky window over the floor.
        // (Wider than the floor, so its own soft edges miss it.)
        let sky = [
            Vec3::new(-4.0, 4.0, -4.0),
            Vec3::new(4.0, 4.0, -4.0),
            Vec3::new(4.0, 4.0, 4.0),
            Vec3::new(-4.0, 4.0, 4.0),
        ];
        c.points.clear();
        c.points.extend_from_slice(&sky);
        let (_, v) = c.add_window(&sun, 0);
        c.windows.push((0, v));
        c.casters[0].windows = 0..1;
        let n = c.carve(&records(&floor), &edges, stride, 0, &at(Vec3::Y, floor[0])).len();
        let (mut lit, mut soft, mut dark) = (0.0, 0.0, 0.0);
        for i in 0..n {
            let p = c.piece(i);
            let a = area(c.records(&p), stride);
            let mut values = Vec::new();
            if p.shadowed != 0 {
                dark += a;
            } else if c.soft_values(&p, &mut values) != 0 {
                soft += a;
            } else {
                lit += a;
            }
        }
        let e = 2.0 * 1f32.to_radians().tan();
        assert!((dark - (1.0 - 2.0 * e).powi(2)).abs() < 1e-3, "core {dark}");
        assert!((dark + soft - (1.0 + 2.0 * e).powi(2)).abs() < 1e-3, "shadow {}", dark + soft);
        assert!((lit + soft + dark - 36.0).abs() < 1e-2);
    }

    #[test]
    fn a_cached_shadow_is_the_carved_one() {
        // A static light of radius 0.2 over a floor, a static square occluder between: the
        // cached pieces cover what carving covers, fully and softly, with the same light
        // at their corners.
        let center = Vec3::new(0.0, 3.0, 0.0);
        let mut light = Light::point(0, center, Vec3::ONE, 100.0);
        light.radius = 0.2;
        light.is_static = true;
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
            is_static: true,
            source: Source::of(&light),
            range: 100.0,
            light,
            whole: Some(0),
            windows: 0..0,
            volumes: 0..1,
            shadows: true,
            beam: None,
        });
        let floor = [
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(-3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, -3.0),
        ];
        let receiver = at(Vec3::Y, floor[0]);
        assert_eq!(c.cache((0, 7, 0), &floor, &receiver), 1);
        // Carved directly.
        let stride = RECORD_FLOATS + 4;
        let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
        let n = c.carve(&records(&floor), &edges, stride, 0, &receiver).len();
        let (mut dark, mut soft) = (0.0, 0.0);
        for i in 0..n {
            let p = c.piece(i);
            let mut values = Vec::new();
            if p.shadowed != 0 {
                dark += area(c.records(&p), stride);
            } else if c.soft_values(&p, &mut values) != 0 {
                soft += area(c.records(&p), stride);
            }
        }
        // Cached.
        let cached = c.cached(0, (0, 7, 0)).unwrap();
        assert!(!cached.full);
        let (mut cached_dark, mut cached_soft) = (0.0, 0.0);
        for (range, soft_edges) in &cached.pieces {
            let v = &cached.vertices[range.start as usize..range.end as usize];
            let a = (1..v.len() - 1)
                .map(|i| (v[i].world - v[0].world).cross(v[i + 1].world - v[0].world))
                .sum::<Vec3>()
                .length()
                / 2.0;
            if soft_edges.is_empty() {
                cached_dark += a;
            } else {
                cached_soft += a;
                for x in v {
                    let light = cached.light(soft_edges, x.world, x.world, x.world);
                    assert!((0.0..=1.0).contains(&light));
                }
            }
        }
        assert!((dark - cached_dark).abs() < 1e-3, "{dark} {cached_dark}");
        assert!((soft - cached_soft).abs() < 1e-3, "{soft} {cached_soft}");
        // Once cached, it isn't carved again: a changed light drops it.
        assert_eq!(c.cache.len(), 1);
    }

    #[test]
    fn surfaces_out_of_reach_are_not_carved() {
        // A 1 m square occluder under a light; a floor square under it is carved. The same
        // square beyond the light's range isn't, nor one outside a spot light's cone, though
        // their planes are in range and face the light.
        let square = [
            Vec3::new(-0.5, 1.0, -0.5),
            Vec3::new(-0.5, 1.0, 0.5),
            Vec3::new(0.5, 1.0, 0.5),
            Vec3::new(0.5, 1.0, -0.5),
        ];
        let floor_at = |x: f32| {
            [
                Vec3::new(x - 1.0, 0.0, -1.0),
                Vec3::new(x - 1.0, 0.0, 1.0),
                Vec3::new(x + 1.0, 0.0, 1.0),
                Vec3::new(x + 1.0, 0.0, -1.0),
            ]
        };
        let carved = |light: Light, x: f32| {
            let mut c = Carver::default();
            c.points.extend_from_slice(&square);
            c.faces.push(0..4);
            c.add_volume(&light, 0, None);
            c.casters.push(Caster {
                bit: 1,
                is_static: false,
                source: Source::of(&light),
                range: light.range,
                light,
                whole: Some(0),
                windows: 0..0,
                volumes: 0..1,
                shadows: true,
                beam: None,
            });
            let floor = floor_at(x);
            let stride = RECORD_FLOATS + 4;
            let edges: Vec<Edge> = (0..4).map(Edge::Input).collect();
            let n = c.carve(&records(&floor), &edges, stride, 0, &at(Vec3::Y, floor[0])).len();
            n > 1
        };
        let point = Light::point(0, Vec3::new(0.0, 3.0, 0.0), Vec3::ONE, 10.0);
        assert!(carved(point, 0.0));
        // 20 m off to the side: its plane is 3 m from the light, but all of it is past 10 m.
        assert!(!carved(point, 20.0));
        // A spot light pointing straight down, 20 degrees at most: the floor under it is
        // in its cone; one 5 m off to the side isn't.
        let spot = Light::spot(0, Vec3::new(0.0, 3.0, 0.0), Vec3::ONE, 10.0, Vec3::NEG_Y, 10.0, 20.0);
        assert!(carved(spot, 0.0));
        assert!(!carved(spot, 5.0));
    }
}
