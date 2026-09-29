//! The two-phase frame: polygon setup and band binning (phase 1), then per-row span
//! resolution and shading (phase 2). See the span buffer module spec.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use glam::Vec3;
use moose_assets::{Assets, MeshId, Light, Texture, TextureId};
use moose_scene::Viewport;
use moose_view::{
    EdgeLine, Object, PolygonKind, ScreenVertex, ViewGeometry, ViewPolygon, pixel_edge,
};
use rayon::prelude::*;

use crate::shader::{
    F32s, Fill,
    Behind, Draw, FACE_NORMAL, LANES, LOD, MAX_STEP, MAX_TEXTURES, MAX_VARYINGS,
    Material, MaterialEntry, MaterialId, POSITION, Params, SampleContext, SpanJob, TextureSet,
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
    /// more than a quarter across such a cell (seen nearly edge-on) goes closer, so grid
    /// points past its edges stay where w is well above zero. Wider saves sample work (and
    /// lighting) on steep surfaces, at the cost of perspective accuracy on them.
    pub min_step: u32,
    /// How much w may change (relative to its smallest value) across a cell held to
    /// `min_step` before the cell is made smaller anyway: a surface seen so nearly edge-on
    /// that it would change more gets finer spacing, so grid points past its edges stay
    /// where w is well above zero. Infinity always holds to `min_step`, whatever it costs
    /// pixels at such edges.
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
    /// Debug overlay of the sample lattice: grid rows tinted red, and the grid points on
    /// them marked green.
    pub show_samples: bool,
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
            penumbra_threshold: 0.125,
            penumbra_spacing: 4,
            penumbra_padding: 16,
            show_samples: false,
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
}

impl Surface {
    pub fn new(material: MaterialId) -> Self {
        Self {
            material,
            params: Params::default(),
            path_override: None,
            textures: [None; MAX_TEXTURES],
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
    /// Where its `sampled` world position is among its sampled values, if it has one.
    position: Option<u16>,
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
    /// Each polygon's lights (see `PolygonSetup::first_light`).
    lights: Vec<Light>,
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
}

/// One thread's phase 2 scratchpad, sized from the viewport width.
#[derive(Default)]
struct RowScratch {
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
    /// One row's sample points for the run being shaded: x, outputs, and whether each is on
    /// a grid row of its tile (for the overlay).
    points_x: Vec<f32>,
    points_v: Vec<f32>,
    points_exact: Vec<bool>,
    /// The tiles' runs of the row's sample points, while `row_points` gathers them.
    segments: Vec<Segment>,
    /// While a lattice is built: the polygon's first and last pixel on each of the band's
    /// rows in the block, and the grid points waiting to be evaluated.
    row_pixels: Vec<(i32, i32)>,
    pending: Vec<Pending>,
    /// Each strip's column and row spacing and its least w, while its lattice is built.
    spacings: Vec<(i32, i32, f32)>,
    world: Vec<u32>,
    span: Vec<u32>,
    pixel: Vec<u32>,
    pixel_spans: Vec<Span>,
    translucent: Vec<u32>,
    translucent_spans: Vec<Span>,
    /// Where one translucent span is in front: pixels, and the opaque span behind (if any).
    visible: Vec<(i32, i32, Option<u32>)>,
    blend: Vec<u32>,
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
}

/// The span buffer renderer. Keeps its arenas and scratchpads between frames.
pub struct Renderer {
    pub config: RasterConfig,
    materials: Vec<MaterialEntry>,
    /// Per (mesh, material): where each vertex stage input comes from, and the `sampled`
    /// built-ins. Validated once, then reused.
    remaps: HashMap<(MeshId, MaterialId), Remap>,
    surfaces: Vec<Surface>,
    bins: Vec<ThreadBins>,
    scratch: Vec<Mutex<RowScratch>>,
    /// What untextured polygons sample: 1x1 opaque white.
    blank: Texture,
}

/// The textures polygons sample, and the frame's focal length and ambient light.
struct Textures<'a> {
    assets: &'a Assets,
    blank: &'a Texture,
    focal: f32,
    ambient: Vec3,
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
            materials: Vec::new(),
            remaps: HashMap::new(),
            surfaces: Vec::new(),
            bins: Vec::new(),
            scratch: Vec::new(),
            blank: Texture::solid("blank", 0xFFFF_FFFF),
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
            ambient: geometry.ambient,
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
            bins.lights.clear();
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

        let rows_started = Instant::now();
        // ---- Phase 2: rows, split into bands. Each thread owns its band's rows.
        let width = target.width as usize;
        let busy = AtomicU64::new(0);
        let rows = &mut target.pixels
            [viewport.y as usize * width..(viewport.y + viewport.height) as usize * width];
        let (bins, scratch, shaders) = (&self.bins, &self.scratch, &self.materials);
        rows.par_chunks_mut(band_rows as usize * width)
            .enumerate()
            .for_each(|(b, chunk)| {
                let band_started = Instant::now();
                let t = rayon::current_thread_index().unwrap_or(0) % scratch.len();
                let mut s = scratch[t].lock().unwrap();
                render_band(
                    &mut s, bins, shaders, config, &textures, viewport, b, band_rows, chunk, width,
                );
                busy.fetch_add(band_started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            });
        let done = Instant::now();

        Ok(RenderStats {
            polygons: self.bins.iter().map(|b| b.polygons.len() as u32).sum(),
            bands: bands as u32,
            threads: threads as u32,
            prepare: setup_started - started,
            setup: rows_started - setup_started,
            rows: done - rows_started,
            rows_busy: Duration::from_nanos(busy.into_inner()),
        })
    }
}

/// Where a vertex stage input comes from.
#[derive(Clone, Copy, Debug)]
enum VertexSource {
    /// A mesh attribute: its index in the mesh's attributes, and its count.
    Attrib(usize, usize),
    /// The source vertex's world position.
    Position,
    /// The source polygon's plane normal, in world space.
    FaceNormal,
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
            _ => None,
        };
        if let Some(source) = builtin {
            if want.count != 3 {
                return Err(mismatch(want.name, want.count, 3));
            }
            vertex.push(source);
            continue;
        }
        let Some(index) = m.attribs.iter().position(|a| a.name == want.name) else {
            return Err(LayoutError::MissingAttribute {
                mesh: m.name.clone(),
                name: want.name,
            });
        };
        let found = m.attribs[index].count;
        if found != want.count {
            return Err(mismatch(want.name, want.count, found));
        }
        vertex.push(VertexSource::Attrib(index, found as usize));
    }
    let mut remap = Remap {
        vertex,
        position: None,
        uv: None,
        lod: None,
    };
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
                VertexSource::Attrib(a, count) => {
                    let data = &mesh.attribs[a].data;
                    for c in 0..count {
                        input[i + c] = data.get_f32(v * count + c);
                    }
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
    if let (Some(uv), Some(lod)) = (remap.uv, remap.lod) {
        let texture = textures.get(s.textures[0]);
        let size = (texture.width() as f32, texture.height() as f32);
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
    let positions = &geometry.world_positions[p.vertices()];
    let normal = Vec3::from_array(face_normal);
    let (lo, hi) = positions
        .iter()
        .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), &q| {
            (lo.min(q), hi.max(q))
        });
    let (center, radius) = ((lo + hi) * 0.5, (hi - lo).length() * 0.5);
    for &li in geometry.polygon_lights(p) {
        let light = geometry.lights[li as usize];
        let height = normal.dot(light.position - positions[0]);
        let near = light.position.clamp(lo, hi).distance(light.position);
        let in_shadow = light.shadow.is_some_and(|k| p.shadowed >> k & 1 != 0);
        if !in_shadow
            && height > 0.0
            && height < light.range
            && near < light.range
            && light.cone_reaches(center, radius)
        {
            bins.lights.push(light);
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
    bins.polygons.push(PolygonSetup {
        first_vertex,
        vertex_count: verts.len() as u16,
        n_vals: n_vals as u16,
        n_out: layout_len(entry.io.interp) as u16,
        first_plane,
        fans: fans as u16,
        row_top,
        row_end,
        max_w: verts.iter().map(|v| v.w).fold(0.0, f32::max),
        material: s.material,
        params: s.params,
        textures: s.textures,
        eye: p
            .mirror
            .map_or(geometry.eye, |m| geometry.mirrors[m as usize].eye),
        object,
        half_rate: p.mirror.is_some(),
        first_light,
        light_count: (bins.lights.len() as u32 - first_light) as u16,
        position: remap.position.map(|p| p as u16),
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
) {
    let vw = viewport.width as usize;
    let vx = viewport.x as i32;
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

        let out = &mut chunk[r * width + viewport.x as usize..r * width + viewport.x as usize + vw];
        out.copy_from_slice(&s.color);
    }
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
        let x = lines[i]
            .unwrap_or(EdgeLine::between((a.x, a.y), (c.x, c.y)))
            .x_at_row(row);
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
    let verts = &b.vertices[range.clone()];
    let ((li, x_left), (ri, x_right)) = crossings(verts, &b.lines[range], row)?;
    let (x0, x1) = (pixel_edge(x_left), pixel_edge(x_right));
    if x0 >= x1 {
        return None; // an edge-on sliver on this row
    }
    let (w_left, _) = edge_at_row(verts, li, row);
    let (w_right, _) = edge_at_row(verts, ri, row);
    let state = s.states.len() as u32;
    s.states.push(RowState {
        id,
        slot,
        x_left,
        x_right,
        w_left,
        w_right,
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

/// A polygon's grid in one block of rows (see the Material Pipeline Spec): one tile per
/// column strip it covers (and the strip after, which holds the point closing its last
/// interval), each built the first time a run there needs it.
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

/// One grid row of a tile: the grid columns it holds, and their outputs.
#[derive(Clone, Copy, Default)]
struct GridRow {
    /// First column (a pixel) and how many, `nx` apart.
    col0: i32,
    cols: u32,
    /// Its outputs in `RowScratch::grid_values`, output by output, `stride` values each
    /// (`cols` rounded up to whole lanes).
    values: u32,
    stride: u32,
}

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
        }
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
) -> SampleContext<'a> {
    let lights = p.first_light as usize..p.first_light as usize + p.light_count as usize;
    SampleContext {
        eye: p.eye,
        object: &p.object,
        params: &p.params,
        lights: &bins.lights[lights],
        ambient: textures.ambient,
        focal: textures.focal,
    }
}

/// Starts a polygon's lattice for one block of rows: the column strips it covers there
/// (and the one after, for the point closing its last interval), with a tile for each, not
/// yet built; and its smallest w there, a lower bound for every tile's.
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
    }
}

/// Builds every tile of a polygon's lattice in one block (for the band's rows): each tile's
/// spacing, then its grid rows, each holding the outputs of `shade_sample` at the tile's grid
/// columns that the rows it serves need. The polygon's pixels on each row are found once for
/// all its tiles, and the grid points of all its tiles are evaluated together, [`LANES`] at
/// a time whichever tiles they belong to: values from the polygon's plane functions
/// ([`fan_planes`]), one reciprocal of w per point.
#[allow(clippy::too_many_arguments)]
fn build_tiles(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    shaders: &[MaterialEntry],
    config: &RasterConfig,
    textures: &Textures,
    lattice: Lattice,
    id: u32,
) {
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
    s.row_pixels.extend((r0..r1).map(|r| match crossings(verts, lines, r) {
        Some(((_, xl), (_, xr))) => (pixel_edge(xl), pixel_edge(xr) - 1),
        None => (i32::MAX, i32::MIN),
    }));

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
            .filter(|l| !l.is_point())
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
        // than the steep limit across a cell. Every grid point a pixel uses is within a cell
        // of it in each direction, so w stays well above zero even at points past the edges.
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
        // A last guard: keep w off zero at points past the edges.
        let w_floor = if w_min.is_finite() { w_min / 8.0 } else { 0.0 };

        // Grid rows from the one at or above the polygon's first row in the block and the
        // band, to the one at or below its last there (the next block's first, at most).
        let first_row = r0 - (r0 - vy).rem_euclid(ny);
        let last = r1 - 1;
        let last_row = last + (ny - (last - vy).rem_euclid(ny)) % ny;
        let rows = s.grid_rows.len() as u32;
        let last_col = xe - nx;
        let mut g = first_row;
        while g <= last_row {
            // Its columns: the run's from the one at or before the first pixel of the rows
            // it serves (those within `ny` of it) to the one at or after the last; the first
            // column also closes the last interval of the run before.
            let (mut first_px, mut last_px) = (i32::MAX, i32::MIN);
            let served = (g - ny + 1).max(r0)..(g + ny).min(r1);
            for &(a, c) in &s.row_pixels[(served.start - r0) as usize..(served.end - r0) as usize]
            {
                first_px = first_px.min(a);
                last_px = last_px.max(c);
            }
            if first_px > last_px || first_px >= xe || last_px < xs - TILE_COLS {
                s.grid_rows.push(GridRow::default());
                g += ny;
                continue;
            }
            let col0 = xs + ((first_px - xs).max(0) / nx) * nx;
            let col_end = if last_px < xs {
                xs
            } else {
                (xs + (last_px - xs + nx - 1) / nx * nx).min(last_col)
            };
            let cols = (col_end - col0) / nx + 1;
            let values = s.grid_values.len();
            s.grid_values.resize(values + n_out * cols as usize, 0.0);
            let dy = g as f32 + 0.5 - planes.oy;
            for j in 0..cols {
                s.pending.push(Pending {
                    dx: (col0 + j * nx) as f32 + 0.5 - planes.ox,
                    dy,
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

    // Every tile's grid points, LANES at a time.
    let ctx = sample_context(p, b, textures);
    let mut inputs = [F32s::default(); MAX_VARYINGS];
    let mut outputs = [F32s::default(); MAX_VARYINGS];
    let (zero, one) = (F32s::fill(0.0), F32s::fill(1.0));
    for chunk in s.pending.chunks(LANES) {
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
        (entry.sample)(&inputs[..n_in], &ctx, &mut outputs[..n_out]);
        for (k, out) in outputs[..n_out].iter().enumerate() {
            let out = out.to_array();
            for (j, q) in chunk.iter().enumerate() {
                s.grid_values[(q.at + k as u32 * q.stride) as usize] = out[j];
            }
        }
    }
}

/// A grid point waiting to be evaluated (see [`build_tiles`]): where it is relative to the
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
/// pair's): the grid columns from the one at or before `x0` to the one at or after
/// `x1 - 1`, tile by tile, each tile's outputs blended between its grid rows above and
/// below (output by output). The points are the grid's, so no pixel depends on how the row
/// was split.
fn row_points(s: &mut RowScratch, index: usize, n_out: usize, x0: i32, x1: i32) {
    let lattice = s.lattices[index];
    let (vx, row) = (s.viewport_x, s.row);
    s.points_x.clear();
    s.points_exact.clear();
    s.segments.clear();
    // The points, tile by tile: each tile's run of them, from its two grid rows.
    let mut strip = (x0 - vx).div_euclid(TILE_COLS);
    'strips: loop {
        debug_assert!(
            (lattice.first_strip..lattice.first_strip + lattice.strips as i32).contains(&strip),
            "row {row}: strip {strip} outside the polygon's"
        );
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
        let c0 = if x0 > xs { xs + (x0 - xs) / nx * nx } else { xs };
        let start = s.points_x.len();
        let mut c = c0;
        let done = loop {
            s.points_x.push(c as f32 + 0.5);
            s.points_exact.push(fy == 0.0);
            if c >= x1 - 1 {
                break true;
            }
            c += nx;
            if c >= xe {
                break false;
            }
        };
        debug_assert!(
            [a, b]
                .iter()
                .all(|r| c0 >= r.col0 && c < r.col0 + r.cols as i32 * nx.max(1) + nx),
            "row {row}: columns {c0}..{c} outside their grid rows"
        );
        s.segments.push(Segment {
            start: start as u32,
            count: (s.points_x.len() - start) as u32,
            a,
            b,
            ia: ((c0 - a.col0) / nx) as u32,
            ib: ((c0 - b.col0) / nx) as u32,
            fy,
        });
        if done {
            break 'strips;
        }
        strip = tile.end_strip;
    }
    // Outputs, output by output, each tile's run blended between its grid rows at once.
    let m = s.points_x.len();
    s.points_v.clear();
    s.points_v.resize(n_out * m, 0.0);
    for seg in &s.segments {
        let (start, count) = (seg.start as usize, seg.count as usize);
        let (a, b) = (seg.a, seg.b);
        for k in 0..n_out {
            let va = &s.grid_values[a.values as usize + k * a.stride as usize + seg.ia as usize..]
                [..count];
            let vb = &s.grid_values[b.values as usize + k * b.stride as usize + seg.ib as usize..]
                [..count];
            let out = &mut s.points_v[k * m + start..k * m + start + count];
            for ((o, &p), &q) in out.iter_mut().zip(va).zip(vb) {
                *o = p + (q - p) * seg.fy;
            }
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
        build_tiles(s, bins, shaders, config, textures, lattice, id);
    }
    // A half-rate pair shows its even pixel, or the row's first where the pair starts
    // before the row, even if hidden.
    let shown = if p.half_rate {
        (x0 - (x0 - vx).rem_euclid(2)).max(pixel_edge(x_left))
    } else {
        x0
    };
    row_points(s, index, p.n_out as usize, shown, x1);
    let job = SpanJob {
        x_left,
        x_right,
        w_left,
        w_right,
        xs: &s.points_x,
        outs: &s.points_v,
        x0,
        row: s.row,
        half_rate: p.half_rate,
        vx,
    };
    let draw = Draw {
        params: &p.params,
        textures: &textures.set(p.textures),
        eye: p.eye,
        focal: textures.focal,
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
        // Pixels on a grid row of their tile tinted red, and the grid points on them green.
        let n = s.points_x.len();
        for i in 0..n {
            if !s.points_exact[i] {
                continue;
            }
            let from = ((s.points_x[i] - 0.5) as i32).max(x0);
            let to = s.points_x.get(i + 1).map_or(from + 1, |&x| (x - 0.5) as i32).min(x1);
            for px in from..to {
                let c = &mut s.color[(px - vx) as usize];
                *c = blend(0x60FF_2020, *c);
            }
            let px = (s.points_x[i] - 0.5) as i32;
            if (x0..x1).contains(&px) {
                s.color[(px - vx) as usize] = 0x20_FF_40;
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
