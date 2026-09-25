//! Shader definition, dispatch, and perspective-correct shading of spans.
//!
//! A shader's varyings are three homogeneous groups, one per interpolation format: 16.16
//! fixed point (`Fixed32`), 8.8 fixed point (`Fixed16`) and `f32` (`Floats`). The
//! [`varyings!`](crate::varyings) macro generates the group structs and the layout
//! descriptor from one attribute list. A registry of type-erased [`ShaderEntry`]s is
//! dispatched once per shading run; inside, the loop is monomorphized per shader.

/// Interpolation format of a varying: how it is stepped across a span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// 16.16 fixed point in an `i32`, stepped with wrapping adds (UV tiling relies on it).
    Fixed32,
    /// 8.8 fixed point in an `i16`, stepped with wrapping adds. Values are bit patterns: a
    /// color channel of 255 is `0xFF00`; read it back with `(bits as u16) >> 8`.
    Fixed16,
    /// Plain `f32`.
    Float,
}

/// One varying a shader reads: matched to a mesh attribute by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttribDesc {
    pub name: &'static str,
    pub format: Format,
    pub count: u8,
}

/// Per-draw constants, copied into the frame at draw time. One shared struct for now
/// (the spec leaves per-shader uniform types open).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Uniforms {
    pub values: [f32; 8],
}

/// Element types of the three groups.
pub trait Elem: Copy + Default {
    /// Converts a true (f32) value to this interpolation format.
    fn from_f32(v: f32) -> Self;
    fn step(self, s: Self) -> Self;
}

impl Elem for i32 {
    #[inline(always)]
    fn from_f32(v: f32) -> Self {
        (v * 65536.0).round() as i64 as i32 // wraps, for tiling
    }
    #[inline(always)]
    fn step(self, s: Self) -> Self {
        self.wrapping_add(s)
    }
}

impl Elem for i16 {
    #[inline(always)]
    fn from_f32(v: f32) -> Self {
        (v * 256.0).round() as i32 as i16 // wraps; read back as u16 for unsigned values
    }
    #[inline(always)]
    fn step(self, s: Self) -> Self {
        self.wrapping_add(s)
    }
}

impl Elem for f32 {
    #[inline(always)]
    fn from_f32(v: f32) -> Self {
        v
    }
    #[inline(always)]
    fn step(self, s: Self) -> Self {
        self + s
    }
}

/// A group of varyings sharing one interpolation format.
pub trait Group: Copy + Default {
    /// Number of values in the group.
    const LEN: usize;
    /// Advances every value by the matching value of `s`.
    fn step(&mut self, s: &Self);
    /// Builds the group from true values, in layout order.
    fn from_values(v: &[f32]) -> Self;
    /// The per-pixel step taking `a` to `b` in `n` pixels.
    fn step_between(a: &[f32], b: &[f32], inv_n: f32) -> Self;
}

/// Declares one group struct. Used by [`varyings!`](crate::varyings).
#[macro_export]
macro_rules! varying_group {
    ($group:ident, $elem:ty; $($field:ident: $n:literal),*) => {
        #[repr(C)]
        #[derive(Clone, Copy, Default, Debug)]
        pub struct $group { $(pub $field: [$elem; $n]),* }

        impl $crate::shader::Group for $group {
            const LEN: usize = 0 $(+ $n)*;

            #[inline(always)]
            #[allow(unused_variables)]
            fn step(&mut self, s: &Self) {
                $(for i in 0..$n {
                    self.$field[i] = $crate::shader::Elem::step(self.$field[i], s.$field[i]);
                })*
            }

            #[inline(always)]
            #[allow(unused_variables, unused_mut, unused_assignments)]
            fn from_values(v: &[f32]) -> Self {
                let mut g = Self::default();
                let mut k = 0;
                $(for i in 0..$n {
                    g.$field[i] = <$elem as $crate::shader::Elem>::from_f32(v[k]);
                    k += 1;
                })*
                g
            }

            #[inline(always)]
            #[allow(unused_variables, unused_mut, unused_assignments)]
            fn step_between(a: &[f32], b: &[f32], inv_n: f32) -> Self {
                let mut g = Self::default();
                let mut k = 0;
                $(for i in 0..$n {
                    g.$field[i] = <$elem as $crate::shader::Elem>::from_f32((b[k] - a[k]) * inv_n);
                    k += 1;
                })*
                g
            }
        }
    };
}

/// Declares a shader's varyings: the three group structs `Fixed32`, `Fixed16` and `Floats`,
/// and `LAYOUT`, all from one list. Varyings are laid out 16.16 first, then 8.8, then f32.
///
/// ```ignore
/// varyings! {
///     fixed32 { uv: 2 }
///     fixed16 { color: 3 }
///     float { normal: 3 }
/// }
/// ```
#[macro_export]
macro_rules! varyings {
    (
        fixed32 { $($a:ident: $an:literal),* $(,)? }
        fixed16 { $($b:ident: $bn:literal),* $(,)? }
        float { $($c:ident: $cn:literal),* $(,)? }
    ) => {
        $crate::varying_group!(Fixed32, i32; $($a: $an),*);
        $crate::varying_group!(Fixed16, i16; $($b: $bn),*);
        $crate::varying_group!(Floats, f32; $($c: $cn),*);

        pub const LAYOUT: &[$crate::shader::AttribDesc] = &[
            $($crate::shader::AttribDesc { name: stringify!($a), format: $crate::shader::Format::Fixed32, count: $an },)*
            $($crate::shader::AttribDesc { name: stringify!($b), format: $crate::shader::Format::Fixed16, count: $bn },)*
            $($crate::shader::AttribDesc { name: stringify!($c), format: $crate::shader::Format::Float, count: $cn },)*
        ];
    };
}

/// Name of the built-in varying holding the world position a pixel shows (three values),
/// from `ViewGeometry::world_positions`. A mesh attribute cannot supply it.
pub const POSITION: &str = "position";

/// A shader: its varyings and a per-pixel function producing a 32-bit color.
///
/// Varyings come from the mesh's attributes by name, except [`POSITION`] and the names in
/// `DERIVED`: values the shader computes itself, in `per_sample`, from the others.
///
/// Along a span, exact (perspective-correct) values are taken at sample points a few pixels
/// apart and stepped linearly between them. `per_sample` runs on the exact values at each
/// sample point, before stepping: the place for work too costly per pixel but not linear
/// on screen (normalizing a view vector, say), whose result is then stepped like any
/// varying.
///
/// Opaque shaders return XRGB (the top byte is ignored). Translucent shaders
/// (`TRANSLUCENT = true`) return ARGB: the renderer draws their polygons after all opaque
/// geometry, back to front, and blends each pixel as `src * a + dst * (1 - a)`. A
/// translucent shader with `READS_BEHIND = true` is shaded with `shade_over` instead of
/// `shade`, which also gets the depth of the opaque surface behind the pixel.
pub trait Shader: 'static {
    type Fixed32: Group;
    type Fixed16: Group;
    type Floats: Group;
    const LAYOUT: &'static [AttribDesc];
    /// Varyings written by `per_sample` rather than read from the mesh (0 at vertices).
    const DERIVED: &'static [&'static str] = &[];
    const TRANSLUCENT: bool = false;
    /// Shade with `shade_over`. Translucent shaders only.
    const READS_BEHIND: bool = false;

    /// Runs on the exact varying values at each sample point, in layout order.
    #[inline(always)]
    fn per_sample(_values: &mut [f32], _uni: &Uniforms) {}

    fn shade(a: &Self::Fixed32, b: &Self::Fixed16, c: &Self::Floats, uni: &Uniforms) -> u32;

    /// Shades a translucent pixel knowing what it is drawn over: `w` is this surface's
    /// `1 / depth` at the pixel, `behind_w` that of the opaque surface behind it (both on
    /// the same ray through the pixel, so Euclidean distances along it scale as `1 / w`).
    #[inline(always)]
    fn shade_over(
        a: &Self::Fixed32,
        b: &Self::Fixed16,
        c: &Self::Floats,
        uni: &Uniforms,
        _w: f32,
        _behind_w: f32,
    ) -> u32 {
        Self::shade(a, b, c, uni)
    }
}

/// Most varyings (as f32 values) any shader may declare.
pub const MAX_VARYINGS: usize = 32;

/// Everything needed to shade one run of pixels of one polygon on one row.
pub struct SpanJob<'a> {
    /// The polygon's exact left and right edge crossings on this row, and w there.
    pub x_left: f32,
    pub x_right: f32,
    pub w_left: f32,
    pub w_right: f32,
    /// True varying values at the two crossings, in the shader's layout order.
    pub left: &'a [f32],
    pub right: &'a [f32],
    /// The polygon's pixels on this row, `row_x0..row_x1`. Sample points sit on a grid
    /// over this whole range, so a pixel's value never depends on how the row was split.
    pub row_x0: i32,
    pub row_x1: i32,
    /// First pixel of the run; the run covers `x0..x0 + color.len()`, inside the row.
    pub x0: i32,
    /// Relative w change per sample interval allowed before sampling more often.
    pub step_threshold: f32,
    /// Smallest sample interval allowed, in pixels.
    pub min_step: i32,
}

/// Shades a run into `color`. `behind` is the opaque `w` under each pixel of the run for
/// translucent shaders, and empty for opaque ones.
pub type DrawSpanFn = fn(job: &SpanJob, color: &mut [u32], behind: &[f32], uni: &Uniforms);

/// A registered shader, type-erased.
#[derive(Clone, Copy)]
pub struct ShaderEntry {
    pub layout: &'static [AttribDesc],
    pub derived: &'static [&'static str],
    pub draw_span: DrawSpanFn,
    pub translucent: bool,
}

impl ShaderEntry {
    pub fn of<S: Shader>() -> Self {
        const {
            assert!(
                S::TRANSLUCENT || !S::READS_BEHIND,
                "only translucent shaders can read what is behind them"
            )
        };
        Self {
            layout: S::LAYOUT,
            derived: S::DERIVED,
            draw_span: draw_span::<S>,
            translucent: S::TRANSLUCENT,
        }
    }
}

/// Blends an ARGB color over an existing one by its alpha: `src * a + dst * (1 - a)` per
/// channel, rounded. The result's top byte is zero.
#[inline(always)]
pub fn blend(src: u32, dst: u32) -> u32 {
    let a = src >> 24;
    let mut out = 0;
    for shift in [0, 8, 16] {
        let (s, d) = ((src >> shift) & 255, (dst >> shift) & 255);
        out |= ((s * a + d * (255 - a) + 127) / 255) << shift;
    }
    out
}

/// Handle to a registered shader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShaderId(pub u16);

/// Number of f32 values in a layout.
pub fn layout_len(layout: &[AttribDesc]) -> usize {
    layout.iter().map(|a| a.count as usize).sum()
}

/// Largest sample interval: 8.8 values reset at least every 32 pixels, keeping their
/// accumulated stepping error under one level.
pub const MAX_STEP: i32 = 32;

/// Picks the sample interval for a row span: the largest power of two from `min_step` up
/// to 32 over which w changes by less than `threshold` relative to its smaller end. Rows
/// of constant w (a floor seen without roll) get the maximum.
pub fn choose_step(w_left: f32, w_right: f32, span_len: f32, threshold: f32, min_step: i32) -> i32 {
    let w_ratio = (w_right - w_left).abs() / (w_left.min(w_right) * span_len.max(1.0));
    let mut n = MAX_STEP;
    while n / 2 >= min_step && w_ratio * n as f32 >= threshold {
        n /= 2;
    }
    n
}

/// True varying values at pixel `px`'s center on the row, perspective-correct: one alpha
/// from w at the two crossings, shared by every varying.
#[inline(always)]
fn values_at(job: &SpanJob, px: i32, out: &mut [f32]) {
    let span = job.x_right - job.x_left;
    let s = if span > 0.0 {
        ((px as f32 + 0.5 - job.x_left) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let den = (1.0 - s) * job.w_left + s * job.w_right;
    let alpha = if den > 0.0 { s * job.w_right / den } else { s };
    for (o, (&l, &r)) in out.iter_mut().zip(job.left.iter().zip(job.right)) {
        *o = l + (r - l) * alpha;
    }
}

/// Shades a run of a polygon's row. Exact values are taken at sample points every N pixels
/// from the row's first pixel, and at its last pixel; between them each group steps
/// linearly in its own format, resetting to exact at every sample point, so stepping
/// error never accumulates past one interval. The grid belongs to the whole row: a run
/// starting mid-interval steps from the interval's start, exactly as a run covering the
/// whole row would, so every pixel gets the same value however the row is split into runs.
pub fn draw_span<S: Shader>(job: &SpanJob, color: &mut [u32], behind: &[f32], uni: &Uniforms) {
    let (x0, x1) = (job.x0, job.x0 + color.len() as i32);
    if x0 >= x1 {
        return;
    }
    debug_assert!(job.row_x0 <= x0 && x1 <= job.row_x1, "run outside its row");
    let n_vals = layout_len(S::LAYOUT);
    let (f32s, f16s) = (S::Fixed32::LEN, S::Fixed16::LEN);
    let n = choose_step(
        job.w_left,
        job.w_right,
        job.x_right - job.x_left,
        job.step_threshold,
        job.min_step,
    );
    let last = job.row_x1 - 1;
    let mut a = [0.0f32; MAX_VARYINGS];
    let mut b = [0.0f32; MAX_VARYINGS];
    let mut a_at = i32::MIN; // the pixel `a` currently holds values for
    let mut p = x0;
    while p < x1 {
        if p == last {
            values_at(job, last, &mut a[..n_vals]);
            S::per_sample(&mut a[..n_vals], uni);
            let (mut ga, mut gb, mut gc) = groups::<S>(&a[..n_vals], f32s, f16s);
            let i = (p - x0) as usize;
            draw_segment::<S>(
                &mut ga,
                &Default::default(),
                &mut gb,
                &Default::default(),
                &mut gc,
                &Default::default(),
                &mut color[i..i + 1],
                behind.get(i..i + 1).unwrap_or(&[]),
                job,
                p,
                uni,
            );
            break;
        }
        // The grid interval holding p: exact at `start` and `end`.
        let start = job.row_x0 + (p - job.row_x0) / n * n;
        let end = (start + n).min(last);
        if a_at != start {
            values_at(job, start, &mut a[..n_vals]);
            S::per_sample(&mut a[..n_vals], uni);
        }
        values_at(job, end, &mut b[..n_vals]);
        S::per_sample(&mut b[..n_vals], uni);
        let inv = 1.0 / (end - start) as f32;
        let (mut ga, mut gb, mut gc) = groups::<S>(&a[..n_vals], f32s, f16s);
        let da = S::Fixed32::step_between(&a[..f32s], &b[..f32s], inv);
        let db = S::Fixed16::step_between(&a[f32s..f32s + f16s], &b[f32s..f32s + f16s], inv);
        let dc = S::Floats::step_between(&a[f32s + f16s..n_vals], &b[f32s + f16s..n_vals], inv);
        // Step to p exactly as a run starting at `start` would have.
        for _ in start..p {
            ga.step(&da);
            gb.step(&db);
            gc.step(&dc);
        }
        let stop = end.min(x1);
        let run = (p - x0) as usize..(stop - x0) as usize;
        draw_segment::<S>(
            &mut ga,
            &da,
            &mut gb,
            &db,
            &mut gc,
            &dc,
            &mut color[run.clone()],
            behind.get(run).unwrap_or(&[]),
            job,
            p,
            uni,
        );
        a = b;
        a_at = end;
        p = stop;
    }
}

#[inline(always)]
fn groups<S: Shader>(v: &[f32], f32s: usize, f16s: usize) -> (S::Fixed32, S::Fixed16, S::Floats) {
    (
        S::Fixed32::from_values(&v[..f32s]),
        S::Fixed16::from_values(&v[f32s..f32s + f16s]),
        S::Floats::from_values(&v[f32s + f16s..]),
    )
}

/// The inner loop, monomorphized per shader so `shade` inlines. `out` starts at pixel
/// `x`; `behind` is parallel to it for shaders that read it.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
pub fn draw_segment<S: Shader>(
    a: &mut S::Fixed32,
    da: &S::Fixed32,
    b: &mut S::Fixed16,
    db: &S::Fixed16,
    c: &mut S::Floats,
    dc: &S::Floats,
    out: &mut [u32],
    behind: &[f32],
    job: &SpanJob,
    x: i32,
    uni: &Uniforms,
) {
    if S::READS_BEHIND {
        debug_assert_eq!(behind.len(), out.len(), "no depths behind the run");
        // w is linear in screen x.
        let span = job.x_right - job.x_left;
        let dw = if span > 0.0 {
            (job.w_right - job.w_left) / span
        } else {
            0.0
        };
        let mut w = job.w_left + dw * (x as f32 + 0.5 - job.x_left);
        for (px, &behind_w) in out.iter_mut().zip(behind) {
            *px = S::shade_over(a, b, c, uni, w, behind_w);
            a.step(da);
            b.step(db);
            c.step(dc);
            w += dw;
        }
    } else {
        for px in out.iter_mut() {
            *px = S::shade(a, b, c, uni);
            a.step(da);
            b.step(db);
            c.step(dc);
        }
    }
}
