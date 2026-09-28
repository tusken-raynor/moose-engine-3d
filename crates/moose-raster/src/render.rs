//! The two-phase frame: polygon setup and band binning (phase 1), then per-row span
//! resolution and shading (phase 2). See the span buffer module spec.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use glam::Vec3;
use moose_assets::{Assets, MeshId, Texture, TextureId};
use moose_scene::Viewport;
use moose_view::{
    EdgeLine, Object, PolygonKind, ScreenVertex, ViewGeometry, ViewPolygon, pixel_edge,
};
use rayon::prelude::*;

use crate::shader::{
    ANISO, ANISO_LOD, Behind, Draw, FACE_NORMAL, LANES, LOD, MAX_STEP, MAX_TEXTURES, MAX_VARYINGS,
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
    /// Smallest sample interval along a span, in pixels (1 to 32; powers of two make the
    /// most sense). Exact perspective values are taken at least this far apart, however
    /// steeply w changes. 4 is cheap but can be off by up to about 15 color levels on
    /// surfaces seen edge-on from very close; 1 is exact there at one division per pixel.
    pub min_step: u32,
    /// Color for pixels no polygon covers.
    pub background: u32,
    /// Polygons per frame from which phase 1 (setup and binning) is split across threads.
    /// Below it, one thread does it: waking the pool costs tens of microseconds, far more
    /// than setting up a few hundred polygons.
    pub parallel_setup: usize,
    /// Debug overlay of the sample lattice: sample rows tinted red, and the pixels sample
    /// points sit on (columns, and the first and last pixels of rows between sample rows)
    /// marked green.
    pub show_samples: bool,
}

impl Default for RasterConfig {
    fn default() -> Self {
        Self {
            actor_path: RasterPath::PerPixel,
            band_rows: 8,
            step_threshold: 1.0 / 16.0,
            min_step: 4,
            background: 0,
            parallel_setup: 1024,
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
    /// Start of this polygon's `sampled` values: `vertex_count * n_vals` values.
    first_value: u32,
    n_vals: u16,
    /// Its material's outputs per sample point.
    n_out: u16,
    /// w's change per pixel across and down the screen (w is affine on screen).
    dwdx: f32,
    dwdy: f32,
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
    values: Vec<f32>,
    bands: Vec<BandBins>,
    /// Scratch for one polygon's vertex texture footprints.
    footprints: Vec<Footprint>,
    /// Scratch for one polygon's vertex stage outputs at its source vertices.
    vertex_out: Vec<f32>,
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
    /// Per polygon slot of the band and block, its lattice (`slot * blocks + block`), and
    /// the lattices' sample rows, knots and outputs.
    lattices: Vec<Lattice>,
    sample_rows: Vec<SampleRow>,
    knots: Vec<Knot>,
    lattice_values: Vec<f32>,
    /// Scratch for building a lattice: its sample rows and their points' inputs.
    sample_ys: Vec<i32>,
    sample_inputs: Vec<f32>,
    /// One row's sample points for the run being shaded: x, and outputs, and whether it is
    /// a sample row.
    points_x: Vec<f32>,
    points_v: Vec<f32>,
    row_exact: bool,
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

/// The textures polygons sample, and the frame's focal length.
struct Textures<'a> {
    assets: &'a Assets,
    blank: &'a Texture,
    focal: f32,
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

#[derive(Clone, Copy, Debug)]
enum FootprintPart {
    Lod,
    Aniso,
    AnisoLod,
}

/// A material mapped onto a mesh.
#[derive(Clone, Debug)]
struct Remap {
    /// The vertex stage's inputs, in declaration order.
    vertex: Vec<VertexSource>,
    /// Offsets of the `sampled` built-ins: the world position, `uv` (what the footprint is
    /// measured from) and the footprint's parts.
    position: Option<usize>,
    uv: Option<usize>,
    footprint: Vec<(usize, FootprintPart)>,
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
        footprint: Vec::new(),
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
                remap.footprint.push((offset, FootprintPart::Lod));
            }
            ANISO => {
                expect(2)?;
                remap.footprint.push((offset, FootprintPart::Aniso));
            }
            ANISO_LOD => {
                expect(1)?;
                remap.footprint.push((offset, FootprintPart::AnisoLod));
            }
            _ => {}
        }
        offset += want.count as usize;
    }
    if !remap.footprint.is_empty() && remap.uv.is_none() {
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
    let first_value = bins.values.len() as u32;
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
    let (dwdx, dwdy) = w_gradient(verts);
    if let Some(uv) = remap.uv.filter(|_| !remap.footprint.is_empty()) {
        let texture = textures.get(s.textures[0]);
        let size = (texture.width() as f32, texture.height() as f32);
        let values = &bins.values[first_value as usize..];
        let uv_at = |v: usize| (values[v * n_vals + uv], values[v * n_vals + uv + 1]);
        vertex_footprints(verts, uv_at, size, &mut bins.footprints);
        for (v, f) in bins.footprints.iter().enumerate() {
            let values = &mut bins.values[first_value as usize + v * n_vals..];
            for &(offset, part) in &remap.footprint {
                match part {
                    FootprintPart::Lod => values[offset] = f.lod,
                    FootprintPart::Aniso => values[offset..offset + 2].copy_from_slice(&f.aniso),
                    FootprintPart::AnisoLod => values[offset] = f.aniso_lod,
                }
            }
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
        first_value,
        n_vals: n_vals as u16,
        n_out: layout_len(entry.io.interp) as u16,
        dwdx,
        dwdy,
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
    s.sample_rows.clear();
    s.knots.clear();
    s.lattice_values.clear();

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

/// Rows per lattice block: sample rows are at most this far apart, and a block's rows share
/// one spacing.
const BLOCK_ROWS: i32 = 8;

/// A polygon's sample lattice in one block of rows (see the Material Pipeline Spec), built
/// the first time one of its runs there is shaded.
#[derive(Clone, Copy, Default)]
struct Lattice {
    built: bool,
    /// Column spacing (in shaded pixels times the stride).
    nx: i32,
    /// Its sample rows, top to bottom, in `RowScratch::sample_rows`, and its edge knots, top
    /// to bottom, in `RowScratch::knots`.
    rows: (u32, u32),
    knots: (u32, u32),
}

/// One sample row of a lattice: exact outputs at its two ends (its first and last pixels'
/// centers, or where it has none, its edge crossings), at its edge crossings (for the rows
/// between, whose ends are interpolated along the edges) and at its columns.
#[derive(Clone, Copy)]
struct SampleRow {
    row: i32,
    /// Where its ends are sampled.
    x_left: f32,
    x_right: f32,
    /// First column (a pixel) and how many, `nx` apart.
    col0: i32,
    cols: u32,
    /// Outputs of the left end, the right end, the left crossing, the right crossing, then
    /// each column, `n_out` each, in `RowScratch::lattice_values`.
    values: u32,
}

/// Points of a sample row before its columns: two ends, two crossings.
const ROW_POINTS: u32 = 4;

/// A polygon vertex between two sample rows, on its left or right edge chain.
#[derive(Clone, Copy)]
struct Knot {
    y: f32,
    left: bool,
    /// Its outputs, in `RowScratch::lattice_values`.
    values: u32,
}

/// The sample context of a polygon.
fn sample_context<'a>(p: &'a PolygonSetup, focal: f32) -> SampleContext<'a> {
    SampleContext {
        eye: p.eye,
        focal,
        object: &p.object,
        params: &p.params,
    }
}

/// Adds the exact `sampled` values at one sample row's points to `inputs` (its left end,
/// right end, left and right edge crossings, then every column `nx` apart from the
/// viewport's left edge strictly between the ends), and returns the row, its values offset
/// counted from the lattice's first point. The ends are its first and last pixels'
/// centers, or where it has no pixels, its edge crossings. `None` if no edges cross the
/// row.
#[allow(clippy::too_many_arguments)]
fn sample_row_inputs(
    b: &ThreadBins,
    p: &PolygonSetup,
    row: i32,
    nx: i32,
    vx: i32,
    first_point: u32,
    inputs: &mut Vec<f32>,
) -> Option<SampleRow> {
    let range = p.first_vertex as usize..p.first_vertex as usize + p.vertex_count as usize;
    let verts = &b.vertices[range.clone()];
    let ((li, x_left), (ri, x_right)) = crossings(verts, &b.lines[range], row)?;
    let n_in = p.n_vals as usize;
    let values = &b.values[p.first_value as usize..];
    let n = verts.len();
    let at = inputs.len();
    inputs.resize(at + 2 * n_in, 0.0);
    let mut w = [0.0f32; 2];
    for (side, &i) in [li, ri].iter().enumerate() {
        let (wi, alpha) = edge_at_row(verts, i, row);
        w[side] = wi;
        let (va, vc) = (
            &values[i * n_in..(i + 1) * n_in],
            &values[((i + 1) % n) * n_in..((i + 1) % n + 1) * n_in],
        );
        let out = &mut inputs[at + side * n_in..at + (side + 1) * n_in];
        for k in 0..n_in {
            out[k] = va[k] + (vc[k] - va[k]) * alpha;
        }
    }
    // Perspective-correct between the crossings.
    let span = x_right - x_left;
    let inv_span = if span > 0.0 { 1.0 / span } else { 0.0 };
    let alpha = |x: f32| {
        let s = ((x - x_left) * inv_span).clamp(0.0, 1.0);
        let den = (1.0 - s) * w[0] + s * w[1];
        if den > 0.0 { s * w[1] / den } else { s }
    };
    let mut crossing = [0.0f32; 2 * MAX_VARYINGS];
    crossing[..2 * n_in].copy_from_slice(&inputs[at..at + 2 * n_in]);
    inputs.extend_from_slice(&crossing[..2 * n_in]);
    let value = |alpha: f32, out: &mut [f32]| {
        for k in 0..n_in {
            let (l, r) = (crossing[k], crossing[n_in + k]);
            out[k] = l + (r - l) * alpha;
        }
    };
    // The ends: the first and last pixels, if any.
    let (row_x0, row_x1) = (pixel_edge(x_left), pixel_edge(x_right));
    let (end_left, end_right) = if row_x0 < row_x1 {
        let (l, r) = (row_x0 as f32 + 0.5, row_x1 as f32 - 0.5);
        value(alpha(l), &mut inputs[at..at + n_in]);
        value(alpha(r), &mut inputs[at + n_in..at + 2 * n_in]);
        (l, r)
    } else {
        (x_left, x_right)
    };
    // Columns strictly between the ends.
    let mut c = vx + ((end_left - 0.5 - vx as f32) / nx as f32).floor() as i32 * nx;
    while c as f32 + 0.5 <= end_left {
        c += nx;
    }
    let col0 = c;
    let mut cols = 0;
    while (c as f32 + 0.5) < end_right {
        let base = inputs.len();
        inputs.resize(base + n_in, 0.0);
        value(alpha(c as f32 + 0.5), &mut inputs[base..]);
        cols += 1;
        c += nx;
    }
    Some(SampleRow {
        row,
        x_left: end_left,
        x_right: end_right,
        col0,
        cols,
        values: first_point,
    })
}

/// Builds a polygon's lattice for the current band: its spacing, its sample rows and edge
/// knots, and `shade_sample` on all their points at once.
#[allow(clippy::too_many_arguments)]
fn build_lattice(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    shaders: &[MaterialEntry],
    config: &RasterConfig,
    textures: &Textures,
    index: usize,
    id: u32,
    block: i32,
) {
    let (t, l) = split_id(id);
    let b = &bins[t];
    let p = &b.polygons[l];
    let entry = &shaders[p.material.0 as usize];
    let (n_in, n_out) = (p.n_vals as usize, p.n_out as usize);
    let range = p.first_vertex as usize..p.first_vertex as usize + p.vertex_count as usize;
    let verts = &b.vertices[range];
    let block_top = s.viewport_y + block * BLOCK_ROWS;
    let (r0, r1) = (
        p.row_top.max(block_top),
        p.row_end.min(block_top + BLOCK_ROWS),
    );

    // Spacing: the material's, or less where w changes by more than the threshold across
    // a cell (relative to its smallest value in the block): across, down, and along the
    // edges (which row ends are interpolated along). Rows between sample rows also skip the
    // columns that lie outside the polygon on a sample row, so their first interval runs
    // as far as an edge moves across in that many rows: that too must stay within the
    // threshold and the material's spacing.
    let (ya, yb) = (r0 as f32, r1 as f32);
    let (mut w_min, mut dwdy, mut slope) = (f32::INFINITY, p.dwdy.abs(), 0.0f32);
    let n = verts.len();
    for i in 0..n {
        let (a, c) = (verts[i], verts[(i + 1) % n]);
        if (ya..=yb).contains(&a.y) {
            w_min = w_min.min(a.w);
        }
        if a.y != c.y && a.y.max(c.y) >= ya && a.y.min(c.y) <= yb {
            dwdy = dwdy.max(((c.w - a.w) / (c.y - a.y)).abs());
            slope = slope.max(((c.x - a.x) / (c.y - a.y)).abs());
        }
        for y in [ya, yb] {
            if a.y != c.y && (a.y.min(c.y)..=a.y.max(c.y)).contains(&y) {
                w_min = w_min.min(a.w + (c.w - a.w) * (y - a.y) / (c.y - a.y));
            }
        }
    }
    let limit = |dw: f32| {
        let perspective = if dw != 0.0 && w_min.is_finite() {
            config.step_threshold * w_min / dw.abs()
        } else {
            f32::INFINITY
        };
        perspective.min(entry.sample_spacing as f32)
    };
    let pow2 = |limit: f32, lo: i32, hi: i32| {
        let mut n = hi;
        while n > lo && n as f32 > limit {
            n /= 2;
        }
        n
    };
    let stride = if p.half_rate { 2 } else { 1 };
    let min_step = config.min_step.clamp(1, MAX_STEP as u32) as i32;
    let nx = pow2(limit(p.dwdx), min_step, MAX_STEP) * stride;
    let travel = if slope > 0.0 {
        limit(p.dwdx * slope).min(entry.sample_spacing as f32 / slope)
    } else {
        f32::INFINITY
    };
    let ny = pow2(limit(dwdy).min(travel), 1, BLOCK_ROWS);

    // Sample rows: the polygon's first row in the block, the grid rows, and its last row
    // or the next block's first row (a grid row).
    s.sample_ys.clear();
    s.sample_ys.push(r0);
    let mut y = r0 + (ny - (r0 - s.viewport_y).rem_euclid(ny)) % ny;
    if y == r0 {
        y += ny;
    }
    while y < r1 {
        s.sample_ys.push(y);
        y += ny;
    }
    let last = if r1 == p.row_end { r1 - 1 } else { r1 };
    if *s.sample_ys.last().unwrap() < last {
        s.sample_ys.push(last);
    }

    // Their points' exact values, then the knots'.
    s.sample_inputs.clear();
    let rows_start = s.sample_rows.len() as u32;
    let values_start = s.lattice_values.len() as u32;
    let mut points = 0u32;
    for k in 0..s.sample_ys.len() {
        let row = s.sample_ys[k];
        if let Some(mut sr) =
            sample_row_inputs(b, p, row, nx, s.viewport_x, points, &mut s.sample_inputs)
        {
            sr.values += values_start;
            points += ROW_POINTS + sr.cols;
            s.sample_rows.push(sr);
        }
    }
    let rows_end = s.sample_rows.len() as u32;
    let knots_start = s.knots.len() as u32;
    if rows_end > rows_start {
        let (top, bottom) = (
            s.sample_rows[rows_start as usize].row as f32 + 0.5,
            s.sample_rows[rows_end as usize - 1].row as f32 + 0.5,
        );
        let values = &b.values[p.first_value as usize..];
        for i in 0..n {
            let (prev, v, next) = (verts[(i + n - 1) % n], verts[i], verts[(i + 1) % n]);
            if v.y <= top || v.y >= bottom {
                continue;
            }
            // On the left chain the outline runs down through it, on the right up.
            let left = if prev.y < v.y && v.y < next.y {
                true
            } else if prev.y > v.y && v.y > next.y {
                false
            } else {
                continue;
            };
            s.sample_inputs
                .extend_from_slice(&values[i * n_in..(i + 1) * n_in]);
            s.knots.push(Knot {
                y: v.y,
                left,
                values: values_start + points * n_out as u32,
            });
            points += 1;
        }
        s.knots[knots_start as usize..].sort_by(|a, b| a.y.total_cmp(&b.y));
    }
    let knots_end = s.knots.len() as u32;
    s.lattice_values
        .resize(values_start as usize + points as usize * n_out, 0.0);
    // Row values offsets were counted in points; make them value offsets.
    for sr in &mut s.sample_rows[rows_start as usize..rows_end as usize] {
        sr.values = values_start + (sr.values - values_start) * n_out as u32;
    }
    (entry.sample)(
        &s.sample_inputs,
        points as usize,
        &sample_context(p, textures.focal),
        &mut s.lattice_values[values_start as usize..],
    );
    s.lattices[index] = Lattice {
        built: true,
        nx,
        rows: (rows_start, rows_end),
        knots: (knots_start, knots_end),
    };
}

/// Where a row's sample point comes from.
#[derive(Clone, Copy, PartialEq)]
enum PointSource {
    /// A sample row's own ends: exact.
    End(u32),
    /// A row between sample rows: its edge crossings (interpolated along the edges) and its
    /// first and last pixels (perspective-correct between the crossing and the nearest
    /// point inside).
    Crossing(bool),
    Pixel(bool),
}

/// A row's sample points, in order: up to two at its left end, columns `nx` apart, up to
/// two at its right end.
struct RowLayout {
    head: [(f32, PointSource); 2],
    heads: usize,
    tail: [(f32, PointSource); 2],
    tails: usize,
    col0: i32,
    cols: usize,
    nx: i32,
}

impl RowLayout {
    fn len(&self) -> usize {
        self.heads + self.cols + self.tails
    }

    /// Point `i`'s x, and its source (`None` for a column).
    fn at(&self, i: usize) -> (f32, Option<PointSource>) {
        if i < self.heads {
            let (x, src) = self.head[i];
            (x, Some(src))
        } else if i < self.heads + self.cols {
            let c = self.col0 + (i - self.heads) as i32 * self.nx;
            (c as f32 + 0.5, None)
        } else {
            let (x, src) = self.tail[i - self.heads - self.cols];
            (x, Some(src))
        }
    }

    /// The number of points at or before `x`.
    fn count_to(&self, x: f32) -> usize {
        let (mut lo, mut hi) = (0, self.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.at(mid).0 <= x {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }
}

/// Fills `s.points_x` and `s.points_v` with a polygon's sample points on the current row
/// that shade pixels `x0..x1` (from the last point at or before `x0`'s center to the first
/// past `x1 - 1`'s): exact on sample rows; elsewhere its crossings interpolated along its
/// edge chains between knots, the columns inside it on both sample rows around it
/// interpolated between them, and its first and last pixels perspective-correct between
/// those. The points are the row's whatever the run, so no pixel depends on how the row
/// was split.
#[allow(clippy::too_many_arguments)]
fn row_points(
    s: &mut RowScratch,
    index: usize,
    st: (f32, f32, f32, f32),
    n_out: usize,
    x0: i32,
    x1: i32,
) {
    let (x_left, x_right, w_left, w_right) = st;
    let lattice = s.lattices[index];
    let rows = &s.sample_rows[lattice.rows.0 as usize..lattice.rows.1 as usize];
    let row = s.row;
    let vals = &s.lattice_values;
    let at = |offset: u32, point: u32| {
        let i = (offset + point * n_out as u32) as usize;
        &vals[i..i + n_out]
    };
    let (xs, vs) = (&mut s.points_x, &mut s.points_v);
    xs.clear();
    vs.clear();
    vs.reserve(n_out * (x1 - x0 + 8) as usize);
    let nx = lattice.nx;
    let below = rows.partition_point(|r| r.row < row);
    let exact = rows.get(below).filter(|r| r.row == row);
    s.row_exact = exact.is_some();
    let brackets = below.checked_sub(1).map(|i| &rows[i]).zip(rows.get(below));
    let none = (0.0, PointSource::End(0));
    let layout = if let Some(r) = exact {
        let two = r.x_right > r.x_left;
        RowLayout {
            head: [(r.x_left, PointSource::End(0)), none],
            heads: 1,
            tail: [(r.x_right, PointSource::End(1)), none],
            tails: two as usize,
            col0: r.col0,
            cols: r.cols as usize,
            nx,
        }
    } else if let Some((a, b)) = brackets {
        // Columns inside this row and both sample rows: one run of them.
        let (lo, hi) = (
            a.col0.max(b.col0),
            (a.col0 + a.cols as i32 * nx).min(b.col0 + b.cols as i32 * nx),
        );
        let mut col0 = lo;
        if (col0 as f32 + 0.5) <= x_left {
            col0 += ((x_left - 0.5 - col0 as f32) / nx as f32).floor() as i32 * nx + nx;
        }
        let mut end = hi;
        let limit = (x_right - 0.5).ceil() as i32; // columns before it are inside
        if end > limit {
            end = col0 + ((limit - col0 + nx - 1) / nx).max(0) * nx;
        }
        let cols = ((end - col0).max(0) / nx) as usize;
        let (first, last) = (
            pixel_edge(x_left) as f32 + 0.5,
            pixel_edge(x_right) as f32 - 0.5,
        );
        let (inner_left, inner_right) = if cols > 0 {
            (
                col0 as f32 + 0.5,
                (col0 + (cols as i32 - 1) * nx) as f32 + 0.5,
            )
        } else {
            (x_right, x_left)
        };
        let mut layout = RowLayout {
            head: [(x_left, PointSource::Crossing(true)), none],
            heads: 1,
            tail: [none, none],
            tails: 0,
            col0,
            cols,
            nx,
        };
        // The pixels only where they fall strictly between their neighbors.
        let first_in = first > x_left && first < inner_left.min(x_right) && first <= last;
        if first_in {
            layout.head[1] = (first, PointSource::Pixel(false));
            layout.heads = 2;
        }
        let after = if cols > 0 {
            inner_right
        } else if first_in {
            first
        } else {
            x_left
        };
        if last > after && last < x_right {
            layout.tail[0] = (last, PointSource::Pixel(true));
            layout.tails = 1;
        }
        layout.tail[layout.tails] = (x_right, PointSource::Crossing(false));
        layout.tails += 1;
        layout
    } else {
        return; // no sample rows around it: nothing to shade from
    };
    let n = layout.len();
    // The points bracketing the run: from the last at or before its first pixel's center
    // (the row's last point shades its own pixel as the end of the interval before it) to
    // the first past its last pixel's.
    let (lo, hi) = (x0 as f32 + 0.5, (x1 - 1) as f32 + 0.5);
    let start = layout
        .count_to(lo)
        .saturating_sub(1)
        .min(n.saturating_sub(2));
    let end = layout.count_to(hi).min(n - 1);
    // Values.
    let mut cross = [[0.0f32; MAX_VARYINGS]; 2];
    let mut fy = 0.0;
    let ab = if exact.is_none() { brackets } else { None };
    if let Some((a, b)) = ab {
        fy = (row - a.row) as f32 / (b.row - a.row) as f32;
        let y = row as f32 + 0.5;
        let knots = &s.knots[lattice.knots.0 as usize..lattice.knots.1 as usize];
        // Along one edge chain: the knots between the two rows' crossings.
        for (side, left) in [(0, true), (1, false)] {
            let (top, bottom) = (a.row as f32 + 0.5, b.row as f32 + 0.5);
            let (mut y0, mut v0) = (top, at(a.values, 2 + side as u32));
            let (mut y1, mut v1) = (bottom, at(b.values, 2 + side as u32));
            for k in knots {
                if k.left != left || k.y <= top || k.y >= bottom {
                    continue;
                }
                if k.y < y {
                    (y0, v0) = (k.y, at(k.values, 0));
                } else if k.y < y1 {
                    (y1, v1) = (k.y, at(k.values, 0));
                }
            }
            let f = if y1 > y0 {
                ((y - y0) / (y1 - y0)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            for (o, (&p, &q)) in cross[side].iter_mut().zip(v0.iter().zip(v1)) {
                *o = p + (q - p) * f;
            }
        }
    }
    // A point's values (not a pixel's).
    let value = |i: usize, out: &mut Vec<f32>| match layout.at(i).1 {
        Some(PointSource::End(side)) => out.extend_from_slice(at(exact.unwrap().values, side)),
        Some(PointSource::Crossing(left)) => out.extend_from_slice(&cross[!left as usize][..n_out]),
        Some(PointSource::Pixel(_)) => unreachable!(),
        None => {
            let c = layout.col0 + (i - layout.heads) as i32 * nx;
            if let Some(r) = exact {
                out.extend_from_slice(at(r.values, ROW_POINTS + ((c - r.col0) / nx) as u32));
            } else {
                let (a, b) = ab.unwrap();
                let va = at(a.values, ROW_POINTS + ((c - a.col0) / nx) as u32);
                let vb = at(b.values, ROW_POINTS + ((c - b.col0) / nx) as u32);
                let o = out.len();
                out.resize(o + n_out, 0.0);
                for ((o, &p), &q) in out[o..].iter_mut().zip(va).zip(vb) {
                    *o = p + (q - p) * fy;
                }
            }
        }
    };
    let w_at = |x: f32| {
        let span = x_right - x_left;
        let t = if span > 0.0 {
            ((x - x_left) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        w_left + (w_right - w_left) * t
    };
    let is_pixel = |i: usize| matches!(layout.at(i).1, Some(PointSource::Pixel(_)));
    // One end point: its own values, or a pixel's, perspective-correct between its nearest
    // neighbors that are not pixels themselves (with no columns, the other pixel lies
    // between).
    let point = |i: usize, vs: &mut Vec<f32>| {
        if !is_pixel(i) {
            value(i, vs);
            return;
        }
        let x = layout.at(i).0;
        let (mut ia, mut ib) = (i - 1, i + 1);
        while is_pixel(ia) {
            ia -= 1;
        }
        while is_pixel(ib) {
            ib += 1;
        }
        let at = vs.len();
        value(ia, vs);
        value(ib, vs);
        let (xa, xb) = (layout.at(ia).0, layout.at(ib).0);
        let (wa, wb) = (w_at(xa), w_at(xb));
        let t = (x - xa) / (xb - xa);
        let den = (1.0 - t) * wa + t * wb;
        let alpha = if den > 0.0 { t * wb / den } else { t };
        for k in 0..n_out {
            let (p, q) = (vs[at + k], vs[at + n_out + k]);
            vs[at + k] = p + (q - p) * alpha;
        }
        vs.truncate(at + n_out);
    };
    let (heads, cols) = (layout.heads, layout.cols);
    for i in start..heads.min(end + 1) {
        point(i, vs);
        xs.push(layout.at(i).0);
    }
    // The columns in the window, all at once: contiguous in their sample rows.
    let (k0, k1) = (
        start.max(heads) - heads,
        (end + 1).min(heads + cols).saturating_sub(heads),
    );
    if k0 < k1 {
        let c0 = layout.col0 + k0 as i32 * nx;
        for k in k0..k1 {
            xs.push((layout.col0 + k as i32 * nx) as f32 + 0.5);
        }
        let len = (k1 - k0) * n_out;
        let first = |r: &SampleRow| {
            (r.values + (ROW_POINTS + ((c0 - r.col0) / nx) as u32) * n_out as u32) as usize
        };
        if let Some(r) = exact {
            let from = first(r);
            vs.extend_from_slice(&vals[from..from + len]);
        } else {
            let (a, b) = ab.unwrap();
            let (fa, fb) = (first(a), first(b));
            let (va, vb) = (&vals[fa..fa + len], &vals[fb..fb + len]);
            let o = vs.len();
            vs.resize(o + len, 0.0);
            for ((o, &p), &q) in vs[o..].iter_mut().zip(va).zip(vb) {
                *o = p + (q - p) * fy;
            }
        }
    }
    for i in start.max(heads + cols)..=end {
        point(i, vs);
        xs.push(layout.at(i).0);
    }
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
    if !s.lattices[index].built {
        build_lattice(s, bins, shaders, config, textures, index, id, block);
    }
    let (t, l) = split_id(id);
    let p = &bins[t].polygons[l];
    row_points(
        s,
        index,
        (x_left, x_right, w_left, w_right),
        p.n_out as usize,
        x0,
        x1,
    );
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
        let exact = s.row_exact;
        for c in &mut s.color[run] {
            if exact {
                *c = blend(0x60FF_2020, *c);
            }
        }
        for &x in &s.points_x {
            let px = (x - 0.5).round() as i32;
            if x.fract() == 0.5 && (x0..x1).contains(&px) {
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

/// w's change per pixel across and down the screen, from the polygon's widest fan
/// triangle (w is affine on screen for a planar polygon); 0 for a degenerate polygon.
fn w_gradient(verts: &[ScreenVertex]) -> (f32, f32) {
    let n = verts.len();
    let area = |i: usize| {
        let (a, b, c) = (verts[0], verts[i], verts[i + 1]);
        (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)
    };
    let Some(i) = (1..n - 1)
        .max_by(|&i, &j| area(i).abs().total_cmp(&area(j).abs()))
        .filter(|&i| area(i).abs() > 1e-6)
    else {
        return (0.0, 0.0);
    };
    let (a, b, c) = (verts[0], verts[i], verts[i + 1]);
    let det = area(i);
    let dwdx = ((b.w - a.w) * (c.y - a.y) - (c.w - a.w) * (b.y - a.y)) / det;
    let dwdy = ((c.w - a.w) * (b.x - a.x) - (b.w - a.w) * (c.x - a.x)) / det;
    (dwdx, dwdy)
}

/// A pixel's texture footprint at a vertex: see [`LOD`], [`ANISO`] and [`ANISO_LOD`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Footprint {
    lod: f32,
    aniso: [f32; 2],
    aniso_lod: f32,
}

impl Footprint {
    /// Everything at the smallest level: for edge-on polygons, where nothing can be measured.
    const TINY: Footprint = Footprint {
        lod: 16.0,
        aniso: [0.0; 2],
        aniso_lod: 16.0,
    };
}

/// Each vertex's texture footprint, from the polygon's exact screen-space derivatives of uv
/// there, in texels of a `size.0` by `size.1` texture:
///
/// - `lod`: log2 of texels per screen pixel, the larger of the x and y footprints
///   (isotropic filtering).
/// - `aniso`, `aniso_lod`: 2x anisotropic probes. Each covers `p = max(short, long / 2)`
///   texels (`aniso_lod = log2 p`), and they sit `±(long - p) / 2` along the long axis (in
///   uv), so together they span it. A round footprint (`short = long`) gives offset 0, and
///   the offset grows smoothly with the ratio up to 2:1. Magnified footprints (at most one
///   texel) take no offset.
///
/// A planar polygon's `u * w`, `v * w` and `w` are linear across the screen, so their
/// gradients come from any three of its vertices (the widest triangle, for precision), and
/// `du/dx = (d(u w)/dx - u dw/dx) / w` at each vertex, and likewise for the rest.
fn vertex_footprints(
    verts: &[ScreenVertex],
    uv: impl Fn(usize) -> (f32, f32),
    size: (f32, f32),
    out: &mut Vec<Footprint>,
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
        // Edge-on: nothing to measure; the smallest level is the safe choice.
        out.extend(std::iter::repeat_n(Footprint::TINY, n));
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
        // The same in texels, and each axis's length.
        let x_len = (dudx * size.0).hypot(dvdx * size.1);
        let y_len = (dudy * size.0).hypot(dvdy * size.1);
        let (long, short, long_uv) = if x_len >= y_len {
            (x_len, y_len, (dudx, dvdx))
        } else {
            (y_len, x_len, (dudy, dvdy))
        };
        let log2 = |t: f32| if t > 0.0 { t.log2() } else { -16.0 };
        let lod = log2(long);
        out.push(if long <= 1.0 {
            // Magnified: one probe, nothing to spread.
            Footprint {
                lod,
                aniso: [0.0; 2],
                aniso_lod: lod,
            }
        } else {
            let probe = short.max(long / 2.0);
            let apart = (long - probe) / 2.0 / long; // of the long axis, each way
            Footprint {
                lod,
                aniso: [long_uv.0 * apart, long_uv.1 * apart],
                aniso_lod: log2(probe),
            }
        });
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
        vertex_footprints(&verts, |i| uvs[i], size, &mut out);
        out.iter().map(|f| f.lod).collect()
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
    fn anisotropic_probes_span_the_long_axis() {
        let footprints = |w: f32, h: f32| {
            let verts: Vec<ScreenVertex> = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]
                .iter()
                .map(|&(x, y)| ScreenVertex { x, y, w: 1.0 })
                .collect();
            let uvs = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
            let mut out = Vec::new();
            vertex_footprints(&verts, |i| uvs[i], (64.0, 64.0), &mut out);
            out
        };
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        // Round (1 texel per pixel both ways, or 2): one probe, at the trilinear level.
        for side in [64.0, 32.0] {
            for f in footprints(side, side) {
                assert!(f.aniso == [0.0; 2] && close(f.aniso_lod, f.lod), "{f:?}");
            }
        }
        // 1 texel per pixel across, 4 down: probes of 2 texels (level 1), a texel either
        // side of the center along v (1/64 in uv).
        for f in footprints(64.0, 16.0) {
            assert!(close(f.lod, 2.0) && close(f.aniso_lod, 1.0), "{f:?}");
            assert!(
                close(f.aniso[0], 0.0) && close(f.aniso[1], 1.0 / 64.0),
                "{f:?}"
            );
        }
        // 1.5 down: probes of 1 texel (the short axis), a quarter texel either side.
        for f in footprints(64.0, 64.0 / 1.5) {
            assert!(close(f.aniso_lod, 0.0), "{f:?}");
            assert!(close(f.aniso[1], 0.25 / 64.0), "{f:?}");
        }
        // Magnified: no probes to spread.
        for f in footprints(256.0, 64.0) {
            assert_eq!(f.aniso, [0.0; 2]);
        }
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
