use std::ops::Range;

use glam::{Affine3A, Mat3A, Quat, Vec3, Vec3A};
use moose_assets::{
    Assets, EntityKind, Mesh, MeshId, Plane, Light, PolyFlags, Polygon, Portal,
};
use moose_scene::{View, World};

use crate::carve::{CachedEdge, Carver, Parts, PieceEdge, Receiver};
use crate::clip::{ClipPlane, Clipper, Edge};

/// Tolerance for treating the eye as lying exactly on a portal's plane, in meters. Camera
/// moves keep 1 mm clear of portals (`moose_scene::PORTAL_CLEARANCE`), so this only
/// catches cameras placed there directly.
const ON_PORTAL: f32 = 1e-5;
/// Limits on portal traversal, against pathological levels.
const MAX_PORTAL_DEPTH: u16 = 64;
const MAX_VISITS: usize = 4096;
/// Portal edges subtending less than this many radians (about 0.01 px at 1280 wide) seen
/// from the eye give no reliable plane; windows smaller than this in every direction are
/// treated as invisible.
const MIN_EDGE_ANGLE: f32 = 1e-5;
/// A window edge's line shorter than this many pixels is too short to walk a longer edge
/// along (see `open_window`): its endpoints' rounding, extended far past them, would put
/// the edge pixels off.
const MIN_LINE_LENGTH: f32 = 4.0;
/// Clipped vertices may land past the viewport edge by at most this many pixels before
/// snapping (float rounding only; checked in debug builds).
const MAX_OVERSHOOT: f32 = 1e-3;
/// Points this close to a mirror's plane, in meters, are its fixed points: reflected to
/// themselves exactly, so the mirror's own corners (and the edges walls share with it)
/// have bit-identical clip coordinates on both sides of the mirror.
const ON_MIRROR: f32 = 1e-5;
/// Most reflections [`ViewConfig::max_reflections`] allows.
pub const MAX_REFLECTIONS: u8 = 16;

/// A projected vertex: exact (unrounded) framebuffer position, y down, within the
/// viewport, and `w = 1 / depth`, larger is closer.
///
/// Positions are rounded only when a polygon's rows are turned into spans, using
/// [`pixel_edge`]:
///
/// - An edge covers rows `pixel_edge(top)..pixel_edge(bottom)` of its two endpoints, and
///   its x on a row is taken at the row's center ([`EdgeLine::x_at_row`], using its
///   carried line from `ViewGeometry::edge_lines` if it has one).
/// - Every polygon faces the camera, so on screen (y down) its edges heading down form its
///   left boundary and its edges heading up form its right one. A row covers pixels
///   `pixel_edge(left)..pixel_edge(right)`, and nothing if that is empty (an edge-on
///   sliver) or if only one side crosses the row.
///
/// Rounding once, at the end, never changes the order of two boundaries, and polygons
/// sharing a boundary compute the bit-identical number for it (the clipper puts shared
/// clip points at bit-identical positions), so neighbors meet with no gap and no overlap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenVertex {
    pub x: f32,
    pub y: f32,
    pub w: f32,
}

/// The pixel boundary a coordinate rounds to: pixel `i` (covering `i..i + 1`) belongs to a
/// span when its center `i + 0.5` lies in `[start, end)`, so a span covers pixels
/// `pixel_edge(start)..pixel_edge(end)`. The same rule applies to rows. Rounds to nearest,
/// with exact halves going down.
pub fn pixel_edge(v: f32) -> i32 {
    (v - 0.5).ceil() as i32
}

/// The line an edge is walked along, by its exact endpoints.
///
/// Normally an edge is walked between its own two vertices. An edge of a sector seen
/// through a portal that lies on one of the portal's edges (clipped in 3D to the plane
/// through that edge) is walked along the portal edge instead: the same line, but given by
/// the same endpoints as the wall on the other side of the portal (the jamb, the lintel),
/// which share that edge. Both sides then compute bit-identical x on every row. Edges on
/// the viewport border carry the border.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeLine {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl EdgeLine {
    pub fn between(a: (f32, f32), b: (f32, f32)) -> Self {
        Self {
            x0: a.0,
            y0: a.1,
            x1: b.0,
            y1: b.1,
        }
    }

    /// The line's exact x at the center of `row` (y = row + 0.5). Always computed from its
    /// top endpoint, whichever order the endpoints are given in, so every polygon walking
    /// this line gets the bit-identical result. Not defined for horizontal lines, which
    /// cover no rows.
    pub fn x_at_row(&self, row: i32) -> f32 {
        let ((tx, ty), (bx, by)) = if (self.y0, self.x0) <= (self.y1, self.x1) {
            ((self.x0, self.y0), (self.x1, self.y1))
        } else {
            ((self.x1, self.y1), (self.x0, self.y0))
        };
        tx + (bx - tx) * ((row as f32 + 0.5 - ty) / (by - ty))
    }
}

/// Where a polygon goes in the span module; the spec's `MeshKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolygonKind {
    /// Level geometry, clipped to its portal window. Never overlaps other world polygons.
    World,
    Prop,
    Actor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolygonSource {
    World { sector: u32, polygon: u32 },
    Entity { entity: u32, polygon: u32 },
    /// A piece of a terrain (the entity it is), in `sector`: `polygon` is in its carved
    /// mesh (see `moose_scene::Terrain`).
    Terrain { entity: u32, sector: u32, polygon: u32 },
}

/// A clipped, projected convex polygon, counter-clockwise from the front before projection.
///
/// Consecutive vertices can snap to the same pixel, so consumers must tolerate zero-length
/// edges (a horizontal or zero-length edge covers no rows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewPolygon {
    pub first_vertex: u32,
    pub vertex_count: u16,
    /// Start of this polygon's weights in `ViewGeometry::weights`: `source_vertices` per
    /// vertex. Each vertex is the weighted sum of the source polygon's vertices (in their
    /// mesh order), so any per-vertex value (a mesh attribute, a vertex shader's output)
    /// at a clipped vertex is the same weighted sum of its values at the source vertices:
    /// clipping is linear.
    pub first_weight: u32,
    pub source_vertices: u16,
    /// Source mesh, for its attribute names and layout.
    pub mesh: MeshId,
    /// Index in `ViewGeometry::objects`: where the source mesh is in the world.
    pub object: u32,
    pub kind: PolygonKind,
    pub source: PolygonSource,
    /// The source polygon's flags.
    pub flags: PolyFlags,
    /// The mirror this polygon is seen in (index in `ViewGeometry::mirrors`), or `None` if
    /// it is seen directly.
    pub mirror: Option<u32>,
    /// The shadow slots (`Light::shadow`) of the lights it is in the full shadow of: each is
    /// a bit. Polygons are carved into pieces wholly in or out of each such light's shadow,
    /// and of each soft edge of it.
    pub shadowed: u32,
    /// The shadow slots of the lights whose shadow covers part of it: for those, its shadow
    /// pieces (`shadow_count` of `ViewGeometry::shadow_pieces` from `first_shadow`) say how
    /// much of the light reaches each of its pixels.
    pub split: u32,
    pub first_shadow: u32,
    pub shadow_count: u16,
    /// For a reflective polygon, the mirror seen through it, if its reflection was drawn:
    /// then its reflection fills its outline behind it, and it must be drawn in the
    /// translucent pass. `None` for every other polygon, which is drawn opaque.
    pub reflection: Option<u32>,
}

impl ViewPolygon {
    pub fn vertices(&self) -> Range<usize> {
        self.first_vertex as usize..self.first_vertex as usize + self.vertex_count as usize
    }

    pub fn weights(&self) -> Range<usize> {
        let len = self.vertex_count as usize * self.source_vertices as usize;
        self.first_weight as usize..self.first_weight as usize + len
    }

    /// The source polygon's index in its mesh.
    pub fn source_polygon(&self) -> u32 {
        match self.source {
            PolygonSource::World { polygon, .. }
            | PolygonSource::Entity { polygon, .. }
            | PolygonSource::Terrain { polygon, .. } => polygon,
        }
    }
}

/// Where a mesh is in the world: model to world is scale, then rotation, then translation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Object {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: f32,
}

impl Object {
    /// Level geometry: already in world space.
    pub const IDENTITY: Object = Object {
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: 1.0,
    };

    pub fn transform(&self) -> Affine3A {
        Affine3A::from_scale_rotation_translation(
            Vec3::splat(self.scale),
            self.rotation,
            self.position,
        )
    }
}

/// One sector reached this frame through one window.
#[derive(Clone, Debug, PartialEq)]
pub struct SectorVisit {
    pub sector: u32,
    /// Portals crossed to get here; 0 for the camera's sector.
    pub depth: u16,
    /// True if the window reaches closer than the near plane, so geometry was near-clipped.
    pub near_clipped: bool,
    /// The window's planes through the eye; see `ViewGeometry::window_normals`.
    pub window: Range<u32>,
    /// The mirror the sector is seen in, or `None` if it is seen directly.
    pub mirror: Option<u32>,
}

/// A reflective polygon's plane seen this frame, directly or in another mirror. What is
/// seen in it is the level reflected across its plane, then across each mirror it is seen
/// in, out to the one seen directly. Reflective polygons on the same plane seen in the
/// same mirror share one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mirror {
    /// The reflective polygon's plane (in the level, unreflected), facing into its sector.
    pub plane: Plane,
    /// The mirror this one is seen in, or `None` if it is seen directly.
    pub parent: Option<u32>,
    /// Reflections between what is seen in this mirror and the level: 1 for a mirror seen
    /// directly.
    pub depth: u8,
    /// The eye for what is seen in this mirror: the camera reflected back through the
    /// mirrors, in level coordinates. Facing tests, and view vectors for shading surfaces
    /// seen in this mirror, use it.
    pub eye: Vec3,
}

/// View processing settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewConfig {
    /// How many reflections deep mirrors are followed: 0 draws reflective polygons as plain
    /// surfaces, 1 shows the level in them, 2 also shows mirrors seen in mirrors, and so
    /// on, up to [`MAX_REFLECTIONS`]. Reflective polygons at the deepest level are drawn
    /// plain.
    pub max_reflections: u8,
    /// Keep static lights' shadows on static polygons (carved once) instead of carving
    /// them every frame. Off only to compare.
    pub cache_shadows: bool,
    /// Carve shadows every frame where they aren't cached: dynamic lights', moving
    /// occluders', and those on moving surfaces. Off, only cached (baked) shadows are drawn:
    /// no carving per frame at all.
    pub dynamic_shadows: bool,
    /// The size of the light's source for shadows carved every frame, as a multiple of the
    /// light's size as given (0: hard): all of a dynamic light's, and moving occluders' of
    /// static lights. Cached (baked) shadows keep the light's size.
    pub dynamic_softness: f32,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            max_reflections: 1,
            cache_shadows: true,
            dynamic_shadows: true,
            dynamic_softness: 1.0,
        }
    }
}

/// Reflects a point across a plane. Points on the plane (within `ON_MIRROR`) stay exactly
/// where they are.
fn reflect(plane: &Plane, p: Vec3) -> Vec3 {
    let d = plane.distance(p);
    if d.abs() <= ON_MIRROR {
        p
    } else {
        p - 2.0 * d * plane.normal
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewStats {
    pub world_backfacing: u32,
    pub world_outside: u32,
    pub world_drawn: u32,
    /// Terrain pieces: drawn, facing away, outside their window, and hidden by a view
    /// blocker.
    pub terrain_drawn: u32,
    pub terrain_backfacing: u32,
    pub terrain_outside: u32,
    pub terrain_blocked: u32,
    /// Entities hidden by a view blocker.
    pub entities_blocked: u32,
    pub entities_culled: u32,
    pub entities_drawn: u32,
    pub entity_polygons_backfacing: u32,
    pub entity_polygons_outside: u32,
}

/// Everything one view sees in one frame: clipped screen-space polygons for the span
/// module. Reuse one per camera; buffers keep their capacity between frames.
#[derive(Default)]
pub struct ViewGeometry {
    pub vertices: Vec<ScreenVertex>,
    /// Per vertex, the line of the edge leaving it; see [`EdgeLine`].
    pub edge_lines: Vec<Option<EdgeLine>>,
    /// Per vertex, the world position the vertex shows: the real point on the surface, not
    /// its reflection, for polygons seen in a mirror.
    pub world_positions: Vec<Vec3>,
    /// Per vertex, its weights over the source polygon's vertices; see
    /// [`ViewPolygon::first_weight`].
    pub weights: Vec<f32>,
    pub polygons: Vec<ViewPolygon>,
    /// Where shadows fall on polygons; see [`ViewPolygon::split`].
    pub shadow_pieces: Vec<ShadowPiece>,
    pub shadow_vertices: Vec<ShadowVertex>,
    /// Where each polygon's mesh is: `[0]` is the level ([`Object::IDENTITY`]), then one per
    /// entity drawn.
    pub objects: Vec<Object>,
    /// The eye the view was built for (mirrors have their own, in `mirrors`), and its focal
    /// length in pixels.
    pub eye: Vec3,
    pub focal: f32,
    /// The world's lights and ambient light; see [`ViewGeometry::polygon_lights`].
    pub lights: Vec<Light>,
    pub ambient: Vec3,
    /// The lights that can reach each polygon, as ranges of `light_lists` (indices into
    /// `lights`): per sector for level polygons, and per object (parallel to `objects`) for
    /// an entity's, the lights of every sector it touches.
    pub light_lists: Vec<u32>,
    pub sector_lights: Vec<Range<u32>>,
    pub object_lights: Vec<Range<u32>>,
    pub visits: Vec<SectorVisit>,
    /// Mirrors seen this frame, parents before children. Visits and polygons seen in a
    /// mirror refer to it by index.
    pub mirrors: Vec<Mirror>,
    pub stats: ViewStats,
    pub config: ViewConfig,
    /// Window planes through the eye for every visit; see [`window_normals`](Self::window_normals).
    window_planes: Vec<ClipPlane>,
    /// The whole-pixel line of each window plane, parallel to `window_planes`.
    window_lines: Vec<EdgeLine>,
    scratch: Scratch,
}

#[derive(Default)]
struct Scratch {
    level: LevelCache,
    /// Per entity, its index in `objects` this frame, once drawn.
    entity_objects: Vec<u32>,
    entity_view: Vec<Vec3>,
    entity_world: Vec<Vec3>,
    record: Vec<f32>,
    planes: Vec<ClipPlane>,
    clipper: Clipper,
    pending: Vec<Pending>,
    /// Visits into mirrors, waiting for the traversal they were found in to finish.
    reflected: Vec<Pending>,
    /// Reflective polygons drawn in the current visit: source index, output indices (its
    /// pieces, when shadows carve it).
    mirror_polygons: Vec<(u32, Range<usize>)>,
    carver: Carver,
    /// For cached shadow pieces: clipping them, and a static polygon's corners.
    piece_clipper: Clipper,
    polygon_points: Vec<Vec3>,
    window: WindowScratch,
    blockers: Blockers,
}

/// Level positions in clip space, each transformed at most once per space.
#[derive(Default)]
struct LevelCache {
    clip: Vec<Vec3>,
    stamp: Vec<u32>,
    current: u32,
}

impl LevelCache {
    /// Invalidates every cached position, for a new frame or a new space.
    fn begin(&mut self, len: usize) {
        self.current = self.current.wrapping_add(1);
        if self.current == 0 {
            self.stamp.iter_mut().for_each(|t| *t = 0);
            self.current = 1;
        }
        self.clip.resize(len, Vec3::ZERO);
        self.stamp.resize(len, 0);
    }

    fn get(&mut self, i: u32, space: &Space, positions: &[Vec3]) -> Vec3 {
        let i = i as usize;
        if self.stamp[i] != self.current {
            self.clip[i] = space.to_clip(positions[i]);
            self.stamp[i] = self.current;
        }
        self.clip[i]
    }
}

#[derive(Default)]
struct WindowScratch {
    /// The outline of the portal or mirror being opened, in clip space.
    points: Vec<Vec3>,
    record: Vec<f32>,
    planes: Vec<ClipPlane>,
    lines: Vec<Option<EdgeLine>>,
    clipper: Clipper,
    window_points: Vec<Vec3>,
    window_edges: Vec<Edge>,
}

struct Pending {
    sector: u32,
    window: Range<u32>,
    near: bool,
    depth: u16,
    mirror: Option<u32>,
}

/// How the level is seen: directly, or in a mirror (possibly seen in other mirrors).
/// World to clip coordinates is `M (T(p) - camera)`, where `T` reflects across the chain of
/// mirror planes, the innermost first. Reflections are applied one plane at a time in world
/// space, where points on each mirror stay exactly where they are, so a mirror's corners
/// have bit-identical clip coordinates in the space it is seen in and the space seen
/// through it. Then the eye is subtracted first as usual.
#[derive(Clone, Copy)]
struct Space {
    /// M: world directions to clip coordinates.
    matrix: Mat3A,
    camera: Vec3,
    /// Mirror planes, outermost (seen directly) first.
    chain: [Plane; MAX_REFLECTIONS as usize],
    len: usize,
    /// The eye for facing tests: the camera, or the virtual eye `T^-1(camera)`, which sees
    /// the level as the camera sees its reflection.
    eye: Vec3,
}

impl Space {
    fn direct(matrix: Mat3A, camera: Vec3) -> Self {
        Self {
            matrix,
            camera,
            chain: [Plane {
                normal: Vec3::Y,
                d: 0.0,
            }; MAX_REFLECTIONS as usize],
            len: 0,
            eye: camera,
        }
    }

    /// The space seen in `mirror` (`None`: the direct one).
    fn of(direct: Space, mirrors: &[Mirror], mirror: Option<u32>) -> Self {
        let mut space = direct;
        let mut next = mirror;
        while let Some(m) = next {
            let m = &mirrors[m as usize];
            space.chain[m.depth as usize - 1] = m.plane;
            space.len = space.len.max(m.depth as usize);
            next = m.parent;
        }
        space.eye = mirror.map_or(direct.camera, |m| mirrors[m as usize].eye);
        space
    }

    fn to_clip(self, p: Vec3) -> Vec3 {
        let p = self.chain[..self.len]
            .iter()
            .rev()
            .fold(p, |p, plane| reflect(plane, p));
        self.matrix * (p - self.camera)
    }

    /// The linear part of `to_clip`: M times each reflection's.
    fn linear(&self) -> Mat3A {
        self.chain[..self.len].iter().fold(self.matrix, |m, plane| {
            let n = Vec3A::from(plane.normal);
            m * Mat3A::from_cols(
                Vec3A::X - 2.0 * n.x * n,
                Vec3A::Y - 2.0 * n.y * n,
                Vec3A::Z - 2.0 * n.z * n,
            )
        })
    }

    /// Each reflection turns clockwise into counter-clockwise, so outlines seen through an
    /// odd number of mirrors are walked backwards to keep front faces winding the same way
    /// on screen.
    fn reversed(&self) -> bool {
        self.len % 2 == 1
    }
}

/// Output buffers, borrowed separately from the scratch space.
struct Out<'a> {
    vertices: &'a mut Vec<ScreenVertex>,
    edge_lines: &'a mut Vec<Option<EdgeLine>>,
    world_positions: &'a mut Vec<Vec3>,
    weights: &'a mut Vec<f32>,
    polygons: &'a mut Vec<ViewPolygon>,
    shadow_pieces: &'a mut Vec<ShadowPiece>,
    shadow_vertices: &'a mut Vec<ShadowVertex>,
    values: Vec<(f32, f32)>,
    edges: Vec<PieceEdge>,
}

impl ViewGeometry {
    /// The lights that can reach polygon `p` (indices into `lights`): those of its sector,
    /// or for an entity's polygon, of every sector the entity touches. Some may still be
    /// out of range of it, or behind it.
    pub fn polygon_lights(&self, p: &ViewPolygon) -> &[u32] {
        let range = match p.source {
            PolygonSource::World { sector, .. } | PolygonSource::Terrain { sector, .. } => {
                self.sector_lights.get(sector as usize)
            }
            PolygonSource::Entity { .. } => self.object_lights.get(p.object as usize),
        };
        range.map_or(&[], |r| &self.light_lists[r.start as usize..r.end as usize])
    }

    /// Empty geometry, with an ambient light of 1 (surfaces show their full color) until
    /// built.
    pub fn new() -> Self {
        Self {
            ambient: Vec3::ONE,
            ..Self::default()
        }
    }

    /// Normals of a visit's window planes, all through the eye, in homogeneous clip
    /// coordinates `(x, y, w)` (the viewport spans -w..w on both axes, w is depth). A point
    /// `p` is inside the window when `normal · p >= 0` for every one.
    pub fn window_normals(&self, visit: &SectorVisit) -> impl Iterator<Item = Vec3> + '_ {
        self.window_planes[range(&visit.window)]
            .iter()
            .map(|p| p.normal)
    }

    /// A polygon's mesh attributes at each of its (clipped) vertices, as f32 in the mesh's
    /// attribute order: the weighted sums of the source vertices' values. `mesh` is the
    /// polygon's source mesh. Appends to `out`; returns the values per vertex.
    pub fn vertex_attributes(&self, p: &ViewPolygon, mesh: &Mesh, out: &mut Vec<f32>) -> usize {
        let source = &mesh.polygons[p.source_polygon() as usize];
        let n = p.source_vertices as usize;
        let stride: usize = mesh.attribs.iter().map(|a| a.count as usize).sum();
        for weights in self.weights[p.weights()].chunks_exact(n) {
            for a in &mesh.attribs {
                let count = a.count as usize;
                for c in 0..count {
                    let mut sum = 0.0;
                    for (k, &wk) in weights.iter().enumerate() {
                        if wk != 0.0 {
                            let v = source.first_vertex as usize + k;
                            sum += wk * a.data.get_f32(v * count + c);
                        }
                    }
                    out.push(sum);
                }
            }
        }
        stride
    }

    /// Where a level point seen in `mirror` appears: reflected across the mirror's plane,
    /// then across each mirror it is seen in, out to the one seen directly.
    pub fn reflect(&self, mirror: Option<u32>, p: Vec3) -> Vec3 {
        let mut p = p;
        let mut next = mirror;
        while let Some(m) = next {
            let m = &self.mirrors[m as usize];
            p = reflect(&m.plane, p);
            next = m.parent;
        }
        p
    }

    /// Forgets every baked and cached shadow, for after the world changes (an editor moved
    /// a static entity or changed a surface): bake again, or they are carved as surfaces
    /// are seen.
    pub fn clear_shadows(&mut self) {
        self.scratch.carver.clear_cache();
    }

    /// The polygon drawn nearest the eye at screen point `(x, y)` (pixels), among those
    /// seen directly (not in a mirror): its source, and its depth there as `w` (1 over
    /// the distance along the view). For picking with the mouse.
    pub fn pick(&self, x: f32, y: f32) -> Option<(PolygonSource, f32)> {
        let mut best: Option<(PolygonSource, f32)> = None;
        for p in &self.polygons {
            if p.mirror.is_some() || p.vertex_count < 3 {
                continue;
            }
            let v = &self.vertices[p.vertices()];
            // Inside: on the same side of every edge (either winding).
            let side = |a: &ScreenVertex, b: &ScreenVertex| {
                (b.x - a.x) * (y - a.y) - (b.y - a.y) * (x - a.x)
            };
            let n = v.len();
            let (mut pos, mut neg) = (false, false);
            for i in 0..n {
                let s = side(&v[i], &v[(i + 1) % n]);
                pos |= s > 0.0;
                neg |= s < 0.0;
            }
            if pos && neg {
                continue;
            }
            // w is affine on screen: from the widest triangle of the fan.
            let (mut area, mut tri) = (0.0f32, None);
            for i in 1..n - 1 {
                let (a, b, c) = (&v[0], &v[i], &v[i + 1]);
                let d = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                if d.abs() > area.abs() {
                    (area, tri) = (d, Some((a, b, c)));
                }
            }
            let Some((a, b, c)) = tri else { continue };
            let l1 = ((x - a.x) * (c.y - a.y) - (y - a.y) * (c.x - a.x)) / area;
            let l2 = ((b.x - a.x) * (y - a.y) - (b.y - a.y) * (x - a.x)) / area;
            let w = a.w + (b.w - a.w) * l1 + (c.w - a.w) * l2;
            if best.is_none_or(|(_, bw)| w > bw) {
                best = Some((p.source, w));
            }
        }
        best
    }

    /// Bakes the shadows of the world's static lights (those with shadow slots) on every
    /// static surface: level polygons and static entities' polygons, from their windows
    /// and static occluders (see the carve module). Views then only clip and project them;
    /// without baking, each surface's is carved the first time it is seen. Returns how
    /// many (light, surface) pairs have a shadow: some of the surface in the light's shadow.
    pub fn bake_shadows(&mut self, world: &World, assets: &Assets) -> usize {
        let carver = &mut self.scratch.carver;
        carver.prepare(world, assets, world.lights(), (false, 1.0));
        let geometry = assets.mesh(world.geometry);
        let mut points = Vec::new();
        let mut count = 0;
        for (si, sector) in world.sectors.iter().enumerate() {
            let sectors = [si as u32];
            for pi in sector.polygons.clone() {
                let polygon = &geometry.polygons[pi as usize];
                if polygon.flags.hidden() {
                    continue;
                }
                points.clear();
                points.extend(geometry.polygon_points(polygon));
                let receiver = Receiver {
                    sectors: &sectors,
                    entity: None,
                    normal: polygon.plane.normal,
                    point: points[0],
                    parts: Parts::All,
                    beams: false,
                    hard: false,
                };
                let bits = carver.cache((0, pi, 0), &points, &receiver);
                count += shadowed(carver, bits, (0, pi, 0));
            }
        }
        for terrain in &world.terrain {
            let mesh = assets.mesh(terrain.mesh);
            for (si, pieces) in terrain.sectors.iter().enumerate() {
                let sectors = [si as u32];
                for pi in pieces.clone() {
                    let polygon = &mesh.polygons[pi as usize];
                    points.clear();
                    points.extend(mesh.polygon_points(polygon));
                    let receiver = Receiver {
                        sectors: &sectors,
                        entity: None,
                        normal: polygon.plane.normal,
                        point: points[0],
                        parts: Parts::All,
                        beams: false,
                        hard: false,
                    };
                    let key = (2, terrain.entity, pi);
                    let bits = carver.cache(key, &points, &receiver);
                    count += shadowed(carver, bits, key);
                }
            }
        }
        for (ei, entity) in world.entities.iter().enumerate() {
            if !entity.is_static || !entity.kind.is_drawn() {
                continue;
            }
            let mesh = assets.mesh(entity.mesh);
            let model = entity.transform();
            for (pi, polygon) in mesh.polygons.iter().enumerate() {
                if polygon.flags.proxy() {
                    continue;
                }
                points.clear();
                points.extend(mesh.polygon_points(polygon).map(|p| model.transform_point3(p)));
                let receiver = Receiver {
                    sectors: &entity.sectors,
                    entity: Some(ei as u32),
                    normal: entity.rotation * polygon.plane.normal,
                    point: points[0],
                    parts: Parts::All,
                    beams: false,
                    hard: false,
                };
                let key = (1, ei as u32, pi as u32);
                let bits = carver.cache(key, &points, &receiver);
                count += shadowed(carver, bits, key);
            }
        }
        count
    }

    /// Processes one view: walks portals out from the view's sector, then transforms,
    /// culls, clips and projects everything visible.
    ///
    /// - Level geometry in the camera's sector is clipped to the frustum, near plane included.
    /// - Level geometry in later sectors is clipped only to its portal window, plus the near
    ///   plane when that window comes closer than it. Since sectors are convex, nothing
    ///   beyond a portal can be closer than the portal itself.
    /// - A reflective polygon is drawn, and also opens a window like a portal: into its own
    ///   sector reflected across its plane. The reflected level is walked like the direct
    ///   one, seen from the virtual eye behind the mirror, with outlines reversed so they
    ///   still wind counter-clockwise on screen. It lies entirely beyond the mirror, so the
    ///   mirror's window bounds it like a portal's. Its polygons tile the mirror's window; the
    ///   mirror polygon itself is left on top of them, for the translucent pass. Reflective
    ///   polygons seen in a mirror open mirrors of their own the same way, up to
    ///   `config.max_reflections` deep.
    /// - Entities are culled unless some window into a sector they touch sees their bounds,
    ///   then drawn once with a frustum clip on only the planes their bounds cross. The same
    ///   again for each mirror, with the entity reflected. Stats count all of them.
    pub fn build(&mut self, world: &World, assets: &Assets, view: &View) {
        self.vertices.clear();
        self.edge_lines.clear();
        self.world_positions.clear();
        self.weights.clear();
        self.polygons.clear();
        self.shadow_pieces.clear();
        self.shadow_vertices.clear();
        self.objects.clear();
        self.objects.push(Object::IDENTITY);
        (self.eye, self.focal) = (view.position, view.focal);
        self.lights.clear();
        self.lights.extend_from_slice(world.lights());
        self.ambient = world.ambient;
        self.light_lists.clear();
        self.sector_lights.clear();
        for sector in 0..world.sectors.len() as u32 {
            let start = self.light_lists.len() as u32;
            self.light_lists.extend_from_slice(world.sector_lights(sector));
            self.sector_lights.push(start..self.light_lists.len() as u32);
        }
        self.object_lights.clear();
        self.object_lights.push(0..0); // the level: per sector instead
        self.scratch
            .carver
            .prepare(
                world,
                assets,
                &self.lights,
                (self.config.dynamic_shadows, self.config.dynamic_softness),
            );
        self.visits.clear();
        self.mirrors.clear();
        self.window_planes.clear();
        self.window_lines.clear();
        self.stats = ViewStats::default();

        let s = &mut self.scratch;
        let mut out = Out {
            vertices: &mut self.vertices,
            edge_lines: &mut self.edge_lines,
            world_positions: &mut self.world_positions,
            weights: &mut self.weights,
            polygons: &mut self.polygons,
            shadow_pieces: &mut self.shadow_pieces,
            shadow_vertices: &mut self.shadow_vertices,
            values: Vec::new(),
            edges: Vec::new(),
        };
        s.entity_objects.clear();
        s.entity_objects.resize(world.entities.len(), u32::MAX);
        let geometry = assets.mesh(world.geometry);
        let eye = view.position;
        // World to homogeneous clip coordinates (x, y, w), where the viewport spans
        // -w <= x <= w and -w <= y <= w, and w is the depth. Computed as M (p - eye) rather than
        // as an affine M p + t: subtracting the eye first is exact for nearby points, while the
        // affine form cancels two large terms.
        let vp = view.viewport;
        let (half_w, half_h) = (vp.width as f32 / 2.0, vp.height as f32 / 2.0);
        let to_clip_matrix =
            Mat3A::from_diagonal(Vec3::new(view.focal / half_w, view.focal / half_h, -1.0))
                * Mat3A::from_quat(view.rotation.conjugate());
        let direct = Space::direct(to_clip_matrix, eye);

        // Start in the sector the eye is really in: a camera placed directly (not moved
        // through the world) can sit just behind a portal of its recorded sector.
        let mut start = view.sector;
        for _ in 0..4 {
            let behind = world.sectors[start as usize]
                .portals
                .clone()
                .map(|p| &world.portals[p as usize])
                .find(|portal| {
                    portal.plane.distance(eye) < -ON_PORTAL
                        && inside_outline(outline(geometry, portal), portal.plane.normal, eye)
                });
            match behind {
                Some(portal) => start = portal.target,
                None => break,
            }
        }

        let frustum = frustum_planes();
        self.window_planes.extend_from_slice(&frustum);
        self.window_lines.extend_from_slice(&border_lines(view));
        let full = 0..4;
        s.pending.push(Pending {
            sector: start,
            window: full.clone(),
            near: true,
            depth: 0,
            mirror: None,
        });
        // Eye exactly on a portal's plane: the portal is edge-on and yields no window, but the
        // sector behind it fills half the view. Treat it like the camera's sector; the doorway
        // wall's faces are edge-on too, so the two sides cannot overlap.
        for p in world.sectors[start as usize].portals.clone() {
            let portal = &world.portals[p as usize];
            if portal.flags.render_through()
                && portal.plane.distance(eye).abs() <= ON_PORTAL
                && inside_outline(outline(geometry, portal), portal.plane.normal, eye)
            {
                s.pending.push(Pending {
                    sector: portal.target,
                    window: full.clone(),
                    near: true,
                    depth: 1,
                    mirror: None,
                });
            }
        }

        // The direct traversal, then one traversal per mirror, each after the traversal it
        // was found in (mirrors are numbered in the order found, so the lowest waiting one
        // always has all of its visits).
        let max_reflections = self.config.max_reflections.min(MAX_REFLECTIONS);
        let mut group: Option<u32> = None;
        loop {
            let space = Space::of(direct, &self.mirrors, group);
            let depth = group.map_or(0, |m| self.mirrors[m as usize].depth);
            s.level.begin(geometry.positions.len());
            s.blockers.build(&world.blockers, space.eye);
            let level = &mut s.level;
            let mut level_pos = |i: u32| level.get(i, &space, &geometry.positions);

            while let Some(visit) = s.pending.pop() {
                if self.visits.len() >= MAX_VISITS {
                    s.pending.clear();
                    s.reflected.clear();
                    break;
                }
                let sector = &world.sectors[visit.sector as usize];

                // Level geometry of this sector, clipped to the window (and near plane if needed).
                s.planes.clear();
                s.planes
                    .extend_from_slice(&self.window_planes[range(&visit.window)]);
                if visit.near {
                    s.planes.push(ClipPlane::near(view.near));
                }
                s.mirror_polygons.clear();
                for pi in sector.polygons.clone() {
                    let polygon = &geometry.polygons[pi as usize];
                    if polygon.flags.hidden() {
                        continue;
                    }
                    if polygon.plane.distance(space.eye) <= 0.0 {
                        self.stats.world_backfacing += 1;
                        continue;
                    }
                    build_record(
                        &mut s.record,
                        geometry,
                        polygon,
                        space.reversed(),
                        &mut |i| (level_pos(i), geometry.positions[i as usize]),
                    );
                    let (clipped, edges) = s.clipper.clip(
                        &s.record,
                        RECORD + polygon.vertex_count as usize,
                        &s.planes,
                    );
                    if clipped.is_empty() {
                        self.stats.world_outside += 1;
                        continue;
                    }
                    s.polygon_points.clear();
                    s.polygon_points.extend(geometry.polygon_points(polygon));
                    let drawn = emit_pieces(
                        &mut out,
                        &mut s.carver,
                        view,
                        Receiving {
                            space: &space,
                            planes: &s.planes,
                            clipper: &mut s.piece_clipper,
                            is_static: self.config.cache_shadows,
                            dynamic: self.config.dynamic_shadows,
                            polygon: &s.polygon_points,
                        },
                        clipped,
                        edges,
                        &self.window_lines[range(&visit.window)],
                        polygon.vertex_count as usize,
                        world.geometry,
                        0,
                        PolygonKind::World,
                        PolygonSource::World {
                            sector: visit.sector,
                            polygon: pi,
                        },
                        polygon.flags,
                        group,
                        &[visit.sector],
                        polygon.plane.normal,
                    );
                    self.stats.world_drawn += 1;
                    if polygon.flags.reflective() && depth < max_reflections {
                        s.mirror_polygons.push((pi, drawn));
                    }
                }

                // Terrain pieces in this sector: clipped to the window like its walls, but
                // sorted in (a terrain's pieces can overlap each other on screen).
                for terrain in &world.terrain {
                    let Some(pieces) = terrain.sectors.get(visit.sector as usize) else {
                        continue;
                    };
                    let mesh = assets.mesh(terrain.mesh);
                    for pi in pieces.clone() {
                        let polygon = &mesh.polygons[pi as usize];
                        if polygon.plane.distance(space.eye) <= 0.0 {
                            self.stats.terrain_backfacing += 1;
                            continue;
                        }
                        s.polygon_points.clear();
                        s.polygon_points.extend(mesh.polygon_points(polygon));
                        if s.blockers.hides(&s.polygon_points) {
                            self.stats.terrain_blocked += 1;
                            continue;
                        }
                        build_record(
                            &mut s.record,
                            mesh,
                            polygon,
                            space.reversed(),
                            &mut |i| {
                                let p = mesh.positions[i as usize];
                                (space.to_clip(p), p)
                            },
                        );
                        let (clipped, edges) = s.clipper.clip(
                            &s.record,
                            RECORD + polygon.vertex_count as usize,
                            &s.planes,
                        );
                        if clipped.is_empty() {
                            self.stats.terrain_outside += 1;
                            continue;
                        }
                        emit_pieces(
                            &mut out,
                            &mut s.carver,
                            view,
                            Receiving {
                                space: &space,
                                planes: &s.planes,
                                clipper: &mut s.piece_clipper,
                                is_static: self.config.cache_shadows,
                                dynamic: self.config.dynamic_shadows,
                                polygon: &s.polygon_points,
                            },
                            clipped,
                            edges,
                            &self.window_lines[range(&visit.window)],
                            polygon.vertex_count as usize,
                            terrain.mesh,
                            0,
                            PolygonKind::Prop,
                            PolygonSource::Terrain {
                                entity: terrain.entity,
                                sector: visit.sector,
                                polygon: pi,
                            },
                            polygon.flags,
                            group,
                            &[visit.sector],
                            polygon.plane.normal,
                        );
                        self.stats.terrain_drawn += 1;
                    }
                }

                if visit.depth < MAX_PORTAL_DEPTH {
                    // Mirrors: each opens a window into this sector, reflected. Seen from here
                    // they are front-facing convex outlines, just like portals.
                    for (pi, drawn) in &s.mirror_polygons {
                        let pi = *pi;
                        let polygon = &geometry.polygons[pi as usize];
                        let w = &mut s.window;
                        w.points.clear();
                        w.points.extend(
                            polygon
                                .vertices()
                                .map(|v| level_pos(geometry.vertex_positions[v])),
                        );
                        if space.reversed() {
                            w.points.reverse();
                        }
                        let Some((window, nearest)) = open_window(
                            &mut self.window_planes,
                            &mut self.window_lines,
                            w,
                            view,
                            &visit,
                        ) else {
                            continue;
                        };
                        let m = match self
                            .mirrors
                            .iter()
                            .position(|m| m.plane == polygon.plane && m.parent == group)
                        {
                            Some(m) => m,
                            None => {
                                self.mirrors.push(Mirror {
                                    plane: polygon.plane,
                                    parent: group,
                                    depth: depth + 1,
                                    eye: reflect(&polygon.plane, space.eye),
                                });
                                self.mirrors.len() - 1
                            }
                        };
                        for piece in drawn.clone() {
                            out.polygons[piece].reflection = Some(m as u32);
                        }
                        s.reflected.push(Pending {
                            sector: visit.sector,
                            window,
                            near: nearest < view.near,
                            depth: visit.depth + 1,
                            mirror: Some(m as u32),
                        });
                    }

                    // Portals out of this sector, each clipped to the window to form a child
                    // window.
                    for p in sector.portals.clone() {
                        let portal = &world.portals[p as usize];
                        if !portal.flags.render_through()
                            || portal.plane.distance(space.eye) <= ON_PORTAL
                        {
                            continue; // closed, facing away, or edge-on
                        }
                        let w = &mut s.window;
                        w.points.clear();
                        w.points
                            .extend(portal.positions.iter().map(|&i| level_pos(i)));
                        if space.reversed() {
                            w.points.reverse();
                        }
                        let Some((window, nearest)) = open_window(
                            &mut self.window_planes,
                            &mut self.window_lines,
                            w,
                            view,
                            &visit,
                        ) else {
                            continue;
                        };
                        s.pending.push(Pending {
                            sector: portal.target,
                            window,
                            near: nearest < view.near,
                            depth: visit.depth + 1,
                            mirror: group,
                        });
                    }
                }
                self.visits.push(SectorVisit {
                    sector: visit.sector,
                    depth: visit.depth,
                    near_clipped: visit.near,
                    window: visit.window,
                    mirror: visit.mirror,
                });
            }

            // Next, every visit into the next mirror.
            let Some(next) = s.reflected.iter().map(|p| p.mirror).min() else {
                break;
            };
            let mut i = 0;
            while i < s.reflected.len() {
                if s.reflected[i].mirror == next {
                    s.pending.push(s.reflected.swap_remove(i));
                } else {
                    i += 1;
                }
            }
            group = next;
        }

        // Entities: cull by windows into any sector they touch, then clip to the frustum only.
        // Once seen directly, and once in each mirror.
        let near = ClipPlane::near(view.near);
        for group in std::iter::once(None).chain((0..self.mirrors.len() as u32).map(Some)) {
            let space = Space::of(direct, &self.mirrors, group);
            s.blockers.build(&world.blockers, space.eye);
            for (ei, entity) in world.entities.iter().enumerate() {
                if !entity.kind.is_drawn() {
                    continue;
                }
                let box_world = box_corners(entity.bounds.min, entity.bounds.max);
                if s.blockers.hides(&box_world) {
                    self.stats.entities_blocked += 1;
                    continue;
                }
                let corners = box_world.map(|c| space.to_clip(c));
                let seen = entity.sectors.iter().any(|&sector| {
                    self.visits
                        .iter()
                        .filter(|v| v.sector == sector && v.mirror == group)
                        .any(|v| {
                            self.window_planes[range(&v.window)]
                                .iter()
                                .all(|p| corners.iter().any(|&c| p.distance(c) >= 0.0))
                        })
                });
                let frustum_all = frustum.iter().chain(std::iter::once(&near));
                if !seen
                    || frustum_all
                        .clone()
                        .any(|p| corners.iter().all(|&c| p.distance(c) < 0.0))
                {
                    self.stats.entities_culled += 1;
                    continue;
                }
                s.planes.clear();
                s.planes
                    .extend(frustum_all.filter(|p| corners.iter().any(|&c| p.distance(c) < 0.0)));

                let mesh = assets.mesh(entity.mesh);
                let model = entity.transform();
                // Model to clip with the eye offset folded in, for the same precision reason.
                let model_to_view = Affine3A::from_mat3_translation(
                    (space.linear() * Mat3A::from_quat(entity.rotation) * entity.scale).into(),
                    space.to_clip(entity.position),
                );
                s.entity_view.clear();
                s.entity_view.extend(
                    mesh.positions
                        .iter()
                        .map(|&p| model_to_view.transform_point3(p)),
                );
                s.entity_world.clear();
                s.entity_world
                    .extend(mesh.positions.iter().map(|&p| model.transform_point3(p)));
                let eye_model = model.inverse().transform_point3(space.eye);
                if s.entity_objects[ei] == u32::MAX {
                    s.entity_objects[ei] = self.objects.len() as u32;
                    self.objects.push(Object {
                        position: entity.position,
                        rotation: entity.rotation,
                        scale: entity.scale,
                    });
                    let start = self.light_lists.len();
                    for &sector in &entity.sectors {
                        self.light_lists
                            .extend_from_slice(world.sector_lights(sector));
                    }
                    self.light_lists[start..].sort_unstable();
                    let mut kept = start;
                    for i in start..self.light_lists.len() {
                        if kept == start || self.light_lists[kept - 1] != self.light_lists[i] {
                            self.light_lists[kept] = self.light_lists[i];
                            kept += 1;
                        }
                    }
                    self.light_lists.truncate(kept);
                    self.object_lights
                        .push(start as u32..self.light_lists.len() as u32);
                }
                let object = s.entity_objects[ei];
                let kind = if entity.kind == EntityKind::Actor {
                    PolygonKind::Actor
                } else {
                    PolygonKind::Prop
                };
                for (pi, polygon) in mesh.polygons.iter().enumerate() {
                    // Shadow proxies aren't drawn.
                    if polygon.flags.proxy() {
                        continue;
                    }
                    if polygon.plane.distance(eye_model) <= 0.0 {
                        self.stats.entity_polygons_backfacing += 1;
                        continue;
                    }
                    let (entity_view, entity_world) = (&s.entity_view, &s.entity_world);
                    build_record(&mut s.record, mesh, polygon, space.reversed(), &mut |i| {
                        (entity_view[i as usize], entity_world[i as usize])
                    });
                    let (clipped, edges) = s.clipper.clip(
                        &s.record,
                        RECORD + polygon.vertex_count as usize,
                        &s.planes,
                    );
                    if clipped.is_empty() {
                        self.stats.entity_polygons_outside += 1;
                        continue;
                    }
                    s.polygon_points.clear();
                    if entity.is_static {
                        s.polygon_points.extend(
                            polygon
                                .vertices()
                                .map(|v| s.entity_world[mesh.vertex_positions[v] as usize]),
                        );
                    }
                    emit_pieces(
                        &mut out,
                        &mut s.carver,
                        view,
                        Receiving {
                            space: &space,
                            planes: &s.planes,
                            clipper: &mut s.piece_clipper,
                            is_static: entity.is_static && self.config.cache_shadows,
                            dynamic: self.config.dynamic_shadows,
                            polygon: &s.polygon_points,
                        },
                        clipped,
                        edges,
                        &[], // entities are not clipped to portals: every edge between its endpoints
                        polygon.vertex_count as usize,
                        entity.mesh,
                        object,
                        kind,
                        PolygonSource::Entity {
                            entity: ei as u32,
                            polygon: pi as u32,
                        },
                        polygon.flags,
                        group,
                        &entity.sectors,
                        entity.rotation * polygon.plane.normal,
                    );
                }
                self.stats.entities_drawn += 1;
            }
        }
    }
}

/// Clips a portal's or mirror's outline (in `w.points`, clip space, counter-clockwise on
/// screen) to the parent visit's window and appends the child window's planes and lines.
/// Returns the child window and its nearest depth, or `None` if nothing is seen through it.
fn open_window(
    window_planes: &mut Vec<ClipPlane>,
    window_lines: &mut Vec<EdgeLine>,
    w: &mut WindowScratch,
    view: &View,
    parent: &Pending,
) -> Option<(Range<u32>, f32)> {
    w.record.clear();
    w.record.extend(w.points.iter().flat_map(|p| p.to_array()));
    w.planes.clear();
    w.planes
        .extend_from_slice(&window_planes[range(&parent.window)]);
    // First, the line of each outline edge: the edge clipped exactly as the parent sector's
    // walls are (window planes, then the near plane if the sector is near-clipped), so the
    // wall sharing that edge ends on the same endpoints.
    if parent.near {
        w.planes.push(ClipPlane::near(view.near));
    }
    w.lines.clear();
    w.lines.resize(w.points.len(), None);
    let (clipped, edges) = w.clipper.clip(&w.record, 3, &w.planes);
    for (i, edge) in edges.iter().enumerate() {
        if let Edge::Input(e) = *edge {
            let j = (i + 1) % edges.len();
            let (a, b) = (
                Vec3::from_slice(&clipped[i * 3..]),
                Vec3::from_slice(&clipped[j * 3..]),
            );
            w.lines[e as usize] = Some(line_between(view, a, b));
        }
    }
    if parent.near {
        w.planes.pop();
    }
    // Then the window itself: the same clip without the near plane (the window must reach
    // all the way to the eye).
    let (clipped, edges) = w.clipper.clip(&w.record, 3, &w.planes);
    if clipped.is_empty() {
        return None;
    }
    w.window_points.clear();
    w.window_points
        .extend(clipped.chunks_exact(3).map(Vec3::from_slice));
    w.window_edges.clear();
    w.window_edges.extend_from_slice(edges);
    if window_too_small(&w.window_points) {
        return None;
    }
    // Window planes (and their lines) come from the edge sources: a parent plane is reused
    // exactly, an outline edge's plane comes from its original corners. Never rebuilt from
    // clipped points.
    let first = window_planes.len() as u32;
    let n = w.window_edges.len();
    for i in 0..n {
        let edge = w.window_edges[i];
        if n > 1 && edge == w.window_edges[(i + n - 1) % n] {
            continue; // consecutive edges on the same line
        }
        match edge {
            Edge::Plane(k) => {
                let k = parent.window.start as usize + k as usize;
                window_planes.push(window_planes[k]);
                window_lines.push(window_lines[k]);
            }
            Edge::Input(e) => {
                let e = e as usize;
                let (a, b) = (w.points[e], w.points[(e + 1) % w.points.len()]);
                let normal = a.cross(b);
                if normal.length() <= MIN_EDGE_ANGLE * a.length() * b.length() {
                    continue; // seen edge-on: no reliable plane
                }
                // The wall's line, unless the near plane cut it to a speck while the window
                // (not near-clipped) runs on along it: then the window's own edge, on the
                // same line, so the long edges behind it are walked from endpoints that far
                // apart. The wall then shows at most a pixel or so of that edge.
                let own = line_between(view, w.window_points[i], w.window_points[(i + 1) % n]);
                let length = |l: &EdgeLine| (l.x1 - l.x0).hypot(l.y1 - l.y0);
                let line = match w.lines[e] {
                    Some(l) if length(&l) >= MIN_LINE_LENGTH || length(&l) >= length(&own) => l,
                    _ => own,
                };
                window_planes.push(ClipPlane::through_eye(normal.normalize()));
                window_lines.push(line);
            }
        }
    }
    let window = first..window_planes.len() as u32;
    if window.len() < 3 {
        window_planes.truncate(first as usize);
        window_lines.truncate(first as usize);
        return None;
    }
    // Everything beyond the outline is at least as deep as the outline itself.
    let nearest = w
        .window_points
        .iter()
        .map(|p| p.z)
        .fold(f32::INFINITY, f32::min); // w = depth
    Some((window, nearest))
}

fn range(r: &Range<u32>) -> Range<usize> {
    r.start as usize..r.end as usize
}

/// Floats in a clip record before the weights: clip-space x, y, w, then world x, y, z.
const RECORD: usize = 6;

/// Writes a polygon as interleaved records: clip-space x, y, w, world position, then its
/// weights over the polygon's vertices (1 on itself). `position` gives a position index's
/// clip and world coordinates. `reversed` walks the outline backwards (for polygons seen in
/// a mirror); weights stay in the polygon's own vertex order.
fn build_record(
    record: &mut Vec<f32>,
    mesh: &Mesh,
    polygon: &Polygon,
    reversed: bool,
    position: &mut impl FnMut(u32) -> (Vec3, Vec3),
) {
    record.clear();
    let vertices = polygon.vertices();
    for k in 0..vertices.len() {
        let v = if reversed {
            vertices.end - 1 - k
        } else {
            vertices.start + k
        };
        let (clip, world) = position(mesh.vertex_positions[v]);
        record.extend_from_slice(&clip.to_array());
        record.extend_from_slice(&world.to_array());
        let own = v - vertices.start;
        record.extend((0..vertices.len()).map(|k| if k == own { 1.0 } else { 0.0 }));
    }
}

/// Divides a clip-space point by w: its exact framebuffer position. The one place positions
/// are projected, so a point shared by several polygons (or a portal line and a wall)
/// projects to the bit-identical position everywhere.
fn to_screen(view: &View, p: Vec3) -> (f32, f32) {
    let vp = view.viewport;
    let (half_w, half_h) = (vp.width as f32 / 2.0, vp.height as f32 / 2.0);
    let (x, y) = (
        view.center.x + (p.x / p.z) * half_w,
        view.center.y - (p.y / p.z) * half_h,
    );
    let (left, top) = (vp.x as f32, vp.y as f32);
    let (right, bottom) = (left + vp.width as f32, top + vp.height as f32);
    debug_assert!(
        x >= left - MAX_OVERSHOOT
            && x <= right + MAX_OVERSHOOT
            && y >= top - MAX_OVERSHOOT
            && y <= bottom + MAX_OVERSHOOT,
        "clipped vertex ({x}, {y}) is outside the viewport by more than rounding"
    );
    // Clipping leaves points inside the viewport up to float rounding; the clamp removes
    // that, so rows and spans derived from these positions are always in bounds.
    (x.clamp(left, right), y.clamp(top, bottom))
}

/// The screen line through two clip-space points.
fn line_between(view: &View, a: Vec3, b: Vec3) -> EdgeLine {
    let ((x0, y0), (x1, y1)) = (to_screen(view, a), to_screen(view, b));
    EdgeLine { x0, y0, x1, y1 }
}

/// The screen line through two clip-space points anywhere (a cut's ends, which can lie far
/// off screen or behind the eye): the segment trimmed to the view frustum (near plane
/// included), so its ends are on screen and the line is exact where it's seen, then
/// projected. `None` if none of it is in view (then no piece's edge on it is either).
/// Pieces sharing a cut trim the same segment, so they get the same line.
fn line_through(view: &View, a: Vec3, b: Vec3) -> Option<EdgeLine> {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let near = ClipPlane::near(view.near);
    for plane in frustum_planes().iter().chain(std::iter::once(&near)) {
        let (da, db) = (plane.distance(a), plane.distance(b));
        if da < 0.0 && db < 0.0 {
            return None;
        }
        if da < 0.0 {
            t0 = t0.max(da / (da - db));
        } else if db < 0.0 {
            t1 = t1.min(da / (da - db));
        }
    }
    if t0 >= t1 {
        return None;
    }
    let (a, b) = (a + (b - a) * t0, a + (b - a) * t1);
    let vp = view.viewport;
    let (half_w, half_h) = (vp.width as f32 / 2.0, vp.height as f32 / 2.0);
    let project = |p: Vec3| {
        (
            view.center.x + (p.x / p.z) * half_w,
            view.center.y - (p.y / p.z) * half_h,
        )
    };
    Some(EdgeLine::between(project(a), project(b)))
}

/// The viewport border lines, matching `frustum_planes` in order.
fn border_lines(view: &View) -> [EdgeLine; 4] {
    let vp = view.viewport;
    let (l, t) = (vp.x as f32, vp.y as f32);
    let (r, b) = (l + vp.width as f32, t + vp.height as f32);
    [
        EdgeLine::between((l, t), (l, b)), // x = -w: left
        EdgeLine::between((r, t), (r, b)), // x = w: right
        EdgeLine::between((l, b), (r, b)), // y = -w: bottom
        EdgeLine::between((l, t), (r, t)), // y = w: top
    ]
}

/// Emits a clipped polygon whole, with the shadows cast on it: it is carved for the lights
/// that cast shadows (see the carve module), and the pieces in a light's shadow (fully, or
/// softly with how much of the light reaches their vertices) become its shadow pieces for
/// that light, which the rasterizer draws into a shadow buffer as it shades the polygon. A
/// light whose shadow covers all of it is simply left out of its lights. `sectors` are the
/// sectors it is in, and `normal` its world-space normal. Returns its output index (as a
/// range).
#[allow(clippy::too_many_arguments)]
fn emit_pieces(
    out: &mut Out,
    carver: &mut Carver,
    view: &View,
    mut receiving: Receiving,
    records: &[f32],
    edges: &[Edge],
    plane_lines: &[EdgeLine],
    source_vertices: usize,
    id: MeshId,
    object: u32,
    kind: PolygonKind,
    source: PolygonSource,
    flags: PolyFlags,
    mirror: Option<u32>,
    sectors: &[u32],
    normal: Vec3,
) -> Range<usize> {
    let first = out.polygons.len();
    let stride = RECORD + source_vertices;
    let mut receiver = Receiver {
        sectors,
        entity: match source {
            PolygonSource::Entity { entity, .. } => Some(entity),
            PolygonSource::World { .. } | PolygonSource::Terrain { .. } => None,
        },
        normal,
        point: Vec3::from_slice(&records[3..6]),
        parts: Parts::All,
        // In reflections, beams are lit at sample points: cheaper, and half-rate there.
        beams: mirror.is_none(),
        // In reflections, shadows carved every frame are hard.
        hard: mirror.is_some(),
    };
    // A static polygon's shadows from static lights are cached (carved the first time it
    // is seen); only moving occluders are carved for them every frame.
    let key = match source {
        PolygonSource::World { polygon, .. } => (0, polygon, 0),
        PolygonSource::Entity { entity, polygon } => (1, entity, polygon),
        PolygonSource::Terrain { entity, polygon, .. } => (2, entity, polygon),
    };
    let cached = if receiving.is_static {
        receiver.point = receiving.polygon[0];
        let bits = carver.cache(key, receiving.polygon, &receiver);
        receiver.point = Vec3::from_slice(&records[3..6]);
        bits
    } else {
        0
    };
    // Without dynamic shadows, only beams are carved every frame (see `Light::beam`).
    receiver.parts = if receiving.dynamic { Parts::Cached(cached) } else { Parts::Beams };
    let pieces = if receiving.dynamic || carver.has_beams() {
        carver
            .carve(records, edges, stride, plane_lines.len(), &receiver)
            .len()
    } else {
        0
    };
    // Per shadow slot: whether any piece is in its shadow, and whether all are fully.
    let (mut touched, mut dark) = (0u32, u32::MAX);
    for i in 0..pieces {
        let piece = carver.piece(i);
        out.values.clear();
        touched |= piece.shadowed | piece.beamed | piece.blurred | carver.soft_values(&piece, &mut out.values);
        dark &= piece.shadowed;
    }
    // And the cached ones.
    let (mut cached_touched, mut cached_full) = (0u32, 0u32);
    let mut bits = cached;
    while bits != 0 {
        let slot = bits.trailing_zeros();
        bits &= bits - 1;
        if let Some(c) = carver.cached(slot as u8, key) {
            if c.full {
                cached_full |= 1 << slot;
            } else if !c.pieces.is_empty() {
                cached_touched |= 1 << slot;
            }
        }
    }
    // A light whose shadow covers all of it is simply left out; one whose shadow covers
    // part of it gets its shadow pieces.
    let whole = (touched & dark) | cached_full;
    let split = (touched | cached_touched) & !whole;
    let first_shadow = out.shadow_pieces.len() as u32;
    if split != 0 {
        for i in 0..pieces {
            let piece = carver.piece(i);
            out.values.clear();
            let soft = carver.soft_values(&piece, &mut out.values);
            let slots = (piece.shadowed | piece.beamed | piece.blurred | soft) & split;
            let per_vertex = soft.count_ones() as usize;
            let (records, edges) = (carver.records(&piece), carver.edges(&piece));
            let mut bits = slots;
            while bits != 0 {
                let slot = bits.trailing_zeros();
                bits &= bits - 1;
                let dark = piece.shadowed >> slot & 1 != 0;
                // Its value among the vertex's soft values, if soft here and not in the full
                // shadow of another occluder of the same light.
                let soft_at = (soft >> slot & 1 != 0 && !dark)
                    .then(|| (soft & ((1 << slot) - 1)).count_ones() as usize);
                // In a blurred caster's hard shadow (and not in full shadow): the renderer
                // blurs it.
                let occluded = piece.blurred >> slot & 1 != 0 && !dark;
                // Inside a beam's pyramid, its cone fades the light too.
                let beam = (piece.beamed >> slot & 1 != 0 && !dark)
                    .then(|| carver.beam(slot as u8))
                    .flatten();
                let first_vertex = out.shadow_vertices.len() as u32;
                for (v, (r, edge)) in records.chunks_exact(stride).zip(edges).enumerate() {
                    let (x, y) = to_screen(view, Vec3::from_slice(r));
                    let world = Vec3::from_slice(&r[3..6]);
                    let vertex = ShadowVertex {
                        x,
                        y,
                        w: 1.0 / r[2],
                        line: edge_line(view, edge, plane_lines),
                        light: 0.0,
                        ray: beam.map_or(Vec3::ZERO, |b| b.ray(world)),
                        width: if occluded {
                            carver.penumbra_width(slot as u8, world, receiver.entity)
                        } else {
                            0.0
                        },
                    };
                    let light = match soft_at {
                        Some(k) => out.values[v * per_vertex + k],
                        None if dark => (0.0, 0.0),
                        None => (1.0, 1.0),
                    };
                    push_shadow_vertex(out.shadow_vertices, vertex, light);
                }
                push_shadow_piece(
                    out.shadow_vertices,
                    out.shadow_pieces,
                    ShadowPiece {
                        slot: slot as u8,
                        first_vertex,
                        vertex_count: (out.shadow_vertices.len() as u32 - first_vertex) as u16,
                        beam: beam.map(|b| b.cone),
                        occluded,
                    },
                );
            }
        }
        // The cached pieces, clipped as the polygon was and projected.
        let mut bits = split & cached;
        while bits != 0 {
            let slot = bits.trailing_zeros();
            bits &= bits - 1;
            let Some(c) = carver.cached(slot as u8, key) else {
                continue;
            };
            emit_cached(out, view, &mut receiving, c, slot as u8, (records, edges, stride), plane_lines);
        }
    }
    let shadow_count = (out.shadow_pieces.len() as u32 - first_shadow) as u16;
    // The polygon itself, whole.
    let mut whole_edges = std::mem::take(&mut out.edges);
    whole_edges.clear();
    whole_edges.extend(edges.iter().map(|&e| PieceEdge::Clip(e)));
    emit(
        out,
        view,
        records,
        &whole_edges,
        plane_lines,
        source_vertices,
        id,
        object,
        kind,
        source,
        flags,
        mirror,
        whole,
        (split, first_shadow, shadow_count),
    );
    out.edges = whole_edges;
    first..out.polygons.len()
}

/// How many of the lights with shadow slot bits `bits` have a cached shadow on the polygon
/// `key`: some of it in their shadow.
fn shadowed(carver: &Carver, bits: u32, key: (u8, u32, u32)) -> usize {
    (0..32u8)
        .filter(|&slot| bits >> slot & 1 != 0)
        .filter(|&slot| carver.cached(slot, key).is_some_and(|c| c.full || !c.pieces.is_empty()))
        .count()
}

/// What [`emit_pieces`] needs to place a static polygon's cached shadow pieces on screen:
/// the space it's seen in and the planes it was clipped to (the pieces are clipped to them
/// too), a clipper, whether it is static, and its corners in world space (in its own order)
/// if so.
struct Receiving<'a> {
    space: &'a Space,
    planes: &'a [ClipPlane],
    clipper: &'a mut Clipper,
    is_static: bool,
    /// Carve what isn't cached (see `ViewConfig::dynamic_shadows`).
    dynamic: bool,
    polygon: &'a [Vec3],
}

/// Emits a static polygon's cached shadow pieces for shadow slot `slot` (world space): each
/// clipped to the planes the polygon was clipped to, and projected. Their edges walk the
/// lines their neighbors walk: along the polygon's own edges as the clipped polygon
/// (`polygon`: its records, edges and stride) walks them, along clip planes as it does, and
/// along cuts through the cuts' world points.
fn emit_cached(
    out: &mut Out,
    view: &View,
    receiving: &mut Receiving,
    cached: &crate::carve::Cached,
    slot: u8,
    (records, edges, stride): (&[f32], &[Edge], usize),
    plane_lines: &[EdgeLine],
) {
    let space = receiving.space;
    let reversed = space.reversed();
    let n = receiving.polygon.len();
    // The line the clipped polygon walks along its own edge `e` (in its own order).
    let polygon_line = |e: u16| {
        let k = if reversed { (2 * n - 2 - e as usize) % n } else { e as usize } as u16;
        let i = edges.iter().position(|&x| x == Edge::Input(k))?;
        let (a, b) = (i, (i + 1) % edges.len());
        Some(line_between(
            view,
            Vec3::from_slice(&records[a * stride..]),
            Vec3::from_slice(&records[b * stride..]),
        ))
    };
    const STRIDE: usize = RECORD;
    let mut piece = Vec::with_capacity(8 * STRIDE);
    let mut labels = Vec::with_capacity(8);
    for (range, soft) in &cached.pieces {
        let vertices = &cached.vertices[range.start as usize..range.end as usize];
        let m = vertices.len();
        piece.clear();
        labels.clear();
        for j in 0..m {
            // Walked backwards when seen in an odd number of mirrors, like the polygon: edge
            // `j` then runs back along the piece's edge `m - 2 - j`.
            let (v, label) = if reversed {
                (vertices[m - 1 - j], vertices[(2 * m - 2 - j) % m].edge)
            } else {
                (vertices[j], vertices[j].edge)
            };
            piece.extend_from_slice(&space.to_clip(v.world).to_array());
            piece.extend_from_slice(&v.world.to_array());
            labels.push(label);
        }
        let (clipped, clip_edges) = receiving.clipper.clip(&piece, STRIDE, receiving.planes);
        if clipped.is_empty() {
            continue;
        }
        let count = clipped.len() / STRIDE;
        let center = clipped
            .chunks_exact(STRIDE)
            .map(|r| Vec3::from_slice(&r[3..6]))
            .sum::<Vec3>()
            / count as f32;
        let first_vertex = out.shadow_vertices.len() as u32;
        for (j, (r, &edge)) in clipped.chunks_exact(STRIDE).zip(clip_edges).enumerate() {
            let (x, y) = to_screen(view, Vec3::from_slice(r));
            let line = match edge {
                Edge::Plane(k) => plane_lines.get(k as usize).copied(),
                Edge::Input(i) => match labels[i as usize] {
                    CachedEdge::Polygon(e) => polygon_line(e),
                    CachedEdge::Line(a, b) => line_through(view, space.to_clip(a), space.to_clip(b)),
                },
            };
            let p = Vec3::from_slice(&r[3..6]);
            let vertex = ShadowVertex {
                x,
                y,
                w: 1.0 / r[2],
                line,
                light: 0.0,
                ray: Vec3::ZERO,
                width: 0.0,
            };
            let light = |along: usize| {
                let along = Vec3::from_slice(&clipped[along * STRIDE + 3..][..3]);
                cached.light(soft, p, along, center)
            };
            let light = (light((j + count - 1) % count), light((j + 1) % count));
            push_shadow_vertex(out.shadow_vertices, vertex, light);
        }
        push_shadow_piece(
            out.shadow_vertices,
            out.shadow_pieces,
            ShadowPiece {
                slot,
                first_vertex,
                vertex_count: (out.shadow_vertices.len() as u32 - first_vertex) as u16,
                beam: None,
                occluded: false,
            },
        );
    }
}

/// Projects clipped records (exactly, no rounding) and appends them as one output polygon.
/// An edge that lies on one of the clip planes with a line in `plane_lines` (portal edges
/// and viewport borders, indexed like the planes), or that is part of a longer edge or a
/// shadow's cut ([`PieceEdge::Line`]), carries that line; see [`EdgeLine`].
#[allow(clippy::too_many_arguments)]
fn emit(
    out: &mut Out,
    view: &View,
    records: &[f32],
    edges: &[PieceEdge],
    plane_lines: &[EdgeLine],
    source_vertices: usize,
    id: MeshId,
    object: u32,
    kind: PolygonKind,
    source: PolygonSource,
    flags: PolyFlags,
    mirror: Option<u32>,
    shadowed: u32,
    (split, first_shadow, shadow_count): (u32, u32, u16),
) {
    let stride = RECORD + source_vertices;
    let first_vertex = out.vertices.len() as u32;
    let first_weight = out.weights.len() as u32;
    for (r, edge) in records.chunks_exact(stride).zip(edges) {
        let (x, y) = to_screen(view, Vec3::from_slice(r));
        out.vertices.push(ScreenVertex {
            x,
            y,
            w: 1.0 / r[2],
        });
        out.edge_lines.push(edge_line(view, edge, plane_lines));
        out.world_positions.push(Vec3::from_slice(&r[3..6]));
        out.weights.extend_from_slice(&r[RECORD..]);
    }
    out.polygons.push(ViewPolygon {
        first_vertex,
        vertex_count: (out.vertices.len() as u32 - first_vertex) as u16,
        first_weight,
        source_vertices: source_vertices as u16,
        mesh: id,
        object,
        kind,
        source,
        flags,
        mirror,
        shadowed,
        split,
        first_shadow,
        shadow_count,
        reflection: None,
    });
}

/// The line an edge is walked along (see [`EdgeLine`]), if not between its own vertices.
fn edge_line(view: &View, edge: &PieceEdge, plane_lines: &[EdgeLine]) -> Option<EdgeLine> {
    match *edge {
        PieceEdge::Clip(Edge::Plane(k)) => plane_lines.get(k as usize).copied(),
        PieceEdge::Clip(Edge::Input(_)) => None,
        PieceEdge::Line(a, b) => Some(line_between(view, a, b)),
    }
}

/// A shadow cast on a polygon: part of it (a convex polygon on it, counter-clockwise like
/// it) that a light's shadow covers, fully or softly. Its vertices (`vertex_count` of
/// `ViewGeometry::shadow_vertices` from `first_vertex`) carry how much of the light reaches
/// them, interpolated across it (perspective-correct).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowPiece {
    /// The light's shadow slot.
    pub slot: u8,
    pub first_vertex: u32,
    pub vertex_count: u16,
    /// Part of a beam (see `Light::beam`): the light's cone as `(scale, offset)` (see
    /// `Light::cone`), which fades it further at each pixel by the angle between the
    /// pixel's ray (its vertices' `ray`, interpolated) and the beam's axis (x).
    pub beam: Option<(f32, f32)>,
    /// In a blurred caster's hard shadow (see `ShadowKind::Blurred`): its vertices' light
    /// is what reaches past everything else (all of it, a beam's cone, or a soft edge's
    /// part), and the renderer blurs the caster's shadow over it.
    pub occluded: bool,
}

/// A vertex of a [`ShadowPiece`]: where it is on screen (exact, like [`ScreenVertex`]), the
/// line its edge leaving it is walked along (if not between its own vertices), and how
/// much of the light reaches it: 0 in full shadow, up to 1 where a soft edge starts.
///
/// A corner where an occluder's edge touches the surface has a value along each of its
/// edges (see the carver's `soft_values`), so it comes as two vertices in the same place:
/// the edge between them has no length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowVertex {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub line: Option<EdgeLine>,
    pub light: f32,
    /// In a beam's piece: the way from the light to it, in the beam's frame (x along its
    /// axis); zero in others.
    pub ray: Vec3,
    /// In a blurred caster's hard shadow (see `ShadowPiece::occluded`): how wide its soft
    /// edge would be here, in world units, square to the light's rays; 0 otherwise.
    pub width: f32,
}

/// Appends a shadow piece's vertex with how much of the light reaches it along the edges
/// arriving and leaving: as two vertices in the same place if they differ (the first ends
/// the edge arriving, the second starts the edge leaving).
fn push_shadow_vertex(
    out: &mut Vec<ShadowVertex>,
    vertex: ShadowVertex,
    (arriving, leaving): (f32, f32),
) {
    if (arriving - leaving).abs() > 1e-3 {
        out.push(ShadowVertex { light: arriving, line: None, ..vertex });
    }
    out.push(ShadowVertex { light: leaving, ..vertex });
}

/// How far apart the values at the ends of each triangle's far edge may be in a fan from a
/// corner with two values (see [`push_shadow_piece`]): the corner has halfway between
/// them, so a triangle's edges from it are off by at most half this near it.
const FAN_STEP: f32 = 0.125;

/// Pushes a shadow piece, its vertices `piece.first_vertex..` of `vertices`.
///
/// One with a corner with two values (where an occluder's edge touches the surface: see
/// [`push_shadow_vertex`]) is pushed as a fan of triangles from that corner instead: the
/// rasterizer interpolates a piece's values down its edges, then across each row, so the
/// corner's rows would split it, the rows above taking one of its values and the rows
/// below the other, across all of it (a hard line level with the corner). What reaches
/// the surface is the same along each ray out from the corner, and in a fan each triangle
/// gets one value at the corner: halfway between those at its far corners, which are
/// close (the far edges are split so, see `FAN_STEP`).
fn push_shadow_piece(vertices: &mut Vec<ShadowVertex>, pieces: &mut Vec<ShadowPiece>, piece: ShadowPiece) {
    let first = piece.first_vertex as usize;
    let m = vertices.len() - first;
    let v = &vertices[first..];
    // The corner: a vertex ending the edge arriving, then one in the same place starting
    // the edge leaving, with another value.
    let twofold = |j: usize| {
        let (a, b) = (v[j], v[(j + 1) % m]);
        a.line.is_none() && (a.x, a.y) == (b.x, b.y) && (a.light - b.light).abs() > 1e-3
    };
    let mut corners = (0..m).filter(|&j| twofold(j));
    let (Some(arriving), None) = (corners.next(), corners.next()) else {
        pieces.push(piece);
        return;
    };
    if m < 4 {
        pieces.push(piece);
        return;
    }
    let corner = v[(arriving + 1) % m];
    // The far corners, from the one after it round to the one before, with more along
    // each edge between them where their values are far apart: at even steps along it in
    // the world (perspective-correct, as the rasterizer interpolates along it).
    let mut far: Vec<ShadowVertex> = Vec::with_capacity(m + 8);
    let ends: Vec<ShadowVertex> = (2..m).map(|k| v[(arriving + k) % m]).collect();
    for pair in ends.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        far.push(a);
        let steps = ((a.light - b.light).abs() / FAN_STEP).ceil().clamp(1.0, 8.0) as u32;
        let (za, zb) = (1.0 / a.w, 1.0 / b.w);
        for step in 1..steps {
            let t = step as f32 / steps as f32;
            let z = za + (zb - za) * t;
            far.push(ShadowVertex {
                x: (a.x * za * (1.0 - t) + b.x * zb * t) / z,
                y: (a.y * za * (1.0 - t) + b.y * zb * t) / z,
                w: 1.0 / z,
                line: a.line,
                light: a.light + (b.light - a.light) * t,
                ray: a.ray + (b.ray - a.ray) * t,
                width: a.width + (b.width - a.width) * t,
            });
        }
    }
    far.push(ends[ends.len() - 1]);
    vertices.truncate(first);
    // The edges out from the corner between triangles, walked along the same line by both.
    let spoke = |p: &ShadowVertex| Some(EdgeLine::between((corner.x, corner.y), (p.x, p.y)));
    let last = far.len() - 1;
    for j in 0..last {
        let (a, b) = (far[j], far[j + 1]);
        let first_vertex = vertices.len() as u32;
        vertices.push(ShadowVertex {
            light: (a.light + b.light) * 0.5,
            line: if j == 0 { corner.line } else { spoke(&a) },
            ..corner
        });
        vertices.push(a);
        vertices.push(ShadowVertex {
            line: if j + 1 == last { b.line } else { spoke(&b) },
            ..b
        });
        pieces.push(ShadowPiece {
            first_vertex,
            vertex_count: 3,
            ..piece
        });
    }
}

/// The four side planes of the view frustum in clip space: x = -w, x = w, y = -w, y = w.
fn frustum_planes() -> [ClipPlane; 4] {
    [
        ClipPlane::frustum_x(-1.0),
        ClipPlane::frustum_x(1.0),
        ClipPlane::frustum_y(-1.0),
        ClipPlane::frustum_y(1.0),
    ]
}

/// True if a clipped window subtends less than `MIN_EDGE_ANGLE` in every direction: too
/// small to see anything through.
fn window_too_small(points: &[Vec3]) -> bool {
    (0..points.len()).all(|i| {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        a.cross(b).length() < MIN_EDGE_ANGLE * a.length() * b.length()
    })
}

fn outline<'a>(geometry: &'a Mesh, portal: &'a Portal) -> impl Iterator<Item = Vec3> + Clone + 'a {
    portal
        .positions
        .iter()
        .map(|&i| geometry.positions[i as usize])
}

/// What the view blockers hide from one eye: for each blocker polygon not edge-on to it,
/// the region behind the polygon within the planes through the eye and its edges (planes
/// keeping `normal · p + d > 0`).
#[derive(Default)]
struct Blockers {
    planes: Vec<(Vec3, f32)>,
    volumes: Vec<Range<usize>>,
}

/// How far inside a blocker's region a point must be to count as hidden, in meters.
const BLOCKED: f32 = 1e-3;

impl Blockers {
    fn build(&mut self, polygons: &[Vec<Vec3>], eye: Vec3) {
        self.planes.clear();
        self.volumes.clear();
        for points in polygons {
            if points.len() < 3 {
                continue;
            }
            let center = points.iter().copied().sum::<Vec3>() / points.len() as f32;
            let Some(normal) = (1..points.len() - 1)
                .map(|i| (points[i] - points[0]).cross(points[i + 1] - points[0]))
                .sum::<Vec3>()
                .try_normalize()
            else {
                continue;
            };
            // Either side hides: the far side from the eye.
            let side = normal.dot(eye - center);
            if side.abs() < BLOCKED {
                continue;
            }
            let start = self.planes.len();
            let behind = if side > 0.0 { -normal } else { normal };
            self.planes.push((behind, -behind.dot(center)));
            for i in 0..points.len() {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                let Some(n) = (a - eye).cross(b - eye).try_normalize() else {
                    continue;
                };
                let n = if n.dot(center - eye) < 0.0 { -n } else { n };
                self.planes.push((n, -n.dot(eye)));
            }
            self.volumes.push(start..self.planes.len());
        }
    }

    /// Whether `points` (the corners of something convex) all lie behind one blocker.
    fn hides(&self, points: &[Vec3]) -> bool {
        self.volumes.iter().any(|v| {
            self.planes[v.clone()]
                .iter()
                .all(|&(n, d)| points.iter().all(|&p| n.dot(p) + d > BLOCKED))
        })
    }
}

fn box_corners(min: Vec3, max: Vec3) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        )
    })
}

/// True if `p`, on the plane of a convex polygon, lies inside its outline or on its edge.
fn inside_outline(points: impl Iterator<Item = Vec3> + Clone, normal: Vec3, p: Vec3) -> bool {
    let pts: Vec<Vec3> = points.collect();
    (0..pts.len()).all(|i| {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        (b - a).cross(p - a).dot(normal) >= -ON_PORTAL * (b - a).length()
    })
}
