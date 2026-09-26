//! The two-phase frame: polygon setup and band binning (phase 1), then per-row span
//! resolution and shading (phase 2). See the span buffer module spec.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use moose_assets::{Assets, MeshId, Texture, TextureId};
use moose_scene::Viewport;
use moose_view::{EdgeLine, PolygonKind, ScreenVertex, ViewGeometry, ViewPolygon, pixel_edge};
use rayon::prelude::*;

use crate::shader::{
    ANISO, ANISO_LOD, LANES, LOD, MAX_VARYINGS, POSITION, Shader, ShaderEntry, ShaderId, SpanJob,
    U32s, Uniforms, blend, blend_lanes, layout_len,
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
        }
    }
}

/// What a polygon is drawn with.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub shader: ShaderId,
    pub uniforms: Uniforms,
    /// Forces a raster path for this polygon's mesh, overriding its kind (not world).
    pub path_override: Option<RasterPath>,
    /// The texture the shader samples; untextured polygons get a 1x1 white one.
    pub texture: Option<TextureId>,
}

impl Surface {
    pub fn new(shader: ShaderId) -> Self {
        Self {
            shader,
            uniforms: Uniforms::default(),
            path_override: None,
            texture: None,
        }
    }
}

/// A mesh lacks what a shader needs.
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
                    "mesh '{mesh}' attribute '{name}' has {found} components, the shader needs {needed}"
                )
            }
            Self::TooManyVaryings { mesh } => write!(
                f,
                "shader for mesh '{mesh}' has more than {MAX_VARYINGS} varyings"
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

/// A polygon after phase 1: screen vertices, varyings converted to its shader's layout
/// order, row range and sort key.
struct PolygonSetup {
    first_vertex: u32,
    vertex_count: u16,
    /// Start of this polygon's varyings: `vertex_count * n_vals` values.
    first_value: u32,
    n_vals: u16,
    row_top: i32,
    row_end: i32,
    /// Nearest point: front-to-back sort key.
    max_w: f32,
    shader: ShaderId,
    uniforms: Uniforms,
    texture: Option<TextureId>,
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

/// A polygon's exact values where its edges cross one row.
struct RowState {
    id: u32,
    x_left: f32,
    x_right: f32,
    w_left: f32,
    w_right: f32,
    /// The polygon's pixels on this row.
    row_x0: i32,
    row_x1: i32,
    /// Start of the left values (then the right ones) in the row's value buffer.
    values: u32,
    n_vals: u16,
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
    values: Vec<f32>,
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
    shaders: Vec<ShaderEntry>,
    /// Per (mesh, shader): where each shader varying comes from. Validated once, then reused.
    remaps: HashMap<(MeshId, ShaderId), Vec<Varying>>,
    surfaces: Vec<Surface>,
    bins: Vec<ThreadBins>,
    scratch: Vec<Mutex<RowScratch>>,
    /// What untextured polygons sample: 1x1 opaque white.
    blank: Texture,
}

/// The textures polygons sample, during phase 2.
struct Textures<'a> {
    assets: &'a Assets,
    blank: &'a Texture,
}

impl Textures<'_> {
    fn get(&self, id: Option<TextureId>) -> &Texture {
        id.map_or(self.blank, |id| self.assets.texture(id))
    }
}

impl Renderer {
    pub fn new(config: RasterConfig) -> Self {
        Self {
            config,
            shaders: Vec::new(),
            remaps: HashMap::new(),
            surfaces: Vec::new(),
            bins: Vec::new(),
            scratch: Vec::new(),
            blank: Texture::solid("blank", 0xFFFF_FFFF),
        }
    }

    pub fn register_shader<S: Shader>(&mut self) -> ShaderId {
        self.shaders.push(ShaderEntry::of::<S>());
        ShaderId((self.shaders.len() - 1) as u16)
    }

    /// Draws one view's geometry into `viewport` of `target`. `surface` says what each
    /// polygon is drawn with; every (mesh, shader) pair is validated before drawing.
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
        // Surfaces, and validation of each (mesh, shader) pair once.
        self.surfaces.clear();
        for p in &geometry.polygons {
            let s = surface(p);
            if !self.remaps.contains_key(&(p.mesh, s.shader)) {
                let remap = remap(assets, p.mesh, &self.shaders[s.shader.0 as usize])?;
                self.remaps.insert((p.mesh, s.shader), remap);
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
            (&self.surfaces, &self.remaps, &self.shaders, &self.config);
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
                let remap = &remaps[&(p.mesh, s.shader)];
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
        let (bins, scratch) = (&self.bins, &self.scratch);
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

/// Where a shader varying's vertex values come from.
#[derive(Clone, Copy, Debug)]
enum Varying {
    /// A mesh attribute: offset and count in the polygon's attribute record.
    Attrib(u16, u8),
    /// The vertex's world position.
    Position,
    /// Computed by the shader per sample; this many zeros at vertices.
    Derived(u8),
    /// Part of the texture footprint (see [`LOD`], [`ANISO`], [`ANISO_LOD`]), from the mesh
    /// attribute `uv` at this offset in the attribute record.
    Footprint(u16, FootprintPart),
}

#[derive(Clone, Copy, Debug)]
enum FootprintPart {
    Lod,
    Aniso,
    AnisoLod,
}

/// Maps a shader's layout onto a mesh's attributes by name.
fn remap(assets: &Assets, mesh: MeshId, shader: &ShaderEntry) -> Result<Vec<Varying>, LayoutError> {
    let m = assets.mesh(mesh);
    if layout_len(shader.layout) > MAX_VARYINGS {
        return Err(LayoutError::TooManyVaryings {
            mesh: m.name.clone(),
        });
    }
    shader
        .layout
        .iter()
        .map(|want| {
            let mismatch = |found| LayoutError::CountMismatch {
                mesh: m.name.clone(),
                name: want.name,
                needed: want.count,
                found,
            };
            if shader.derived.contains(&want.name) {
                return Ok(Varying::Derived(want.count));
            }
            if want.name == POSITION {
                return if want.count == 3 {
                    Ok(Varying::Position)
                } else {
                    Err(mismatch(3))
                };
            }
            let part = match want.name {
                LOD => Some((FootprintPart::Lod, 1)),
                ANISO => Some((FootprintPart::Aniso, 2)),
                ANISO_LOD => Some((FootprintPart::AnisoLod, 1)),
                _ => None,
            };
            if let Some((part, count)) = part {
                if want.count != count {
                    return Err(mismatch(count));
                }
                let mut offset = 0;
                for a in &m.attribs {
                    if a.name == "uv" && a.count == 2 {
                        return Ok(Varying::Footprint(offset as u16, part));
                    }
                    offset += a.count as usize;
                }
                return Err(LayoutError::MissingAttribute {
                    mesh: m.name.clone(),
                    name: "uv",
                });
            }
            let mut offset = 0;
            for a in &m.attribs {
                if a.name == want.name {
                    if a.count != want.count {
                        return Err(mismatch(a.count));
                    }
                    return Ok(Varying::Attrib(offset as u16, a.count));
                }
                offset += a.count as usize;
            }
            Err(LayoutError::MissingAttribute {
                mesh: m.name.clone(),
                name: want.name,
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn setup_polygon(
    bins: &mut ThreadBins,
    thread: usize,
    geometry: &ViewGeometry,
    p: &ViewPolygon,
    s: &Surface,
    remap: &[Varying],
    shaders: &[ShaderEntry],
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
    let category = match p.kind {
        // Translucent surfaces are drawn after all opaque geometry, whatever their kind.
        _ if shaders[s.shader.0 as usize].translucent => Category::Translucent,
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
    // Varyings in the shader's order, converted from the mesh's attribute order.
    let n_vals = layout_len(shaders[s.shader.0 as usize].layout);
    let first_value = bins.values.len() as u32;
    let attrs = &geometry.attributes[p.attributes()];
    let positions = &geometry.world_positions[p.vertices()];
    let stride = p.attrib_stride as usize;
    if let Some(uv) = remap.iter().find_map(|v| match v {
        Varying::Footprint(uv, _) => Some(*uv as usize),
        _ => None,
    }) {
        let texture = textures.get(s.texture);
        let size = (texture.width() as f32, texture.height() as f32);
        let uv_at = |v: usize| (attrs[v * stride + uv], attrs[v * stride + uv + 1]);
        vertex_footprints(verts, uv_at, size, &mut bins.footprints);
    }
    for (v, position) in positions.iter().enumerate() {
        for &source in remap {
            match source {
                Varying::Attrib(offset, count) => {
                    let at = v * stride + offset as usize;
                    bins.values
                        .extend_from_slice(&attrs[at..at + count as usize]);
                }
                Varying::Position => bins.values.extend_from_slice(&position.to_array()),
                Varying::Derived(count) => {
                    bins.values.extend(std::iter::repeat_n(0.0, count as usize))
                }
                Varying::Footprint(_, part) => {
                    let f = bins.footprints[v];
                    match part {
                        FootprintPart::Lod => bins.values.push(f.lod),
                        FootprintPart::Aniso => bins.values.extend_from_slice(&f.aniso),
                        FootprintPart::AnisoLod => bins.values.push(f.aniso_lod),
                    }
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
        row_top,
        row_end,
        max_w: verts.iter().map(|v| v.w).fold(0.0, f32::max),
        shader: s.shader,
        uniforms: s.uniforms,
        texture: s.texture,
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
    shaders: &[ShaderEntry],
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

    let rows_in_chunk = chunk.len() / width;
    for r in 0..rows_in_chunk {
        let row = viewport.y as i32 + band as i32 * band_rows + r as i32;
        s.row = row;
        s.states.clear();
        s.values.clear();
        s.spans.clear();

        // World spans: portal clipping guarantees they never overlap. Append, sort by x.
        for i in 0..s.world.len() {
            let id = s.world[i];
            if let Some(span) = row_span(s, bins, id, row) {
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
            if let Some(span) = row_span(s, bins, id, row) {
                insert_resolved(&s.spans, span, &mut s.spans_next);
                std::mem::swap(&mut s.spans, &mut s.spans_next);
            }
        }

        // Per-pixel actors, if any cover this row.
        s.pixel_spans.clear();
        for i in 0..s.pixel.len() {
            let id = s.pixel[i];
            if let Some(span) = row_span(s, bins, id, row) {
                s.pixel_spans.push(span);
            }
        }

        // Translucent polygons covering this row, back to front.
        s.translucent_spans.clear();
        for i in 0..s.translucent.len() {
            let id = s.translucent[i];
            if let Some(span) = row_span(s, bins, id, row) {
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
                for k in 0..s.visible.len() {
                    let (x0, x1, behind) = s.visible[k];
                    if reads_behind {
                        // The opaque w behind, for just these pixels.
                        for px in x0..x1 {
                            s.vis_w[(px - vx) as usize] =
                                behind.map_or(0.0, |o| s.spans[o as usize].w(px));
                        }
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

/// Computes a polygon's exact values where its edges cross `row`, adds them to the row's
/// states, and returns its span. `None` if it covers no pixels on this row.
///
/// On screen every polygon faces the camera, so edges heading down form its left boundary
/// and edges heading up its right one. Each edge covers rows `pixel_edge(top)..
/// pixel_edge(bottom)`; its x comes from its carried line if it has one (so it matches the
/// wall on the other side of a portal exactly), w and the varyings from its own endpoints,
/// with one perspective-correct alpha per edge.
fn row_span(s: &mut RowScratch, bins: &[ThreadBins], id: u32, row: i32) -> Option<Span> {
    let (t, l) = split_id(id);
    let b = &bins[t];
    let p = &b.polygons[l];
    if row < p.row_top || row >= p.row_end {
        return None;
    }
    let n = p.vertex_count as usize;
    let verts = &b.vertices[p.first_vertex as usize..p.first_vertex as usize + n];
    let lines = &b.lines[p.first_vertex as usize..p.first_vertex as usize + n];
    let nv = p.n_vals as usize;
    let values = &b.values[p.first_value as usize..p.first_value as usize + n * nv];

    // The left and right edges crossing this row.
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
    let ((li, x_left), (ri, x_right)) = (left?, right?);
    let (x0, x1) = (pixel_edge(x_left), pixel_edge(x_right));
    if x0 >= x1 {
        return None; // an edge-on sliver on this row
    }

    // w and perspective-correct varyings where each edge crosses the row's center line.
    let at = s.values.len() as u32;
    s.values.resize(s.values.len() + 2 * nv, 0.0);
    let mut cross = |i: usize, out_at: usize| -> f32 {
        let (a, c) = (verts[i], verts[(i + 1) % n]);
        let t = ((row as f32 + 0.5 - a.y) / (c.y - a.y)).clamp(0.0, 1.0);
        let den = (1.0 - t) * a.w + t * c.w;
        let alpha = if den > 0.0 { t * c.w / den } else { t };
        let (va, vc) = (
            &values[i * nv..(i + 1) * nv],
            &values[((i + 1) % n) * nv..((i + 1) % n + 1) * nv],
        );
        for k in 0..nv {
            s.values[out_at + k] = va[k] + (vc[k] - va[k]) * alpha;
        }
        a.w + (c.w - a.w) * t
    };
    let w_left = cross(li, at as usize);
    let w_right = cross(ri, at as usize + nv);

    let state = s.states.len() as u32;
    s.states.push(RowState {
        id,
        x_left,
        x_right,
        w_left,
        w_right,
        row_x0: x0,
        row_x1: x1,
        values: at,
        n_vals: nv as u16,
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

/// Shades pixels `x0..x1` of a polygon's row with its shader.
#[allow(clippy::too_many_arguments)]
fn shade_run(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    shaders: &[ShaderEntry],
    config: &RasterConfig,
    textures: &Textures,
    state: u32,
    x0: i32,
    x1: i32,
    vx: i32,
) {
    let st = &s.states[state as usize];
    let (t, l) = split_id(st.id);
    let p = &bins[t].polygons[l];
    let (v, nv) = (st.values as usize, st.n_vals as usize);
    let job = SpanJob {
        x_left: st.x_left,
        x_right: st.x_right,
        w_left: st.w_left,
        w_right: st.w_right,
        left: &s.values[v..v + nv],
        right: &s.values[v + nv..v + 2 * nv],
        row_x0: st.row_x0,
        row_x1: st.row_x1,
        x0,
        row: s.row,
        half_rate: p.half_rate,
        step_threshold: config.step_threshold,
        min_step: config.min_step.clamp(1, crate::shader::MAX_STEP as u32) as i32,
    };
    let out = &mut s.color[(x0 - vx) as usize..(x1 - vx) as usize];
    (draw_fn(&shaders[p.shader.0 as usize], p.half_rate))(
        &job,
        out,
        &[],
        &p.uniforms,
        textures.get(p.texture),
    );
}

/// Shades pixels `x0..x1` of a translucent polygon's row and blends them over the row.
#[allow(clippy::too_many_arguments)]
fn blend_run(
    s: &mut RowScratch,
    bins: &[ThreadBins],
    shaders: &[ShaderEntry],
    config: &RasterConfig,
    textures: &Textures,
    state: u32,
    x0: i32,
    x1: i32,
    vx: i32,
) {
    let st = &s.states[state as usize];
    let (t, l) = split_id(st.id);
    let p = &bins[t].polygons[l];
    let (v, nv) = (st.values as usize, st.n_vals as usize);
    let job = SpanJob {
        x_left: st.x_left,
        x_right: st.x_right,
        w_left: st.w_left,
        w_right: st.w_right,
        left: &s.values[v..v + nv],
        right: &s.values[v + nv..v + 2 * nv],
        row_x0: st.row_x0,
        row_x1: st.row_x1,
        x0,
        row: s.row,
        half_rate: p.half_rate,
        step_threshold: config.step_threshold,
        min_step: config.min_step.clamp(1, crate::shader::MAX_STEP as u32) as i32,
    };
    let len = (x1 - x0) as usize;
    s.blend.resize(len.max(s.blend.len()), 0);
    let behind = &s.vis_w[(x0 - vx) as usize..(x1 - vx) as usize];
    (draw_fn(&shaders[p.shader.0 as usize], p.half_rate))(
        &job,
        &mut s.blend[..len],
        behind,
        &p.uniforms,
        textures.get(p.texture),
    );
    let dst = &mut s.color[(x0 - vx) as usize..(x1 - vx) as usize];
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
}

/// The span function for a polygon's shading rate.
fn draw_fn(shader: &ShaderEntry, half_rate: bool) -> crate::shader::DrawSpanFn {
    if half_rate {
        shader.draw_span_half
    } else {
        shader.draw_span
    }
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
    shaders: &'a [ShaderEntry],
    state: u32,
) -> &'a ShaderEntry {
    let (t, l) = split_id(s.states[state as usize].id);
    &shaders[bins[t].polygons[l].shader.0 as usize]
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
