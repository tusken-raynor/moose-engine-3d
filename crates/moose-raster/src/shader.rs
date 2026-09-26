//! Shader definition, dispatch, and perspective-correct shading of spans.
//!
//! A shader's varyings are three homogeneous groups, one per interpolation format: 16.16
//! fixed point (`Fixed32`), 8.8 fixed point (`Fixed16`) and `f32` (`Floats`). The
//! [`varyings!`](crate::varyings) macro generates the group structs and the layout
//! descriptor from one attribute list. A registry of type-erased [`ShaderEntry`]s is
//! dispatched once per shading run; inside, the loop is monomorphized per shader.
//!
//! Pixels are shaded [`LANES`] at a time with portable SIMD (the `wide` crate: NEON on ARM,
//! SSE/AVX on x86): every varying of a group holds one value per lane, and `shade` computes
//! [`LANES`] pixels at once.

use std::ops::Range;

use moose_assets::{MipLevel, Texture};

use wide::{f32x4, f32x8, i16x8, i32x4, i32x8, u16x8, u32x4, u32x8};

/// Pixels shaded at once.
pub const LANES: usize = 8;
/// One 8.8 varying for [`LANES`] pixels.
pub type I16s = i16x8;
/// One 16.16 varying for [`LANES`] pixels.
pub type I32s = i32x8;
/// One float varying for [`LANES`] pixels.
pub type F32s = f32x8;
/// One color (or other 32-bit result) for [`LANES`] pixels.
pub type U32s = u32x8;

/// Where a block of pixels is on screen: lane `i` is framebuffer pixel
/// `(x + i * stride, y)`. The stride is 2 for half-rate shading (each lane's color also
/// covers the pixel after it), otherwise 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pixels {
    pub x: i32,
    pub y: i32,
    pub stride: i32,
}

impl Pixels {
    /// Each lane's framebuffer x.
    #[inline(always)]
    pub fn x_lanes(self) -> I32s {
        i32::lanes(I32s::fill(self.x), I32s::fill(self.stride), 0, 1)
    }
}

/// All lanes set to one value. Use this rather than `wide`'s `splat`, whose `[v; N]` array
/// LLVM turns into a `memset_pattern16` library call on Apple targets when `v` is a
/// constant, keeping the vector out of registers. This builds 4-lane halves from listed
/// elements, which become one `dup` instruction each.
pub trait Fill: Sized {
    type Elem;
    fn fill(v: Self::Elem) -> Self;
}

impl Fill for F32s {
    type Elem = f32;
    #[inline(always)]
    fn fill(v: f32) -> Self {
        let half = f32x4::new([v, v, v, v]);
        // SAFETY: two 4-lane halves have the size and lane order of the 8-lane type.
        unsafe { core::mem::transmute::<[f32x4; 2], F32s>([half, half]) }
    }
}

impl Fill for I32s {
    type Elem = i32;
    #[inline(always)]
    fn fill(v: i32) -> Self {
        let half = i32x4::new([v, v, v, v]);
        // SAFETY: two 4-lane halves have the size and lane order of the 8-lane type.
        unsafe { core::mem::transmute::<[i32x4; 2], I32s>([half, half]) }
    }
}

impl Fill for U32s {
    type Elem = u32;
    #[inline(always)]
    fn fill(v: u32) -> Self {
        let half = u32x4::new([v, v, v, v]);
        // SAFETY: two 4-lane halves have the size and lane order of the 8-lane type.
        unsafe { core::mem::transmute::<[u32x4; 2], U32s>([half, half]) }
    }
}

impl Fill for I16s {
    type Elem = i16;
    #[inline(always)]
    fn fill(v: i16) -> Self {
        // Four 32-bit lanes each holding `v` twice (see the trait's note).
        let pair = (v as u16 as u32) * 0x0001_0001;
        wide::bytemuck::cast(u32x4::new([pair, pair, pair, pair]))
    }
}

/// Each lane's 16 bits as an unsigned integer.
#[inline(always)]
pub fn widen(v: I16s) -> U32s {
    let bits: u16x8 = wide::bytemuck::cast(v);
    U32s::from(bits)
}

/// The nearest texel of one mip level for each lane, wrapping (the texture tiles): `u` and
/// `v` are 16.16 fixed point with 1.0 across the whole texture, `v` down from its top row.
/// The level must be at most 65536 texels on a side.
#[inline(always)]
pub fn sample(tex: &MipLevel, u: I32s, v: I32s) -> U32s {
    let x = (u >> (16 - tex.width_log2 as i32)) & I32s::fill(tex.width as i32 - 1);
    let y = (v >> (16 - tex.height_log2 as i32)) & I32s::fill(tex.height as i32 - 1);
    gather(tex, (y << tex.width_log2 as i32) | x)
}

/// Texels at `indices` (one per lane): a gather, one load per lane.
#[inline(always)]
fn gather(tex: &MipLevel, indices: I32s) -> U32s {
    let index = indices.to_array();
    let mask = tex.texels.len() - 1;
    U32s::from(std::array::from_fn::<u32, LANES, _>(|i| {
        tex.texels[index[i] as usize & mask]
    }))
}

/// The bilinearly filtered color of one mip level for each lane, wrapping (the texture
/// tiles): the four texels around the sample point, weighted by how near their centers are,
/// in all four channels (so a roughness alpha is filtered too). Coordinates as for
/// [`sample`]; the weights have 8-bit precision.
#[inline(always)]
pub fn sample_bilinear(tex: &MipLevel, u: I32s, v: I32s) -> U32s {
    // Texel coordinates with an 8-bit fraction, half a texel back so that texel centers
    // land on whole numbers.
    let to_texels = |c: I32s, log2: u32| {
        let shift = 8 - log2 as i32;
        let c = if shift >= 0 { c >> shift } else { c << -shift };
        c - I32s::fill(128)
    };
    let (u, v) = (to_texels(u, tex.width_log2), to_texels(v, tex.height_log2));
    let (x_mask, y_mask) = (
        I32s::fill(tex.width as i32 - 1),
        I32s::fill(tex.height as i32 - 1),
    );
    let (x, y): (I32s, I32s) = (u >> 8, v >> 8);
    let (x0, x1) = (x & x_mask, (x + I32s::fill(1)) & x_mask);
    let row = tex.width_log2 as i32;
    let (y0, y1) = ((y & y_mask) << row, ((y + I32s::fill(1)) & y_mask) << row);
    let byte = I32s::fill(255);
    let (fx, fy): (U32s, U32s) = (
        wide::bytemuck::cast(u & byte),
        wide::bytemuck::cast(v & byte),
    );
    let top = lerp_texels(gather(tex, y0 | x0), gather(tex, y0 | x1), fx);
    let bottom = lerp_texels(gather(tex, y1 | x0), gather(tex, y1 | x1), fx);
    lerp_texels(top, bottom, fy)
}

/// The trilinearly filtered color for each lane: bilinear on the two mip levels around
/// `lod`, blended by its fraction. `lod` is 8.8 fixed point, log2 of the texels per pixel
/// (see [`LOD`]): its integer part picks the level, clamped to the texture's levels (below
/// 0 is the full-size level), and its 8-bit fraction weights the next level down.
/// Coordinates as for [`sample`].
///
/// A block's lanes nearly always share a level; where they straddle a level boundary, each
/// level they touch is sampled for all lanes, and each lane keeps its own.
#[inline(always)]
pub fn sample_trilinear(tex: &Texture, u: I32s, v: I32s, lod: I16s) -> U32s {
    let last = (tex.levels.len() - 1) as i32;
    let lod = I32s::from_i16x8(lod)
        .max(I32s::fill(0))
        .min(I32s::fill(last << 8));
    let level: I32s = lod >> 8;
    let fraction: U32s = wide::bytemuck::cast(lod & I32s::fill(255));
    let levels = level.to_array();
    let (lo, hi) = (*levels.iter().min().unwrap(), *levels.iter().max().unwrap());
    let mut out = U32s::fill(0);
    // Magnified (or exactly on a level) across the block: the next level has no weight.
    let whole = fraction == U32s::fill(0);
    for l in lo..=hi {
        let near = sample_bilinear(&tex.levels[l as usize], u, v);
        let blended = if l < last && !whole {
            let far = sample_bilinear(&tex.levels[l as usize + 1], u, v);
            lerp_texels(near, far, fraction)
        } else {
            near // the smallest level: nothing further to blend
        };
        if lo == hi {
            return blended;
        }
        let this: U32s = wide::bytemuck::cast(level.simd_eq(I32s::fill(l)));
        out = (blended & this) | (out & !this);
    }
    out
}

/// 2x anisotropic filtering: two trilinear probes at `uv ± offset` (along the long axis of the
/// pixel's footprint; see [`ANISO`]) with level of detail `probe_lod` (see [`ANISO_LOD`]),
/// averaged. Where no lane of the block needs it (offset 0: a round or magnified footprint),
/// one trilinear probe at `lod`, which is then the same level.
#[inline(always)]
pub fn sample_anisotropic(
    tex: &Texture,
    u: I32s,
    v: I32s,
    offset: [I32s; 2],
    lod: I16s,
    probe_lod: I16s,
) -> U32s {
    if (offset[0] | offset[1]) == I32s::fill(0) {
        return sample_trilinear(tex, u, v, lod);
    }
    let a = sample_trilinear(tex, u + offset[0], v + offset[1], probe_lod);
    let b = sample_trilinear(tex, u - offset[0], v - offset[1], probe_lod);
    lerp_texels(a, b, U32s::fill(128))
}

/// Where each lane's direction `d` lands on a cube map (see [`CUBE_FACES`](moose_assets::CUBE_FACES)): its face, and
/// its coordinates on the face, -1 to 1 from left to right and top to bottom. The face is
/// that of the largest component (by masks, no branches); one reciprocal of it scales the
/// other two.
#[inline(always)]
pub fn cube_face_coords(d: [F32s; 3]) -> (I32s, F32s, F32s) {
    let [x, y, z] = d;
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    let x_major = ax.simd_ge(ay) & ax.simd_ge(az);
    let y_major = !x_major & ay.simd_ge(az);
    let m = x_major.select(x, y_major.select(y, z));
    let r = F32s::fill(1.0) / m;
    let ra = r.abs();
    // (right, -up) over the major component, per CUBE_FACES: on x faces (z, -y) / x
    // (with the sign of x folded into what flips), on y faces (x, -z) / y, on z faces
    // (-x, -y) / z.
    let u = x_major.select(z * r, y_major.select(x * ra, -(x * r)));
    let v = y_major.select(-(z * r), -(y * ra));
    let bits = |m: F32s| wide::bytemuck::cast::<F32s, I32s>(m);
    // x faces are 0 and 1, y faces 2 and 3, z faces 4 and 5; odd faces look down -axis.
    let axis = (bits(y_major) & I32s::fill(2)) | (!bits(x_major | y_major) & I32s::fill(4));
    let negative = bits(m.simd_lt(F32s::fill(0.0))) & I32s::fill(1);
    (axis | negative, u, v)
}

/// Bilinear on one level of a cube map ([`Texture::cube`]) at face coordinates from
/// [`cube_face_coords`], clamped to the face (it does not blend across into the next).
#[inline(always)]
pub fn sample_cube_level(level: &MipLevel, face: I32s, u: F32s, v: F32s) -> U32s {
    let size = level.width as i32;
    // Texel coordinates with an 8-bit fraction, texel centers on whole numbers:
    // ((c + 1) / 2 * size - 0.5) * 256.
    let (scale, offset) = (size as f32 * 128.0, size as f32 * 128.0 - 128.0);
    let last = I32s::fill(size - 1);
    let to_texels = |c: F32s| {
        (c * F32s::fill(scale) + F32s::fill(offset))
            .round_int()
            .max(I32s::fill(0))
            .min(I32s::fill((size - 1) << 8))
    };
    let (u, v) = (to_texels(u), to_texels(v));
    let (x0, y0): (I32s, I32s) = (u >> 8, v >> 8);
    let (x1, y1) = (
        (x0 + I32s::fill(1)).min(last),
        (y0 + I32s::fill(1)).min(last),
    );
    let row = level.width_log2 as i32;
    let base = face << (2 * row);
    let (y0, y1) = (base + (y0 << row), base + (y1 << row));
    let at = |i: I32s| {
        let i = i.to_array();
        U32s::from(std::array::from_fn::<u32, LANES, _>(|k| {
            level.texels[i[k] as usize]
        }))
    };
    let byte = I32s::fill(255);
    let (fx, fy): (U32s, U32s) = (
        wide::bytemuck::cast(u & byte),
        wide::bytemuck::cast(v & byte),
    );
    let top = lerp_texels(at(y0 + x0), at(y0 + x1), fx);
    let bottom = lerp_texels(at(y1 + x0), at(y1 + x1), fx);
    lerp_texels(top, bottom, fy)
}

/// A cube map's ([`Texture::cube`]) color in each lane's direction `d` (any length): bilinear
/// on the two levels around `lod` (log2 of face texels per pixel, the same for every lane),
/// blended by its fraction.
#[inline(always)]
pub fn sample_cube(tex: &Texture, d: [F32s; 3], lod: f32) -> U32s {
    let (face, u, v) = cube_face_coords(d);
    let last = tex.levels.len() - 1;
    let lod = lod.clamp(0.0, last as f32);
    let level = lod as usize;
    let fraction = ((lod - level as f32) * 256.0) as u32;
    let near = sample_cube_level(&tex.levels[level], face, u, v);
    if fraction == 0 || level == last {
        return near;
    }
    let far = sample_cube_level(&tex.levels[level + 1], face, u, v);
    lerp_texels(near, far, U32s::fill(fraction))
}

/// `a` to `b` by `f` / 256 in every 8-bit channel of every lane: two channels at a time,
/// each in a 16-bit slot of the lane (red and blue, then alpha and green), so one multiply
/// scales both and no slot overflows into the next (255 * 256 fits in 16 bits).
#[inline(always)]
fn lerp_texels(a: U32s, b: U32s, f: U32s) -> U32s {
    let slots = U32s::fill(0x00FF_00FF);
    let g = U32s::fill(256) - f;
    let rb = (((a & slots) * g + (b & slots) * f) >> 8) & slots;
    // Alpha and green, shifted down into the slots; the weighted sum is 256 times too big,
    // which is exactly the shift back up.
    let ag = (((a >> 8) & slots) * g + ((b >> 8) & slots) * f) & U32s::fill(0xFF00_FF00);
    rb | ag
}

/// The high byte of each 8.8 value, as an unsigned 0-255 integer: a color channel's value
/// (`(bits as u16) >> 8` per lane).
#[inline(always)]
pub fn high_byte(v: I16s) -> U32s {
    let bits: u16x8 = wide::bytemuck::cast(v);
    U32s::from(bits >> 8)
}

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

/// Element types of the three groups, and their [`LANES`]-wide vectors.
pub trait Elem: Copy + Default {
    type Lanes: Copy + Default + std::fmt::Debug;
    /// Converts a true (f32) value to this interpolation format.
    fn from_f32(v: f32) -> Self;
    /// `v` in every lane.
    fn splat(v: Self) -> Self::Lanes;
    /// Lane `j` holds `base + (offset + j * stride) * step` (`base` and `step` hold one value
    /// in every lane): the value that many pixels along a line from `base`. In fixed point
    /// the arithmetic wraps, exactly as stepping that many times would.
    fn lanes(base: Self::Lanes, step: Self::Lanes, offset: i32, stride: i32) -> Self::Lanes;
}

impl Elem for i32 {
    type Lanes = I32s;
    #[inline(always)]
    fn from_f32(v: f32) -> Self {
        (v * 65536.0).round() as i64 as i32 // wraps, for tiling
    }
    #[inline(always)]
    fn splat(v: Self) -> I32s {
        I32s::fill(v)
    }
    #[inline(always)]
    fn lanes(base: I32s, step: I32s, offset: i32, stride: i32) -> I32s {
        let iota = I32s::from([0, 1, 2, 3, 4, 5, 6, 7]);
        let j = if stride == 1 {
            iota
        } else {
            iota * I32s::fill(stride)
        } + I32s::fill(offset);
        base + j * step
    }
}

impl Elem for i16 {
    type Lanes = I16s;
    #[inline(always)]
    fn from_f32(v: f32) -> Self {
        (v * 256.0).round() as i32 as i16 // wraps; read back as u16 for unsigned values
    }
    #[inline(always)]
    fn splat(v: Self) -> I16s {
        I16s::fill(v)
    }
    #[inline(always)]
    fn lanes(base: I16s, step: I16s, offset: i32, stride: i32) -> I16s {
        let iota = I16s::from([0, 1, 2, 3, 4, 5, 6, 7]);
        let j = if stride == 1 {
            iota
        } else {
            iota * I16s::fill(stride as i16)
        } + I16s::fill(offset as i16);
        base + j * step
    }
}

impl Elem for f32 {
    type Lanes = F32s;
    #[inline(always)]
    fn from_f32(v: f32) -> Self {
        v
    }
    #[inline(always)]
    fn splat(v: Self) -> F32s {
        F32s::fill(v)
    }
    #[inline(always)]
    fn lanes(base: F32s, step: F32s, offset: i32, stride: i32) -> F32s {
        let iota = F32s::from([0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        let j = if stride == 1 {
            iota
        } else {
            iota * F32s::fill(stride as f32)
        } + F32s::fill(offset as f32);
        j.mul_add(step, base)
    }
}

/// A group of varyings sharing one interpolation format, each holding [`LANES`] pixels.
pub trait Group: Copy + Default {
    type Elem: Elem;
    /// Number of values in the group.
    const LEN: usize;
    /// The group's lines within one sample interval: see [`Line`].
    type Line: Copy;
    /// The lines from true values `a` to `b` (this group's, in layout order), `1 / inv`
    /// pixels apart.
    fn line(a: &[f32], b: &[f32], inv: f32) -> Self::Line;
    /// The group for [`LANES`] pixels `offset + j * stride` pixels along `line`.
    fn lanes(line: &Self::Line, offset: i32, stride: i32) -> Self;
}

/// Where each of a group's `N` values lies within one sample interval, in the group's
/// format: its value at the interval's start and its change per pixel, each already in
/// every lane (set once per interval, used by every block in it).
#[derive(Clone, Copy, Debug)]
pub struct Line<E: Elem, const N: usize> {
    pub base: [E::Lanes; N],
    pub step: [E::Lanes; N],
}

impl<E: Elem, const N: usize> Line<E, N> {
    #[inline(always)]
    pub fn new(a: &[f32], b: &[f32], inv: f32) -> Self {
        let mut line = Self {
            base: [E::Lanes::default(); N],
            step: [E::Lanes::default(); N],
        };
        for k in 0..N {
            line.base[k] = E::splat(E::from_f32(a[k]));
            line.step[k] = E::splat(E::from_f32((b[k] - a[k]) * inv));
        }
        line
    }
}

/// Declares one group struct. Used by [`varyings!`](crate::varyings).
#[macro_export]
macro_rules! varying_group {
    ($group:ident, $elem:ty; $($field:ident: $n:literal),*) => {
        #[derive(Clone, Copy, Default, Debug)]
        pub struct $group {
            $(pub $field: [<$elem as $crate::shader::Elem>::Lanes; $n]),*
        }

        impl $crate::shader::Group for $group {
            type Elem = $elem;
            const LEN: usize = 0 $(+ $n)*;
            type Line = $crate::shader::Line<$elem, { 0 $(+ $n)* }>;

            #[inline(always)]
            fn line(a: &[f32], b: &[f32], inv: f32) -> Self::Line {
                $crate::shader::Line::new(a, b, inv)
            }

            #[inline(always)]
            #[allow(unused_variables, unused_mut, unused_assignments)]
            fn lanes(line: &Self::Line, offset: i32, stride: i32) -> Self {
                let mut g = Self::default();
                let mut k = 0;
                $(for i in 0..$n {
                    g.$field[i] = <$elem as $crate::shader::Elem>::lanes(
                        line.base[k],
                        line.step[k],
                        offset,
                        stride,
                    );
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

/// Name of the built-in varying holding the texture level of detail (one value, meant for
/// an 8.8 varying): log2 of the polygon's texture's full-size texels per screen pixel. It
/// is computed for each vertex when the polygon is set up, from the polygon's exact
/// screen-space derivatives of the mesh attribute `uv`, and interpolated across it like any
/// varying; see [`sample_trilinear`]. A mesh attribute cannot supply it.
pub const LOD: &str = "lod";

/// Name of the built-in varying holding the anisotropic probe offset (two values, meant for
/// a 16.16 varying): half the distance between the two probes along the long axis of the
/// pixel's texture footprint, in uv. 0 where the footprint is round or magnified. See
/// [`sample_anisotropic`].
pub const ANISO: &str = "aniso";

/// Name of the built-in varying holding the anisotropic probes' level of detail (one value,
/// meant for an 8.8 varying): log2 of the texels each probe covers, the larger of the
/// footprint's short axis and half its long axis. See [`sample_anisotropic`].
pub const ANISO_LOD: &str = "aniso_lod";

/// A shader: its varyings and a function producing 32-bit colors for [`LANES`] pixels at
/// once.
///
/// Varyings come from the mesh's attributes by name, except [`POSITION`] and the names in
/// `DERIVED`: values the shader computes itself, in `per_sample`, from the others.
///
/// Along a span, exact (perspective-correct) values are taken at sample points a few pixels
/// apart and interpolated linearly between them. `per_sample` runs on the exact values at
/// each sample point, before interpolating: the place for work too costly per pixel but
/// not linear on screen (normalizing a view vector, say), whose result is then
/// interpolated like any varying.
///
/// `shade` gets [`LANES`] consecutive pixels in each varying's lanes and returns their
/// colors in the same lanes. Lanes past the end of a run are computed too, and discarded.
///
/// Opaque shaders return XRGB (the top byte is ignored). Translucent shaders
/// (`TRANSLUCENT = true`) return ARGB: the renderer draws their polygons after all opaque
/// geometry, back to front, and blends each pixel as `src * a + dst * (1 - a)`. A
/// translucent shader with `READS_BEHIND = true` is shaded with `shade_over` instead of
/// `shade`, which also gets the depth of the opaque surface behind the pixel.
pub trait Shader: 'static {
    type Fixed32: Group<Elem = i32>;
    type Fixed16: Group<Elem = i16>;
    type Floats: Group<Elem = f32>;
    const LAYOUT: &'static [AttribDesc];
    /// Varyings written by `per_sample` rather than read from the mesh (0 at vertices).
    const DERIVED: &'static [&'static str] = &[];
    const TRANSLUCENT: bool = false;
    /// Shade with `shade_over`. Translucent shaders only.
    const READS_BEHIND: bool = false;

    /// Runs on the exact varying values at each sample point, in layout order.
    #[inline(always)]
    fn per_sample(_values: &mut [f32], _uni: &Uniforms) {}

    /// Shades [`LANES`] pixels, at screen position `at`, with the polygon's texture `tex`
    /// (see [`sample`]).
    fn shade(
        a: &Self::Fixed32,
        b: &Self::Fixed16,
        c: &Self::Floats,
        uni: &Uniforms,
        tex: &Texture,
        at: Pixels,
    ) -> U32s;

    /// Shades translucent pixels knowing what they are drawn over: `w` is this surface's
    /// `1 / depth` at each pixel, `behind_w` that of the opaque surface behind it (both on
    /// the same ray through the pixel, so Euclidean distances along it scale as `1 / w`).
    /// Lanes past the end of a run have `behind_w` 0.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn shade_over(
        a: &Self::Fixed32,
        b: &Self::Fixed16,
        c: &Self::Floats,
        uni: &Uniforms,
        tex: &Texture,
        at: Pixels,
        _w: F32s,
        _behind_w: F32s,
    ) -> U32s {
        Self::shade(a, b, c, uni, tex, at)
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
    /// The framebuffer row.
    pub row: i32,
    /// Shade every other pixel (counted from each sample interval's start) and repeat it in
    /// the next, with sample intervals twice as long: set for polygons seen in a mirror.
    pub half_rate: bool,
    /// Relative w change per sample interval allowed before sampling more often.
    pub step_threshold: f32,
    /// Smallest sample interval allowed, in pixels.
    pub min_step: i32,
}

/// Shades a run into `color`. `behind` is the opaque `w` under each pixel of the run for
/// translucent shaders, and empty for opaque ones.
pub type DrawSpanFn =
    fn(job: &SpanJob, color: &mut [u32], behind: &[f32], uni: &Uniforms, tex: &Texture);

/// A registered shader, type-erased.
#[derive(Clone, Copy)]
pub struct ShaderEntry {
    pub layout: &'static [AttribDesc],
    pub derived: &'static [&'static str],
    pub draw_span: DrawSpanFn,
    /// `draw_span` for half-rate spans (see [`SpanJob::half_rate`]).
    pub draw_span_half: DrawSpanFn,
    pub translucent: bool,
    pub reads_behind: bool,
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
            draw_span: span::<S, 1>,
            draw_span_half: span::<S, 2>,
            translucent: S::TRANSLUCENT,
            reads_behind: S::READS_BEHIND,
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

/// [`blend`] for [`LANES`] pixels at once, with bit-identical results.
#[inline(always)]
pub fn blend_lanes(src: U32s, dst: U32s) -> U32s {
    let a = src >> 24;
    let inv = U32s::fill(255) - a;
    let byte = U32s::fill(255);
    let mut out = U32s::fill(0);
    for shift in [0, 8, 16] {
        let (s, d) = ((src >> shift) & byte, (dst >> shift) & byte);
        let x = s * a + d * inv + U32s::fill(127);
        // x / 255 for x < 2^16, exactly.
        out |= ((x * U32s::fill(0x8081)) >> 23) << shift;
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

/// Perspective-correct interpolation across one row span, set up once per run: the
/// reciprocal of the span's width is taken here, leaving one divide per sample point.
struct Perspective {
    x_left: f32,
    inv_span: f32,
    w_left: f32,
    w_right: f32,
}

impl Perspective {
    #[inline(always)]
    fn new(job: &SpanJob) -> Self {
        let span = job.x_right - job.x_left;
        Self {
            x_left: job.x_left,
            inv_span: if span > 0.0 { 1.0 / span } else { 0.0 },
            w_left: job.w_left,
            w_right: job.w_right,
        }
    }

    /// The interpolation parameter at pixel `px`'s center: 0 at the left crossing, 1 at the
    /// right, perspective-correct.
    #[inline(always)]
    fn alpha(&self, px: i32) -> f32 {
        let s = ((px as f32 + 0.5 - self.x_left) * self.inv_span).clamp(0.0, 1.0);
        let den = (1.0 - s) * self.w_left + s * self.w_right;
        if den > 0.0 { s * self.w_right / den } else { s }
    }
}

/// True varying values at pixel `px`'s center on the row, perspective-correct: one alpha
/// from w at the two crossings, shared by every varying.
#[inline(always)]
fn values_at(job: &SpanJob, perspective: &Perspective, px: i32, out: &mut [f32]) {
    let alpha = perspective.alpha(px);
    for (o, (&l, &r)) in out.iter_mut().zip(job.left.iter().zip(job.right)) {
        *o = l + (r - l) * alpha;
    }
}

/// Shades a run of a polygon's row. Exact values are taken at sample points every N pixels
/// from the row's first pixel, and at its last pixel. Between them, pixels are shaded in
/// blocks of [`LANES`] counted from the interval's start: each block's varyings are the
/// lines between the interval's two sample points, evaluated at all its pixels at once
/// (`start + offset * step`, in each group's own format), then shaded at once. The grid
/// belongs to the whole row, so every pixel gets the same value however the row is split
/// into runs.
pub fn draw_span<S: Shader>(
    job: &SpanJob,
    color: &mut [u32],
    behind: &[f32],
    uni: &Uniforms,
    tex: &Texture,
) {
    if job.half_rate {
        span::<S, 2>(job, color, behind, uni, tex);
    } else {
        span::<S, 1>(job, color, behind, uni, tex);
    }
}

/// [`draw_span`], shading every `STRIDE`th pixel from each sample interval's start (each
/// block's [`LANES`] lanes cover `LANES * STRIDE` pixels, every lane's color repeated
/// `STRIDE` times).
pub fn span<S: Shader, const STRIDE: i32>(
    job: &SpanJob,
    color: &mut [u32],
    behind: &[f32],
    uni: &Uniforms,
    tex: &Texture,
) {
    let (x0, x1) = (job.x0, job.x0 + color.len() as i32);
    if x0 >= x1 {
        return;
    }
    debug_assert!(job.row_x0 <= x0 && x1 <= job.row_x1, "run outside its row");
    let n_vals = S::Fixed32::LEN + S::Fixed16::LEN + S::Floats::LEN;
    // Sample points are spaced by the chosen interval in shaded pixels, so half-rate spans
    // sample half as often too.
    let n = choose_step(
        job.w_left,
        job.w_right,
        job.x_right - job.x_left,
        job.step_threshold,
        job.min_step,
    ) * STRIDE;
    let last = job.row_x1 - 1;
    let perspective = Perspective::new(job);
    // Exact values at the current interval's start (`a`) and end (`b`); the end becomes the
    // next interval's start by swapping the two, not copying.
    let (mut a_buf, mut b_buf) = ([0.0f32; MAX_VARYINGS], [0.0f32; MAX_VARYINGS]);
    let (mut a, mut b) = (&mut a_buf[..n_vals], &mut b_buf[..n_vals]);
    let mut a_at = i32::MIN; // the pixel `a` currently holds values for
    let mut run = Run {
        job,
        x0,
        color,
        behind,
        uni,
        tex,
    };
    let mut p = x0;
    while p < x1 {
        if p == last {
            values_at(job, &perspective, last, a);
            S::per_sample(a, uni);
            let lines = lines::<S>(a, a, 0.0);
            run.block::<S, STRIDE>(&lines, 0, last, last..last + 1);
            break;
        }
        // The grid interval holding p: exact at `start` and `end`.
        let start = job.row_x0 + (p - job.row_x0) / n * n;
        let end = (start + n).min(last);
        if a_at != start {
            values_at(job, &perspective, start, a);
            S::per_sample(a, uni);
        }
        values_at(job, &perspective, end, b);
        S::per_sample(b, uni);
        let lines = lines::<S>(a, b, 1.0 / (end - start) as f32);
        let stop = end.min(x1);
        let width = LANES as i32 * STRIDE;
        let mut block = start + (p - start) / width * width;
        while block < stop {
            let pixels = block.max(p)..(block + width).min(stop);
            run.block::<S, STRIDE>(&lines, block - start, block, pixels);
            block += width;
        }
        std::mem::swap(&mut a, &mut b);
        a_at = end;
        p = stop;
    }
}

/// The lines of all three groups within one sample interval.
type Lines<S> = (
    <<S as Shader>::Fixed32 as Group>::Line,
    <<S as Shader>::Fixed16 as Group>::Line,
    <<S as Shader>::Floats as Group>::Line,
);

/// The lines from true values `a` to `b` (layout order), `1 / inv` pixels apart.
#[inline(always)]
fn lines<S: Shader>(a: &[f32], b: &[f32], inv: f32) -> Lines<S> {
    let (f32s, f16s) = (S::Fixed32::LEN, S::Fixed16::LEN);
    let (a32, a16, af) = (&a[..f32s], &a[f32s..f32s + f16s], &a[f32s + f16s..]);
    let (b32, b16, bf) = (&b[..f32s], &b[f32s..f32s + f16s], &b[f32s + f16s..]);
    (
        S::Fixed32::line(a32, b32, inv),
        S::Fixed16::line(a16, b16, inv),
        S::Floats::line(af, bf, inv),
    )
}

/// One run being shaded: its pixels `x0..x0 + color.len()`.
struct Run<'a> {
    job: &'a SpanJob<'a>,
    x0: i32,
    color: &'a mut [u32],
    behind: &'a [f32],
    uni: &'a Uniforms,
    tex: &'a Texture,
}

impl Run<'_> {
    /// Shades the block of `LANES * STRIDE` pixels starting at pixel `first`, `offset`
    /// pixels along `lines` (one lane every `STRIDE` pixels), and stores the ones in
    /// `pixels`.
    #[inline(always)]
    fn block<S: Shader, const STRIDE: i32>(
        &mut self,
        lines: &Lines<S>,
        offset: i32,
        first: i32,
        pixels: Range<i32>,
    ) {
        let ga = S::Fixed32::lanes(&lines.0, offset, STRIDE);
        let gb = S::Fixed16::lanes(&lines.1, offset, STRIDE);
        let gc = S::Floats::lanes(&lines.2, offset, STRIDE);
        let at = Pixels {
            x: first,
            y: self.job.row,
            stride: STRIDE,
        };
        let out = if S::READS_BEHIND {
            // w is linear in screen x.
            let job = self.job;
            let span = job.x_right - job.x_left;
            let dw = if span > 0.0 {
                (job.w_right - job.w_left) / span
            } else {
                0.0
            };
            let w0 = job.w_left + dw * (first as f32 + 0.5 - job.x_left);
            let w = f32::lanes(F32s::fill(w0), F32s::fill(dw), 0, STRIDE);
            // What is behind each lane's pixel, or the pixel after it if that one is in the
            // run and the lane's is not; 0 for lanes past the run.
            let mut behind = [0.0f32; LANES];
            for (j, b) in behind.iter_mut().enumerate() {
                let px = first + j as i32 * STRIDE;
                if let Some(q) = (px..px + STRIDE).find(|q| pixels.contains(q)) {
                    *b = self.behind[(q - self.x0) as usize];
                }
            }
            S::shade_over(&ga, &gb, &gc, self.uni, self.tex, at, w, F32s::from(behind))
        } else {
            S::shade(&ga, &gb, &gc, self.uni, self.tex, at)
        };
        let lanes = out.to_array();
        let at = (pixels.start - self.x0) as usize;
        let skip = (pixels.start - first) as usize;
        if STRIDE == 1 {
            if pixels.len() == LANES {
                // A whole block: one fixed-size store.
                let dst: &mut [u32; LANES] = (&mut self.color[at..at + LANES]).try_into().unwrap();
                *dst = lanes;
            } else {
                for (i, c) in self.color[at..at + pixels.len()].iter_mut().enumerate() {
                    *c = lanes[skip + i];
                }
            }
        } else {
            // Each lane's color covers its pixel and the next.
            let mut out = [0u32; 2 * LANES];
            for (j, &c) in lanes.iter().enumerate() {
                out[2 * j] = c;
                out[2 * j + 1] = c;
            }
            if pixels.len() == 2 * LANES {
                let dst: &mut [u32; 2 * LANES] =
                    (&mut self.color[at..at + 2 * LANES]).try_into().unwrap();
                *dst = out;
            } else {
                for (i, c) in self.color[at..at + pixels.len()].iter_mut().enumerate() {
                    *c = out[skip + i];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The direction through the center of texel `(x, y)` of face `face` of a `size` cube map.
    fn cube_texel_direction(face: usize, x: u32, y: u32, size: u32) -> [f32; 3] {
        let [forward, right, up] = moose_assets::CUBE_FACES[face];
        let at = |i: u32| (i as f32 + 0.5) / size as f32 * 2.0 - 1.0;
        let (u, v) = (at(x), at(y));
        std::array::from_fn(|k| forward[k] + right[k] * u - up[k] * v)
    }

    #[test]
    fn cube_maps_find_the_texel_in_each_direction() {
        // Every texel of a 4x4 cube map (a unique value each), looked up through its center,
        // at several lengths: the face and texel come back exactly, unblended.
        let size = 4;
        let faces: [Vec<u32>; 6] = std::array::from_fn(|f| {
            (0..size * size)
                .map(|i| ((f as u32) << 20) | (i << 4))
                .collect()
        });
        let tex = Texture::cube("cube", size, faces).unwrap();
        let mut dirs = Vec::new();
        for face in 0..6 {
            for y in 0..size {
                for x in 0..size {
                    let want = ((face as u32) << 20) | ((y * size + x) << 4);
                    let d = cube_texel_direction(face, x, y, size);
                    for scale in [0.01f32, 1.0, 37.0] {
                        dirs.push((d.map(|c| c * scale), want));
                    }
                }
            }
        }
        for chunk in dirs.chunks(LANES) {
            let lane = |k: usize| F32s::from(std::array::from_fn(|i| chunk[i % chunk.len()].0[k]));
            let got = sample_cube(&tex, [lane(0), lane(1), lane(2)], 0.0).to_array();
            for (i, &(d, want)) in chunk.iter().enumerate() {
                assert_eq!(got[i], want, "direction {d:?}");
            }
        }
    }

    #[test]
    fn cube_maps_blend_toward_the_next_level() {
        // Level 0 has black texels, level 1 their faces' averages; lod 0.5 is halfway, and past
        // the last level the smallest one is used.
        let faces: [Vec<u32>; 6] = std::array::from_fn(|_| vec![0, 0x00FE_FEFE, 0x00FE_FEFE, 0]);
        let tex = Texture::cube("cube", 2, faces).unwrap();
        assert_eq!(tex.levels[1].texels, [0x007F_7F7F; 6]);
        let d = cube_texel_direction(5, 0, 0, 2).map(F32s::fill);
        assert_eq!(sample_cube(&tex, d, 0.0), U32s::fill(0));
        assert_eq!(sample_cube(&tex, d, 0.5), U32s::fill(0x003F_3F3F));
        assert_eq!(sample_cube(&tex, d, 9.0), U32s::fill(0x007F_7F7F));
    }

    /// Test shader writing its raw fixed-point varyings as the color: one 8.8 value in the
    /// low half, the low 16 bits of one 16.16 value in the high half.
    mod raw {
        use super::super::{AttribDesc, Fill, I32s, Pixels, Shader, U32s, Uniforms};
        use moose_assets::Texture;
        crate::varyings! { fixed32 { u: 1 } fixed16 { c: 1 } float {} }
        pub struct Raw;
        impl Shader for Raw {
            type Fixed32 = Fixed32;
            type Fixed16 = Fixed16;
            type Floats = Floats;
            const LAYOUT: &'static [AttribDesc] = LAYOUT;
            fn shade(
                a: &Fixed32,
                b: &Fixed16,
                _: &Floats,
                _: &Uniforms,
                _: &Texture,
                _: Pixels,
            ) -> U32s {
                let c: U32s = wide::bytemuck::cast(I32s::from_i16x8(b.c[0]));
                let u: U32s = wide::bytemuck::cast(a.u[0]);
                (c & U32s::fill(0xFFFF)) | (u << 16)
            }
        }
    }

    /// The span shading before SIMD, one pixel at a time: exact at sample points and at the
    /// row's last pixel, stepped by repeated wrapping adds in between.
    fn stepped(job: &SpanJob, px: i32) -> u32 {
        // Half-rate spans sample half as often.
        let n = choose_step(
            job.w_left,
            job.w_right,
            job.x_right - job.x_left,
            job.step_threshold,
            job.min_step,
        ) * if job.half_rate { 2 } else { 1 };
        let last = job.row_x1 - 1;
        let perspective = Perspective::new(job);
        let (mut a, mut b) = ([0.0; 2], [0.0; 2]);
        let (u, c) = if px == last {
            values_at(job, &perspective, last, &mut a);
            (i32::from_f32(a[0]), i16::from_f32(a[1]))
        } else {
            let start = job.row_x0 + (px - job.row_x0) / n * n;
            let end = (start + n).min(last);
            values_at(job, &perspective, start, &mut a);
            values_at(job, &perspective, end, &mut b);
            let inv = 1.0 / (end - start) as f32;
            let (mut u, mut c) = (i32::from_f32(a[0]), i16::from_f32(a[1]));
            let (du, dc) = (
                i32::from_f32((b[0] - a[0]) * inv),
                i16::from_f32((b[1] - a[1]) * inv),
            );
            for _ in start..px {
                u = u.wrapping_add(du);
                c = c.wrapping_add(dc);
            }
            (u, c)
        };
        (c as u16 as u32) | ((u as u32) << 16)
    }

    #[test]
    fn blocks_match_stepping_bit_for_bit() {
        let mut seed = 12345u64;
        let mut rnd = |lo: f32, hi: f32| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            lo + (hi - lo) * ((seed >> 40) as f32 / (1u64 << 24) as f32)
        };
        for round in 0..4000 {
            let row_x0 = rnd(0.0, 50.0) as i32;
            let row_x1 = row_x0 + 1 + rnd(0.0, 200.0) as i32;
            let (left, right) = (
                [rnd(-3.0, 3.0), rnd(0.0, 255.0)],
                [rnd(-3.0, 3.0), rnd(0.0, 255.0)],
            );
            let mut job = SpanJob {
                x_left: row_x0 as f32 + rnd(-0.5, 0.5),
                x_right: row_x1 as f32 + rnd(-0.5, 0.5),
                w_left: rnd(0.01, 2.0),
                w_right: rnd(0.01, 2.0),
                left: &left,
                right: &right,
                row_x0,
                row_x1,
                x0: row_x0,
                row: 0,
                // Half the rows at half rate: each pixel then shows its pair's even pixel,
                // counted from its sample interval's start.
                half_rate: round % 2 == 1,
                step_threshold: 1.0 / 16.0,
                min_step: [1, 2, 4, 8][rnd(0.0, 3.99) as usize],
            };
            let n = choose_step(
                job.w_left,
                job.w_right,
                job.x_right - job.x_left,
                job.step_threshold,
                job.min_step,
            ) * 2;
            let shown = |px: i32| {
                if !job.half_rate || px == row_x1 - 1 {
                    px
                } else {
                    let start = row_x0 + (px - row_x0) / n * n;
                    px - (px - start) % 2
                }
            };
            // The row split into random runs, as visibility would.
            let mut x = row_x0;
            while x < row_x1 {
                let end = (x + 1 + rnd(0.0, 40.0) as i32).min(row_x1);
                job.x0 = x;
                let mut color = vec![0u32; (end - x) as usize];
                let blank = Texture::solid("blank", 0);
                draw_span::<raw::Raw>(&job, &mut color, &[], &Uniforms::default(), &blank);
                for (i, &got) in color.iter().enumerate() {
                    let px = x + i as i32;
                    assert_eq!(
                        got,
                        stepped(&job, shown(px)),
                        "pixel {px} of row {row_x0}..{row_x1} (half rate {})",
                        job.half_rate
                    );
                }
                x = end;
            }
        }
    }

    #[test]
    fn bilinear_sampling_matches_exact_filtering() {
        use moose_assets::Texture;
        let mut seed = 99u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as u32
        };
        for (w, h) in [(64u32, 64u32), (4, 16), (256, 2)] {
            let tex = Texture::new("t", w, h, (0..w * h).map(|_| next()).collect()).unwrap();
            let mut worst = 0;
            for _ in 0..2000 {
                // uv anywhere, including negative and many tiles out (it wraps).
                let uv: [(f32, f32); LANES] = std::array::from_fn(|_| {
                    let f = |n: u32| (n % 40000) as f32 / 1000.0 - 20.0;
                    (f(next()), f(next()))
                });
                let fixed = |c: f32| (c * 65536.0).round() as i32;
                let got = sample_bilinear(
                    tex.base(),
                    I32s::from(uv.map(|(u, _)| fixed(u))),
                    I32s::from(uv.map(|(_, v)| fixed(v))),
                )
                .to_array();
                for (i, &(u, v)) in uv.iter().enumerate() {
                    // Exact: texel centers at whole numbers after the half-texel shift.
                    let (x, y) = (u * w as f32 - 0.5, v * h as f32 - 0.5);
                    let (x0, y0) = (x.floor(), y.floor());
                    let (fx, fy) = (x - x0, y - y0);
                    let t = |dx: f32, dy: f32| {
                        tex.texel((x0 + dx) as i32 as u32, (y0 + dy) as i32 as u32)
                    };
                    for shift in [0, 8, 16, 24] {
                        let c = |t: u32| ((t >> shift) & 255) as f32;
                        let want = (c(t(0.0, 0.0)) * (1.0 - fx) + c(t(1.0, 0.0)) * fx) * (1.0 - fy)
                            + (c(t(0.0, 1.0)) * (1.0 - fx) + c(t(1.0, 1.0)) * fx) * fy;
                        let d = (((got[i] >> shift) & 255) as f32 - want).abs();
                        worst = worst.max(d.ceil() as u32);
                    }
                }
            }
            // 8-bit weights and two truncating steps.
            assert!(worst <= 3, "{w}x{h}: worst difference {worst}");
        }
        // At a texel's center it is exactly that texel.
        let tex = Texture::new("t", 4, 4, (0..16).map(|i| i * 0x0101_0101).collect()).unwrap();
        let center = |i: i32| ((i as f32 + 0.5) / 4.0 * 65536.0) as i32;
        let got = sample_bilinear(
            tex.base(),
            I32s::from(std::array::from_fn::<i32, LANES, _>(|i| {
                center(i as i32 % 4)
            })),
            I32s::from([center(2); LANES]),
        )
        .to_array();
        for (i, &c) in got.iter().enumerate() {
            assert_eq!(c, tex.texel(i as u32 % 4, 2));
        }
    }

    #[test]
    fn trilinear_blends_the_two_levels_around_the_lod() {
        use moose_assets::Texture;
        let mut seed = 7u64;
        let texels: Vec<u32> = (0..64 * 64)
            .map(|_| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (seed >> 33) as u32
            })
            .collect();
        let tex = Texture::new("t", 64, 64, texels).unwrap();
        let u = I32s::from(std::array::from_fn::<i32, LANES, _>(|i| {
            1000 + i as i32 * 4567
        }));
        let v = I32s::from(std::array::from_fn::<i32, LANES, _>(|i| {
            20000 - i as i32 * 3111
        }));
        let at = |level: usize| sample_bilinear(&tex.levels[level], u, v).to_array();
        let channels_near = |a: u32, b: u32, tol: u32| {
            (0..4).all(|k| ((a >> (8 * k)) & 255).abs_diff((b >> (8 * k)) & 255) <= tol)
        };
        // Whole levels: exactly bilinear on that level (the next is weighted 0).
        for level in 0..tex.levels.len() {
            let got = sample_trilinear(&tex, u, v, I16s::fill(level as i16 * 256)).to_array();
            assert_eq!(got, at(level), "level {level}");
        }
        // Below 0 is level 0; past the last is the last.
        assert_eq!(
            sample_trilinear(&tex, u, v, I16s::fill(-700)).to_array(),
            at(0)
        );
        assert_eq!(
            sample_trilinear(&tex, u, v, I16s::fill(40 * 256)).to_array(),
            at(6)
        );
        // Fractions blend toward the next level.
        let got = sample_trilinear(&tex, u, v, I16s::fill(2 * 256 + 64)).to_array();
        let (a, b) = (at(2), at(3));
        for i in 0..LANES {
            let want = (0..4).fold(0, |c, k| {
                let (x, y) = ((a[i] >> (8 * k)) & 255, (b[i] >> (8 * k)) & 255);
                c | ((x * 192 + y * 64) / 256) << (8 * k)
            });
            assert!(channels_near(got[i], want, 1), "lane {i}");
        }
        // Lanes on different levels each get their own.
        let lods: [i16; LANES] = [0, 256, 300, 512, 1024, 1100, 256, 0];
        let got = sample_trilinear(&tex, u, v, I16s::from(lods)).to_array();
        for (i, &lod) in lods.iter().enumerate() {
            let alone = sample_trilinear(&tex, u, v, I16s::fill(lod)).to_array();
            assert_eq!(got[i], alone[i], "lane {i} at lod {lod}");
        }
    }

    #[test]
    fn anisotropic_averages_two_probes() {
        use moose_assets::Texture;
        let texels: Vec<u32> = (0..64 * 64u32)
            .map(|i| i.wrapping_mul(2654435761))
            .collect();
        let tex = Texture::new("t", 64, 64, texels).unwrap();
        let u = I32s::from(std::array::from_fn::<i32, LANES, _>(|i| {
            3000 + i as i32 * 5000
        }));
        let v = I32s::from(std::array::from_fn::<i32, LANES, _>(|i| {
            9000 - i as i32 * 2000
        }));
        let (lod, probe_lod) = (I16s::fill(2 * 256 + 100), I16s::fill(256 + 100));
        // No offset: plain trilinear at `lod`.
        let flat = [I32s::fill(0); 2];
        assert_eq!(
            sample_anisotropic(&tex, u, v, flat, lod, probe_lod).to_array(),
            sample_trilinear(&tex, u, v, lod).to_array()
        );
        // An offset: the rounded average of the probes either side, at `probe_lod`.
        let offset = [I32s::fill(700), I32s::fill(-300)];
        let got = sample_anisotropic(&tex, u, v, offset, lod, probe_lod).to_array();
        let a = sample_trilinear(&tex, u + offset[0], v + offset[1], probe_lod).to_array();
        let b = sample_trilinear(&tex, u - offset[0], v - offset[1], probe_lod).to_array();
        for i in 0..LANES {
            for k in 0..4 {
                let (x, y) = ((a[i] >> (8 * k)) & 255, (b[i] >> (8 * k)) & 255);
                assert_eq!(
                    (got[i] >> (8 * k)) & 255,
                    (x + y) / 2,
                    "lane {i} channel {k}"
                );
            }
        }
    }

    #[test]
    fn blend_lanes_matches_blend() {
        for a in 0..256u32 {
            for s in (0..256u32).step_by(3) {
                let src: [u32; LANES] = std::array::from_fn(|i| {
                    a << 24 | s << 16 | ((s * 7 + i as u32 * 31) & 255) << 8 | (255 - s)
                });
                let dst: [u32; LANES] =
                    std::array::from_fn(|i| ((a * 13 + s + i as u32 * 97) & 0xFF_FFFF) * 0x0101);
                let got = blend_lanes(U32s::from(src), U32s::from(dst)).to_array();
                for i in 0..LANES {
                    assert_eq!(
                        got[i],
                        blend(src[i], dst[i] & 0xFF_FFFF),
                        "a {a} s {s} lane {i}"
                    );
                }
            }
        }
    }
}
