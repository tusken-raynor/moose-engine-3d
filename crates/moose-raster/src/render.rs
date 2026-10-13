//! The two-phase frame: polygon setup and band binning (phase 1), then per-row span
//! resolution and shading (phase 2). See the span buffer module spec.

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use glam::{Vec2, Vec3};
use moose_assets::{Assets, MeshId, Light, Texture, TextureId};
use moose_scene::Viewport;
use moose_view::{
    EdgeLine, Object, PolygonKind, ScreenVertex, ViewGeometry, ViewPolygon, pixel_edge,
    ShadowPiece,
};
use rayon::prelude::*;

use crate::shader::{
    F32s, Fill,
    Behind, Draw, FACE_BITANGENT, FACE_NORMAL, FACE_TANGENT, LANES, LOD, MAX_STEP, MAX_TEXTURES, MAX_VARYINGS, MAX_SPLIT, NO_SPLIT, split_outputs,
    LIGHT, Material, MaterialEntry, MaterialId, NORMAL, POSITION, Params, SampleContext, SpanJob, TextureSet,
    U32s, UV, VertexContext, blend, blend_lanes, layout_len,
};

/// How an opaque polygon is resolved against others on a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterPath {
    /// Edge-walked into the row span list, resolved by sorted insertion.
    Span,
    /// Edge-walked and z-tested per pixel into the visibility buffer.
    PerPixel,
}

#[derive(Clone, Copy, Debug)]
pub struct RasterConfig {
    /// Path for actor meshes: per-pixel by default; `Span` routes them like props.
    pub actor_path: RasterPath,
    /// Rows per band handed to a thread in phase 2.
    pub band_rows: u32,
    /// Relative change of w allowed across one sample interval along a span.
    pub step_threshold: f32,
    /// Smallest grid spacing, in pixels, both ways (1 to 32; powers of two make the most
    /// sense): steep surfaces, where perspective alone would ask for sample points closer
    /// together, get them at most this close. Only a surface so steep that w would change by
    /// more than a quarter across such a cell (seen nearly edge-on) goes closer. Wider saves
    /// sample work (and lighting) on steep surfaces, at the cost of perspective accuracy on
    /// them.
    pub min_step: u32,
    /// How much w may change (relative to its smallest value) across a cell held to
    /// `min_step` before the cell is made smaller anyway: a surface seen so nearly edge-on
    /// that it would change more gets finer spacing. Infinity always holds to `min_step`,
    /// whatever it costs such surfaces in perspective accuracy.
    pub steep_limit: f32,
    /// Color for pixels no polygon covers.
    pub background: u32,
    /// Polygons per frame from which phase 1 (setup and binning) is split across threads.
    /// Below it, one thread does it: waking the pool costs tens of microseconds, far more
    /// than setting up a few hundred polygons.
    pub parallel_setup: usize,
    /// How much a spot light's cone may fade (its factor, from 1 inside to 0 outside) across
    /// one cell of sample points in a tile its penumbra crosses: such tiles space their
    /// points so that it fades by at most this across each cell, but no closer than
    /// `penumbra_spacing`. A narrow penumbra (or one seen up close) gets close points, a
    /// wide soft one keeps wide cells. 0 turns it off.
    pub penumbra_threshold: f32,
    /// The closest sample points `penumbra_threshold` asks for, in pixels.
    pub penumbra_spacing: u32,
    /// How far past each side of a tile the fade is measured, in pixels: a margin that
    /// catches penumbras crossing just a tile's corner.
    pub penumbra_padding: u32,
    /// Largest spacing of sample points on a lit polygon (one some light reaches), in
    /// pixels, in both directions: lighting is evaluated at sample points and interpolated
    /// between them. Materials and perspective may ask for less.
    pub light_spacing: u32,
    /// Probe and refine: how far off (in levels of 255, of the encoded light) a lit
    /// polygon's light may be in the middle of its part of a tile, interpolated from that
    /// part's corners, before the tile's sample points are moved closer together (see
    /// [`probe_tiles`]). The light is probed there, whatever it is of: no light's kind or
    /// place is looked at. 0 turns it off.
    pub refine_threshold: f32,
    /// The closest sample points `refine_threshold` asks for, in pixels.
    pub refine_spacing: u32,
    /// Debug overlay of the sample lattice: grid rows tinted red, and each row's sample
    /// points marked: grid points green, edges' knots yellow, vertices white.
    pub show_samples: bool,
    /// Beams' fades (see `Light::beam`) are drawn as a stipple, each pixel lit or not by a
    /// 4×4 ordered dither: a retro look, and cheap, as no pixel needs a partial light.
    pub beam_dither: bool,
    /// Pixels per cell of the grid blurred casters' shadows are blurred on (see
    /// [`BlurGrid`]), each way: its resolution.
    pub blur_scale: u32,
    /// Blur grid cells twice as wide as they are tall (half the columns), like the
    /// half-rate shading of reflections.
    pub blur_half_rate: bool,
    /// Edges between surfaces smoothed: each pixel an edge passes through takes some of its
    /// neighbor across the edge, as much as the edge leaves on that side (see
    /// [`find_edges`]). Only where the surfaces differ (not between pieces of one surface),
    /// and not in reflections, per-pixel actors or translucent surfaces.
    pub smooth_edges: bool,
}

impl Default for RasterConfig {
    fn default() -> Self {
        Self {
            actor_path: RasterPath::PerPixel,
            band_rows: 8,
            step_threshold: 1.0 / 16.0,
            min_step: 4,
            steep_limit: 0.25,
            background: 0,
            parallel_setup: 1024,
            light_spacing: 32,
            refine_threshold: 2.0,
            refine_spacing: 4,
            penumbra_threshold: 0.125,
            penumbra_spacing: 4,
            penumbra_padding: 16,
            show_samples: false,
            beam_dither: false,
            blur_scale: 8,
            blur_half_rate: false,
            smooth_edges: false,
        }
    }
}

/// What a polygon is drawn with.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub material: MaterialId,
    pub params: Params,
    /// Forces a raster path for this polygon's mesh, overriding its kind (not world).
    pub path_override: Option<RasterPath>,
    /// The textures the material samples (see [`TextureSet`]); unbound slots get a 1x1
    /// white one.
    pub textures: [Option<TextureId>; MAX_TEXTURES],
    /// Each texture slot's sampler (one of [`filter::ALL`](crate::shader::filter::ALL)),
    /// which the material reads it with: picked at run time, per polygon.
    pub filters: [u8; MAX_TEXTURES],
    /// How far behind its plane a light may be and still reach it, as the sine of the angle
    /// (0, the default: none). Such a light lights none of the flat surface, but bump-mapped
    /// programs' normals tilted toward it catch it, fading from all of it at the plane to
    /// none at the angle (a rounded edge's normals, past where the face turns away).
    pub back_light: f32,
    /// The lights it isn't lit by (see [`Light::excluded_by`]): left out of its lights,
    /// so none of their light, bumps, highlights or shadows reach it (0, the default:
    /// none). It still casts their shadows.
    pub excluded_lights: moose_assets::LightMask,
}

impl Surface {
    pub fn new(material: MaterialId) -> Self {
        Self {
            material,
            params: Params::default(),
            path_override: None,
            textures: [None; MAX_TEXTURES],
            filters: [crate::shader::filter::BILINEAR_MIPMAP_LINEAR; MAX_TEXTURES],
            back_light: 0.0,
            excluded_lights: 0,
        }
    }
}

/// A mesh lacks what a material needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    MissingAttribute {
        mesh: String,
        name: &'static str,
    },
    CountMismatch {
        mesh: String,
        name: &'static str,
        needed: u8,
        found: u8,
    },
    TooManyVaryings {
        mesh: String,
    },
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAttribute { mesh, name } => {
                write!(f, "mesh '{mesh}' has no attribute '{name}'")
            }
            Self::CountMismatch {
                mesh,
                name,
                needed,
                found,
            } => {
                write!(
                    f,
                    "mesh '{mesh}' attribute '{name}' has {found} components, the material needs {needed}"
                )
            }
            Self::TooManyVaryings { mesh } => write!(
                f,
                "material for mesh '{mesh}' has more than {MAX_VARYINGS} values of one kind"
            ),
        }
    }
}

impl std::error::Error for LayoutError {}

/// A framebuffer: `width * height` 32-bit XRGB pixels, row by row.
pub struct Target<'a> {
    pub pixels: &'a mut [u32],
    pub width: u32,
    pub height: u32,
}

/// Surface IDs are (thread, polygon index in that thread's arena).
const LOCAL_BITS: u32 = 26;
const MAX_THREADS: usize = 1 << (32 - LOCAL_BITS);
const EMPTY: u32 = u32::MAX;

fn surface_id(thread: usize, local: usize) -> u32 {
    debug_assert!(
        local < 1 << LOCAL_BITS,
        "too many polygons for one thread in one frame"
    );
    (thread as u32) << LOCAL_BITS | local as u32
}

fn split_id(id: u32) -> (usize, usize) {
    (
        (id >> LOCAL_BITS) as usize,
        (id & ((1 << LOCAL_BITS) - 1)) as usize,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    World,
    Span,
    PerPixel,
    Translucent,
}

#[derive(Clone, Copy)]
struct SetupVertex {
    x: f32,
    y: f32,
    w: f32,
}

/// A polygon after phase 1: screen vertices, its material's `sampled` values at each, row
/// range and sort key.
struct PolygonSetup {
    first_vertex: u32,
    vertex_count: u16,
    n_vals: u16,
    /// Lights whose shadows cover part of it (see `ViewPolygon::split`): each one's
    /// strength follows the material's outputs at its sample points (1 value; counted in
    /// `n_out`), its light being its color (`split_colors`) times that, and its shadow
    /// pieces (`shadows` from `first_shadow` in `ThreadBins::shadows`) say how much of it
    /// reaches each pixel.
    splits: u8,
    split_colors: [Vec3; MAX_SPLIT],
    /// Which split light is the player's flashlight, if one is.
    split_flashlight: Option<u8>,
    /// Each split light's shadow slot.
    split_slots: [u8; MAX_SPLIT],
    first_shadow: u32,
    shadows: u16,
    /// Its material's outputs per sample point.
    n_out: u16,
    /// Its plane functions in `ThreadBins::planes` (see [`fan_planes`]), and its fan's
    /// triangle count.
    first_plane: u32,
    fans: u16,
    row_top: i32,
    row_end: i32,
    /// Nearest point: front-to-back sort key.
    max_w: f32,
    material: MaterialId,
    params: Params,
    textures: [Option<TextureId>; MAX_TEXTURES],
    filters: [u8; MAX_TEXTURES],
    /// What its sample stage sees: the eye of the space it is seen in, and its object.
    eye: Vec3,
    object: Object,
    /// Seen in a mirror: shaded at half horizontal rate (every other pixel, repeated in the
    /// next), with sample points half as often. Which polygon owns each pixel, and so every
    /// edge, stays at full resolution; the shiny surface drawn over a reflection softens it
    /// anyway.
    half_rate: bool,
    /// The lights that reach it, in `ThreadBins::lights`.
    first_light: u32,
    light_count: u16,
    /// The shadow slots of the lights it is in the full shadow of (see
    /// `ViewPolygon::shadowed`).
    shadowed: u32,
    /// Where its `sampled` world position is among its sampled values, if it has one.
    position: Option<u16>,
    /// Where its `sampled` `uv` and level of detail are, and texture 0's size in texels,
    /// if it has a level of detail: its sample points work theirs out exactly (see
    /// [`point_lods`]).
    lod: Option<(u16, u16, f32, f32)>,
    /// Its surface's back light (see [`Surface::back_light`]).
    back: f32,
    /// Where its material's `light` output is among its outputs (see [`LIGHT`]), if it has
    /// one and is lit: its tiles are probed and refined by it (see [`probe_tiles`]).
    light_out: Option<u16>,
    /// Its tiles' refined spacings in `ThreadBins::refine`: one per block of rows it is in
    /// (from `refine_block`) and column strip (`refine_strips` of them, from
    /// `refine_strip`), block by block.
    refine: u32,
    refine_block: i32,
    refine_strip: i32,
    refine_strips: u16,
}

#[derive(Default)]
struct BandBins {
    world: Vec<u32>,
    span: Vec<u32>,
    pixel: Vec<u32>,
    translucent: Vec<u32>,
}

/// One thread's phase 1 output. Written only by that thread in phase 1, read by all in
/// phase 2.
#[derive(Default)]
struct ThreadBins {
    polygons: Vec<PolygonSetup>,
    vertices: Vec<SetupVertex>,
    lines: Vec<Option<EdgeLine>>,
    /// Scratch for one polygon's `sampled` values at each vertex, until its planes are made.
    values: Vec<f32>,
    bands: Vec<BandBins>,
    /// Scratch for one polygon's vertex levels of detail.
    lods: Vec<f32>,
    /// Scratch for one polygon's vertex stage outputs at its source vertices.
    vertex_out: Vec<f32>,
    /// Every polygon's plane functions (see [`fan_planes`]).
    planes: Vec<f32>,
    /// Every probed polygon's tiles' refined spacings (see `PolygonSetup::refine`), worked
    /// out by the first band to shade the tile and read by the rest, so they agree: 0 not
    /// yet, 1 as it is, else 2 more than the log2 of the spacing. Whichever band works one
    /// out gets the same answer (it is the polygon's and the tile's alone).
    refine: Vec<AtomicU8>,

    /// Each polygon's lights (see `PolygonSetup::first_light`), and per light, its split
    /// index ([`NO_SPLIT`] for none; see `SampleContext::light_split`).
    lights: Vec<Light>,
    light_split: Vec<u8>,
    /// Polygons' shadow pieces; their vertices, edge lines, how much of the light reaches
    /// each vertex, and each vertex's ray (in a beam's pieces; see `ShadowVertex::ray`).
    shadows: Vec<ShadowSetup>,
    shadow_vertices: Vec<SetupVertex>,
    shadow_lines: Vec<Option<EdgeLine>>,
    shadow_light: Vec<f32>,
    shadow_rays: Vec<Vec3>,
    shadow_width: Vec<f32>,
    /// Each vertex's world position, and soft pieces' soft edges and their wedges (see
    /// `moose_view::soft_reach`).
    shadow_world: Vec<Vec3>,
    shadow_softs: Vec<moose_view::ShadowSoft>,
    shadow_wedges: Vec<moose_view::ShadowWedge>,
}

/// A polygon's shadow piece, binned: its vertices (`count` from `first`), its light's split
/// index, its beam's cone, if it is part of one (see `ShadowPiece::beam`), and the rows
/// its edges cross (`row_top..row_end`: no other row is in it).
#[derive(Clone, Copy)]
struct ShadowSetup {
    first: u32,
    count: u16,
    split: u8,
    beam: Option<(f32, f32)>,
    row_top: i32,
    row_end: i32,
    /// See `ShadowPiece::occluded`.
    occluded: bool,
    /// Its soft edges (`ThreadBins::shadow_softs`), if it is in any: how much of the light
    /// reaches each pixel is worked out there.
    softs: (u32, u32),
    /// In full shadow (all dark, in no soft edge, not blurred): its edges are hard, and
    /// smoothed (see [`smooth_shadow_edges`]).
    dark: bool,
}

/// A piece of a polygon's row: pixels `x0..x1`, and w as a linear function of x.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    x0: i32,
    x1: i32,
    /// w at the anchor x (a screen position, not a pixel), and its change per pixel.
    xa: f32,
    wa: f32,
    dwdx: f32,
    /// Index into the row's polygon states.
    state: u32,
}

impl Span {
    #[inline(always)]
    fn w(&self, px: i32) -> f32 {
        self.wa + self.dwdx * (px as f32 + 0.5 - self.xa)
    }

    fn piece(&self, x0: i32, x1: i32) -> Span {
        Span { x0, x1, ..*self }
    }
}

/// Where a polygon's edges cross one row, and w there.
struct RowState {
    id: u32,
    /// The polygon's slot in the band (its lattice's index).
    slot: u32,
    x_left: f32,
    x_right: f32,
    w_left: f32,
    w_right: f32,
    /// The edges crossed (for smoothing them; see [`find_edges`]).
    left_edge: EdgeSlope,
    right_edge: EdgeSlope,
}

/// A polygon edge crossing a row: x per row along it, and the rows it spans (y, top to
/// bottom).
#[derive(Clone, Copy, Debug, Default)]
struct EdgeSlope {
    dxdy: f32,
    top: f32,
    bottom: f32,
}

/// A full shadow piece's edge crossing a row (see [`smooth_shadow_edges`]): its split
/// light, its x at the row's center, its slope and rows, and which way is out of the
/// shadow (1 right, -1 left).
#[derive(Clone, Copy, Debug)]
struct ShadowEdge {
    split: u8,
    x: f32,
    slope: EdgeSlope,
    side: f32,
}

/// Which neighbor a pixel an edge passes through takes some of: the one across the edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Across {
    Left,
    Right,
    Up,
    Down,
}

/// A pixel of a row that an edge passes through (from the viewport's left), and how much
/// of its neighbor across the edge it takes, in 256ths.
#[derive(Clone, Copy, Debug)]
struct EdgeBlend {
    px: u32,
    across: Across,
    k: u32,
}

/// A band's edge pixels that take some of a row in the next band or the one before, done
/// once every band is drawn: its first and last rows as shaded (before smoothing, for the
/// bands next to it), and the pixels of those rows that take from above and below them.
#[derive(Default)]
struct Seam {
    first: Vec<u32>,
    last: Vec<u32>,
    up: Vec<(u32, u32)>,
    down: Vec<(u32, u32)>,
}

/// One thread's phase 2 scratchpad, sized from the viewport width.
#[derive(Default)]
struct RowScratch {
    /// How much of each split light reaches each pixel of the run being shaded (see
    /// `SpanJob::reaches`).
    reaches: Vec<f32>,
    /// A soft shadow piece's light across its stretch of the row (see `shadow_run`).
    soft_row: Vec<f32>,
    /// The edges of full shadow pieces crossing the run being shaded, to be smoothed (see
    /// [`smooth_shadow_edges`]).
    shadow_edges: Vec<ShadowEdge>,
    /// The framebuffer row being drawn.
    row: i32,
    color: Vec<u32>,
    vis_w: Vec<f32>,
    vis_state: Vec<u32>,
    spans: Vec<Span>,
    spans_next: Vec<Span>,
    states: Vec<RowState>,
    /// The viewport's corner, which the sample grid counts from, and the lattice blocks the
    /// band's rows touch (`blocks` of them, from `first_block`).
    viewport_x: i32,
    viewport_y: i32,
    first_block: i32,
    blocks: usize,
    /// The band's framebuffer rows: lattices build only the grid rows these need.
    band_rows: std::ops::Range<i32>,
    /// Per polygon slot of the band and block, its lattice (`slot * blocks + block`), and
    /// the lattices' tiles, grid rows and outputs.
    lattices: Vec<Lattice>,
    tiles: Vec<Tile>,
    grid_rows: Vec<GridRow>,
    grid_values: Vec<f32>,
    /// Where each grid row's points are down their columns (see `GridRow::ys`).
    grid_ys: Vec<f32>,
    /// The lattices' row ends: per row of the band in the block, its left and its right.
    row_ends: Vec<RowEnd>,
    /// One row's sample points for the run being shaded: x, outputs, and what each is (for
    /// the overlay; see [`POINT_GRID`]).
    points_x: Vec<f32>,
    points_v: Vec<f32>,
    points_kind: Vec<u8>,
    /// The tiles' runs of the row's sample points, while `row_points` gathers them, and one
    /// run's weights between its grid rows, column by column.
    segments: Vec<Segment>,
    weights: Vec<f32>,
    /// While a lattice is built: the polygon's first and last pixel on each of the band's
    /// rows in the block and the edges it crosses there, and the sample points waiting to
    /// be evaluated.
    row_pixels: Vec<(i32, i32)>,
    row_edges: Vec<(u16, u16)>,
    pending: Vec<Pending>,
    /// One edge's knots in the block, in order along it (how far from its first vertex to
    /// its second, 0 to 1), and where each one's outputs are in `grid_values` (once a row
    /// needs it); and where each vertex's are.
    knots: Vec<f32>,
    knot_values: Vec<u32>,
    vertex_values: Vec<u32>,
    /// Each strip's column and row spacing and its least w, while its lattice is built.
    spacings: Vec<(i32, i32, f32)>,
    /// While a polygon's tiles are probed (see [`probe_tiles`]): its part of a tile (and
    /// the clipping's other buffer), the probe points and their outputs, each probed
    /// part's corners among the points (neighboring parts share theirs), and each probed
    /// strip (the strip, its first corner in `probe_corners`, how many, the point in its
    /// middle, and its part's size).
    piece: Vec<Vec2>,
    piece_next: Vec<Vec2>,
    probes: Vec<Pending>,
    probe_values: Vec<f32>,
    probe_corners: Vec<u32>,
    probed: Vec<(usize, usize, usize, usize, f32)>,
    world: Vec<u32>,
    span: Vec<u32>,
    pixel: Vec<u32>,
    pixel_spans: Vec<Span>,
    translucent: Vec<u32>,
    translucent_spans: Vec<Span>,
    /// Where one translucent span is in front: pixels, and the opaque span behind (if any).
    visible: Vec<(i32, i32, Option<u32>)>,
    blend: Vec<u32>,
    /// How much of each split light the run's pixels get past occluders' hard shadows to
    /// be blurred (see `ShadowPiece::occluded`): 1 or 0, until softened.
    occluded: Vec<f32>,
    /// The blur grid's columns of cells under a run (see `BlurGrid::soften`).
    blur_cells: Vec<(f32, f32, f32)>,
    /// Edge smoothing (see `RasterConfig::smooth_edges`): the row above and this row as
    /// shaded, before smoothing; this row's edge pixels; and the row above's that take from
    /// this one, waiting for it.
    edge_above: Vec<u32>,
    edge_here: Vec<u32>,
    edge_blends: Vec<EdgeBlend>,
    edge_down: Vec<(u32, u32)>,
}

/// Counts and timings from the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub polygons: u32,
    pub bands: u32,
    pub threads: u32,
    /// Wall time of each part of the frame: surfaces and validation (one thread), phase 1
    /// (polygon setup and binning) and phase 2 (rows).
    pub prepare: Duration,
    pub setup: Duration,
    pub rows: Duration,
    /// Time threads spent drawing bands in phase 2, summed over all threads. Compared with
    /// `rows * threads`, the rest is waiting: for the slowest band, and to start and join.
    pub rows_busy: Duration,
    /// Building and blurring the shadow blur grid (see `RasterConfig::shadow_blur`).
    pub blur: Duration,
}

/// The span buffer renderer. Keeps its arenas and scratchpads between frames.
pub struct Renderer {
    pub config: RasterConfig,
    /// The frame's time in seconds, for pixels that move with it (see
    /// `PixelContext::time`); the app sets it each frame.
    pub time: f32,
    materials: Vec<MaterialEntry>,
    /// Per (mesh, material): where each vertex stage input comes from, and the `sampled`
    /// built-ins. Validated once, then reused.
    remaps: HashMap<(MeshId, MaterialId), Remap>,
    surfaces: Vec<Surface>,
    bins: Vec<ThreadBins>,
    scratch: Vec<Mutex<RowScratch>>,
    /// What untextured polygons sample: 1x1 opaque white.
    blank: Texture,
    blur: BlurGrid,
    /// Per band, its edge pixels waiting for the bands next to it.
    seams: Vec<Seam>,
}

/// The textures polygons sample, and the frame's focal length and ambient light; and the
/// shadow blur grid, if shadows are blurred.
#[derive(Clone, Copy)]
struct Textures<'a> {
    assets: &'a Assets,
    blank: &'a Texture,
    focal: f32,
    time: f32,
    ambient: Vec3,
    blur: Option<&'a BlurGrid>,
}

impl Textures<'_> {
    fn get(&self, id: Option<TextureId>) -> &Texture {
        id.map_or(self.blank, |id| self.assets.texture(id))
    }

    fn set(&self, ids: [Option<TextureId>; MAX_TEXTURES]) -> TextureSet<'_> {
        ids.map(|id| self.get(id))
    }
}

impl Renderer {
    pub fn new(config: RasterConfig) -> Self {
        Self {
            config,
            time: 0.0,
            materials: Vec::new(),
            remaps: HashMap::new(),
            surfaces: Vec::new(),
            bins: Vec::new(),
            scratch: Vec::new(),
            blank: Texture::solid("blank", 0xFFFF_FFFF),
            blur: BlurGrid::default(),
            seams: Vec::new(),
        }
    }

    pub fn register_material<M: Material>(&mut self) -> MaterialId {
        self.materials.push(MaterialEntry::of::<M>());
        MaterialId((self.materials.len() - 1) as u16)
    }

    /// Draws one view's geometry into `viewport` of `target`. `surface` says what each
    /// polygon is drawn with; every (mesh, material) pair is validated before drawing.
    pub fn render(
        &mut self,
        target: &mut Target,
        viewport: Viewport,
        geometry: &ViewGeometry,
        assets: &Assets,
        surface: impl Fn(&ViewPolygon) -> Surface,
    ) -> Result<RenderStats, LayoutError> {
        let started = Instant::now();
        assert!(
            viewport.x + viewport.width <= target.width
                && viewport.y + viewport.height <= target.height,
            "viewport outside the target"
        );
        // Surfaces, and validation of each (mesh, material) pair once.
        self.surfaces.clear();
        for p in &geometry.polygons {
            let s = surface(p);
            if !self.remaps.contains_key(&(p.mesh, s.material)) {
                let remap = remap(assets, p.mesh, &self.materials[s.material.0 as usize])?;
                self.remaps.insert((p.mesh, s.material), remap);
            }
            self.surfaces.push(s);
        }

        let threads = rayon::current_num_threads().clamp(1, MAX_THREADS);
        let band_rows = self.config.band_rows.max(1) as i32;
        let bands = (viewport.height as i32 + band_rows - 1) / band_rows;
        self.bins.resize_with(threads, ThreadBins::default);
        self.scratch
            .resize_with(threads, || Mutex::new(RowScratch::default()));

        let textures = Textures {
            assets,
            blank: &self.blank,
            focal: geometry.focal,
            time: self.time,
            ambient: geometry.ambient,
            blur: None,
        };
        let setup_started = Instant::now();
        // ---- Phase 1: polygon setup and binning, split by polygon. Each thread writes only
        // its own arena and bins.
        // Small frames are set up on this thread, into the first thread's arena.
        let parallel = geometry.polygons.len() >= self.config.parallel_setup;
        let per_thread = if parallel {
            geometry.polygons.len().div_ceil(threads)
        } else {
            geometry.polygons.len()
        };
        let (surfaces, remaps, shaders, config) =
            (&self.surfaces, &self.remaps, &self.materials, &self.config);
        let set_up = |(t, bins): (usize, &mut ThreadBins)| {
            bins.polygons.clear();
            bins.vertices.clear();
            bins.lines.clear();
            bins.values.clear();
            bins.planes.clear();
            bins.refine.clear();
            bins.lights.clear();
            bins.light_split.clear();
            bins.shadows.clear();
            bins.shadow_vertices.clear();
            bins.shadow_lines.clear();
            bins.shadow_light.clear();
            bins.shadow_rays.clear();
            bins.shadow_width.clear();
            bins.shadow_world.clear();
            bins.shadow_softs.clear();
            bins.shadow_wedges.clear();
            bins.bands.resize_with(bands as usize, BandBins::default);
            for band in &mut bins.bands {
                band.world.clear();
                band.span.clear();
                band.pixel.clear();
                band.translucent.clear();
            }
            let start = (t * per_thread).min(geometry.polygons.len());
            let end = ((t + 1) * per_thread).min(geometry.polygons.len());
            for (p, s) in geometry.polygons[start..end]
                .iter()
                .zip(&surfaces[start..end])
            {
                let remap = &remaps[&(p.mesh, s.material)];
                setup_polygon(
                    bins, t, geometry, p, s, remap, shaders, config, &textures, viewport, band_rows,
                );
            }
        };
        if parallel {
            self.bins.par_iter_mut().enumerate().for_each(set_up);
        } else {
            self.bins.iter_mut().enumerate().for_each(set_up);
        }

        let blur_started = Instant::now();
        // Blurred casters' shadows (see `ShadowPiece::occluded`), if any are in view.
        let blur_on = self.bins.iter().any(|b| b.shadows.iter().any(|piece| piece.occluded))
            && self.config.blur_scale > 0
            && band_rows % self.config.blur_scale as i32 == 0;
        if blur_on {
            self.blur.build(&self.bins, &self.scratch, &self.config, viewport, band_rows, geometry.focal);
        }
        let textures = Textures {
            blur: blur_on.then_some(&self.blur),
            ..textures
        };
        let rows_started = Instant::now();
        // ---- Phase 2: rows, split into bands. Each thread owns its band's rows.
        let width = target.width as usize;
        let busy = AtomicU64::new(0);
        let rows = &mut target.pixels
            [viewport.y as usize * width..(viewport.y + viewport.height) as usize * width];
        let (bins, scratch, shaders) = (&self.bins, &self.scratch, &self.materials);
        self.seams.resize_with(bands as usize, Seam::default);
        let seams = &mut self.seams[..bands as usize];
        rows.par_chunks_mut(band_rows as usize * width)
            .zip(seams.par_iter_mut())
            .enumerate()
            .for_each(|(b, (chunk, seam))| {
                let band_started = Instant::now();
                let t = rayon::current_thread_index().unwrap_or(0) % scratch.len();
                let mut s = scratch[t].lock().unwrap();
                render_band(
                    &mut s, bins, shaders, config, &textures, viewport, b, band_rows, chunk, width,
                    seam,
                );
                busy.fetch_add(band_started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            });
        if config.smooth_edges {
            join_seams(seams, rows, width, viewport, band_rows as usize);
        }
        let done = Instant::now();

        Ok(RenderStats {
            polygons: self.bins.iter().map(|b| b.polygons.len() as u32).sum(),
            bands: bands as u32,
            threads: threads as u32,
            prepare: setup_started - started,
            setup: blur_started - setup_started,
            blur: rows_started - blur_started,
            rows: done - rows_started,
            rows_busy: Duration::from_nanos(busy.into_inner()),
        })
    }
}

/// A polygon's tangent and bitangent, in model space: the directions its `uv` attribute's
/// u and v grow along it, each unit length (zero without `uv`, or for a polygon whose
/// texture is degenerate on it). From the triangle of its fan (its first vertex and two
/// neighbors) whose texture is least degenerate: its first three vertices can lie in a
/// line (a corner added on an edge), which tells nothing across it.
fn face_tangents(mesh: &moose_assets::Mesh, polygon: &moose_assets::Polygon) -> (Vec3, Vec3) {
    let Some(uv) = mesh.attribs.iter().find(|a| a.name == UV && a.count == 2) else {
        return (Vec3::ZERO, Vec3::ZERO);
    };
    let vs: Vec<usize> = polygon.vertices().collect();
    if vs.len() < 3 {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let p = |v: usize| mesh.positions[mesh.vertex_positions[v] as usize];
    let t = |v: usize| (uv.data.get_f32(v * 2), uv.data.get_f32(v * 2 + 1));
    // Per fan triangle (0, k, k + 1): its texture's determinant; the largest.
    let det = |k: usize| {
        let ((u0, v0), (u1, v1), (u2, v2)) = (t(vs[0]), t(vs[k]), t(vs[k + 1]));
        (u1 - u0) * (v2 - v0) - (u2 - u0) * (v1 - v0)
    };
    let k = (1..vs.len() - 1).max_by(|&a, &b| det(a).abs().total_cmp(&det(b).abs())).unwrap();
    let (e1, e2) = (p(vs[k]) - p(vs[0]), p(vs[k + 1]) - p(vs[0]));
    let ((u0, v0), (u1, v1), (u2, v2)) = (t(vs[0]), t(vs[k]), t(vs[k + 1]));
    let (du1, dv1, du2, dv2) = (u1 - u0, v1 - v0, u2 - u0, v2 - v0);
    let det = du1 * dv2 - du2 * dv1;
    if det.abs() < 1e-12 {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let tangent = (e1 * dv2 - e2 * dv1) / det;
    let bitangent = (e2 * du1 - e1 * du2) / det;
    (tangent.normalize_or_zero(), bitangent.normalize_or_zero())
}

/// Where a vertex stage input comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum VertexSource {
    /// A mesh attribute: its index in the mesh's attributes, its count in the mesh, and
    /// the count the material reads (components past the mesh's read 0).
    Attrib(usize, usize, usize),
    /// An attribute the mesh doesn't have: this many zeros (black, for a color), so a
    /// material on a mesh without what it reads still draws.
    Zero(usize),
    /// The source vertex's world position.
    Position,
    /// The source polygon's plane normal, in world space.
    FaceNormal,
    /// The source polygon's tangent and bitangent: which way, in world space, its `uv`
    /// attribute's u and v grow (see [`face_tangents`]).
    FaceTangent,
    FaceBitangent,
}

/// A material mapped onto a mesh.
#[derive(Clone, Debug)]
struct Remap {
    /// The vertex stage's inputs, in declaration order.
    vertex: Vec<VertexSource>,
    /// Offsets of the `sampled` built-ins: the world position, `uv` (what the level of
    /// detail is measured from) and the level of detail.
    position: Option<usize>,
    uv: Option<usize>,
    lod: Option<usize>,
    /// Offset of the `sampled` normal the material lights with (see [`NORMAL`]), if it has
    /// one.
    normal: Option<usize>,
    /// Offset of the material's `light` output among its outputs (see [`LIGHT`]), if it has
    /// one.
    light: Option<usize>,
}

/// Maps a material's vertex inputs onto a mesh's attributes by name, and finds its
/// `sampled` built-ins.
fn remap(assets: &Assets, mesh: MeshId, material: &MaterialEntry) -> Result<Remap, LayoutError> {
    let m = assets.mesh(mesh);
    let io = &material.io;
    if [io.vertex, io.sampled, io.interp]
        .iter()
        .any(|layout| layout_len(layout) > MAX_VARYINGS)
    {
        return Err(LayoutError::TooManyVaryings {
            mesh: m.name.clone(),
        });
    }
    let mismatch = |name, needed, found| LayoutError::CountMismatch {
        mesh: m.name.clone(),
        name,
        needed,
        found,
    };
    let mut vertex = Vec::new();
    for want in io.vertex {
        let builtin = match want.name {
            POSITION => Some(VertexSource::Position),
            FACE_NORMAL => Some(VertexSource::FaceNormal),
            FACE_TANGENT => Some(VertexSource::FaceTangent),
            FACE_BITANGENT => Some(VertexSource::FaceBitangent),
            _ => None,
        };
        if let Some(source) = builtin {
            if want.count != 3 {
                return Err(mismatch(want.name, want.count, 3));
            }
            vertex.push(source);
            continue;
        }
        // An attribute the mesh doesn't have reads 0; one with fewer components reads 0
        // past them (and with more, only those the material reads).
        vertex.push(match m.attribs.iter().position(|a| a.name == want.name) {
            Some(index) => VertexSource::Attrib(index, m.attribs[index].count as usize, want.count as usize),
            None => VertexSource::Zero(want.count as usize),
        });
    }
    let mut remap = Remap {
        vertex,
        position: None,
        uv: None,
        lod: None,
        normal: None,
        light: None,
    };
    let mut offset = 0;
    for out in io.interp {
        if out.name == LIGHT && out.count == 3 {
            remap.light = Some(offset);
        }
        offset += out.count as usize;
    }
    let mut offset = 0;
    for want in io.sampled {
        let expect = |count: u8| {
            if want.count == count {
                Ok(())
            } else {
                Err(mismatch(want.name, count, want.count))
            }
        };
        match want.name {
            POSITION => {
                expect(3)?;
                remap.position = Some(offset);
            }
            UV => {
                expect(2)?;
                remap.uv = Some(offset);
            }
            LOD => {
                expect(1)?;
                remap.lod = Some(offset);
            }
            NORMAL if want.count == 3 => remap.normal = Some(offset),
            _ => {}
        }
        offset += want.count as usize;
    }
    if remap.lod.is_some() && remap.uv.is_none() {
        return Err(LayoutError::MissingAttribute {
            mesh: m.name.clone(),
            name: UV,
        });
    }
    Ok(remap)
}

/// Which of a polygon's partly shadowing lights keep their shadows on it when it has room
/// for fewer (see `MAX_SPLIT`): the player's flashlight first, then by how much light each
/// brings to the polygon's `center` (facing `normal`): its brightness there, as the sample
/// points light it (falloff, angle, cone), a little at least while it reaches.
fn split_priority(light: &Light, center: Vec3, normal: Vec3) -> f32 {
    if light.id == moose_assets::FLASHLIGHT_ID {
        return f32::INFINITY;
    }
    let to = light.position - center;
    let d = to.length().max(1e-4);
    let t = (1.0 - (d * d) / (light.range * light.range)).max(0.0);
    let cos = (normal.dot(to) / d).max(0.05);
    let (scale, offset) = light.cone();
    let c = (offset - to.dot(light.direction) / d * scale).clamp(0.0, 1.0);
    let luma = 0.2126 * light.color.x + 0.7152 * light.color.y + 0.0722 * light.color.z;
    luma * t * t * cos * c * c * (3.0 - c - c)
}

/// Whether a light `height` in front of a polygon's plane (below 0, behind it) lights it:
/// in front, or behind by up to the polygon's back light angle (`back`, its sine; see
/// `Surface::back_light`) as seen from `far` away (as far as the polygon's farthest point
/// may be from the light, where the angle is smallest).
fn lights_face(height: f32, far: f32, back: f32) -> bool {
    height > 0.0 || (back > 0.0 && height > -back * far)
}

/// Whether any normal on a polygon faces a light at `light`, the polygon's vertices being at
/// `positions` and lit by `normals` of their own (a smooth mesh's), one each: whether some
/// vertex's normal faces the light as seen from some vertex. Every normal and position on
/// the polygon is a blend of its vertices', so where none does, no normal anywhere on it
/// faces the light: its cosine is 0 all over the polygon, and leaving the light out changes
/// nothing (sample points are all on the polygon). Each normal against the light from its
/// own vertex only would miss a light close to a big polygon.
fn normals_face(normals: impl Iterator<Item = Vec3>, positions: &[Vec3], light: Vec3) -> bool {
    normals.into_iter().any(|n| positions.iter().any(|&q| n.dot(light - q) > 0.0))
}

#[allow(clippy::too_many_arguments)]
fn setup_polygon(
    bins: &mut ThreadBins,
    thread: usize,
    geometry: &ViewGeometry,
    p: &ViewPolygon,
    s: &Surface,
    remap: &Remap,
    shaders: &[MaterialEntry],
    config: &RasterConfig,
    textures: &Textures,
    viewport: Viewport,
    band_rows: i32,
) {
    let verts = &geometry.vertices[p.vertices()];
    let (min_y, max_y) = verts
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v.y), hi.max(v.y))
        });
    let (row_top, row_end) = (pixel_edge(min_y), pixel_edge(max_y));
    if row_top >= row_end {
        return; // covers no rows
    }
    let entry = &shaders[s.material.0 as usize];
    let category = match p.kind {
        // Translucent surfaces are drawn after all opaque geometry, whatever their kind.
        _ if entry.translucent => Category::Translucent,
        PolygonKind::World => Category::World,
        PolygonKind::Prop | PolygonKind::Actor => {
            let default = if p.kind == PolygonKind::Actor {
                config.actor_path
            } else {
                RasterPath::Span
            };
            match s.path_override.unwrap_or(default) {
                RasterPath::Span => Category::Span,
                RasterPath::PerPixel => Category::PerPixel,
            }
        }
    };

    // The vertex stage, on the source polygon's vertices.
    let mesh = textures.assets.mesh(p.mesh);
    let source = &mesh.polygons[p.source_polygon() as usize];
    let object = geometry.objects[p.object as usize];
    let model = object.transform();
    let face_normal = (object.rotation * source.plane.normal).to_array();
    let tangents = remap
        .vertex
        .iter()
        .any(|v| matches!(v, VertexSource::FaceTangent | VertexSource::FaceBitangent))
        .then(|| face_tangents(mesh, source))
        .map(|(t, b)| ((object.rotation * t).to_array(), (object.rotation * b).to_array()));
    let ctx = VertexContext {
        object: &object,
        params: &s.params,
    };
    let (n_in, n_vals) = (layout_len(entry.io.vertex), layout_len(entry.io.sampled));
    let mut input = [0.0f32; MAX_VARYINGS];
    let mut outs = std::mem::take(&mut bins.vertex_out);
    outs.clear();
    for v in source.vertices() {
        let mut i = 0;
        for &from in &remap.vertex {
            match from {
                VertexSource::Attrib(a, stride, count) => {
                    let data = &mesh.attribs[a].data;
                    for c in 0..count {
                        input[i + c] = if c < stride { data.get_f32(v * stride + c) } else { 0.0 };
                    }
                    i += count;
                }
                VertexSource::Zero(count) => {
                    input[i..i + count].fill(0.0);
                    i += count;
                }
                VertexSource::Position => {
                    let world =
                        model.transform_point3(mesh.positions[mesh.vertex_positions[v] as usize]);
                    input[i..i + 3].copy_from_slice(&world.to_array());
                    i += 3;
                }
                VertexSource::FaceNormal => {
                    input[i..i + 3].copy_from_slice(&face_normal);
                    i += 3;
                }
                VertexSource::FaceTangent | VertexSource::FaceBitangent => {
                    let (t, b) = tangents.unwrap_or_default();
                    let v = if from == VertexSource::FaceTangent { t } else { b };
                    input[i..i + 3].copy_from_slice(&v);
                    i += 3;
                }
            }
        }
        let at = outs.len();
        outs.resize(at + n_vals, 0.0);
        (entry.vertex)(&input[..n_in], &ctx, &mut outs[at..]);
    }
    // Each (clipped) vertex: the same weighted sum of the outputs that clipping made of its
    // position, then the built-ins.
    bins.values.clear();
    let positions = &geometry.world_positions[p.vertices()];
    let n = p.source_vertices as usize;
    for (weights, position) in geometry.weights[p.weights()].chunks_exact(n).zip(positions) {
        let at = bins.values.len();
        bins.values.resize(at + n_vals, 0.0);
        let values = &mut bins.values[at..];
        for (k, &wk) in weights.iter().enumerate() {
            if wk != 0.0 {
                for (v, &o) in values.iter_mut().zip(&outs[k * n_vals..(k + 1) * n_vals]) {
                    *v += wk * o;
                }
            }
        }
        if let Some(offset) = remap.position {
            values[offset..offset + 3].copy_from_slice(&position.to_array());
        }
    }
    bins.vertex_out = outs;
    let mut lod_at = None;
    if let (Some(uv), Some(lod)) = (remap.uv, remap.lod) {
        let texture = textures.get(s.textures[0]);
        let size = (texture.width() as f32, texture.height() as f32);
        lod_at = Some((uv as u16, lod as u16, size.0, size.1));
        let values = &bins.values;
        let uv_at = |v: usize| (values[v * n_vals + uv], values[v * n_vals + uv + 1]);
        vertex_lods(verts, uv_at, size, &mut bins.lods);
        for (v, &l) in bins.lods.iter().enumerate() {
            bins.values[v * n_vals + lod] = l;
        }
    }
    let first_plane = bins.planes.len() as u32;
    let fans = fan_planes(verts, &bins.values, n_vals, &mut bins.planes);
    // Its lights: those that can reach its sector (or its entity's sectors), in range of it,
    // in front of it, (spot lights) with it in their cone, and not casting a shadow over it
    // (the view carves polygons into pieces wholly in or out of each shadow). Mirrored
    // polygons are lit where they really are.
    let first_light = bins.lights.len() as u32;
    let (mut splits, mut split_slots) = (0usize, [0u8; MAX_SPLIT]);
    let mut split_colors = [Vec3::ZERO; MAX_SPLIT];
    let mut split_flashlight = None;
    let positions = &geometry.world_positions[p.vertices()];
    let normal = Vec3::from_array(face_normal);
    let (lo, hi) = positions
        .iter()
        .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), &q| {
            (lo.min(q), hi.max(q))
        });
    let (center, radius) = ((lo + hi) * 0.5, (hi - lo).length() * 0.5);
    // Lights up to the surface's back light angle behind its plane reach it too (its bumps).
    let back = s.back_light.clamp(0.0, 1.0);
    // A polygon lit by normals of its own (a smooth mesh's, not its plane's) is lit by a
    // light behind its plane too, as long as some normal on it faces the light (see
    // `normals_face`).
    let values = &bins.values;
    let own_normals = remap
        .normal
        .map(|at| (0..positions.len()).map(move |v| Vec3::from_slice(&values[v * n_vals + at..][..3])))
        .filter(|normals| {
            normals.clone().any(|n| {
                n.cross(normal).length_squared() > 1e-6 * n.length_squared() || n.dot(normal) < 0.0
            })
        });
    let lights_it = |light: &Light| {
        let height = normal.dot(light.position - positions[0]);
        let far = light.position.distance(center) + radius;
        let near = light.position.clamp(lo, hi).distance(light.position);
        let in_shadow = light.shadow.is_some_and(|k| p.shadowed >> k & 1 != 0);
        !in_shadow
            && !light.excluded_by(s.excluded_lights)
            && height.abs() < light.range
            && near < light.range
            && light.cone_reaches(center, radius)
            && (lights_face(height, far, back)
                || own_normals
                    .clone()
                    .is_some_and(|normals| normals_face(normals, positions, light.position)))
    };
    // Its split lights: those whose shadow covers part of it (see `SampleContext`), as
    // many as it has room for, by priority (see `split_priority`). The rest are lit whole
    // here: their shadows on it are lost, the least of them first.
    let room = (0..=MAX_SPLIT)
        .rev()
        .find(|&n| layout_len(entry.io.interp) + split_outputs(n) <= MAX_VARYINGS)
        .unwrap_or(0);
    let mut chosen: [(f32, u32); MAX_SPLIT] = [(f32::NEG_INFINITY, u32::MAX); MAX_SPLIT];
    for &li in geometry.polygon_lights(p) {
        let light = geometry.lights[li as usize];
        let Some(k) = light.shadow else { continue };
        if p.split >> k & 1 == 0 || room == 0 || !lights_it(&light) {
            continue;
        }
        // Into the best `room`, kept in order (highest first).
        let priority = split_priority(&light, center, normal);
        if let Some(at) = chosen[..room].iter().position(|&(q, _)| priority > q) {
            chosen.copy_within(at..room - 1, at + 1);
            chosen[at] = (priority, li);
        }
    }
    for &li in geometry.polygon_lights(p) {
        let light = geometry.lights[li as usize];
        if lights_it(&light) {
            let split = match light.shadow {
                Some(k) if chosen[..room].iter().any(|&(_, c)| c == li) => {
                    split_slots[splits] = k;
                    split_colors[splits] = light.color;
                    if light.id == moose_assets::FLASHLIGHT_ID {
                        split_flashlight = Some(splits as u8);
                    }
                    splits += 1;
                    (splits - 1) as u8
                }
                _ => NO_SPLIT,
            };
            // A beam's cone is drawn with its shadow only where it is split off, and not in
            // reflections (the view carves none there); elsewhere it is lit at sample
            // points like any spot light's.
            bins.lights.push(Light {
                beam: light.beam && split != NO_SPLIT && p.mirror.is_none(),
                ..light
            });
            bins.light_split.push(split);
        }
    }
    // The shadow pieces of its split lights.
    let first_shadow = bins.shadows.len() as u32;
    let pieces: &[ShadowPiece] =
        &geometry.shadow_pieces[p.first_shadow as usize..][..p.shadow_count as usize];
    for piece in pieces {
        let Some(j) = split_slots[..splits].iter().position(|&k| k == piece.slot) else {
            continue;
        };
        let vertices = &geometry.shadow_vertices[piece.first_vertex as usize..][..piece.vertex_count as usize];
        let (top, bottom) = vertices
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(t, b), v| (t.min(v.y), b.max(v.y)));
        let first_soft = bins.shadow_softs.len() as u32;
        for soft in &geometry.shadow_softs[piece.first_soft as usize..][..piece.soft_count as usize] {
            let start = bins.shadow_wedges.len() as u32;
            bins.shadow_wedges.extend_from_slice(&geometry.shadow_wedges[soft.wedges.start as usize..soft.wedges.end as usize]);
            bins.shadow_softs.push(moose_view::ShadowSoft { wedges: start..bins.shadow_wedges.len() as u32, window: soft.window });
        }
        bins.shadows.push(ShadowSetup {
            first: bins.shadow_vertices.len() as u32,
            count: piece.vertex_count,
            split: j as u8,
            beam: piece.beam,
            row_top: pixel_edge(top),
            row_end: pixel_edge(bottom),
            occluded: piece.occluded,
            softs: (first_soft, bins.shadow_softs.len() as u32),
            dark: piece.soft_count == 0 && !piece.occluded && vertices.iter().all(|v| v.light <= 0.0),
        });
        for v in vertices {
            bins.shadow_vertices.push(SetupVertex { x: v.x, y: v.y, w: v.w });
            bins.shadow_world.push(v.world);
            bins.shadow_rays.push(v.ray);
            bins.shadow_width.push(v.width);
            bins.shadow_lines.push(v.line);
            bins.shadow_light.push(v.light);
        }
    }
    let first_vertex = bins.vertices.len() as u32;
    bins.vertices.extend(verts.iter().map(|v| SetupVertex {
        x: v.x,
        y: v.y,
        w: v.w,
    }));
    bins.lines
        .extend_from_slice(&geometry.edge_lines[p.vertices()]);
    let local = bins.polygons.len();
    // Its tiles' refined spacings, not yet worked out: for a lit polygon whose material
    // says which of its outputs is the light.
    let light_out = remap
        .light
        .filter(|_| bins.lights.len() as u32 > first_light && config.refine_threshold > 0.0)
        .map(|at| at as u16);
    let (min_x, max_x) = verts
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| (lo.min(v.x), hi.max(v.x)));
    let strip = |x: f32| (x.floor() as i32 - viewport.x as i32).div_euclid(TILE_COLS);
    let block = |row: i32| (row - viewport.y as i32).div_euclid(BLOCK_ROWS);
    let (refine_strip, refine_block) = (strip(min_x), block(row_top));
    let refine_strips = strip(max_x) - refine_strip + 2;
    let refine = bins.refine.len() as u32;
    if light_out.is_some() {
        let tiles = refine_strips * (block(row_end - 1) - refine_block + 1);
        bins.refine.resize_with(refine as usize + tiles as usize, AtomicU8::default);
    }
    bins.polygons.push(PolygonSetup {
        first_vertex,
        vertex_count: verts.len() as u16,
        n_vals: n_vals as u16,
        splits: splits as u8,
        split_colors,
        split_flashlight,
        split_slots,
        first_shadow,
        shadows: (bins.shadows.len() as u32 - first_shadow) as u16,
        n_out: (layout_len(entry.io.interp) + split_outputs(splits)) as u16,
        first_plane,
        fans: fans as u16,
        row_top,
        row_end,
        max_w: verts.iter().map(|v| v.w).fold(0.0, f32::max),
        material: s.material,
        params: s.params,
        textures: s.textures,
        filters: s.filters,
        eye: p
            .mirror
            .map_or(geometry.eye, |m| geometry.mirrors[m as usize].eye),
        object,
        half_rate: p.mirror.is_some(),
        first_light,
        light_count: (bins.lights.len() as u32 - first_light) as u16,
        shadowed: p.shadowed,
        position: remap.position.map(|p| p as u16),
        lod: lod_at,
        back,
        light_out,
        refine,
        refine_block,
        refine_strip,
        refine_strips: refine_strips as u16,
    });
    // Bin a reference into every band the polygon's rows touch.
    let id = surface_id(thread, local);
    let first_band = ((row_top - viewport.y as i32).max(0) / band_rows) as usize;
    let last_band = ((row_end - 1 - viewport.y as i32).max(0) / band_rows) as usize;
    let last_band = last_band.min(bins.bands.len() - 1);
    for band in &mut bins.bands[first_band..=last_band] {
        match category {
            Category::World => band.world.push(id),
            Category::Span => band.span.push(id),
            Category::PerPixel => band.pixel.push(id),
            Category::Translucent => band.translucent.push(id),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_band(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    shaders: &[MaterialEntry],
    config: &RasterConfig,
    textures: &Textures,
    viewport: Viewport,
    band: usize,
    band_rows: i32,
    chunk: &mut [u32],
    width: usize,
    seam: &mut Seam,
) {
    let vw = viewport.width as usize;
    let vx = viewport.x as i32;
    s.edge_down.clear();
    seam.up.clear();
    seam.down.clear();
    // Lattices are built per block of rows, whatever the band size, so no band size or
    // thread count changes a pixel.
    let band_top = band as i32 * band_rows;
    let band_end = band_top + (chunk.len() / width) as i32;
    s.first_block = band_top.div_euclid(BLOCK_ROWS);
    s.blocks = ((band_end - 1).div_euclid(BLOCK_ROWS) - s.first_block + 1) as usize;
    (s.viewport_x, s.viewport_y) = (vx, viewport.y as i32);
    s.band_rows = viewport.y as i32 + band_top..viewport.y as i32 + band_end;
    s.color.resize(vw, 0);
    s.vis_w.resize(vw, 0.0);
    s.vis_state.resize(vw, EMPTY);

    // Gather this band's polygons from every thread's bins.
    s.world.clear();
    s.span.clear();
    s.pixel.clear();
    s.translucent.clear();
    for b in bins {
        let Some(bb) = b.bands.get(band) else {
            continue;
        };
        s.world.extend_from_slice(&bb.world);
        s.span.extend_from_slice(&bb.span);
        s.pixel.extend_from_slice(&bb.pixel);
        s.translucent.extend_from_slice(&bb.translucent);
    }
    // Front to back for sorted insertion (nearest point first).
    let poly = |id: u32| {
        let (t, l) = split_id(id);
        &bins[t].polygons[l]
    };
    s.span
        .sort_by(|&a, &b| poly(b).max_w.total_cmp(&poly(a).max_w));
    // Back to front for blending (farthest nearest-point first).
    s.translucent
        .sort_by(|&a, &b| poly(a).max_w.total_cmp(&poly(b).max_w));
    // No lattices yet: each is built the first time its polygon is shaded.
    let slots = s.world.len() + s.span.len() + s.pixel.len() + s.translucent.len();
    s.lattices.clear();
    s.lattices.resize(slots * s.blocks, Lattice::default());
    s.tiles.clear();
    s.grid_rows.clear();
    s.grid_values.clear();
    s.grid_ys.clear();
    s.row_ends.clear();

    let rows_in_chunk = chunk.len() / width;
    for r in 0..rows_in_chunk {
        let row = viewport.y as i32 + band as i32 * band_rows + r as i32;
        s.row = row;
        s.states.clear();
        s.spans.clear();

        // World spans: portal clipping guarantees they never overlap. Append, sort by x.
        let (n_world, n_span, n_pixel) = (s.world.len(), s.span.len(), s.pixel.len());
        for i in 0..s.world.len() {
            let id = s.world[i];
            if let Some(span) = row_span(s, bins, id, i as u32, row) {
                s.spans.push(span);
            }
        }
        s.spans.sort_by_key(|sp| sp.x0);
        debug_assert!(
            s.spans.windows(2).all(|p| p[0].x1 <= p[1].x0),
            "world spans overlap on row {row}"
        );

        // Props and span-path actors, front to back: sorted insertion with the exact
        // two-point overlap resolve.
        for i in 0..s.span.len() {
            let id = s.span[i];
            if let Some(span) = row_span(s, bins, id, (n_world + i) as u32, row) {
                insert_resolved(&s.spans, span, &mut s.spans_next);
                std::mem::swap(&mut s.spans, &mut s.spans_next);
            }
        }

        // Per-pixel actors, if any cover this row.
        s.pixel_spans.clear();
        for i in 0..s.pixel.len() {
            let id = s.pixel[i];
            if let Some(span) = row_span(s, bins, id, (n_world + n_span + i) as u32, row) {
                s.pixel_spans.push(span);
            }
        }

        // Translucent polygons covering this row, back to front.
        s.translucent_spans.clear();
        for i in 0..s.translucent.len() {
            let id = s.translucent[i];
            let slot = (n_world + n_span + n_pixel + i) as u32;
            if let Some(span) = row_span(s, bins, id, slot, row) {
                s.translucent_spans.push(span);
            }
        }

        if s.pixel_spans.is_empty() {
            // Shade straight from the span list.
            let mut x = vx;
            for i in 0..s.spans.len() {
                let sp = s.spans[i];
                fill(&mut s.color, x - vx, sp.x0 - vx, config.background);
                shade_run(
                    s, bins, shaders, config, textures, sp.state, sp.x0, sp.x1, vx,
                );
                x = sp.x1;
            }
            fill(&mut s.color, x - vx, vw as i32, config.background);

            // Translucent pass, back to front, still in spans: each translucent span is
            // blended where it is in front of the opaque spans, found with the same exact
            // two-point test as prop insertion (w is linear along both).
            for i in 0..s.translucent_spans.len() {
                let t = s.translucent_spans[i];
                visible_runs(&s.spans, &t, &mut s.visible);
                let reads_behind = shader_of(s, bins, shaders, t.state).reads_behind;
                // Runs that touch are blended as one (the sample grid belongs to the row, so
                // this changes no pixel), so a shader reading the colors behind it along the
                // row can read across what is behind from one opaque span into the next.
                let mut k = 0;
                while k < s.visible.len() {
                    let x0 = s.visible[k].0;
                    let mut x1 = x0;
                    while k < s.visible.len() && s.visible[k].0 == x1 {
                        let (a, b, behind) = s.visible[k];
                        if reads_behind {
                            // The opaque w behind, for just these pixels.
                            for px in a..b {
                                s.vis_w[(px - vx) as usize] =
                                    behind.map_or(0.0, |o| s.spans[o as usize].w(px));
                            }
                        }
                        x1 = b;
                        k += 1;
                    }
                    blend_run(s, bins, shaders, config, textures, t.state, x0, x1, vx);
                }
            }
        } else {
            // Fill the visibility buffer from the resolved spans, z-test per-pixel actors,
            // then shade runs of equal polygon.
            s.vis_w.iter_mut().for_each(|w| *w = 0.0);
            s.vis_state.iter_mut().for_each(|v| *v = EMPTY);
            for sp in &s.spans {
                for px in sp.x0..sp.x1 {
                    let i = (px - vx) as usize;
                    s.vis_w[i] = sp.w(px);
                    s.vis_state[i] = sp.state;
                }
            }
            for sp in &s.pixel_spans {
                for px in sp.x0..sp.x1 {
                    let (i, w) = ((px - vx) as usize, sp.w(px));
                    if w > s.vis_w[i] {
                        s.vis_w[i] = w;
                        s.vis_state[i] = sp.state;
                    }
                }
            }
            let mut x = 0usize;
            while x < vw {
                let state = s.vis_state[x];
                let mut end = x + 1;
                while end < vw && s.vis_state[end] == state {
                    end += 1;
                }
                if state == EMPTY {
                    fill(&mut s.color, x as i32, end as i32, config.background);
                } else {
                    shade_run(
                        s,
                        bins,
                        shaders,
                        config,
                        textures,
                        state,
                        x as i32 + vx,
                        end as i32 + vx,
                        vx,
                    );
                }
                x = end;
            }

            // Translucent pass, back to front: test w against the opaque surfaces (without
            // writing it) and blend the passing runs over what is there.
            for i in 0..s.translucent_spans.len() {
                let sp = s.translucent_spans[i];
                let mut px = sp.x0;
                while px < sp.x1 {
                    if sp.w(px) <= s.vis_w[(px - vx) as usize] {
                        px += 1;
                        continue;
                    }
                    let start = px;
                    while px < sp.x1 && sp.w(px) > s.vis_w[(px - vx) as usize] {
                        px += 1;
                    }
                    blend_run(s, bins, shaders, config, textures, sp.state, start, px, vx);
                }
            }
        }

        if config.smooth_edges {
            let per_pixel = !s.pixel_spans.is_empty();
            smooth_row(s, bins, viewport, r, chunk, width, seam, per_pixel);
        }
        let out = &mut chunk[r * width + viewport.x as usize..r * width + viewport.x as usize + vw];
        out.copy_from_slice(&s.color);
    }
}

/// Smooths the edges the row's pixels are on (see [`find_edges`]), from the row as
/// shaded and the rows above and below. Rows of the band below this one are taken from
/// once it is shaded; its first and last rows' pixels that take from another band wait in
/// `seam` (see [`join_seams`]).
#[allow(clippy::too_many_arguments)]
fn smooth_row(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    viewport: Viewport,
    r: usize,
    chunk: &mut [u32],
    width: usize,
    seam: &mut Seam,
    per_pixel: bool,
) {
    let rows = chunk.len() / width;
    let at = |r: usize| r * width + viewport.x as usize;
    // This row as shaded, and the row above's pixels waiting for it.
    std::mem::swap(&mut s.edge_above, &mut s.edge_here);
    s.edge_here.clear();
    s.edge_here.extend_from_slice(&s.color);
    for &(px, k) in &s.edge_down {
        let i = at(r - 1) + px as usize;
        chunk[i] = mix(chunk[i], s.edge_here[px as usize], k);
    }
    s.edge_down.clear();

    find_edges(s, bins, viewport, per_pixel);
    let (top, bottom) = (s.row == viewport.y as i32, s.row + 1 == (viewport.y + viewport.height) as i32);
    for e in &s.edge_blends {
        let (px, k) = (e.px as usize, e.k);
        let c = &mut s.color[px];
        match e.across {
            // Nothing to blend with past the screen's left and right edges (as above and
            // below them).
            Across::Left if px == 0 => {}
            Across::Right if px + 1 >= s.edge_here.len() => {}
            Across::Left => *c = mix(*c, s.edge_here[px - 1], k),
            Across::Right => *c = mix(*c, s.edge_here[px + 1], k),
            Across::Up if top => {}
            Across::Up if r == 0 => seam.up.push((e.px, k)),
            Across::Up => *c = mix(*c, s.edge_above[px], k),
            Across::Down if bottom => {}
            Across::Down if r + 1 == rows => seam.down.push((e.px, k)),
            Across::Down => s.edge_down.push((e.px, k)),
        }
    }
    if r == 0 {
        seam.first.clear();
        seam.first.extend_from_slice(&s.edge_here);
    }
    if r + 1 == rows {
        seam.last.clear();
        seam.last.extend_from_slice(&s.edge_here);
    }
}

/// The edge pixels on bands' first and last rows that take from the band before or after
/// them, once every band is drawn (from those rows as shaded).
fn join_seams(seams: &[Seam], rows: &mut [u32], width: usize, viewport: Viewport, band_rows: usize) {
    let height = viewport.height as usize;
    for (b, seam) in seams.iter().enumerate() {
        let (first, last) = (b * band_rows, ((b + 1) * band_rows).min(height) - 1);
        let at = |row: usize, px: u32| row * width + viewport.x as usize + px as usize;
        if b > 0 {
            for &(px, k) in &seam.up {
                let i = at(first, px);
                rows[i] = mix(rows[i], seams[b - 1].last[px as usize], k);
            }
        }
        if let Some(below) = seams.get(b + 1) {
            for &(px, k) in &seam.down {
                let i = at(last, px);
                rows[i] = mix(rows[i], below.first[px as usize], k);
            }
        }
    }
}

/// `a` with `k` 256ths of `b` (at most 256), in each byte (the top one, an HDR material's
/// past-white amount, too).
#[inline]
fn mix(a: u32, b: u32, k: u32) -> u32 {
    let lerp = |a: u32, b: u32| ((a * (256 - k) + b * k) >> 8) & 0x00FF_00FF;
    lerp(a & 0x00FF_00FF, b & 0x00FF_00FF) | lerp(a >> 8 & 0x00FF_00FF, b >> 8 & 0x00FF_00FF) << 8
}

/// Finds the row's edge pixels into `s.edge_blends`: where its opaque spans meet (or meet
/// the background), the polygon edge there is followed through the row's pixels, and each
/// pixel it passes through takes as much of its neighbor across it as the edge leaves on
/// that side (its area there, a box filter). An edge steeper than 45 degrees passes
/// through about one pixel a row, which takes from its neighbor left or right; a
/// shallower one runs along the row, each pixel taking from the one above or below.
///
/// Only where the surfaces differ: not between pieces of one surface (same material,
/// lights and values on both sides; see [`continuous`]). Not in reflections, which are
/// drawn at half rate; on a row with per-pixel actors, not where they are drawn.
fn find_edges(s: &mut RowScratch, bins: &[ThreadBins], viewport: Viewport, per_pixel: bool) {
    s.edge_blends.clear();
    let (vx, end) = (viewport.x as i32, (viewport.x + viewport.width) as i32);
    let n = s.spans.len();
    // Each place spans meet, or meet the background: the span (if any) on either side,
    // the pixel it is at, and how far each side's pixels reach from it.
    let meet = |s: &mut RowScratch, a: Option<Span>, b: Option<Span>, lo: i32, at: i32, hi: i32| {
        if lo < at && at < hi {
            edge_pixels(s, bins, a, b, lo, at, hi, vx, per_pixel);
        }
    };
    if n == 0 {
        return;
    }
    let first = s.spans[0];
    meet(s, None, Some(first), vx, first.x0, first.x1);
    for i in 0..n {
        let a = s.spans[i];
        match s.spans.get(i + 1).copied() {
            Some(b) if b.x0 == a.x1 => meet(s, Some(a), Some(b), a.x0, a.x1, b.x1),
            next => {
                let gap = next.map_or(end, |b| b.x0);
                meet(s, Some(a), None, a.x0, a.x1, gap);
                if let Some(b) = next {
                    meet(s, None, Some(b), a.x1, b.x0, b.x1);
                }
            }
        }
    }
}

/// One place spans meet (see [`find_edges`]): `a`'s pixels (or the background's) are
/// `lo..at`, `b`'s are `at..hi`.
#[allow(clippy::too_many_arguments)]
fn edge_pixels(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    a: Option<Span>,
    b: Option<Span>,
    lo: i32,
    at: i32,
    hi: i32,
    vx: i32,
    per_pixel: bool,
) {
    let states = &s.states;
    let st = |sp: &Span| &states[sp.state as usize];
    let poly = |sp: &Span| {
        let (t, l) = split_id(st(sp).id);
        &bins[t].polygons[l]
    };
    if a.is_some_and(|a| poly(&a).half_rate) || b.is_some_and(|b| poly(&b).half_rate) {
        return; // a reflection
    }
    // The edge between them: a's right edge, if it ends there, or b's left edge, if it
    // starts there (where both do, the nearer one's); where neither does, the surfaces
    // meet where they pass through each other, which is left alone.
    let on_a = a.filter(|a| a.x1 == pixel_edge(st(a).x_right));
    let on_b = b.filter(|b| b.x0 == pixel_edge(st(b).x_left));
    let right = |a: &Span| (st(a).x_right, st(a).right_edge);
    let left = |b: &Span| (st(b).x_left, st(b).left_edge);
    let (x, edge) = match (on_a, on_b) {
        (Some(a), Some(b)) if a.w(at) >= b.w(at) => right(&a),
        (_, Some(b)) => left(&b),
        (Some(a), None) => right(&a),
        (None, None) => return,
    };
    let y = s.row as f32 + 0.5;
    if let (Some(a), Some(b)) = (a, b)
        && continuous(bins, st(&a).id, st(&b).id, x, y)
    {
        return;
    }
    // Pixels drawn over by per-pixel actors are theirs.
    let owner = |px: i32| {
        let side = if px < at { a } else { b };
        side.map_or(EMPTY, |sp| sp.state)
    };
    let shown = |px: i32| !per_pixel || s.vis_state[(px - vx) as usize] == owner(px);
    let mut push = |px: i32, across: Across, k: f32| {
        let k = (k * 256.0).round() as u32;
        let neighbor = match across {
            Across::Left => shown(px - 1),
            Across::Right => shown(px + 1),
            Across::Up | Across::Down => true,
        };
        if k > 0 && shown(px) && neighbor {
            s.edge_blends.push(EdgeBlend { px: (px - vx) as u32, across, k: k.min(128) });
        }
    };
    if !edge.dxdy.is_finite() {
        return;
    }
    // Each pixel of the row the edge passes through (within its own ends) takes as much of
    // its neighbor across the edge as the edge cuts off of it: left or right of a steep edge,
    // above or below a shallow one.
    let len = (1.0 + edge.dxdy * edge.dxdy).sqrt();
    let normal = Vec2::new(1.0, -edge.dxdy) / len; // square to the edge, to its right
    let reach = edge_reach(normal) * len; // along the row
    for px in pixel_edge(x - reach).max(lo)..pixel_edge(x + reach).min(hi) {
        let d = (px as f32 + 0.5 - x) / len; // its center's distance right of the edge
        let foot = y + d * edge.dxdy / len; // the nearest point of the edge's line, its y
        if foot < edge.top || foot > edge.bottom {
            continue;
        }
        let across = if edge.dxdy.abs() <= 1.0 {
            if d > 0.0 { Across::Left } else { Across::Right }
        } else if (px as f32 + 0.5 - x) / edge.dxdy < 0.0 {
            Across::Up // the edge passes above its center
        } else {
            Across::Down
        };
        push(px, across, 1.0 - covered(d.abs(), normal));
    }
}

/// How much of a pixel lies on its center's side of a straight edge `d` from the center
/// (square to it; negative past it), the edge's unit normal being `normal`: the area of
/// the pixel's square there, a box filter.
fn covered(d: f32, normal: Vec2) -> f32 {
    let (a, b) = (normal.x.abs().max(normal.y.abs()), normal.x.abs().min(normal.y.abs()));
    // Within `(a - b) / 2` of the center the edge crosses two opposite sides, and the area
    // changes evenly; further out it cuts a corner off, a triangle.
    let (corner, reach) = ((a - b) * 0.5, (a + b) * 0.5);
    if d >= reach {
        1.0
    } else if d <= -reach {
        0.0
    } else if d.abs() <= corner {
        0.5 + d / a
    } else if d > 0.0 {
        1.0 - (reach - d) * (reach - d) / (2.0 * a * b)
    } else {
        (reach + d) * (reach + d) / (2.0 * a * b)
    }
}

/// How far (square to it) a straight edge with unit normal `normal` can be from a pixel's
/// center and still cross the pixel.
fn edge_reach(normal: Vec2) -> f32 {
    (normal.x.abs() + normal.y.abs()) * 0.5
}

/// Whether two polygons meeting at screen point (`x`, `y`) look like one surface there:
/// the same material and lights, and the same w and `sampled` values (but the level of
/// detail) on both sides.
fn continuous<'a>(bins: &'a [ThreadBins], a: u32, b: u32, x: f32, y: f32) -> bool {
    if a == b {
        return true;
    }
    let ((ta, la), (tb, lb)) = (split_id(a), split_id(b));
    let (ba, bb) = (&bins[ta], &bins[tb]);
    let (pa, pb) = (&ba.polygons[la], &bb.polygons[lb]);
    if pa.material != pb.material
        || pa.params != pb.params
        || pa.textures != pb.textures
        || pa.filters != pb.filters
    {
        return false;
    }
    // Not a hard shadow's edge: one in the full shadow of a light that reaches the other
    // whole (polygons are carved along shadows' edges; where the other is split, its
    // shadow pieces fade the light out to there). Their lights can differ otherwise, but
    // only where a light has faded to nothing: past its range, outside its cone or behind
    // them.
    let lights = |b: &'a ThreadBins, p: &PolygonSetup| {
        let range = p.first_light as usize..p.first_light as usize + p.light_count as usize;
        b.lights[range.clone()].iter().zip(&b.light_split[range])
    };
    let differ = pa.shadowed ^ pb.shadowed;
    if lights(ba, pa)
        .chain(lights(bb, pb))
        .any(|(l, &split)| split == NO_SPLIT && l.shadow.is_some_and(|k| differ >> k & 1 != 0))
    {
        return false;
    }
    let n = pa.n_vals as usize;
    let qa = Planes::of(&ba.planes[pa.first_plane as usize..], pa.fans as usize, n);
    let qb = Planes::of(&bb.planes[pb.first_plane as usize..], pb.fans as usize, n);
    let (wa, wb) = (qa.w_at(x, y), qb.w_at(x, y));
    if (wa - wb).abs() > 2e-3 * wa.abs().max(wb.abs()) || wa <= 0.0 || wb <= 0.0 {
        return false;
    }
    let lod = pa.lod.map(|(_, lod, _, _)| lod as usize);
    let (fa, fb) = (qa.fan_at(x, y), qb.fan_at(x, y));
    (0..n).filter(|&k| Some(k) != lod).all(|k| {
        let (va, vb) = (qa.value_at(fa, k, x, y) / wa, qb.value_at(fb, k, x, y) / wb);
        (va - vb).abs() <= 2e-3 + 1e-3 * va.abs().max(vb.abs())
    })
}

fn fill(color: &mut [u32], from: i32, to: i32, value: u32) {
    if from < to {
        color[from as usize..to as usize]
            .iter_mut()
            .for_each(|c| *c = value);
    }
}

/// An edge crossing a row: the edge's index and its x there.
type Crossing = Option<(usize, f32)>;

/// The left and right edges crossing `row`'s center line: each edge's index and its x
/// there, walked along the edge's line exactly as row spans are.
fn crossings(
    verts: &[SetupVertex],
    lines: &[Option<EdgeLine>],
    row: i32,
) -> Option<((usize, f32), (usize, f32))> {
    let n = verts.len();
    let (mut left, mut right): (Crossing, Crossing) = (None, None);
    for i in 0..n {
        let (a, c) = (verts[i], verts[(i + 1) % n]);
        let (top, bottom) = (a.y.min(c.y), a.y.max(c.y));
        if row < pixel_edge(top) || row >= pixel_edge(bottom) {
            continue;
        }
        // Within the edge's own ends: exactly, a row it crosses is crossed there, so this
        // only takes off a carried line's rounding (which, far along a short line, could
        // put the span past the polygon, and past its lattice's columns).
        let x = lines[i]
            .unwrap_or(EdgeLine::between((a.x, a.y), (c.x, c.y)))
            .x_at_row(row)
            .clamp(a.x.min(c.x), a.x.max(c.x));
        if c.y > a.y {
            if left.is_none_or(|(_, lx)| x > lx) {
                left = Some((i, x));
            }
        } else if right.is_none_or(|(_, rx)| x < rx) {
            right = Some((i, x));
        }
    }
    Some((left?, right?))
}

/// Edge `i`'s w where it crosses `row`'s center line, and its perspective-correct weight
/// toward its end vertex.
fn edge_at_row(verts: &[SetupVertex], i: usize, row: i32) -> (f32, f32) {
    let (a, c) = (verts[i], verts[(i + 1) % verts.len()]);
    let t = ((row as f32 + 0.5 - a.y) / (c.y - a.y)).clamp(0.0, 1.0);
    let den = (1.0 - t) * a.w + t * c.w;
    let alpha = if den > 0.0 { t * c.w / den } else { t };
    (a.w + (c.w - a.w) * t, alpha)
}

/// Edge `i`'s slope along the line it is walked on (see [`crossings`]), and its rows.
fn edge_slope(verts: &[SetupVertex], lines: &[Option<EdgeLine>], i: usize) -> EdgeSlope {
    let (a, c) = (verts[i], verts[(i + 1) % verts.len()]);
    let line = lines[i].unwrap_or(EdgeLine::between((a.x, a.y), (c.x, c.y)));
    EdgeSlope {
        dxdy: (line.x1 - line.x0) / (line.y1 - line.y0),
        top: a.y.min(c.y),
        bottom: a.y.max(c.y),
    }
}

/// The span a polygon covers on `row`, if any: its pixels and w along them. Only x and w;
/// values are worked out when (and if) it is shaded.
fn row_span(s: &mut RowScratch, bins: &[ThreadBins], id: u32, slot: u32, row: i32) -> Option<Span> {
    let (t, l) = split_id(id);
    let b = &bins[t];
    let p = &b.polygons[l];
    if row < p.row_top || row >= p.row_end {
        return None;
    }
    let range = p.first_vertex as usize..p.first_vertex as usize + p.vertex_count as usize;
    let (verts, lines) = (&b.vertices[range.clone()], &b.lines[range]);
    let ((li, x_left), (ri, x_right)) = crossings(verts, lines, row)?;
    let (x0, x1) = (pixel_edge(x_left), pixel_edge(x_right));
    if x0 >= x1 {
        return None; // an edge-on sliver on this row
    }
    let (w_left, _) = edge_at_row(verts, li, row);
    let (w_right, _) = edge_at_row(verts, ri, row);
    let slope = |i: usize| edge_slope(verts, lines, i);
    let state = s.states.len() as u32;
    s.states.push(RowState {
        id,
        slot,
        x_left,
        x_right,
        w_left,
        w_right,
        left_edge: slope(li),
        right_edge: slope(ri),
    });
    let dwdx = if x_right > x_left {
        (w_right - w_left) / (x_right - x_left)
    } else {
        0.0
    };
    Some(Span {
        x0,
        x1,
        xa: x_left,
        wa: w_left,
        dwdx,
        state,
    })
}

/// Rows per lattice block: grid rows are at most this far apart, and a block's rows share
/// one spacing (chosen from the whole block, so however the rows are split into bands, a
/// polygon's grid is the same; each band builds only the grid rows its own rows need).
const BLOCK_ROWS: i32 = 32;

/// Columns per lattice tile: a polygon's grid in a block of rows is split into tiles of this
/// many columns (counted from the viewport's left edge), each with its own spacing, from the
/// polygon's nearest part within it. Spacings are powers of two up to this, so every tile's
/// columns lie on the one global grid, and each point on a row belongs to exactly one tile.
const TILE_COLS: i32 = MAX_STEP;

/// A polygon's sample points in one block of rows (see the Material Pipeline Spec), all of
/// them on the polygon: its vertices; its edges' knots, where they cross the grid's rows and
/// columns; and the grid's points inside it, one tile per column strip it covers, built the
/// first time a run there needs them.
#[derive(Clone, Copy, Default)]
struct Lattice {
    started: bool,
    /// Its first strip (from the viewport's left edge, in tiles), its strips, and their tiles
    /// in `RowScratch::tiles`.
    first_strip: i32,
    strips: u32,
    tiles: u32,
    /// The smallest w of the polygon in the block: a lower bound for every tile's.
    w_min: f32,
    /// The polygon's rows in the block.
    rows: (i32, i32),
    /// Its row ends in `RowScratch::row_ends`: a left and a right per row of the band in
    /// the block.
    ends: u32,
}

/// A polygon's grid in one tile: its spacing and grid rows. Neighboring tiles whose
/// spacing is the same share grid rows, as one run of strips (each holding a copy of this).
#[derive(Clone, Copy, Default)]
struct Tile {
    built: bool,
    /// The run of strips sharing this spacing and these grid rows: its first strip, and the
    /// strip after its last.
    first_strip: i32,
    end_strip: i32,
    /// Column spacing and row spacing, in pixels.
    nx: i32,
    ny: i32,
    /// Its first grid row, and its grid rows (`ny` apart) in `RowScratch::grid_rows`.
    first_row: i32,
    rows: u32,
}

/// One grid row of a tile: the grid columns it holds (those inside the polygon on a row it
/// serves), and their outputs.
#[derive(Clone, Copy, Default)]
struct GridRow {
    /// First column (a pixel) and how many, `nx` apart.
    col0: i32,
    cols: u32,
    /// Its outputs in `RowScratch::grid_values`, output by output, `stride` values each.
    values: u32,
    stride: u32,
    /// Where each column's point is down its column, in `RowScratch::grid_ys`: on the grid
    /// row where the polygon reaches it, or else the nearest point of the polygon along the
    /// column (on its edge: the knot where the edge crosses the column). Whether any is off
    /// the grid row.
    ys: u32,
    snapped: bool,
}

/// One end of a polygon's row: where the row crosses an edge. Its outputs are those of the
/// edge's knots on either side of it (in `RowScratch::grid_values`, together), `f` of the
/// way from `a`'s to `b`'s along the edge.
#[derive(Clone, Copy, Default)]
struct RowEnd {
    a: u32,
    b: u32,
    f: f32,
    /// What it is on this row, for the overlay.
    kind: u8,
}

/// What a row's sample point is, for the overlay (see `RasterConfig::show_samples`): between
/// sample points; on an edge's knot, or a grid point moved onto an edge; on a grid point;
/// on a vertex.
const POINT_BETWEEN: u8 = 0;
const POINT_KNOT: u8 = 1;
const POINT_GRID: u8 = 2;
const POINT_VERTEX: u8 = 3;

/// A polygon's plane functions (see [`fan_planes`]), read from `ThreadBins::planes`.
struct Planes<'a> {
    /// The origin they are relative to: the polygon's first vertex.
    ox: f32,
    oy: f32,
    /// w: `w0 + wx * dx + wy * dy`.
    w0: f32,
    wx: f32,
    wy: f32,
    /// Per fan triangle, per `sampled` value: `[c, a, b]` for `value * w = c + a dx + b dy`.
    fans: &'a [f32],
    /// The fan's diagonals from the first vertex: `[ex, ey]`, oriented so that a point is
    /// past one when `ex * dy - ey * dx > 0`.
    diagonals: &'a [f32],
    n_fans: usize,
    n_in: usize,
}

impl<'a> Planes<'a> {
    fn of(data: &'a [f32], n_fans: usize, n_in: usize) -> Self {
        let fans_end = 5 + n_fans * n_in * 3;
        Self {
            ox: data[0],
            oy: data[1],
            w0: data[2],
            wx: data[3],
            wy: data[4],
            fans: &data[5..fans_end],
            diagonals: &data[fans_end..fans_end + 2 * n_fans.saturating_sub(1)],
            n_fans,
            n_in,
        }
    }

    /// w at screen point (`x`, `y`).
    fn w_at(&self, x: f32, y: f32) -> f32 {
        self.w0 + self.wx * (x - self.ox) + self.wy * (y - self.oy)
    }

    /// The fan triangle whose planes hold at (`x`, `y`): how many diagonals it is past.
    fn fan_at(&self, x: f32, y: f32) -> usize {
        let (dx, dy) = (x - self.ox, y - self.oy);
        self.diagonals
            .chunks_exact(2)
            .filter(|d| d[0] * dy - d[1] * dx > 0.0)
            .count()
            .min(self.n_fans - 1)
    }

    /// `sampled` value `k` times w at (`x`, `y`), on fan triangle `fan`.
    fn value_at(&self, fan: usize, k: usize, x: f32, y: f32) -> f32 {
        let c = &self.fans[(fan * self.n_in + k) * 3..][..3];
        c[0] + c[1] * (x - self.ox) + c[2] * (y - self.oy)
    }
}

/// A polygon's values as plane functions of the screen, so they are defined anywhere,
/// on it and past its edges (the grid's points need not lie on it): w, which is affine on
/// screen for a planar polygon, and each `sampled` value times w, per triangle of the fan
/// from the first vertex. Values are only affine across a whole polygon if its vertex values
/// agree, so each triangle has its own planes, exactly as if the polygon were triangulated
/// (for triangles, and polygons whose values agree, they are all the same). Appends
/// `[ox, oy, w0, wx, wy]`, then each triangle's planes, then its diagonals, to `out`;
/// returns the number of triangles.
fn fan_planes(verts: &[ScreenVertex], values: &[f32], n_in: usize, out: &mut Vec<f32>) -> usize {
    let n = verts.len();
    let v0 = verts[0];
    let area = |i: usize| {
        let (b, c) = (verts[i], verts[i + 1]);
        (b.x - v0.x) * (c.y - v0.y) - (c.x - v0.x) * (b.y - v0.y)
    };
    // The plane through a triangle's three values of `q`, relative to the first vertex.
    let plane = |i: usize, q: &dyn Fn(usize) -> f32| {
        let (b, c) = (verts[i], verts[i + 1]);
        let (dx1, dy1, dx2, dy2) = (b.x - v0.x, b.y - v0.y, c.x - v0.x, c.y - v0.y);
        let det = dx1 * dy2 - dx2 * dy1;
        let (q0, dq1, dq2) = (q(0), q(i) - q(0), q(i + 1) - q(0));
        [
            q0,
            (dq1 * dy2 - dq2 * dy1) / det,
            (dx1 * dq2 - dx2 * dq1) / det,
        ]
    };
    let n_fans = n - 2;
    let orient = if (1..n - 1).map(area).sum::<f32>() < 0.0 { -1.0 } else { 1.0 };
    let widest = (1..n - 1).max_by(|&i, &j| area(i).abs().total_cmp(&area(j).abs()));
    // A triangle gives a trustworthy plane only if it has some size: slivers left by
    // clipping make wild planes, and extending one across its wedge magnifies that.
    let min_area = widest.map_or(0.0, |i| area(i).abs()).mul_add(0.02, 0.0).max(1.0);
    let flat = |i: usize| area(i).abs() >= min_area;
    out.extend_from_slice(&[v0.x, v0.y]);
    match widest.filter(|&i| flat(i)) {
        Some(i) => out.extend_from_slice(&plane(i, &|v| verts[v].w)),
        None => out.extend_from_slice(&[v0.w, 0.0, 0.0]),
    }
    for t in 0..n_fans {
        // A degenerate triangle (collinear after clipping) takes the nearest good one's
        // planes; with none, the values are constant.
        let i = (t + 1..n - 1)
            .chain((1..t + 1).rev())
            .find(|&i| flat(i));
        for k in 0..n_in {
            let q = |v: usize| values[v * n_in + k] * verts[v].w;
            match i {
                Some(i) => out.extend_from_slice(&plane(i, &q)),
                None => out.extend_from_slice(&[q(0), 0.0, 0.0]),
            }
        }
    }
    for v in &verts[2..n - 1] {
        out.extend_from_slice(&[orient * (v.x - v0.x), orient * (v.y - v0.y)]);
    }
    n_fans
}

/// The sample context of a polygon.
fn sample_context<'a>(
    p: &'a PolygonSetup,
    bins: &'a ThreadBins,
    textures: &Textures<'a>,
    split: &'a [Cell<F32s>],
) -> SampleContext<'a> {
    let lights = p.first_light as usize..p.first_light as usize + p.light_count as usize;
    SampleContext {
        eye: p.eye,
        object: &p.object,
        params: &p.params,
        lights: &bins.lights[lights.clone()],
        light_split: &bins.light_split[lights],
        split,
        ambient: textures.ambient,
        focal: textures.focal,
        back: p.back,
    }
}

/// Starts a polygon's lattice for one block of rows: the column strips it covers there
/// (and the one after), with a tile for each, not yet built; and its smallest w there, a
/// lower bound for every tile's.
fn start_lattice(s: &mut RowScratch, p: &PolygonSetup, verts: &[SetupVertex], block: i32) -> Lattice {
    let block_top = s.viewport_y + block * BLOCK_ROWS;
    let (r0, r1) = (
        p.row_top.max(block_top),
        p.row_end.min(block_top + BLOCK_ROWS),
    );
    // Over the block's rows, a convex polygon's extremes are at its vertices there and its
    // edges' crossings of the rows' outer lines.
    let (ya, yb) = (r0 as f32, r1 as f32);
    let (mut w_min, mut x_min, mut x_max) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY);
    let n = verts.len();
    let mut take = |x: f32, w: f32| {
        w_min = w_min.min(w);
        x_min = x_min.min(x);
        x_max = x_max.max(x);
    };
    for i in 0..n {
        let (a, c) = (verts[i], verts[(i + 1) % n]);
        if (ya..=yb).contains(&a.y) {
            take(a.x, a.w);
        }
        for y in [ya, yb] {
            if a.y != c.y && (a.y.min(c.y)..=a.y.max(c.y)).contains(&y) {
                let t = (y - a.y) / (c.y - a.y);
                take(a.x + (c.x - a.x) * t, a.w + (c.w - a.w) * t);
            }
        }
    }
    let vx = s.viewport_x;
    let strip = |x: f32| (x.floor() as i32 - vx).div_euclid(TILE_COLS);
    let (first_strip, last_strip) = if x_min <= x_max {
        (strip(x_min), strip(x_max) + 1)
    } else {
        (0, 0)
    };
    let tiles = s.tiles.len() as u32;
    let strips = (last_strip - first_strip + 1) as u32;
    s.tiles
        .resize(s.tiles.len() + strips as usize, Tile::default());
    Lattice {
        started: true,
        first_strip,
        strips,
        tiles,
        w_min,
        rows: (r0, r1),
        ends: 0,
    }
}

/// Builds a polygon's lattice in one block (for the band's rows), and returns where its row
/// ends are (`Lattice::ends`). Every sample point is on the polygon:
///
/// - **Grid points:** each tile's spacing, then its grid rows, each holding the outputs of
///   `shade_sample` at the tile's grid columns inside the polygon on the rows it serves.
///   Where the polygon doesn't reach the grid row along a column, the point is moved along
///   the column onto the polygon: to where its edge crosses the column.
/// - **Row ends:** where each row crosses the polygon's edges, its values are those of the
///   edge's knots on either side: the edge's vertices, and where it crosses the grid's rows
///   and columns (of the tile it is in there).
///
/// The polygon's pixels on each row are found once, and all the points are evaluated
/// together, [`LANES`] at a time whatever they are: values from the polygon's plane
/// functions ([`fan_planes`]), one reciprocal of w per point.
#[allow(clippy::too_many_arguments)]
fn build_tiles(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    shaders: &[MaterialEntry],
    config: &RasterConfig,
    textures: &Textures,
    lattice: Lattice,
    id: u32,
) -> u32 {
    let (t, l) = split_id(id);
    let b = &bins[t];
    let p = &b.polygons[l];
    let entry = &shaders[p.material.0 as usize];
    let (n_in, n_out) = (p.n_vals as usize, p.n_out as usize);
    let range = p.first_vertex as usize..p.first_vertex as usize + p.vertex_count as usize;
    let (verts, lines) = (&b.vertices[range.clone()], &b.lines[range]);
    let planes = Planes::of(&b.planes[p.first_plane as usize..], p.fans as usize, n_in);
    let (vx, vy) = (s.viewport_x, s.viewport_y);
    let (block_r0, block_r1) = lattice.rows;
    // The rows this band draws, and the polygon's pixels on each.
    let (r0, r1) = (block_r0.max(s.band_rows.start), block_r1.min(s.band_rows.end));
    s.row_pixels.clear();
    s.row_edges.clear();
    for r in r0..r1 {
        let (pixels, edges) = match crossings(verts, lines, r) {
            Some(((li, xl), (ri, xr))) => {
                ((pixel_edge(xl), pixel_edge(xr) - 1), (li as u16, ri as u16))
            }
            None => ((i32::MAX, i32::MIN), (u16::MAX, u16::MAX)),
        };
        s.row_pixels.push(pixels);
        s.row_edges.push(edges);
    }
    // Where the polygon is along a grid column (at `x`): from where the column enters it
    // to where it leaves, on its edges.
    let column_span = |x: f32| {
        let (mut top, mut bottom) = (f32::INFINITY, f32::NEG_INFINITY);
        for i in 0..verts.len() {
            let (a, c) = (verts[i], verts[(i + 1) % verts.len()]);
            if x < a.x.min(c.x) || x > a.x.max(c.x) {
                continue;
            }
            let (ya, yc) = if a.x == c.x {
                (a.y, c.y)
            } else {
                let y = a.y + (c.y - a.y) * ((x - a.x) / (c.x - a.x));
                (y, y)
            };
            top = top.min(ya.min(yc));
            bottom = bottom.max(ya.max(yc));
        }
        (top, bottom)
    };

    let pow2 = |limit: f32, lo: i32, hi: i32| {
        let mut n = hi;
        while n > lo && n as f32 > limit {
            n /= 2;
        }
        n
    };
    let stride = if p.half_rate { 2 } else { 1 };
    let spacing = if p.light_count > 0 {
        entry.sample_spacing.min(config.light_spacing.max(1) as i32)
    } else {
        entry.sample_spacing
    } as f32;
    let min_step = config.min_step.clamp(1, MAX_STEP as u32) as f32;
    let w_at = |x: f32, y: f32| planes.w0 + planes.wx * (x - planes.ox) + planes.wy * (y - planes.oy);
    // The polygon's lights, whose spot lights' penumbra tightens the spacing of the tiles
    // it crosses (found from the world position among the polygon's values: it is affine on
    // the polygon, so the first fan triangle's planes hold everywhere).
    let lights = p.first_light as usize..p.first_light as usize + p.light_count as usize;
    let spots: &[Light] = match p.position {
        Some(_) if config.penumbra_threshold > 0.0 => &b.lights[lights],
        _ => &[],
    };
    let position_at = |x: f32, y: f32, w: f32| {
        let at = |k: usize| {
            let c = &planes.fans[k * 3..k * 3 + 3];
            (c[0] + c[1] * (x - planes.ox) + c[2] * (y - planes.oy)) / w
        };
        let k = p.position.unwrap_or(0) as usize;
        Vec3::new(at(k), at(k + 1), at(k + 2))
    };
    // How far a spot light's cone factor (0 outside, 1 inside, before easing) changes
    // across the rectangle, the most of any spot light's (at its corners and middle, where
    // w is safely positive, and in range).
    let penumbra = |xa: f32, xb: f32, ya: f32, yb: f32, w_min: f32| {
        let points = [(xa, ya), (xb, ya), (xa, yb), (xb, yb), ((xa + xb) / 2.0, (ya + yb) / 2.0)];
        spots
            .iter()
            .filter(|l| !l.is_point() && !l.beam && !l.coarse)
            .map(|l| {
                let (scale, offset) = l.cone();
                let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                for &(x, y) in &points {
                    let w = w_at(x, y);
                    if w <= w_min * 0.25 {
                        continue;
                    }
                    let d = l.position - position_at(x, y, w);
                    if d.length_squared() >= l.range * l.range {
                        continue;
                    }
                    let c = (offset - d.dot(l.direction) / d.length().max(1e-6) * scale)
                        .clamp(0.0, 1.0);
                    (lo, hi) = (lo.min(c), hi.max(c));
                }
                (hi - lo).max(0.0)
            })
            .fold(0.0, f32::max)
    };
    s.pending.clear();
    // Each strip's spacing first, so runs of strips with the same one can share grid rows.
    s.spacings.clear();
    for i in 0..lattice.strips {
        let xs = vx + (lattice.first_strip + i as i32) * TILE_COLS;
        // The polygon's smallest w in the tile, at least: w is a plane, so over the tile's
        // rectangle it is least at a corner; the block's smallest w bounds it too.
        let (xa, xb, ya, yb) = (
            xs as f32,
            (xs + TILE_COLS) as f32,
            block_r0 as f32,
            block_r1 as f32,
        );
        let corners = w_at(xa, ya).min(w_at(xb, ya)).min(w_at(xa, yb)).min(w_at(xb, yb));
        let w_min = corners.max(lattice.w_min);
        // Across a penumbra: cells over which the cone fades by at most the threshold. It is
        // measured past the tile by `penumbra_padding` on every side, as the fade needn't be
        // spread across the tile: a penumbra crossing just its corner fades fast there, which
        // the corners alone would take for a slow fade across all of it.
        let fade = if spots.is_empty() {
            0.0
        } else {
            let pad = config.penumbra_padding as f32;
            penumbra(xa - pad, xb + pad, ya - pad, yb + pad, w_min)
        };
        let spacing = if fade > 0.0 {
            spacing.min(
                (TILE_COLS as f32 * config.penumbra_threshold / fade)
                    .max(config.penumbra_spacing.max(1) as f32),
            )
        } else {
            spacing
        };
        // Spacing: the material's (twice as wide across at half rate, which shades every
        // other pixel; for lit polygons at most the light spacing), or less where w changes
        // by more than the threshold across a cell (relative to its smallest value on the
        // polygon in the tile), but never less than `min_step` unless w would change by more
        // than the steep limit across a cell.
        let relative = |limit: f32, dw: f32| {
            if dw != 0.0 && w_min.is_finite() {
                limit * w_min / dw.abs()
            } else {
                f32::INFINITY
            }
        };
        let floor = |dw: f32| min_step.min(relative(config.steep_limit, dw));
        let nx = pow2(
            relative(config.step_threshold, planes.wx)
                .min(spacing * stride as f32)
                .max(floor(planes.wx)),
            1,
            TILE_COLS,
        );
        let ny = pow2(
            relative(config.step_threshold, planes.wy)
                .min(spacing)
                .max(floor(planes.wy)),
            1,
            BLOCK_ROWS,
        );
        s.spacings.push((nx, ny, w_min));
    }
    // Probe and refine: tiles where the light bends between their sample points get closer
    // ones.
    if let Some(light) = p.light_out {
        let block = (block_r0 - vy).div_euclid(BLOCK_ROWS);
        let first = p.refine as usize
            + ((block - p.refine_block) * p.refine_strips as i32 + lattice.first_strip
                - p.refine_strip) as usize;
        let tiles = &b.refine[first..first + lattice.strips as usize];
        if tiles.iter().any(|t| t.load(Ordering::Relaxed) == 0) {
            probe_tiles(s, config, textures, lattice, p, b, entry, &planes, verts, tiles, light as usize);
        }
        for (spacing, tile) in s.spacings.iter_mut().zip(tiles) {
            let refined = tile.load(Ordering::Relaxed);
            if refined > 1 {
                let to = 1 << (refined - 2);
                (spacing.0, spacing.1) = (spacing.0.min(to), spacing.1.min(to));
            }
        }
    }
    // The polygon's pixels on the last grid rows looked at outside the band's rows (runs of
    // strips mostly share them).
    let mut outside_rows = [(i32::MIN, (0, 0)); 2];
    let mut outside_next = 0;
    let mut i = 0;
    while i < lattice.strips as usize {
        // A run of strips with the same spacing; the least w of any of them guards all.
        let (nx, ny, _) = s.spacings[i];
        let mut end = i + 1;
        while end < s.spacings.len() && (s.spacings[end].0, s.spacings[end].1) == (nx, ny) {
            end += 1;
        }
        let w_min = s.spacings[i..end]
            .iter()
            .map(|&(_, _, w)| w)
            .fold(f32::INFINITY, f32::min);
        let (first_strip, end_strip) = (
            lattice.first_strip + i as i32,
            lattice.first_strip + end as i32,
        );
        let (xs, xe) = (vx + first_strip * TILE_COLS, vx + end_strip * TILE_COLS);
        // A last guard: keep w off zero (every point is on the polygon, give or take rounding).
        let w_floor = if w_min.is_finite() { w_min / 8.0 } else { 0.0 };

        // Grid rows from the one at or above the polygon's first row in the block and the
        // band, to the one at or below its last there (the next block's first, at most).
        let first_row = r0 - (r0 - vy).rem_euclid(ny);
        let last = r1 - 1;
        let last_row = last + (ny - (last - vy).rem_euclid(ny)) % ny;
        let rows = s.grid_rows.len() as u32;
        // A row's columns of the run: those among its pixels, from `first` on.
        let columns = |first_px: i32, last_px: i32| {
            (
                xs + ((first_px - xs).max(0) + nx - 1) / nx * nx,
                (xs + (last_px - xs).div_euclid(nx) * nx).min(xe - nx),
            )
        };
        let mut g = first_row;
        while g <= last_row {
            // Its columns: the run's among the pixels of the rows it serves (those within
            // `ny` of it), each one's point where the polygon is nearest the grid row along
            // the column.
            let (mut first_px, mut last_px) = (i32::MAX, i32::MIN);
            let served = (g - ny + 1).max(r0)..(g + ny).min(r1);
            for &(a, c) in &s.row_pixels[(served.start - r0) as usize..(served.end - r0) as usize]
            {
                first_px = first_px.min(a);
                last_px = last_px.max(c);
            }
            let (col0, col_end) = columns(first_px, last_px);
            if first_px > last_px || col0 > col_end {
                s.grid_rows.push(GridRow::default());
                g += ny;
                continue;
            }
            let cols = (col_end - col0) / nx + 1;
            let values = s.grid_values.len();
            s.grid_values.resize(values + n_out * cols as usize, 0.0);
            let ys = s.grid_ys.len();
            let on_row = g as f32 + 0.5;
            // The polygon's own pixels on the grid row: the columns among them are on it.
            let (inside0, inside1) = if (r0..r1).contains(&g) {
                s.row_pixels[(g - r0) as usize]
            } else if let Some(&(_, pixels)) = outside_rows.iter().find(|&&(row, _)| row == g) {
                pixels
            } else {
                let pixels = match crossings(verts, lines, g) {
                    Some(((_, xl), (_, xr))) => (pixel_edge(xl), pixel_edge(xr) - 1),
                    None => (i32::MAX, i32::MIN),
                };
                outside_rows[outside_next % 2] = (g, pixels);
                outside_next += 1;
                pixels
            };
            let mut snapped = false;
            for j in 0..cols {
                let c = col0 + j * nx;
                let y = if (inside0..=inside1).contains(&c) {
                    on_row
                } else {
                    let (top, bottom) = column_span(c as f32 + 0.5);
                    if top <= bottom { on_row.max(top).min(bottom) } else { on_row }
                };
                snapped |= y != on_row;
                s.grid_ys.push(y);
                s.pending.push(Pending {
                    dx: c as f32 + 0.5 - planes.ox,
                    dy: y - planes.oy,
                    w_floor,
                    at: (values + j as usize) as u32,
                    stride: cols as u32,
                });
            }
            s.grid_rows.push(GridRow {
                col0,
                cols: cols as u32,
                values: values as u32,
                stride: cols as u32,
                ys: ys as u32,
                snapped,
            });
            g += ny;
        }
        let tile = Tile {
            built: true,
            first_strip,
            end_strip,
            nx,
            ny,
            first_row,
            rows,
        };
        for k in i..end {
            s.tiles[lattice.tiles as usize + k] = tile;
        }
        i = end;
    }

    // The row ends: each edge's knots in the block, in order along it, and the two on either
    // side of each row crossing it. They are the block's whatever the band (only the ones
    // its rows use are evaluated), so no band size changes a pixel.
    let ends = s.row_ends.len();
    s.row_ends.resize(ends + 2 * s.row_edges.len(), RowEnd::default());
    let block_top = vy + (block_r0 - vy).div_euclid(BLOCK_ROWS) * BLOCK_ROWS;
    let (block_y0, block_y1) = (block_top as f32 + 0.5, (block_top + BLOCK_ROWS) as f32 + 0.5);
    let w_floor = if lattice.w_min.is_finite() { lattice.w_min / 8.0 } else { 0.0 };
    let n = verts.len();
    s.vertex_values.clear();
    s.vertex_values.resize(n, u32::MAX);
    for e in 0..n {
        // The band's rows crossing the edge.
        let crosses = |&(l, r): &(u16, u16)| l as usize == e || r as usize == e;
        let Some(first) = s.row_edges.iter().position(crosses) else {
            continue;
        };
        let last = s.row_edges.iter().rposition(crosses).unwrap_or(first);
        let (a, c) = (verts[e], verts[(e + 1) % n]);
        let (dx, dy) = (c.x - a.x, c.y - a.y);
        let per_row = if dy != 0.0 { 1.0 / dy } else { 0.0 };
        let along = |y: f32| ((y - a.y) * per_row).clamp(0.0, 1.0);
        // Its stretch in the block: between its vertices there, or the block's first grid
        // row and the next block's (grid rows whatever the spacing).
        let (t0, t1) = (along(block_y0), along(block_y1));
        let (t_lo, t_hi) = (t0.min(t1), t0.max(t1));
        // Only the knots near the band's rows are needed: the nearest on either side of a
        // crossing is within a tile's width of it across, as the edge crosses a column at
        // least that often (or ends first).
        let (xa, xb) = (
            a.x + dx * along((r0 + first as i32) as f32 + 0.5),
            a.x + dx * along((r0 + last as i32) as f32 + 0.5),
        );
        let near = (xa.min(xb) - TILE_COLS as f32, xa.max(xb) + TILE_COLS as f32);
        let strip = |x: f32| {
            ((x - 0.5).floor() as i32 - vx).div_euclid(TILE_COLS) - lattice.first_strip
        };
        let last_strip = lattice.strips as i32 - 1;
        let near_strips = strip(near.0).clamp(0, last_strip)..=strip(near.1).clamp(0, last_strip);
        s.knots.clear();
        s.knots.extend([t_lo, t_hi]);
        for i in near_strips {
            let i = i as usize;
            let (nx, ny, _) = s.spacings[i];
            // The strip's stretch of the edge (the first and last strips' go on past them).
            let xs = vx + (lattice.first_strip + i as i32) * TILE_COLS;
            let strip_x0 = if i == 0 { f32::NEG_INFINITY } else { xs as f32 + 0.5 };
            let strip_x1 = if i + 1 == lattice.strips as usize {
                f32::INFINITY
            } else {
                (xs + TILE_COLS) as f32 + 0.5
            };
            let (mut u0, mut u1) = (t_lo, t_hi);
            if dx == 0.0 {
                if a.x < strip_x0 || a.x >= strip_x1 {
                    continue;
                }
            } else {
                let (p, q) = ((strip_x0 - a.x) / dx, (strip_x1 - a.x) / dx);
                (u0, u1) = (u0.max(p.min(q)), u1.min(p.max(q)));
            }
            if u0 >= u1 {
                continue;
            }
            // Where it crosses the strip's grid rows.
            let (ya, yb) = (a.y + dy * u0, a.y + dy * u1);
            let mut g = vy + ((ya.min(yb) - 0.5 - vy as f32) / ny as f32).ceil() as i32 * ny;
            while g as f32 + 0.5 <= ya.max(yb) {
                s.knots.push(along(g as f32 + 0.5));
                g += ny;
            }
            // Where it crosses the strip's grid columns.
            if dx != 0.0 {
                let (xa, xb) = (a.x + dx * u0, a.x + dx * u1);
                let (from, to) = (xa.min(xb), xa.max(xb));
                let mut col = vx + ((from - 0.5 - vx as f32) / nx as f32).ceil() as i32 * nx;
                while col as f32 + 0.5 <= to {
                    s.knots.push(((col as f32 + 0.5 - a.x) / dx).clamp(0.0, 1.0));
                    col += nx;
                }
            }
        }
        s.knots.sort_unstable_by(f32::total_cmp);
        s.knots.dedup();
        if s.knots.len() < 2 {
            s.knots.push(t_hi);
        }
        s.knot_values.clear();
        s.knot_values.resize(s.knots.len(), u32::MAX);
        // An edge going down the screen is the left end of the rows it crosses.
        let side = if dy > 0.0 { 0 } else { 1 };
        // The knots on either side of the last row's crossing: the next row's is mostly
        // between them too.
        let (mut i, mut at, mut ta, mut tb, mut per_t) = (0, [0; 2], 0.0, 0.0, 0.0);
        for k in first..=last {
            let (l, r) = s.row_edges[k];
            if [l, r][side] as usize != e {
                continue;
            }
            let t = along((r0 + k as i32) as f32 + 0.5);
            if i == 0 || t < ta || (t >= tb && i + 1 < s.knots.len()) {
                i = s.knots.partition_point(|&q| q <= t).clamp(1, s.knots.len() - 1);
                (ta, tb) = (s.knots[i - 1], s.knots[i]);
                per_t = if tb > ta { 1.0 / (tb - ta) } else { 0.0 };
                // The knots' outputs, each evaluated once (a vertex's for both its edges).
                for (j, at) in at.iter_mut().enumerate() {
                    let knot = s.knots[i - 1 + j];
                    let vertex = if knot == 0.0 {
                        Some(e)
                    } else if knot == 1.0 {
                        Some((e + 1) % n)
                    } else {
                        None
                    };
                    let known = vertex.map_or(s.knot_values[i - 1 + j], |v| s.vertex_values[v]);
                    *at = if known != u32::MAX {
                        known
                    } else {
                        let values = s.grid_values.len() as u32;
                        s.grid_values.resize(values as usize + n_out, 0.0);
                        s.pending.push(Pending {
                            dx: a.x + dx * knot - planes.ox,
                            dy: a.y + dy * knot - planes.oy,
                            w_floor,
                            at: values,
                            stride: 1,
                        });
                        values
                    };
                    s.knot_values[i - 1 + j] = *at;
                    if let Some(v) = vertex {
                        s.vertex_values[v] = *at;
                    }
                }
            }
            let f = ((t - ta) * per_t).clamp(0.0, 1.0);
            // For the overlay: the knot the row is on (within half a row), if any.
            let kind = |knot: f32| {
                if ((t - knot) * dy).abs() > 0.5 {
                    POINT_BETWEEN
                } else if knot == 0.0 || knot == 1.0 {
                    POINT_VERTEX
                } else {
                    POINT_KNOT
                }
            };
            let kind = if config.show_samples { kind(ta).max(kind(tb)) } else { POINT_BETWEEN };
            s.row_ends[ends + 2 * k + side] = RowEnd { a: at[0], b: at[1], f, kind };
        }
    }

    shade_pending(&s.pending, &mut s.grid_values, p, b, entry, &planes, textures);
    ends as u32
}

/// Probe and refine. Works out which of a polygon's tiles in a block need sample points
/// closer together than their spacing (`s.spacings`), and how close, and stores it in
/// `tiles` (see `ThreadBins::refine`) for every band that shades them.
///
/// A tile is probed over the polygon's part of it: the light (`light`: where the material's
/// `light` output is, and the split lights' strengths) is worked out at the part's corners
/// and at their mean, a point in the middle of it. Were the light linear across the part,
/// the mean of the corners' light would be the light at that point exactly, whatever the
/// part's shape; how far off it is says how far off pixels between sample points that far
/// apart would be. Past `RasterConfig::refine_threshold`, the tile's points are put closer by
/// as much as it is over (an error that halves with the spacing, as across a light's
/// terminator or a shadow's edge; a smooth bend shrinks faster), no closer than
/// `refine_spacing`.
///
/// Nothing here knows what the light is of: only that the material's own answer in the
/// middle isn't what its answers at the corners suggest. It depends on the polygon and the
/// tile alone, not on the band, so every band gets the same answer.
#[allow(clippy::too_many_arguments)]
fn probe_tiles(
    s: &mut RowScratch,
    config: &RasterConfig,
    textures: &Textures,
    lattice: Lattice,
    p: &PolygonSetup,
    b: &ThreadBins,
    entry: &MaterialEntry,
    planes: &Planes,
    verts: &[SetupVertex],
    tiles: &[AtomicU8],
    light: usize,
) {
    /// Most corners probed: with the point in the middle, one batch of sample points.
    const CORNERS: usize = LANES - 1;
    let n_out = p.n_out as usize;
    let floor = (config.refine_spacing.max(1) as i32).min(MAX_STEP);
    let block_top = s.viewport_y + (lattice.rows.0 - s.viewport_y).div_euclid(BLOCK_ROWS) * BLOCK_ROWS;
    let (top, bottom) = (block_top as f32, (block_top + BLOCK_ROWS) as f32);
    let w_floor = if lattice.w_min.is_finite() { lattice.w_min / 8.0 } else { 0.0 };
    s.probes.clear();
    s.probe_corners.clear();
    s.probed.clear();
    // Which way round the polygon goes, to tell a point inside it.
    let corner = |k: usize| Vec2::new(verts[k % verts.len()].x, verts[k % verts.len()].y);
    let turn = (1..verts.len() - 1)
        .map(|k| (corner(k) - corner(0)).perp_dot(corner(k + 1) - corner(0)))
        .sum::<f32>()
        .signum();
    let inside = |q: Vec2| {
        (0..verts.len()).all(|k| (corner(k + 1) - corner(k)).perp_dot(q - corner(k)) * turn >= 0.0)
    };
    for (i, tile) in tiles.iter().enumerate() {
        if tile.load(Ordering::Relaxed) != 0 {
            continue;
        }
        // Already as close as probing would put them: nothing to find out.
        let (nx, ny, _) = s.spacings[i];
        if nx.max(ny) <= floor {
            tile.store(1, Ordering::Relaxed);
            continue;
        }
        // The polygon's part of the tile: all of it, or the polygon clipped to its four
        // sides in turn.
        let left = (s.viewport_x + (lattice.first_strip + i as i32) * TILE_COLS) as f32;
        let right = left + TILE_COLS as f32;
        let whole = [
            Vec2::new(left, top),
            Vec2::new(right, top),
            Vec2::new(right, bottom),
            Vec2::new(left, bottom),
        ];
        s.piece.clear();
        if whole.iter().all(|&q| inside(q)) {
            s.piece.extend(whole);
        } else {
            s.piece.extend((0..verts.len()).map(corner));
            let sides = [(Vec2::X, left), (Vec2::NEG_X, -right), (Vec2::Y, top), (Vec2::NEG_Y, -bottom)];
            for (inward, at) in sides {
                s.piece_next.clear();
                for (k, &a) in s.piece.iter().enumerate() {
                    let c = s.piece[(k + 1) % s.piece.len()];
                    let (da, dc) = (inward.dot(a) - at, inward.dot(c) - at);
                    if da >= 0.0 {
                        s.piece_next.push(a);
                    }
                    if (da < 0.0) != (dc < 0.0) {
                        s.piece_next.push(a + (c - a) * (da / (da - dc)));
                    }
                }
                std::mem::swap(&mut s.piece, &mut s.piece_next);
            }
        }
        let (lo, hi) = s
            .piece
            .iter()
            .fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), &q| (lo.min(q), hi.max(q)));
        let size = (hi - lo).max_element();
        // Too small a part for points any closer to fall in it.
        if s.piece.len() < 3 || size <= floor as f32 {
            tile.store(1, Ordering::Relaxed);
            continue;
        }
        // Its corners (evenly picked, where there are too many), then their mean. A corner
        // the part before it has too (on the tiles' shared side) is probed once.
        let before = match s.probed.last() {
            Some(&(strip, first, n, ..)) if strip + 1 == i => first..first + n,
            _ => 0..0,
        };
        let (first, n) = (s.probe_corners.len(), s.piece.len().min(CORNERS));
        let mut mean = Vec2::ZERO;
        let point = |q: Vec2, probes: &mut Vec<Pending>| {
            probes.push(Pending { dx: q.x - planes.ox, dy: q.y - planes.oy, w_floor, at: 0, stride: 1 });
            probes.len() as u32 - 1
        };
        for k in 0..n {
            let q = s.piece[k * s.piece.len() / n];
            mean += q / n as f32;
            let (dx, dy) = (q.x - planes.ox, q.y - planes.oy);
            let shared = s.probe_corners[before.clone()].iter().copied().find(|&at| {
                let other = s.probes[at as usize];
                (other.dx - dx).abs() < 1e-3 && (other.dy - dy).abs() < 1e-3
            });
            let at = shared.unwrap_or_else(|| point(q, &mut s.probes));
            s.probe_corners.push(at);
        }
        let middle = point(mean, &mut s.probes) as usize;
        s.probed.push((i, first, n, middle, size));
    }
    if s.probes.is_empty() {
        return;
    }
    for (k, probe) in s.probes.iter_mut().enumerate() {
        probe.at = (k * n_out) as u32;
    }
    s.probe_values.clear();
    s.probe_values.resize(s.probes.len() * n_out, 0.0);
    shade_pending(&s.probes, &mut s.probe_values, p, b, entry, planes, textures);
    // The light's values: the material's, then each split light's strength.
    let n_material = n_out - split_outputs(p.splits as usize);
    let lights = (light..light + 3).chain(n_material..n_out);
    for &(i, first, n, middle, size) in &s.probed {
        let at = |point: usize, k: usize| s.probe_values[point * n_out + k];
        let corners = &s.probe_corners[first..first + n];
        let off = lights
            .clone()
            .map(|k| {
                let mean = corners.iter().map(|&point| at(point as usize, k)).sum::<f32>() / n as f32;
                (at(middle, k) - mean).abs()
            })
            .fold(0.0, f32::max)
            * 255.0;
        let refined = if off > config.refine_threshold {
            let mut to = MAX_STEP;
            while to > floor && to as f32 > size * config.refine_threshold / off {
                to /= 2;
            }
            2 + to.trailing_zeros() as u8
        } else {
            1
        };
        tiles[i].store(refined, Ordering::Relaxed);
    }
}

/// Runs `shade_sample` at sample points of polygon `p` (`pending`), [`LANES`] at a time, and
/// stores their outputs in `values`: their `sampled` values from the polygon's plane
/// functions ([`fan_planes`]), one reciprocal of w per point; the material's outputs, then
/// the split lights' (see `SampleContext::light_split`).
fn shade_pending(
    pending: &[Pending],
    values: &mut [f32],
    p: &PolygonSetup,
    b: &ThreadBins,
    entry: &MaterialEntry,
    planes: &Planes,
    textures: &Textures,
) {
    let (n_in, n_out) = (p.n_vals as usize, p.n_out as usize);
    let (splits, n_material) = (p.splits as usize, n_out - split_outputs(p.splits as usize));
    let split: [Cell<F32s>; MAX_SPLIT] = Default::default();
    let mut inputs = [F32s::default(); MAX_VARYINGS];
    let mut outputs = [F32s::default(); MAX_VARYINGS];
    let (zero, one) = (F32s::fill(0.0), F32s::fill(1.0));
    for chunk in pending.chunks(LANES) {
        // Lanes past the points repeat the last one.
        let lane = |f: fn(&Pending) -> f32| {
            F32s::from(std::array::from_fn::<f32, LANES, _>(|j| {
                f(&chunk[j.min(chunk.len() - 1)])
            }))
        };
        let (dx, dy) = (lane(|q| q.dx), lane(|q| q.dy));
        let w = (F32s::fill(planes.w0) + F32s::fill(planes.wx) * dx + F32s::fill(planes.wy) * dy)
            .max(lane(|q| q.w_floor));
        let inv = one / w;
        // Each lane's fan triangle: how many diagonals it is past.
        let mut fan = zero;
        for d in planes.diagonals.chunks_exact(2) {
            let past = (F32s::fill(d[0]) * dy - F32s::fill(d[1]) * dx).simd_gt(zero);
            fan += past.select(one, zero);
        }
        for (k, input) in inputs[..n_in].iter_mut().enumerate() {
            let at = |t: usize| {
                let c = &planes.fans[(t * n_in + k) * 3..(t * n_in + k) * 3 + 3];
                F32s::fill(c[0]) + F32s::fill(c[1]) * dx + F32s::fill(c[2]) * dy
            };
            let mut q = at(0);
            for t in 1..planes.n_fans {
                let here = fan.simd_eq(F32s::fill(t as f32));
                if here.to_bitmask() != 0 {
                    q = here.select(at(t), q);
                }
            }
            *input = q * inv;
        }
        // The level of detail, worked out at each point rather than interpolated from the
        // corners (which, across a big polygon seen at a grazing angle, comes out far too
        // sharp in the middle: the true one grows with the log of the distance).
        if let Some((uv, lod, width, height)) = p.lod {
            let (uv, lod) = (uv as usize, lod as usize);
            // The gradients of u w and v w (the fan triangle's planes), and w's.
            let gradient = |k: usize| {
                let c = |t: usize| &planes.fans[(t * n_in + k) * 3..(t * n_in + k) * 3 + 3];
                let (mut gx, mut gy) = (F32s::fill(c(0)[1]), F32s::fill(c(0)[2]));
                for t in 1..planes.n_fans {
                    let here = fan.simd_eq(F32s::fill(t as f32));
                    if here.to_bitmask() != 0 {
                        gx = here.select(F32s::fill(c(t)[1]), gx);
                        gy = here.select(F32s::fill(c(t)[2]), gy);
                    }
                }
                (gx, gy)
            };
            inputs[lod] = point_lods(
                gradient(uv),
                gradient(uv + 1),
                (F32s::fill(planes.wx), F32s::fill(planes.wy)),
                [inputs[uv], inputs[uv + 1]],
                inv,
                (width, height),
            );
        }
        let ctx = sample_context(p, b, textures, &split[..splits]);
        (entry.sample)(&inputs[..n_in], &ctx, &mut outputs[..n_material]);
        for (j, strength) in split[..splits].iter().enumerate() {
            outputs[n_material + j] = strength.take().max(zero).sqrt();
        }
        for (k, out) in outputs[..n_out].iter().enumerate() {
            let out = out.to_array();
            for (j, q) in chunk.iter().enumerate() {
                values[(q.at + k as u32 * q.stride) as usize] = out[j];
            }
        }
    }
}


/// A sample point waiting to be evaluated (see [`build_tiles`]): where it is relative to the
/// polygon's plane functions' origin, the least w allowed there, and where its first output
/// goes in `RowScratch::grid_values` (the rest `stride` apart).
#[derive(Clone, Copy)]
struct Pending {
    dx: f32,
    dy: f32,
    w_floor: f32,
    at: u32,
    stride: u32,
}

/// Fills `s.points_x` and `s.points_v` with a polygon's sample points on the current row
/// for pixels `x0..x1` (`x0` being the first pixel they show: at half rate, the first
/// pair's), the row crossing the polygon's edges at `x_left` and `x_right`: the row's
/// points from the one at or before `x0` to the one at or after `x1 - 1`. The row's points
/// are its left end (on the edge, between the edge's knots), the grid columns among its
/// pixels, tile by tile, each tile's outputs blended between its grid rows above and below
/// (output by output), and its right end. They are the row's, so no pixel depends on how
/// the row was split.
fn row_points(
    s: &mut RowScratch,
    index: usize,
    n_out: usize,
    x0: i32,
    x1: i32,
    (x_left, x_right): (f32, f32),
    kinds: bool,
) {
    let lattice = s.lattices[index];
    let (vx, row) = (s.viewport_x, s.row);
    s.points_x.clear();
    s.points_kind.clear();
    s.segments.clear();
    let (first_px, last_px) = (pixel_edge(x_left), pixel_edge(x_right) - 1);
    let ends = lattice.ends as usize
        + 2 * (row - lattice.rows.0.max(s.band_rows.start)) as usize;
    let strips = lattice.first_strip..lattice.first_strip + lattice.strips as i32;
    // Where the row's ends are among the points, if the run reaches them.
    let (mut left, mut right) = (None, None);
    // The columns, tile by tile: each tile's run of them, from its two grid rows.
    let mut strip = (x0 - vx).div_euclid(TILE_COLS).clamp(strips.start, strips.end - 1);
    let mut first = true;
    loop {
        if strip >= strips.end {
            right = Some(s.points_x.len());
            s.points_x.push(x_right);
            break;
        }
        let tile = s.tiles[(lattice.tiles as i32 + strip - lattice.first_strip) as usize];
        debug_assert!(tile.built);
        let (nx, ny) = (tile.nx, tile.ny);
        let g = row - (row - s.viewport_y).rem_euclid(ny);
        let fy = (row - g) as f32 / ny as f32;
        let above = tile.rows as usize + ((g - tile.first_row) / ny) as usize;
        let a = s.grid_rows[above];
        let b = if fy > 0.0 { s.grid_rows[above + 1] } else { a };
        // The run of strips sharing this tile's grid, from the strip holding x0 (or its
        // first) to its end.
        let (xs, xe) = (vx + tile.first_strip * TILE_COLS, vx + tile.end_strip * TILE_COLS);
        let mut c = xs;
        if first {
            // From the column at or before x0, or the row's left end where that is before
            // the row.
            first = false;
            c = xs + (x0 - xs).max(0) / nx * nx;
            if c < first_px {
                left = Some(s.points_x.len());
                s.points_x.push(x_left);
                c += nx;
            }
        }
        let (start, c0) = (s.points_x.len(), c);
        let mut done = false;
        while c < xe && !done {
            if c > last_px {
                // Past the row's last pixel: its right end closes the last interval.
                right = Some(s.points_x.len());
                done = true;
            } else {
                s.points_x.push(c as f32 + 0.5);
                done = c >= x1 - 1;
                c += nx;
            }
        }
        let count = s.points_x.len() - start;
        if right.is_some() {
            s.points_x.push(x_right);
        }
        if count > 0 {
            debug_assert!(
                [a, b].iter().all(|r| c0 >= r.col0
                    && c0 + (count as i32 - 1) * nx < r.col0 + r.cols as i32 * nx),
                "row {row}: columns from {c0} outside their grid rows"
            );
            s.segments.push(Segment {
                start: start as u32,
                count: count as u32,
                a,
                b,
                ia: ((c0 - a.col0) / nx) as u32,
                ib: ((c0 - b.col0) / nx) as u32,
                fy,
            });
        }
        if done {
            break;
        }
        strip = tile.end_strip;
    }
    // Outputs, output by output, each tile's run blended between its grid rows at once.
    let m = s.points_x.len();
    if kinds {
        s.points_kind.resize(m, POINT_BETWEEN);
    }
    s.points_v.clear();
    s.points_v.resize(n_out * m, 0.0);
    let y = row as f32 + 0.5;
    for seg in &s.segments {
        let (start, count) = (seg.start as usize, seg.count as usize);
        let (a, b) = (seg.a, seg.b);
        // How far each column's point is from the one above to the one below: the same for
        // all of them unless some were moved onto the polygon's edges.
        let moved = a.snapped || b.snapped;
        if moved {
            let ya = &s.grid_ys[a.ys as usize + seg.ia as usize..][..count];
            let yb = &s.grid_ys[b.ys as usize + seg.ib as usize..][..count];
            s.weights.clear();
            s.weights.extend(ya.iter().zip(yb).map(|(&ya, &yb)| {
                if yb > ya { ((y - ya) / (yb - ya)).clamp(0.0, 1.0) } else { 0.0 }
            }));
            if kinds {
                let kinds = &mut s.points_kind[start..start + count];
                for ((kind, &ya), &yb) in kinds.iter_mut().zip(ya).zip(yb) {
                    let near = (y - ya).abs().min((y - yb).abs());
                    *kind = if near == 0.0 {
                        POINT_GRID
                    } else if near <= 0.5 {
                        POINT_KNOT
                    } else {
                        POINT_BETWEEN
                    };
                }
            }
        } else if kinds && seg.fy == 0.0 {
            s.points_kind[start..start + count].fill(POINT_GRID);
        }
        for k in 0..n_out {
            let va = &s.grid_values[a.values as usize + k * a.stride as usize + seg.ia as usize..]
                [..count];
            let vb = &s.grid_values[b.values as usize + k * b.stride as usize + seg.ib as usize..]
                [..count];
            let out = &mut s.points_v[k * m + start..k * m + start + count];
            if moved {
                for (((o, &p), &q), &f) in out.iter_mut().zip(va).zip(vb).zip(&s.weights) {
                    *o = p + (q - p) * f;
                }
            } else {
                for ((o, &p), &q) in out.iter_mut().zip(va).zip(vb) {
                    *o = p + (q - p) * seg.fy;
                }
            }
        }
    }
    // The ends: between their edges' knots.
    for (side, at) in [left, right].into_iter().enumerate() {
        let Some(at) = at else { continue };
        let end = s.row_ends[ends + side];
        if kinds {
            s.points_kind[at] = end.kind;
        }
        let (va, vb) = (&s.grid_values[end.a as usize..][..n_out], &s.grid_values[end.b as usize..][..n_out]);
        for (k, (&p, &q)) in va.iter().zip(vb).enumerate() {
            s.points_v[k * m + at] = p + (q - p) * end.f;
        }
    }
}

/// One tile's run of a row's sample points (see [`row_points`]): where they are among the
/// row's points, the tile's grid rows above and below, where the run starts in each, and
/// how far the row is from the one above to the one below.
#[derive(Clone, Copy)]
struct Segment {
    start: u32,
    count: u32,
    a: GridRow,
    b: GridRow,
    ia: u32,
    ib: u32,
    fy: f32,
}

/// A 4×4 ordered (Bayer) dither: a pixel of a dithered beam is lit where the light reaching
/// it is over its entry (plus a half, over 16) at its row and column, modulo 4.
const BEAM_DITHER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Pixels between the points where a soft shadow piece's light is worked out (see
/// `soft_stretch`), and how far from straight between them it may bend, halfway, to be
/// taken straight across them (well under one level of an 8-bit color).
const SOFT_STEP: i32 = 8;
const SOFT_BEND: f32 = 1.0 / 512.0;

/// A soft shadow piece's light at pixels `from..to` of a row into `out`, `at(x)` being it at
/// pixel `x`: at every `SOFT_STEP`th pixel, and between two of them straight across where
/// it is that, halfway, to within `SOFT_BEND` (a penumbra's light changes smoothly), or
/// else at each pixel (a narrow soft edge, a hard one, or two edges meeting). Every value
/// comes from the edges' planes themselves, so nothing depends on how the carver cut the
/// pieces.
fn soft_stretch(out: &mut Vec<f32>, from: i32, to: i32, at: impl Fn(i32) -> f32) {
    if from >= to {
        return;
    }
    let (mut a, mut fa) = (from, at(from));
    while a < to - 1 {
        let b = (a + SOFT_STEP).min(to - 1);
        let fb = at(b);
        let straight = b - a < 2 || {
            let m = a + (b - a) / 2;
            let between = fa + (fb - fa) * (m - a) as f32 / (b - a) as f32;
            (at(m) - between).abs() <= SOFT_BEND
        };
        out.push(fa);
        for x in a + 1..b {
            out.push(if straight { fa + (fb - fa) * (x - a) as f32 / (b - a) as f32 } else { at(x) });
        }
        (a, fa) = (b, fb);
    }
    out.push(fa);
}

/// Pixels between the points where the shadow buffer checks a beam's cone (see
/// `shadow_run`).
const BEAM_STEP: i32 = 8;

/// The most a beam's cone may change across a stretch of `BEAM_STEP` pixels to be blended
/// between its ends (see `shadow_run`). Blending strays from it by at most about 0.75 times
/// the square of that (the ease's bend): about 1% of the light.
const BEAM_BLEND: f32 = 1.0 / 8.0;

/// Fills `s.reaches` with how much of each of the polygon's split lights reaches each pixel
/// `x0..x1` of the current row: all of it, except where its shadow pieces cover the row,
/// which each give their vertices' values interpolated along their edges to the row, then
/// across it (perspective-correct, like Gouraud shading). A beam's piece fades it by its
/// cone too, and with `dither`, each of its pixels gets all of the light or none, by
/// `BEAM_DITHER`.
fn shadow_run(
    s: &mut RowScratch,
    bins: &ThreadBins,
    p: &PolygonSetup,
    x0: i32,
    x1: i32,
    dither: bool,
    smooth: Option<(f32, f32)>,
) {
    let len = (x1 - x0).max(0) as usize;
    s.shadow_edges.clear();
    s.reaches.clear();
    s.reaches.resize(p.splits as usize * len, 1.0);
    s.occluded.clear();
    s.occluded.resize(p.splits as usize * len, 1.0);
    let row = s.row;
    for piece in &bins.shadows[p.first_shadow as usize..][..p.shadows as usize] {
        if row < piece.row_top || row >= piece.row_end {
            continue;
        }
        let range = piece.first as usize..piece.first as usize + piece.count as usize;
        let (verts, lines, light, rays, world) = (
            &bins.shadow_vertices[range.clone()],
            &bins.shadow_lines[range.clone()],
            &bins.shadow_light[range.clone()],
            &bins.shadow_rays[range.clone()],
            &bins.shadow_world[range],
        );
        let softs = &bins.shadow_softs[piece.softs.0 as usize..piece.softs.1 as usize];
        let Some(((il, xl), (ir, xr))) = crossings(verts, lines, row) else {
            continue;
        };
        let (from, to) = (pixel_edge(xl).max(x0), pixel_edge(xr).min(x1));
        if from >= to {
            continue;
        }
        // In an occluder's shadow, to be blurred: dark in the occlusion buffer, and lit
        // (by a beam's cone, inside its pyramid) here.
        if piece.occluded {
            let split = piece.split as usize * len;
            s.occluded[split + (from - x0) as usize..split + (to - x0) as usize].fill(0.0);
            if piece.beam.is_none() {
                continue;
            }
        }
        // At each crossing: w, the light times w, the ray times w, and the world position
        // times w (all linear across the row).
        let at = |i: usize| {
            let (w, alpha) = edge_at_row(verts, i, row);
            let k = (i + 1) % verts.len();
            let l = light[i] + (light[k] - light[i]) * alpha;
            let ray = rays[i] + (rays[k] - rays[i]) * alpha;
            let p = world[i] + (world[k] - world[i]) * alpha;
            (w, l * w, ray * w, p * w)
        };
        let ((wl, ql, rl, pl), (wr, qr, rr, pr)) = (at(il), at(ir));
        let out = &mut s.reaches[piece.split as usize * len..(piece.split as usize + 1) * len];
        let span = (xr - xl).max(1e-6);
        let t = |x: i32| (x as f32 + 0.5 - xl) / span;
        // In soft edges: worked out at the pixels' world positions, across its stretch of
        // the row (see `soft_stretch`). Otherwise the same at every vertex (in full shadow,
        // or only in a beam), the same all over.
        let flat = (softs.is_empty() && light.iter().all(|&l| l == light[0])).then_some(light[0]);
        // A full shadow's edges, to be smoothed, but not where they are the polygon's own.
        if let Some((x_left, x_right)) = smooth
            && piece.dark
        {
            let split = piece.split;
            for (i, x, out, own) in [(il, xl, -1.0, x_left), (ir, xr, 1.0, x_right)] {
                let slope = edge_slope(verts, lines, i);
                if (x - own).abs() > 1e-3 && slope.dxdy.is_finite() {
                    s.shadow_edges.push(ShadowEdge { split, x, slope, side: out });
                }
            }
        }
        let soft_row = &mut s.soft_row;
        soft_row.clear();
        if !softs.is_empty() {
            let at = |x: i32| {
                let t = t(x);
                let w = wl + (wr - wl) * t;
                if w > 0.0 { moose_view::soft_reach(softs, &bins.shadow_wedges, (pl + (pr - pl) * t) / w) } else { 0.0 }
            };
            soft_stretch(soft_row, from, to, at);
        }
        let soft_row = &*soft_row;
        let reach = |x: i32| match flat {
            Some(l) => l,
            None if !soft_row.is_empty() => soft_row[(x - from) as usize],
            None => {
                let t = t(x);
                let w = wl + (wr - wl) * t;
                if w > 0.0 { ((ql + (qr - ql) * t) / w).clamp(0.0, 1.0) } else { 0.0 }
            }
        };
        // Lowers the buffer to the piece's value over pixels `a..b`: in bulk where the
        // piece is the same all over (untouched where that is all of the light).
        let lower = |out: &mut [f32], a: i32, b: i32| {
            let run = &mut out[(a - x0) as usize..(b - x0) as usize];
            match flat {
                Some(l) if l >= 1.0 => {}
                Some(l) if l <= 0.0 => run.fill(0.0),
                Some(l) => run.iter_mut().for_each(|o| *o = o.min(l)),
                None => {
                    for (x, o) in (a..b).zip(run) {
                        *o = o.min(reach(x));
                    }
                }
            }
        };
        let Some((scale, offset)) = piece.beam else {
            lower(out, from, to);
            continue;
        };
        // A beam's cone, by the angle between the pixel's ray and the axis (x), eased like
        // the cone at sample points. The row crosses each cone in one stretch (a cone is
        // convex), found where the ray's angle from the axis equals the cone's: outside the
        // outer cone it is dark, inside the inner one it is all lit, and only between, in
        // the fade, is the cone taken: every `BEAM_STEP` pixels, blended between where it
        // can change by at most `BEAM_BLEND` (by the angle between their rays, at its
        // steepest), otherwise pixel by pixel.
        let (cos_outer, cos_inner) = (-offset / scale, (1.0 - offset) / scale);
        // The pixels whose rays are within the cone of cosine `k`: where
        // `ray.x^2 - k^2 |ray|^2 >= 0`, a quadratic along the row, and `ray.x > 0`. Empty as
        // `(to, to)`.
        let within = |k: f32| {
            let (p, q) = (rl.as_dvec3(), (rr - rl).as_dvec3());
            let k2 = (k as f64) * (k as f64);
            let a = q.x * q.x - k2 * q.length_squared();
            let b = 2.0 * (p.x * q.x - k2 * p.dot(q));
            let c = p.x * p.x - k2 * p.length_squared();
            let forward = |t: f64| p.x + q.x * t > 0.0;
            let disc = b * b - 4.0 * a * c;
            let (lo, hi) = if a.abs() <= 1e-12 * (b.abs() + c.abs()) {
                // Along a line parallel to one of the cone's: once across, if at all.
                if b > 0.0 {
                    (-c / b, f64::INFINITY)
                } else if b < 0.0 {
                    (f64::NEG_INFINITY, -c / b)
                } else if c >= 0.0 {
                    (f64::NEG_INFINITY, f64::INFINITY)
                } else {
                    return (to, to);
                }
            } else if disc < 0.0 {
                if a < 0.0 {
                    return (to, to);
                }
                (f64::NEG_INFINITY, f64::INFINITY)
            } else {
                let root = disc.sqrt();
                let (r0, r1) = ((-b - root) / (2.0 * a), (-b + root) / (2.0 * a));
                let (r0, r1) = (r0.min(r1), r0.max(r1));
                if a < 0.0 {
                    (r0, r1)
                } else if forward(r0) {
                    // The other side is the cone's reflection, behind the light.
                    (f64::NEG_INFINITY, r0)
                } else {
                    (r1, f64::INFINITY)
                }
            };
            // Only ahead of the light (a beam too wide for a pyramid may reach behind it).
            let (lo, hi) = if q.x > 0.0 {
                (lo.max(-p.x / q.x), hi)
            } else if q.x < 0.0 {
                (lo, hi.min(-p.x / q.x))
            } else if p.x > 0.0 {
                (lo, hi)
            } else {
                return (to, to);
            };
            let x = |t: f64| {
                let v = xl as f64 + t.clamp(-1e6, 1e6) * span as f64;
                pixel_edge(v as f32).clamp(from, to)
            };
            let (a, b) = (x(lo), x(hi));
            if a >= b { (to, to) } else { (a, b) }
        };
        let (o0, o1) = within(cos_outer);
        let (i0, i1) = within(cos_inner);
        let (i0, i1) = if i0 >= i1 { (o0, o0) } else { (i0.max(o0), i1.min(o1)) };
        // The most the eased cone changes per radian: its steepest (1.5) times the cosine's.
        let steepest = 1.5 * scale * (1.0 - cos_outer * cos_outer).max(0.0).sqrt();
        let ray = |x: i32| {
            let r = rl + (rr - rl) * t(x);
            r / r.length().max(1e-12)
        };
        let cone = |cos: f32| {
            let c = (cos * scale + offset).clamp(0.0, 1.0);
            c * c * (3.0 - 2.0 * c)
        };
        let fade = |out: &mut [f32], from: i32, to: i32| {
            let mut a = from;
            while a < to {
                let b = (a + BEAM_STEP).min(to - 1).max(a);
                let end = if b == to - 1 { to } else { b };
                let (ra, rb) = (ray(a), ray(b));
                if b > a && (ra - rb).length() * steepest <= BEAM_BLEND {
                    let (ka, kb) = (cone(ra.x), cone(rb.x));
                    for x in a..end {
                        let k = ka + (kb - ka) * (x - a) as f32 / (b - a) as f32;
                        let o = &mut out[(x - x0) as usize];
                        *o = o.min(reach(x) * k);
                    }
                } else {
                    for x in a..end {
                        let o = &mut out[(x - x0) as usize];
                        *o = o.min(reach(x) * cone(ray(x).x));
                    }
                }
                a = end;
            }
        };
        out[(from - x0) as usize..(o0 - x0) as usize].fill(0.0);
        // Dithered: each pixel all of the light or none, where it may be partly lit (in the
        // fade, and within the inner cone unless the piece is all lit or all dark there).
        let thresholds = &BEAM_DITHER[(row & 3) as usize];
        let stipple = |out: &mut [f32], a: i32, b: i32| {
            if dither {
                for x in a..b {
                    let o = &mut out[(x - x0) as usize];
                    let threshold = (thresholds[(x & 3) as usize] as f32 + 0.5) / 16.0;
                    *o = if *o > threshold { 1.0 } else { 0.0 };
                }
            }
        };
        fade(out, o0, i0);
        stipple(out, o0, i0);
        lower(out, i0, i1.max(i0));
        if !matches!(flat, Some(l) if l <= 0.0 || l >= 1.0) {
            stipple(out, i0, i1.max(i0));
        }
        fade(out, i1.max(o0), o1);
        stipple(out, i1.max(o0), o1);
        out[(o1.max(o0) - x0) as usize..(to - x0) as usize].fill(0.0);
    }
    if smooth.is_some() {
        smooth_shadow_edges(s, bins, p, x0, x1);
    }
}

/// Smooths full shadows' edges in the shadow buffer where no penumbra a pixel wide is next
/// to them (hard shadows, a soft edge's narrow end, or a far one). For each pixel near an
/// edge, the light is taken one and two pixels out from the nearest point of the edge,
/// square to it: where it is no brighter at two than at
/// one, there is less than a pixel of penumbra, and each pixel the edge passes through
/// gets that light for the part of it out of the shadow (its area there, a box filter),
/// and pixels further out, to a pixel from the edge, at least that light (a penumbra
/// narrower than that is taken in). Where the light is still rising, a penumbra wide enough
/// to see is there, and the smoothing fades out (fully at half the light at one pixel as at
/// two). Where full shadow goes on past the edge (no light a pixel out), nothing changes,
/// so pieces meeting show no seam. Only within the run.
fn smooth_shadow_edges(s: &mut RowScratch, bins: &ThreadBins, p: &PolygonSetup, x0: i32, x1: i32) {
    let len = (x1 - x0) as usize;
    let y = s.row as f32 + 0.5;
    for e in &s.shadow_edges {
        let ShadowEdge { split, x, slope, side } = *e;
        let dxdy = slope.dxdy;
        let out_dir = Vec2::new(1.0, -dxdy).normalize() * side;
        let out = &mut s.reaches[split as usize * len..(split as usize + 1) * len];
        // From the pixels the edge passes through, to a pixel out from it (square to it),
        // along the row: a long way for a shallow edge.
        let along = (1.0 + dxdy * dxdy).sqrt();
        let reach = edge_reach(out_dir);
        let (a, b) = (x - side * reach * along, x + side * along);
        for px in pixel_edge(a.min(b)).max(x0)..pixel_edge(a.max(b)).min(x1) {
            // How far out of the shadow its center is, square to the edge, and the nearest
            // point of the edge's line, which must be on the edge.
            let center = Vec2::new(px as f32 + 0.5, y);
            let d = (center.x - x) * side / along;
            let foot = center - d * out_dir;
            if foot.y < slope.top || foot.y > slope.bottom {
                continue;
            }
            // The light past the edge there, and how hard the edge is.
            let at = |k: f32| light_at(bins, p, split, foot.x + out_dir.x * k, foot.y + out_dir.y * k);
            let Some(light) = at(1.0).filter(|&l| l > 0.0) else { continue };
            let hard = match at(2.0) {
                Some(further) if further > light => ((light / further - 0.5) / 0.4).clamp(0.0, 1.0),
                _ => 1.0,
            };
            let lit = covered(d, out_dir) * light;
            let o = &mut out[(px - x0) as usize];
            let smooth = if (0.0..reach).contains(&d) { lit } else { o.max(lit) };
            *o += (smooth - *o) * hard;
        }
    }
}

/// How much of split light `split` reaches screen point (`x`, `y`) of polygon `p`, as its
/// shadow pieces give it in the shadow buffer (see [`shadow_run`]; not counting blurred
/// casters' shadows): the least of the pieces holding it, or all of it in none. `None`
/// off the polygon.
fn light_at(bins: &ThreadBins, p: &PolygonSetup, split: u8, x: f32, y: f32) -> Option<f32> {
    let q = Vec2::new(x, y);
    let corner = |v: &SetupVertex| Vec2::new(v.x, v.y);
    // Within a convex outline, whichever way round it goes.
    let inside = |vs: &[SetupVertex]| {
        let (mut left, mut right) = (false, false);
        for i in 0..vs.len() {
            let (a, b) = (corner(&vs[i]), corner(&vs[(i + 1) % vs.len()]));
            let c = (b - a).perp_dot(q - a);
            left |= c > 0.0;
            right |= c < 0.0;
        }
        !(left && right)
    };
    if !inside(&bins.vertices[p.first_vertex as usize..][..p.vertex_count as usize]) {
        return None;
    }
    let mut light = 1.0f32;
    for piece in &bins.shadows[p.first_shadow as usize..][..p.shadows as usize] {
        if piece.split != split || (piece.occluded && piece.beam.is_none()) {
            continue;
        }
        let first = piece.first as usize;
        let vs = &bins.shadow_vertices[first..first + piece.count as usize];
        if !inside(vs) {
            continue;
        }
        if piece.dark {
            return Some(0.0);
        }
        // Its corners' weights at the point, perspective-correct, in the triangle of its fan
        // holding it.
        let Some(weights) = (1..vs.len() - 1).find_map(|t| {
            let (a, b, c) = (corner(&vs[0]), corner(&vs[t]), corner(&vs[t + 1]));
            let area = (b - a).perp_dot(c - a);
            if area.abs() < 1e-9 {
                return None;
            }
            let (l1, l2) = ((q - a).perp_dot(c - a) / area, (b - a).perp_dot(q - a) / area);
            let l0 = 1.0 - l1 - l2;
            (l0 >= -1e-4 && l1 >= -1e-4 && l2 >= -1e-4).then(|| {
                let (w0, w1, w2) = (l0 * vs[0].w, l1 * vs[t].w, l2 * vs[t + 1].w);
                let sum = w0 + w1 + w2;
                [(0, w0 / sum), (t, w1 / sum), (t + 1, w2 / sum)]
            })
        }) else {
            continue;
        };
        let mix3 = |f: &dyn Fn(usize) -> Vec3| weights.iter().fold(Vec3::ZERO, |acc, &(i, k)| acc + f(i) * k);
        let softs = &bins.shadow_softs[piece.softs.0 as usize..piece.softs.1 as usize];
        let mut here = if softs.is_empty() {
            weights.iter().map(|&(i, k)| bins.shadow_light[first + i] * k).sum::<f32>().clamp(0.0, 1.0)
        } else {
            moose_view::soft_reach(softs, &bins.shadow_wedges, mix3(&|i| bins.shadow_world[first + i]))
        };
        if let Some((scale, offset)) = piece.beam {
            let ray = mix3(&|i| bins.shadow_rays[first + i]);
            let c = (ray.x / ray.length().max(1e-12) * scale + offset).clamp(0.0, 1.0);
            here *= c * c * (3.0 - 2.0 * c);
        }
        light = light.min(here);
    }
    Some(light)
}

/// Shades pixels `x0..x1` of a polygon's row into `out` (the row's colors, or the blend
/// buffer), over `behind`.
#[allow(clippy::too_many_arguments)]
fn shade_points(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    shaders: &[MaterialEntry],
    config: &RasterConfig,
    textures: &Textures,
    state: u32,
    x0: i32,
    x1: i32,
    vx: i32,
    translucent: bool,
) {
    let st = &s.states[state as usize];
    let (id, slot) = (st.id, st.slot);
    let (x_left, x_right, w_left, w_right) = (st.x_left, st.x_right, st.w_left, st.w_right);
    // The lattice of the block holding this row.
    let block = (s.row - s.viewport_y).div_euclid(BLOCK_ROWS);
    let index = slot as usize * s.blocks + (block - s.first_block) as usize;
    let (t, l) = split_id(id);
    let p = &bins[t].polygons[l];
    if !s.lattices[index].started {
        let range = p.first_vertex as usize..p.first_vertex as usize + p.vertex_count as usize;
        let lattice = start_lattice(s, p, &bins[t].vertices[range], block);
        s.lattices[index] = lattice;
        s.lattices[index].ends = build_tiles(s, bins, shaders, config, textures, lattice, id);
    }
    // A half-rate pair shows its even pixel, or the row's first where the pair starts
    // before the row, even if hidden.
    let shown = if p.half_rate {
        (x0 - (x0 - vx).rem_euclid(2)).max(pixel_edge(x_left))
    } else {
        x0
    };
    row_points(s, index, p.n_out as usize, shown, x1, (x_left, x_right), config.show_samples);
    // The shadow buffer for the run: how much of each split light reaches each pixel.
    let smooth = config.smooth_edges && !config.beam_dither && !p.half_rate;
    shadow_run(s, &bins[t], p, x0, x1, config.beam_dither, smooth.then_some((x_left, x_right)));
    if let Some(grid) = textures.blur {
        grid.soften(s, &bins[t], p, x0, x1);
        for (r, o) in s.reaches.iter_mut().zip(&s.occluded) {
            *r *= o;
        }
    }
    let job = SpanJob {
        x_left,
        x_right,
        w_left,
        w_right,
        xs: &s.points_x,
        outs: &s.points_v,
        x0,
        splits: p.splits as usize,
        split_colors: p.split_colors,
        split_flashlight: p.split_flashlight,
        reaches: &s.reaches,
        row: s.row,
        half_rate: p.half_rate,
        vx,
    };
    let draw = Draw {
        params: &p.params,
        textures: &textures.set(p.textures),
        filters: p.filters,
        eye: p.eye,
        focal: textures.focal,
        time: textures.time,
        object: &p.object,
    };
    let span = draw_fn(&shaders[p.material.0 as usize], p.half_rate);
    let run = (x0 - vx) as usize..(x1 - vx) as usize;
    if translucent {
        let len = (x1 - x0) as usize;
        s.blend.resize(len.max(s.blend.len()), 0);
        let behind = Behind {
            w: &s.vis_w[run.clone()],
            colors: &s.color[run.clone()],
        };
        span(&job, &mut s.blend[..len], behind, &draw);
        let dst = &mut s.color[run.clone()];
        let mut dst_blocks = dst.chunks_exact_mut(LANES);
        let mut src_blocks = s.blend[..len].chunks_exact(LANES);
        for (d, src) in (&mut dst_blocks).zip(&mut src_blocks) {
            let out = blend_lanes(
                U32s::from(<[u32; LANES]>::try_from(src).unwrap()),
                U32s::from(<[u32; LANES]>::try_from(&*d).unwrap()),
            );
            d.copy_from_slice(&out.to_array());
        }
        for (d, &src) in dst_blocks
            .into_remainder()
            .iter_mut()
            .zip(src_blocks.remainder())
        {
            *d = blend(src, *d);
        }
    } else {
        span(&job, &mut s.color[run.clone()], Behind::default(), &draw);
    }
    if config.show_samples {
        // Pixels on a grid row of their tile tinted red, and the sample points on the row
        // marked: grid points green, edges' knots (and grid points moved onto an edge)
        // yellow, vertices white.
        let n = s.points_x.len();
        let last = pixel_edge(x_right) - 1;
        for i in 0..n {
            let px = pixel_edge(s.points_x[i]).min(last);
            let color = match s.points_kind[i] {
                POINT_GRID => {
                    let to = s.points_x.get(i + 1).map_or(px + 1, |&x| pixel_edge(x)).min(x1);
                    for px in px.max(x0)..to {
                        let c = &mut s.color[(px - vx) as usize];
                        *c = blend(0x60FF_2020, *c);
                    }
                    0x20_FF_40
                }
                POINT_KNOT => 0xFF_E0_20,
                POINT_VERTEX => 0xFF_FF_FF,
                _ => continue,
            };
            if (x0..x1).contains(&px) {
                s.color[(px - vx) as usize] = color;
            }
        }
    }
}

/// Shades pixels `x0..x1` of a polygon's row.
#[allow(clippy::too_many_arguments)]
fn shade_run(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    shaders: &[MaterialEntry],
    config: &RasterConfig,
    textures: &Textures,
    state: u32,
    x0: i32,
    x1: i32,
    vx: i32,
) {
    shade_points(s, bins, shaders, config, textures, state, x0, x1, vx, false);
}

/// Shades pixels `x0..x1` of a translucent polygon's row and blends them over the row.
#[allow(clippy::too_many_arguments)]
fn blend_run(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    shaders: &[MaterialEntry],
    config: &RasterConfig,
    textures: &Textures,
    state: u32,
    x0: i32,
    x1: i32,
    vx: i32,
) {
    shade_points(s, bins, shaders, config, textures, state, x0, x1, vx, true);
}

/// The span function for a polygon's shading rate.
fn draw_fn(shader: &MaterialEntry, half_rate: bool) -> crate::shader::DrawSpanFn {
    if half_rate {
        shader.draw_span_half
    } else {
        shader.draw_span
    }
}

/// Each vertex's texture level of detail ([`LOD`]), from the polygon's exact screen-space
/// derivatives of uv there, in texels of a `size.0` by `size.1` texture: log2 of texels per
/// screen pixel, the larger of the x and y footprints. Edge-on polygons, where nothing can
/// be measured, get the smallest level.
///
/// A planar polygon's `u * w`, `v * w` and `w` are linear across the screen, so their
/// gradients come from any three of its vertices (the widest triangle, for precision), and
/// `du/dx = (d(u w)/dx - u dw/dx) / w` at each vertex, and likewise for the rest.
fn vertex_lods(
    verts: &[ScreenVertex],
    uv: impl Fn(usize) -> (f32, f32),
    size: (f32, f32),
    out: &mut Vec<f32>,
) {
    out.clear();
    let n = verts.len();
    // The widest triangle of the fan from vertex 0.
    let area = |i: usize| {
        let (a, b, c) = (verts[0], verts[i], verts[i + 1]);
        (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)
    };
    let widest = (1..n - 1).max_by(|&i, &j| area(i).abs().total_cmp(&area(j).abs()));
    let Some(i) = widest.filter(|&i| area(i).abs() > 1e-6) else {
        out.extend(std::iter::repeat_n(16.0, n));
        return;
    };
    let (a, b, c) = (verts[0], verts[i], verts[i + 1]);
    let det = area(i);
    let gradient = |fa: f32, fb: f32, fc: f32| {
        (
            ((fb - fa) * (c.y - a.y) - (fc - fa) * (b.y - a.y)) / det,
            ((fc - fa) * (b.x - a.x) - (fb - fa) * (c.x - a.x)) / det,
        )
    };
    let (uva, uvb, uvc) = (uv(0), uv(i), uv(i + 1));
    let du = gradient(uva.0 * a.w, uvb.0 * b.w, uvc.0 * c.w);
    let dv = gradient(uva.1 * a.w, uvb.1 * b.w, uvc.1 * c.w);
    let dw = gradient(a.w, b.w, c.w);
    for (k, vert) in verts.iter().enumerate() {
        let (u, v) = uv(k);
        // uv per pixel along screen x and y.
        let d = |g: (f32, f32), c: f32| ((g.0 - c * dw.0) / vert.w, (g.1 - c * dw.1) / vert.w);
        let (dudx, dudy) = d(du, u);
        let (dvdx, dvdy) = d(dv, v);
        // The same in texels, and the longer axis's length.
        let x_len = (dudx * size.0).hypot(dvdx * size.1);
        let y_len = (dudy * size.0).hypot(dvdy * size.1);
        let long = x_len.max(y_len);
        out.push(if long > 0.0 { long.log2() } else { -16.0 });
    }
}

/// The level of detail at [`LANES`] points of a polygon, as [`vertex_lods`] works it out at
/// its corners: from the screen-space gradients of `u w` (`du`), `v w` (`dv`) and `w` (`dw`),
/// constant across a flat polygon, and each point's `uv` and `1 / w` (`inv`), in texels of
/// a `size.0` by `size.1` texture: log2 of texels per screen pixel, the larger of the x and y
/// footprints.
#[inline(always)]
fn point_lods(
    du: (F32s, F32s),
    dv: (F32s, F32s),
    dw: (F32s, F32s),
    uv: [F32s; 2],
    inv: F32s,
    size: (f32, f32),
) -> F32s {
    let d = |g: (F32s, F32s), c: F32s| ((g.0 - c * dw.0) * inv, (g.1 - c * dw.1) * inv);
    let ((dudx, dudy), (dvdx, dvdy)) = (d(du, uv[0]), d(dv, uv[1]));
    let (w, h) = (F32s::fill(size.0), F32s::fill(size.1));
    let len2 = |a: F32s, b: F32s| (a * w) * (a * w) + (b * h) * (b * h);
    let long2 = len2(dudx, dvdx).max(len2(dudy, dvdy));
    // log2 of the length: half log2 of its square. None measured: the sharpest.
    let lod = long2.max(F32s::fill(1e-30)).ln() * F32s::fill(0.5 / std::f32::consts::LN_2);
    long2.simd_gt(F32s::fill(0.0)).select(lod, F32s::fill(-16.0))
}

/// The shader drawing a row state's polygon.
fn shader_of<'a>(
    s: &RowScratch,
    bins: &[ThreadBins],
    shaders: &'a [MaterialEntry],
    state: u32,
) -> &'a MaterialEntry {
    let (t, l) = split_id(s.states[state as usize].id);
    &shaders[bins[t].polygons[l].material.0 as usize]
}

/// The runs of translucent span `t` in front of a row's opaque spans (non-overlapping,
/// sorted by x), each with the index of the opaque span behind it, or `None` over the
/// background. The same exact two-point test as [`insert_resolved`]: `t` wins a pixel where
/// its w is larger.
fn visible_runs(opaque: &[Span], t: &Span, out: &mut Vec<(i32, i32, Option<u32>)>) {
    out.clear();
    let mut cursor = t.x0; // t's pixels before this are decided
    let first = opaque.partition_point(|o| o.x1 <= t.x0);
    for (k, o) in opaque[first..].iter().enumerate() {
        if o.x0 >= t.x1 {
            break;
        }
        let (a, b) = (o.x0.max(t.x0), o.x1.min(t.x1));
        if cursor < a {
            out.push((cursor, a, None)); // over the background
        }
        let behind = Some((first + k) as u32);
        let d0 = t.w(a) - o.w(a);
        let d1 = t.w(b - 1) - o.w(b - 1);
        if d0 > 0.0 && d1 > 0.0 {
            out.push((a, b, behind));
        } else if d0 > 0.0 || d1 > 0.0 {
            // The surfaces cross once: split at the first pixel past the crossing.
            let k = d0 / (d0 - d1) * (b - 1 - a) as f32;
            let xc = (a + k.floor() as i32 + 1).clamp(a + 1, b);
            if d0 > 0.0 {
                out.push((a, xc, behind));
            } else {
                out.push((xc, b, behind));
            }
        }
        cursor = b;
    }
    if cursor < t.x1 {
        out.push((cursor, t.x1, None));
    }
}

/// Inserts `new` into a row's non-overlapping, x-sorted span list, resolving each overlap
/// with the exact two-point test: w is linear along both spans, so comparing it at the
/// overlap's two ends tells which is closer across the whole overlap, or where they cross.
fn insert_resolved(list: &[Span], new: Span, out: &mut Vec<Span>) {
    out.clear();
    let push = |out: &mut Vec<Span>, s: &Span, a: i32, b: i32| {
        if a < b {
            out.push(s.piece(a, b));
        }
    };
    let mut cursor = new.x0; // new's pixels before this are placed
    for old in list {
        if old.x1 <= new.x0 {
            out.push(*old);
            continue;
        }
        if old.x0 >= new.x1 {
            push(out, &new, cursor, new.x1);
            cursor = new.x1;
            out.push(*old);
            continue;
        }
        let (a, b) = (old.x0.max(new.x0), old.x1.min(new.x1));
        push(out, &new, cursor, a); // new, before this old span
        push(out, old, old.x0, a); // old, before the overlap
        let d0 = new.w(a) - old.w(a);
        let d1 = new.w(b - 1) - old.w(b - 1);
        if d0 <= 0.0 && d1 <= 0.0 {
            push(out, old, a, b); // new hidden across the overlap
        } else if d0 > 0.0 && d1 > 0.0 {
            push(out, &new, a, b); // new wins the overlap
        } else {
            // The surfaces cross once: split at the first pixel past the crossing.
            let k = d0 / (d0 - d1) * (b - 1 - a) as f32;
            let xc = (a + k.floor() as i32 + 1).clamp(a + 1, b);
            let (first, second) = if d0 > 0.0 { (&new, old) } else { (old, &new) };
            push(out, first, a, xc);
            push(out, second, xc, b);
        }
        push(out, old, b, old.x1); // old, past the overlap
        cursor = cursor.max(b);
    }
    push(out, &new, cursor, new.x1);
}

/// Shadow slots the blur grid softens (see `RasterConfig::shadow_blur`): 0 to this.
const BLUR_CHANNELS: usize = 4;

/// The farthest, in pixels, a soft edge reaches from a hard shadow's edge: how far the
/// blur looks for one, and its largest radius.
const BLUR_REACH: i32 = 96;

/// One cell of the blur grid: what is seen at its center pixel.
#[derive(Clone, Copy)]
struct BlurCell {
    /// The surface seen there, as w on screen (`w = a + b x + c y`, at pixel centers), and
    /// w at the cell's center; 0 for none.
    plane: [f32; 3],
    w: f32,
    /// Per channel (shadow slot): 1 lit, 0 in its hard shadow; NaN where the light doesn't
    /// light the surface at all.
    hard: [f32; BLUR_CHANNELS],
    /// Per channel, in hard shadow: how wide its soft edge would be there, in pixels.
    width: [f32; BLUR_CHANNELS],
}

impl Default for BlurCell {
    fn default() -> Self {
        Self { plane: [0.0; 3], w: 0.0, hard: [f32::NAN; BLUR_CHANNELS], width: [0.0; BLUR_CHANNELS] }
    }
}

/// Soft shadows by blurring hard ones on screen, a prototype. Before the rows are drawn,
/// a grid of cells (`RasterConfig::blur_scale` pixels each way, or twice that across at
/// half rate) records, at each one's
/// center, the surface seen there, whether it is in each light's hard shadow, and how wide
/// that shadow's soft edge would be (from the view: 0 where an occluder touches the
/// surface, growing away from it). Each cell near a shadow's edge then takes the width of
/// the nearest cell in shadow, and the grid is blurred by a box that wide (in rows, then
/// columns), only across cells on the same plane as it (w on screen of one plane), so a
/// shadow doesn't spread onto the surfaces in front or behind. As a row is drawn, each
/// pixel of a polygon split for a light takes the blurred grid at it (from the four
/// nearest cells on its plane), mixed with its own hard shadow where the soft edge is
/// narrower than a few cells.
#[derive(Default)]
struct BlurGrid {
    /// Pixels per cell: across and down.
    scale: (i32, i32),
    cols: usize,
    rows: usize,
    /// The viewport's corner.
    origin: (i32, i32),
    cells: Vec<BlurCell>,
    /// Per channel, per cell: blurred, and the blur's width (pixels; 0 for none).
    soft: Vec<f32>,
    radius: Vec<f32>,
    /// Scratch: per cell, the nearest cell in shadow along its row (x offset, width), and
    /// the row pass's result.
    near: Vec<Option<(i32, f32)>>,
    across: Vec<f32>,
    /// Per channel: some cell is in its hard shadow.
    active: [bool; BLUR_CHANNELS],
}

impl BlurGrid {
    /// Pixel center of cell `(i, j)`.
    fn center(&self, i: usize, j: usize) -> (f32, f32) {
        (
            self.origin.0 as f32 + (i as i32 * self.scale.0 + self.scale.0 / 2) as f32 + 0.5,
            self.origin.1 as f32 + (j as i32 * self.scale.1 + self.scale.1 / 2) as f32 + 0.5,
        )
    }

    /// Whether cell `k` (at pixel center `at`) is on the plane `plane`.
    fn on(&self, plane: [f32; 3], k: usize, at: (f32, f32)) -> bool {
        let w = self.cells[k].w;
        w > 0.0 && (plane[0] + plane[1] * at.0 + plane[2] * at.1 - w).abs() <= 0.01 * w
    }

    fn build(
        &mut self,
        bins: &[ThreadBins],
        scratch: &[Mutex<RowScratch>],
        config: &RasterConfig,
        viewport: Viewport,
        band_rows: i32,
        focal: f32,
    ) {
        let down = config.blur_scale as i32;
        let scale = (if config.blur_half_rate { 2 * down } else { down }, down);
        self.scale = scale;
        self.cols = (viewport.width as i32 + scale.0 - 1) as usize / scale.0 as usize;
        self.rows = (viewport.height as i32 + scale.1 - 1) as usize / scale.1 as usize;
        self.origin = (viewport.x as i32, viewport.y as i32);
        let n = self.cols * self.rows;
        self.cells.clear();
        self.cells.resize(n, BlurCell::default());
        // The cells, band by band (each band holds `band_rows / scale.1` of their rows).
        let per_band = (band_rows / scale.1) as usize;
        let (cols, origin) = (self.cols, self.origin);
        self.cells.par_chunks_mut(cols * per_band).enumerate().for_each(|(band, cells)| {
            let t = rayon::current_thread_index().unwrap_or(0) % scratch.len();
            let mut s = scratch[t].lock().unwrap();
            mask_band(&mut s, bins, band, (cols, per_band, scale, origin), focal, cells);
        });
        // Each channel blurred.
        self.soft.clear();
        self.soft.resize(n * BLUR_CHANNELS, 1.0);
        self.radius.clear();
        self.radius.resize(n * BLUR_CHANNELS, 0.0);
        for ch in 0..BLUR_CHANNELS {
            self.blur_channel(ch);
        }
    }

    fn blur_channel(&mut self, ch: usize) {
        let (cols, rows, n) = (self.cols, self.rows, self.cols * self.rows);
        let base = ch * n;
        self.active[ch] = self.cells.iter().any(|c| c.hard[ch] < 0.5);
        if !self.active[ch] {
            return;
        }
        let hard: Vec<f32> = self.cells.iter().map(|c| c.hard[ch]).collect();
        let (sx, sy) = self.scale;
        let (reach, reach_y) = (BLUR_REACH / sx, BLUR_REACH / sy);
        let (mut near, mut radius, mut across, mut soft) = (
            std::mem::take(&mut self.near),
            std::mem::take(&mut self.radius),
            std::mem::take(&mut self.across),
            std::mem::take(&mut self.soft),
        );
        let grid = &*self;
        // Along rows: the nearest cell in shadow on the same plane, within reach (each one
        // in shadow is its own).
        near.clear();
        near.resize(n, None);
        near.par_chunks_mut(cols).enumerate().for_each(|(j, near)| {
            let darks: Vec<i32> = (0..cols as i32).filter(|&i| hard[j * cols + i as usize] < 0.5).collect();
            if darks.is_empty() {
                return;
            }
            for (i, slot) in near.iter_mut().enumerate() {
                let k = j * cols + i;
                let cell = &grid.cells[k];
                if cell.w <= 0.0 {
                    continue;
                }
                let at = darks.partition_point(|&d| d < i as i32);
                let (mut lo, mut hi) = (at as i32 - 1, at as i32);
                while lo >= 0 || (hi as usize) < darks.len() {
                    let dl = if lo >= 0 { i as i32 - darks[lo as usize] } else { i32::MAX };
                    let dh = if (hi as usize) < darks.len() { darks[hi as usize] - i as i32 } else { i32::MAX };
                    let x = if dh <= dl { darks[hi as usize] } else { darks[lo as usize] };
                    if dh.min(dl) > reach {
                        break;
                    }
                    let kk = j * cols + x as usize;
                    if grid.on(cell.plane, kk, grid.center(x as usize, j)) {
                        *slot = Some((x - i as i32, grid.cells[kk].width[ch]));
                        break;
                    }
                    if dh <= dl { hi += 1 } else { lo -= 1 }
                }
            }
        });
        // How many cells of each column, down to each row, have one: to skip columns with
        // none within reach.
        let mut counts = vec![0u32; (rows + 1) * cols];
        for j in 0..rows {
            for i in 0..cols {
                counts[(j + 1) * cols + i] = counts[j * cols + i] + near[j * cols + i].is_some() as u32;
            }
        }
        // Down columns: the nearest of those, on the cell's plane, is its blur's width.
        radius[base..base + n].par_chunks_mut(cols).enumerate().for_each(|(j, out)| {
            let (top, bottom) = ((j as i32 - reach_y).max(0) as usize, (j as i32 + reach_y).min(rows as i32 - 1) as usize);
            for (i, r) in out.iter_mut().enumerate() {
                let k = j * cols + i;
                let cell = &grid.cells[k];
                if cell.w <= 0.0 || hard[k].is_nan() || counts[(bottom + 1) * cols + i] == counts[top * cols + i] {
                    continue;
                }
                if hard[k] < 0.5 {
                    *r = cell.width[ch];
                    continue;
                }
                let mut best: Option<(i32, f32)> = None;
                for jj in top..=bottom {
                    let Some((dx, width)) = near[jj * cols + i] else {
                        continue;
                    };
                    let dy = jj as i32 - j as i32;
                    let d2 = (dx * sx) * (dx * sx) + (dy * sy) * (dy * sy);
                    if best.is_some_and(|(b, _)| b <= d2) {
                        continue;
                    }
                    let x = (i as i32 + dx) as usize;
                    if grid.on(cell.plane, jj * cols + x, grid.center(x, jj)) {
                        best = Some((d2, width));
                    }
                }
                if let Some((_, width)) = best {
                    *r = width;
                }
            }
        });
        // A box as wide as that: along rows, then down columns, on the cell's plane. Where
        // every cell in its reach has the same value, that is the average whatever their
        // planes: sums of values and of cells with one tell.
        let half = |r: f32| ((r / sx as f32 * 0.5).ceil() as i32).min(reach);
        let half_y = |r: f32| ((r / sy as f32 * 0.5).ceil() as i32).min(reach_y);
        let valid = |v: f32| if v.is_nan() { (0.0, 0.0) } else { (v, 1.0) };
        across.clear();
        across.resize(n, 0.0);
        let radius_ch = &radius[base..base + n];
        across.par_chunks_mut(cols).enumerate().for_each(|(j, out)| {
            let mut sums = vec![(0.0f32, 0.0f32); cols + 1];
            for i in 0..cols {
                let (v, c) = valid(hard[j * cols + i]);
                sums[i + 1] = (sums[i].0 + v, sums[i].1 + c);
            }
            for (i, o) in out.iter_mut().enumerate() {
                let k = j * cols + i;
                let r = half(radius_ch[k]);
                *o = hard[k];
                if r <= 0 || hard[k].is_nan() {
                    continue;
                }
                let (a, b) = ((i as i32 - r).max(0) as usize, (i as i32 + r).min(cols as i32 - 1) as usize);
                let (v, c) = (sums[b + 1].0 - sums[a].0, sums[b + 1].1 - sums[a].1);
                if v <= 0.0 || v >= c {
                    continue;
                }
                let plane = grid.cells[k].plane;
                let (mut sum, mut count) = (0.0, 0.0);
                for ii in a..=b {
                    let kk = j * cols + ii;
                    if !hard[kk].is_nan() && grid.on(plane, kk, grid.center(ii, j)) {
                        sum += hard[kk];
                        count += 1.0;
                    }
                }
                if count > 0.0 {
                    *o = sum / count;
                }
            }
        });
        let mut column = vec![(0.0f32, 0.0f32); (rows + 1) * cols];
        for j in 0..rows {
            for i in 0..cols {
                let (v, c) = valid(across[j * cols + i]);
                let prev = column[j * cols + i];
                column[(j + 1) * cols + i] = (prev.0 + v, prev.1 + c);
            }
        }
        soft[base..base + n].par_chunks_mut(cols).enumerate().for_each(|(j, out)| {
            for (i, o) in out.iter_mut().enumerate() {
                let k = j * cols + i;
                let r = half_y(radius_ch[k]);
                *o = across[k];
                if r <= 0 || hard[k].is_nan() {
                    continue;
                }
                let (a, b) = ((j as i32 - r).max(0) as usize, (j as i32 + r).min(rows as i32 - 1) as usize);
                let (v, c) = (
                    column[(b + 1) * cols + i].0 - column[a * cols + i].0,
                    column[(b + 1) * cols + i].1 - column[a * cols + i].1,
                );
                if c > 0.0 && (v <= 0.0 || v >= c) {
                    continue;
                }
                let plane = grid.cells[k].plane;
                let (mut sum, mut count) = (0.0, 0.0);
                for jj in a..=b {
                    let kk = jj * cols + i;
                    if !across[kk].is_nan() && grid.on(plane, kk, grid.center(i, jj)) {
                        sum += across[kk];
                        count += 1.0;
                    }
                }
                if count > 0.0 {
                    *o = sum / count;
                }
            }
        });
        self.near = near;
        self.radius = radius;
        self.across = across;
        self.soft = soft;
    }

    /// Softens the shadow buffer of a run of polygon `p`'s row (see `shadow_run`): each
    /// pixel of each split light it blurs takes the blurred grid, from the (up to) four
    /// nearest cells on its plane, mixed with its hard shadow where the soft edge there is
    /// under two cells wide.
    fn soften(&self, s: &mut RowScratch, b: &ThreadBins, p: &PolygonSetup, x0: i32, x1: i32) {
        let len = (x1 - x0).max(0) as usize;
        if len == 0 {
            return;
        }
        let data = &b.planes[p.first_plane as usize..];
        let (ox, oy, w0, wx, wy) = (data[0], data[1], data[2], data[3], data[4]);
        let plane = [w0 - wx * ox - wy * oy, wx, wy];
        let n = self.cols * self.rows;
        let (sx, sy) = (self.scale.0 as f32, self.scale.1 as f32);
        let to_grid = |v: f32, o: i32, scale: f32| (v + 0.5 - o as f32 - scale * 0.5) / scale;
        let gy = to_grid(s.row as f32, self.origin.1, sy);
        let j0 = gy.floor() as i32;
        let fy = gy - j0 as f32;
        // The cells the run's pixels fall between, on its two grid rows.
        let i_first = to_grid(x0 as f32, self.origin.0, sx).floor() as i32;
        let i_last = to_grid((x1 - 1) as f32, self.origin.0, sx).floor() as i32 + 1;
        let count = (i_last - i_first + 1) as usize;
        for split in 0..p.splits as usize {
            let ch = p.split_slots[split] as usize;
            if ch >= BLUR_CHANNELS {
                continue;
            }
            if !self.active[ch] {
                continue;
            }
            // Per column of cells: the two cells on its grid rows on the polygon's plane,
            // blended down to the row (value and width times weight, and weight).
            let columns = &mut s.blur_cells;
            columns.clear();
            let mut any = false;
            for c in 0..count {
                let i = i_first + c as i32;
                let mut column = (0.0f32, 0.0f32, 0.0f32);
                for (d, wgt) in [(0, 1.0 - fy), (1, fy)] {
                    let j = j0 + d;
                    if i < 0 || j < 0 || i >= self.cols as i32 || j >= self.rows as i32 || wgt <= 0.0 {
                        continue;
                    }
                    let k = j as usize * self.cols + i as usize;
                    let (v, r) = (self.soft[ch * n + k], self.radius[ch * n + k]);
                    if !v.is_nan() && self.on(plane, k, self.center(i as usize, j as usize)) {
                        column = (column.0 + v * wgt, column.1 + r * wgt, column.2 + wgt);
                    }
                }
                any |= column.1 > 0.0;
                columns.push(column);
            }
            if !any {
                continue;
            }
            // Between each two columns: the pixels whose cells they are, blended across.
            let out = &mut s.occluded[split * len..(split + 1) * len];
            let column_x = |c: usize| self.origin.0 + (i_first + c as i32) * self.scale.0 + self.scale.0 / 2;
            for c in 0..count - 1 {
                let (a, b) = (s.blur_cells[c], s.blur_cells[c + 1]);
                if a.1 <= 0.0 && b.1 <= 0.0 {
                    continue;
                }
                let (from, to) = (column_x(c).max(x0), column_x(c + 1).min(x1));
                for px in from..to {
                    let fx = (px - column_x(c)) as f32 / sx;
                    let weight = a.2 + (b.2 - a.2) * fx;
                    if weight <= 1e-6 {
                        continue;
                    }
                    let soft = (a.0 + (b.0 - a.0) * fx) / weight;
                    let radius = (a.1 + (b.1 - a.1) * fx) / weight;
                    let t = (radius / (2.0 * sx.max(sy))).clamp(0.0, 1.0);
                    let o = &mut out[(px - x0) as usize];
                    *o += (soft - *o) * t;
                }
            }
        }
    }
}

/// Fills band `band`'s cells of the blur grid (`per_band` rows of `cols`, from the
/// band's first; `scale` pixels each, across and down, from the viewport's corner
/// `origin`): at each one's
/// center pixel, the opaque surface seen there, as the rows find it, and its hard shadow.
fn mask_band(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    band: usize,
    (cols, per_band, scale, origin): (usize, usize, (i32, i32), (i32, i32)),
    focal: f32,
    cells: &mut [BlurCell],
) {
    s.world.clear();
    s.span.clear();
    s.pixel.clear();
    for b in bins {
        let Some(bb) = b.bands.get(band) else {
            continue;
        };
        s.world.extend_from_slice(&bb.world);
        s.span.extend_from_slice(&bb.span);
        s.pixel.extend_from_slice(&bb.pixel);
    }
    let poly = |id: u32| {
        let (t, l) = split_id(id);
        &bins[t].polygons[l]
    };
    s.span.sort_by(|&a, &b| poly(b).max_w.total_cmp(&poly(a).max_w));
    for (jr, row_cells) in cells.chunks_mut(cols).enumerate().take(per_band) {
        let j = band * per_band + jr;
        let row = origin.1 + j as i32 * scale.1 + scale.1 / 2;
        s.row = row;
        s.states.clear();
        s.spans.clear();
        for i in 0..s.world.len() {
            if let Some(span) = row_span(s, bins, s.world[i], 0, row) {
                s.spans.push(span);
            }
        }
        s.spans.sort_by_key(|sp| sp.x0);
        for i in 0..s.span.len() {
            if let Some(span) = row_span(s, bins, s.span[i], 0, row) {
                insert_resolved(&s.spans, span, &mut s.spans_next);
                std::mem::swap(&mut s.spans, &mut s.spans_next);
            }
        }
        s.pixel_spans.clear();
        for i in 0..s.pixel.len() {
            if let Some(span) = row_span(s, bins, s.pixel[i], 0, row) {
                s.pixel_spans.push(span);
            }
        }
        // The surface at each cell's center: the opaque span there, unless a per-pixel
        // actor is in front.
        s.visible.clear();
        let mut next = 0;
        for (i, cell) in row_cells.iter_mut().enumerate() {
            let x = origin.0 + i as i32 * scale.0 + scale.0 / 2;
            while next < s.spans.len() && s.spans[next].x1 <= x {
                next += 1;
            }
            let mut seen = s.spans.get(next).filter(|sp| sp.x0 <= x).map(|sp| (sp.w(x), sp.state));
            for sp in &s.pixel_spans {
                if sp.x0 <= x && x < sp.x1 && seen.is_none_or(|(w, _)| sp.w(x) > w) {
                    seen = Some((sp.w(x), sp.state));
                }
            }
            *cell = BlurCell::default();
            if let Some((w, state)) = seen {
                cell.w = w;
                s.visible.push((i as i32, i as i32 + 1, Some(state)));
            }
        }
        // Runs of cells on one polygon, its shadow pieces walked once per run.
        let mut r = 0;
        while r < s.visible.len() {
            let (first, _, state) = s.visible[r];
            let mut last = first + 1;
            while r + 1 < s.visible.len() && s.visible[r + 1].2 == state && s.visible[r + 1].0 == last {
                last += 1;
                r += 1;
            }
            r += 1;
            let (t, l) = split_id(s.states[state.unwrap() as usize].id);
            let b = &bins[t];
            let p = &b.polygons[l];
            let data = &b.planes[p.first_plane as usize..];
            let (ox, oy, w0, wx, wy) = (data[0], data[1], data[2], data[3], data[4]);
            let plane = [w0 - wx * ox - wy * oy, wx, wy];
            let run = &mut row_cells[first as usize..last as usize];
            for cell in run.iter_mut() {
                cell.plane = plane;
            }
            for k in p.first_light as usize..p.first_light as usize + p.light_count as usize {
                let Some(ch) = b.lights[k].shadow.map(|c| c as usize).filter(|&c| c < BLUR_CHANNELS) else {
                    continue;
                };
                for cell in run.iter_mut() {
                    cell.hard[ch] = 1.0;
                }
                let split = b.light_split[k];
                if split == NO_SPLIT {
                    continue;
                }
                let x_of = |i: usize| origin.0 + (first + i as i32) * scale.0 + scale.0 / 2;
                for piece in &b.shadows[p.first_shadow as usize..][..p.shadows as usize] {
                    if !piece.occluded || piece.split != split || row < piece.row_top || row >= piece.row_end {
                        continue;
                    }
                    let range = piece.first as usize..piece.first as usize + piece.count as usize;
                    let verts = &b.shadow_vertices[range.clone()];
                    let Some(((il, xl), (ir, xr))) = crossings(verts, &b.shadow_lines[range.clone()], row) else {
                        continue;
                    };
                    let (from, to) = (pixel_edge(xl), pixel_edge(xr));
                    let width = &b.shadow_width[range];
                    let at = |i: usize| {
                        let (w, alpha) = edge_at_row(verts, i, row);
                        let k = (i + 1) % verts.len();
                        (w, (width[i] + (width[k] - width[i]) * alpha) * w)
                    };
                    let ((wl, dl), (wr, dr)) = (at(il), at(ir));
                    let span = (xr - xl).max(1e-6);
                    for (i, cell) in run.iter_mut().enumerate() {
                        let x = x_of(i);
                        if x < from || x >= to {
                            continue;
                        }
                        let t = ((x as f32 + 0.5 - xl) / span).clamp(0.0, 1.0);
                        let w = wl + (wr - wl) * t;
                        if w <= 0.0 {
                            continue;
                        }
                        cell.hard[ch] = 0.0;
                        cell.width[ch] = (dl + (dr - dl) * t) / w * focal * cell.w;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lods(verts: &[(f32, f32, f32)], uvs: &[(f32, f32)], size: (f32, f32)) -> Vec<f32> {
        let verts: Vec<ScreenVertex> = verts
            .iter()
            .map(|&(x, y, w)| ScreenVertex { x, y, w })
            .collect();
        let mut out = Vec::new();
        vertex_lods(&verts, |i| uvs[i], size, &mut out);
        out
    }

    #[test]
    fn the_flashlight_keeps_its_shadow_first_then_the_brightest_lights() {
        // A floor at the origin facing up.
        let (center, up) = (Vec3::ZERO, Vec3::Y);
        let lamp = |at: Vec3, bright: f32| Light::point(0, at, Vec3::splat(bright), 10.0);
        let near = split_priority(&lamp(Vec3::new(0.0, 2.0, 0.0), 1.0), center, up);
        let far = split_priority(&lamp(Vec3::new(0.0, 6.0, 0.0), 1.0), center, up);
        let dim = split_priority(&lamp(Vec3::new(0.0, 2.0, 0.0), 0.2), center, up);
        let low = split_priority(&lamp(Vec3::new(5.0, 0.3, 0.0), 1.0), center, up);
        assert!(near > far && near > dim && near > low, "{near} {far} {dim} {low}");
        // A spot aimed away brings next to nothing; the flashlight, however dim or aimed
        // away, comes first.
        let away = Light::spot(0, Vec3::new(0.0, 2.0, 0.0), Vec3::ONE, 10.0, Vec3::Y, 10.0, 20.0);
        assert_eq!(split_priority(&away, center, up), 0.0);
        let flashlight = Light { id: moose_assets::FLASHLIGHT_ID, ..away };
        assert_eq!(split_priority(&flashlight, center, up), f32::INFINITY);
    }

    #[test]
    fn a_soft_stretch_is_taken_every_few_pixels_where_it_is_smooth_and_at_each_where_not() {
        use std::cell::Cell;
        // A gentle ramp: taken at every 8th pixel and the halfway points, straight between,
        // and exact (it is a straight line).
        let calls = Cell::new(0);
        let ramp = |x: i32| {
            calls.set(calls.get() + 1);
            x as f32 / 100.0
        };
        let mut out = Vec::new();
        soft_stretch(&mut out, 10, 43, ramp);
        assert_eq!(out.len(), 33);
        assert!(out.iter().enumerate().all(|(i, &v)| (v - (10 + i as i32) as f32 / 100.0).abs() < 1e-6));
        assert!(calls.get() <= 12, "{} calls", calls.get());
        // A hard edge at pixel 27: every pixel exactly, the step where it is.
        let mut out = Vec::new();
        soft_stretch(&mut out, 10, 43, |x| if x >= 27 { 1.0 } else { 0.0 });
        assert!(out.iter().enumerate().all(|(i, &v)| v == if 10 + i as i32 >= 27 { 1.0 } else { 0.0 }), "{out:?}");
        // One pixel, and none.
        let mut out = Vec::new();
        soft_stretch(&mut out, 5, 6, |_| 0.25);
        soft_stretch(&mut out, 7, 7, |_| 0.5);
        assert_eq!(out, [0.25]);
    }

    #[test]
    fn a_light_behind_a_face_lights_it_only_within_its_back_light_angle() {
        let back = 20f32.to_radians().sin();
        // In front, whatever the angle.
        assert!(lights_face(0.01, 1.0, 0.0) && lights_face(0.01, 1.0, back));
        // Just behind: not without a back light angle; with one, up to it.
        assert!(!lights_face(-0.01, 1.0, 0.0));
        assert!(lights_face(-0.01, 1.0, back));
        assert!(lights_face(-0.33, 1.0, back) && !lights_face(-0.35, 1.0, back));
        // On the plane, a face doesn't face it.
        assert!(!lights_face(0.0, 1.0, 0.0));
    }

    #[test]
    fn a_smooth_face_turned_from_a_light_is_lit_while_a_normal_on_it_faces_the_light() {
        // A triangle in the floor, the light under it: its plane faces away.
        let positions = [Vec3::ZERO, Vec3::X, Vec3::Z];
        let light = Vec3::new(-2.0, -0.5, 0.0);
        let up = [Vec3::Y; 3];
        assert!(!normals_face(up.into_iter(), &positions, light));
        // One corner's normal leans over toward the light: that corner is lit.
        let leaning = [Vec3::new(-1.0, 0.2, 0.0), Vec3::Y, Vec3::Y];
        assert!(normals_face(leaning.into_iter(), &positions, light));
        // All of them lean away: none of it is.
        let away = [Vec3::new(1.0, 0.2, 0.0); 3];
        assert!(!normals_face(away.into_iter(), &positions, light));
        // A light close under the middle of a big face: no corner's normal faces it from its
        // own corner, but the far corner's does from the near one, as it does in between.
        let positions = [Vec3::ZERO, Vec3::X * 10.0, Vec3::Z];
        let light = Vec3::new(5.0, -0.5, 0.0);
        let normals = [Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::Y];
        let own = normals.iter().zip(&positions).any(|(n, &q)| n.dot(light - q) > 0.0);
        assert!(!own && normals_face(normals.into_iter(), &positions, light));
    }

    #[test]
    fn a_face_whose_first_corners_are_in_a_line_still_has_tangents() {
        // A 1 x 4 m floor quad with a corner added on its long edge, first: its first
        // three corners lie in a line. Its texture runs u along x and v along z, as the
        // same quad without the extra corner has it.
        let obj = |first: &str| {
            format!(
                "v 0 0 0\nv 0 0 2\nv 0 0 4\nv 1 0 4\nv 1 0 0\n\
                 vt 0 0\nvt 0 2\nvt 0 4\nvt 1 4\nvt 1 0\n{first}\n"
            )
        };
        let path = std::path::Path::new("t.obj");
        let lined = moose_assets::parse_obj(path, "lined", &obj("f 1/1 2/2 3/3 4/4 5/5")).unwrap();
        let plain = moose_assets::parse_obj(path, "plain", &obj("f 1/1 3/3 4/4 5/5")).unwrap();
        let (t, b) = face_tangents(&lined, &lined.polygons[0]);
        assert!(t.distance(Vec3::X) < 1e-5 && b.distance(Vec3::Z) < 1e-5, "{t} {b}");
        assert_eq!((t, b), face_tangents(&plain, &plain.polygons[0]));
    }

    #[test]
    fn a_crates_faces_have_tangents_along_their_texture() {
        // Each face's tangent and bitangent are unit length, in its plane, and point the
        // way its u and v grow across it.
        let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let id = assets.load_mesh("crate.obj").unwrap();
        let mesh = assets.mesh(id);
        let uv = mesh.attribs.iter().find(|a| a.name == UV).unwrap();
        for polygon in &mesh.polygons {
            let (t, b) = face_tangents(mesh, polygon);
            let n = polygon.plane.normal;
            assert!((t.length() - 1.0).abs() < 1e-4 && (b.length() - 1.0).abs() < 1e-4);
            assert!(t.dot(n).abs() < 1e-4 && b.dot(n).abs() < 1e-4);
            let vs: Vec<usize> = polygon.vertices().collect();
            let at = |v: usize| mesh.positions[mesh.vertex_positions[v] as usize];
            let (du, dv) = (
                uv.data.get_f32(vs[1] * 2) - uv.data.get_f32(vs[0] * 2),
                uv.data.get_f32(vs[1] * 2 + 1) - uv.data.get_f32(vs[0] * 2 + 1),
            );
            let along = at(vs[1]) - at(vs[0]);
            if du.abs() > 1e-4 {
                assert_eq!(along.dot(t) > 0.0, du > 0.0, "u grows along the tangent");
            }
            if dv.abs() > 1e-4 {
                assert_eq!(along.dot(b) > 0.0, dv > 0.0, "v grows along the bitangent");
            }
        }
    }

    #[test]
    fn a_points_level_of_detail_is_its_own_not_a_blend_of_its_corners() {
        // A floor 1 m below the eye, 1 to 20 m away, 1 m a repeat (seen at a grazing angle):
        // a point (x, z) on it is at screen (f x / z, f / z), w = 1 / z, uv (x, z).
        let (f, size) = (500.0, (64.0, 64.0));
        let at = |x: f32, z: f32| ((f * x / z, f / z, 1.0 / z), (x, z));
        let corners = [at(-1.0, 1.0), at(1.0, 1.0), at(1.0, 20.0), at(-1.0, 20.0)];
        let middle = at(0.0, 10.0);
        // The corners' own (exact), and the middle's, exact as a corner of a polygon there.
        let quad = lods(&corners.map(|c| c.0), &corners.map(|c| c.1), size);
        let exact = lods(&[middle.0, corners[0].0, corners[1].0], &[middle.1, corners[0].1, corners[1].1], size)[0];
        // The quad's gradients of u w, v w and w (any three corners: it is flat).
        let [(a, ua), (b, ub), (c, uc)] = [corners[0], corners[1], corners[2]];
        let det = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
        let gradient = |fa: f32, fb: f32, fc: f32| {
            (
                F32s::fill(((fb - fa) * (c.1 - a.1) - (fc - fa) * (b.1 - a.1)) / det),
                F32s::fill(((fc - fa) * (b.0 - a.0) - (fb - fa) * (c.0 - a.0)) / det),
            )
        };
        let du = gradient(ua.0 * a.2, ub.0 * b.2, uc.0 * c.2);
        let dv = gradient(ua.1 * a.2, ub.1 * b.2, uc.1 * c.2);
        let dw = gradient(a.2, b.2, c.2);
        let point = |(_, uv): ((f32, f32, f32), (f32, f32)), z: f32| {
            point_lods(du, dv, dw, [F32s::fill(uv.0), F32s::fill(uv.1)], F32s::fill(z), size).to_array()[0]
        };
        // At the corners, what the corners have; in the middle, its own.
        assert!((point(corners[0], 1.0) - quad[0]).abs() < 1e-3);
        assert!((point(corners[2], 20.0) - quad[2]).abs() < 1e-3);
        assert!((point(middle, 10.0) - exact).abs() < 1e-3, "{} vs {exact}", point(middle, 10.0));
        // The corners' blend there (as varyings are, along the floor) is far sharper.
        let t = (10.0 - 1.0) / (20.0 - 1.0);
        let blend = quad[1] + (quad[2] - quad[1]) * t;
        assert!(exact - blend > 2.0, "exact {exact}, blended {blend}");
    }

    #[test]
    fn level_of_detail_is_log2_texels_per_pixel() {
        let square = |side: f32| {
            [
                (10.0, 10.0, 1.0),
                (10.0 + side, 10.0, 1.0),
                (10.0 + side, 10.0 + side, 1.0),
                (10.0, 10.0 + side, 1.0),
            ]
        };
        let uvs = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let near = |got: Vec<f32>, want: f32| got.iter().all(|l| (l - want).abs() < 1e-4);
        // A 64-texel texture over 64 pixels: 1 texel per pixel. Over 32: 2. Over 256: 1/4.
        assert!(near(lods(&square(64.0), &uvs, (64.0, 64.0)), 0.0));
        assert!(near(lods(&square(32.0), &uvs, (64.0, 64.0)), 1.0));
        assert!(near(lods(&square(256.0), &uvs, (64.0, 64.0)), -2.0));
        // Squashed vertically 4x: the larger footprint counts.
        let flat = [
            (0.0, 0.0, 1.0),
            (64.0, 0.0, 1.0),
            (64.0, 16.0, 1.0),
            (0.0, 16.0, 1.0),
        ];
        assert!(near(lods(&flat, &uvs, (64.0, 64.0)), 2.0));
        // Edge-on: the smallest level.
        let line = [(0.0, 0.0, 1.0), (10.0, 10.0, 1.0), (20.0, 20.0, 1.0)];
        assert!(
            lods(&line, &uvs[..3], (64.0, 64.0))
                .iter()
                .all(|&l| l >= 8.0)
        );
    }

    #[test]
    fn level_of_detail_follows_perspective() {
        // A floor seen by a pinhole camera (focal 500 px, looking down -z from 1.7 m up),
        // uv = world x and z in meters with a 64-texel texture. At each vertex the footprint
        // is measured independently: rays half a pixel either side, hit on the plane.
        let (f, cx, cy, h) = (500.0f32, 320.0f32, 180.0f32, 1.7f32);
        let project = |x: f32, z: f32| {
            let d = -z; // depth
            (cx + f * x / d, cy + f * h / d, 1.0 / d)
        };
        let hit = |sx: f32, sy: f32| {
            // The ray through pixel (sx, sy) meets y = 0 at depth f h / (sy - cy).
            let d = f * h / (sy - cy);
            ((sx - cx) * d / f, -d)
        };
        let corners = [(-2.0f32, -2.0f32), (2.0, -2.0), (2.0, -9.0), (-2.0, -9.0)];
        let verts: Vec<(f32, f32, f32)> = corners.iter().map(|&(x, z)| project(x, z)).collect();
        let uvs: Vec<(f32, f32)> = corners.to_vec();
        let got = lods(&verts, &uvs, (64.0, 64.0));
        for (k, &(sx, sy, _)) in verts.iter().enumerate() {
            let texels = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).hypot(a.1 - b.1) * 64.0;
            let across = texels(hit(sx + 0.5, sy), hit(sx - 0.5, sy));
            let down = texels(hit(sx, sy + 0.5), hit(sx, sy - 0.5));
            let want = across.max(down).log2();
            assert!(
                (got[k] - want).abs() < 0.01,
                "vertex {k}: {} vs {want}",
                got[k]
            );
        }
    }

    fn span(x0: i32, x1: i32, w0: f32, w1: f32, state: u32) -> Span {
        let dwdx = if x1 - x0 > 1 {
            (w1 - w0) / (x1 - x0 - 1) as f32
        } else {
            0.0
        };
        Span {
            x0,
            x1,
            xa: x0 as f32 + 0.5,
            wa: w0,
            dwdx,
            state,
        }
    }

    fn owners(list: &[Span], width: i32) -> Vec<u32> {
        let mut o = vec![EMPTY; width as usize];
        for s in list {
            for x in s.x0..s.x1 {
                assert_eq!(o[x as usize], EMPTY, "overlap at {x}");
                o[x as usize] = s.state;
            }
        }
        o
    }

    #[test]
    fn two_point_resolve() {
        let mut out = Vec::new();
        // A far wall across 0..20; a near prop in 5..10 wins; a farther one in 12..15 is hidden.
        let wall = [span(0, 20, 0.1, 0.1, 0)];
        insert_resolved(&wall, span(5, 10, 0.5, 0.5, 1), &mut out);
        let l1 = out.clone();
        insert_resolved(&l1, span(12, 15, 0.05, 0.05, 2), &mut out);
        let o = owners(&out, 20);
        assert_eq!(
            &o[..],
            &[0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        // A span crossing the wall: behind at its left end, in front at its right end.
        insert_resolved(&wall, span(0, 20, 0.0, 0.2, 3), &mut out);
        let o = owners(&out, 20);
        let first_new = o.iter().position(|&s| s == 3).unwrap();
        assert!(o[..first_new].iter().all(|&s| s == 0) && o[first_new..].iter().all(|&s| s == 3));
        assert!(
            (9..=11).contains(&first_new),
            "crossing near the middle, got {first_new}"
        );
        // New extends past both ends of an old span and into empty space.
        insert_resolved(
            &[span(5, 8, 0.1, 0.1, 0)],
            span(2, 12, 0.5, 0.5, 1),
            &mut out,
        );
        assert_eq!(owners(&out, 12)[2..12], [1; 10]);
    }
}
