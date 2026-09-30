//! Material programs (see the Material Pipeline Spec), dispatch, and perspective-correct
//! shading of spans.
//!
//! A material has three stages: `shade_vertex` per source polygon vertex, `shade_sample`
//! at sample points, and `shade_pixel` per pixel. The [`material_io!`](crate::material_io)
//! macro declares what flows between them. What pixels get is three homogeneous groups,
//! one per interpolation format: 16.16 fixed point (`Fixed32`), 8.8 fixed point (`Fixed16`)
//! and `f32` (`Floats`). A registry of type-erased [`MaterialEntry`]s is dispatched once
//! per shading run; inside, the loop is monomorphized per material.
//!
//! Sample points and pixels are shaded [`LANES`] at a time with portable SIMD (the `wide`
//! crate: NEON on ARM, SSE/AVX on x86): every value holds one sample point or pixel per
//! lane.

use std::cell::Cell;
use std::ops::Range;

use glam::Vec3;
use moose_assets::{MipLevel, Light, Texture};
use moose_view::Object;

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

/// Texture samplers: how texels are read within a mip level (nearest, bilinear, or dithered
/// as Unreal's software renderer did), and how mip levels are chosen (not at all, the
/// nearest, blended between the two around the level of detail, or dithered between them).
/// A sampler is one `u8`, used as a material's `FILTER` parameter so that each is its own
/// monomorphized shading loop, with no per-pixel branching on it.
pub mod filter {
    /// Texel methods: the nearest texel, the four around the point blended, or the nearest
    /// after moving the point by a 4x4 screen-space pattern of sub-texel offsets
    /// ([`TEXEL_DITHER`](super::TEXEL_DITHER)), which approximates bilinear filtering with
    /// one texel read.
    pub const NEAREST: u8 = 0;
    pub const BILINEAR: u8 = 1;
    pub const DITHERED: u8 = 2;
    /// Mip transitions: the full-size level only, the level nearest the level of detail,
    /// the two levels around it blended by its fraction, or one of those two per pixel by
    /// a 4x4 screen-space ordered pattern ([`MIP_DITHER`](super::MIP_DITHER)).
    pub const MIPMAP_NONE: u8 = 0;
    pub const MIPMAP_NEAREST: u8 = 1;
    pub const MIPMAP_LINEAR: u8 = 2;
    pub const MIPMAP_DITHERED: u8 = 3;

    /// The sampler with texel method `method` and mip transition `mip`.
    pub const fn of(method: u8, mip: u8) -> u8 {
        method << 2 | mip
    }
    /// A sampler's texel method.
    pub const fn method(filter: u8) -> u8 {
        filter >> 2
    }
    /// A sampler's mip transition.
    pub const fn mip(filter: u8) -> u8 {
        filter & 3
    }

    pub const NEAREST_MIPMAP_NONE: u8 = of(NEAREST, MIPMAP_NONE);
    pub const BILINEAR_MIPMAP_NONE: u8 = of(BILINEAR, MIPMAP_NONE);
    pub const DITHERED_MIPMAP_NONE: u8 = of(DITHERED, MIPMAP_NONE);
    pub const NEAREST_MIPMAP_NEAREST: u8 = of(NEAREST, MIPMAP_NEAREST);
    pub const NEAREST_MIPMAP_LINEAR: u8 = of(NEAREST, MIPMAP_LINEAR);
    pub const NEAREST_MIPMAP_DITHERED: u8 = of(NEAREST, MIPMAP_DITHERED);
    pub const BILINEAR_MIPMAP_NEAREST: u8 = of(BILINEAR, MIPMAP_NEAREST);
    pub const BILINEAR_MIPMAP_LINEAR: u8 = of(BILINEAR, MIPMAP_LINEAR);
    pub const BILINEAR_MIPMAP_DITHERED: u8 = of(BILINEAR, MIPMAP_DITHERED);
    pub const DITHERED_MIPMAP_NEAREST: u8 = of(DITHERED, MIPMAP_NEAREST);
    pub const DITHERED_MIPMAP_LINEAR: u8 = of(DITHERED, MIPMAP_LINEAR);
    pub const DITHERED_MIPMAP_DITHERED: u8 = of(DITHERED, MIPMAP_DITHERED);

    /// Every sampler, in the order the app cycles through them.
    pub const ALL: [u8; 12] = [
        NEAREST_MIPMAP_NONE,
        BILINEAR_MIPMAP_NONE,
        DITHERED_MIPMAP_NONE,
        NEAREST_MIPMAP_NEAREST,
        NEAREST_MIPMAP_LINEAR,
        NEAREST_MIPMAP_DITHERED,
        BILINEAR_MIPMAP_NEAREST,
        BILINEAR_MIPMAP_LINEAR,
        BILINEAR_MIPMAP_DITHERED,
        DITHERED_MIPMAP_NEAREST,
        DITHERED_MIPMAP_LINEAR,
        DITHERED_MIPMAP_DITHERED,
    ];

    /// A sampler's name, as its sample function's without `sample_`: "bilinear_mipmap_linear".
    pub fn name(filter: u8) -> &'static str {
        const NAMES: [[&str; 4]; 3] = [
            [
                "nearest_mipmap_none",
                "nearest_mipmap_nearest",
                "nearest_mipmap_linear",
                "nearest_mipmap_dithered",
            ],
            [
                "bilinear_mipmap_none",
                "bilinear_mipmap_nearest",
                "bilinear_mipmap_linear",
                "bilinear_mipmap_dithered",
            ],
            [
                "dithered_mipmap_none",
                "dithered_mipmap_nearest",
                "dithered_mipmap_linear",
                "dithered_mipmap_dithered",
            ],
        ];
        NAMES[method(filter) as usize][mip(filter) as usize]
    }

    /// The sampler named `name` (see [`name`]).
    pub fn named(name: &str) -> Option<u8> {
        ALL.into_iter().find(|&f| self::name(f) == name)
    }
}

/// Registers a material generic over a sampler (its `FILTER` parameter, like
/// [`Textured`](crate::shaders::Textured)) once per sampler, in [`filter::ALL`] order:
/// `register_per_filter!(renderer, Textured)` is a `[MaterialId; 12]`.
#[macro_export]
macro_rules! register_per_filter {
    ($renderer:expr, $material:ident) => {{
        let r = &mut $renderer;
        [
            r.register_material::<$material<{ $crate::shader::filter::ALL[0] }>>(),
            r.register_material::<$material<{ $crate::shader::filter::ALL[1] }>>(),
            r.register_material::<$material<{ $crate::shader::filter::ALL[2] }>>(),
            r.register_material::<$material<{ $crate::shader::filter::ALL[3] }>>(),
            r.register_material::<$material<{ $crate::shader::filter::ALL[4] }>>(),
            r.register_material::<$material<{ $crate::shader::filter::ALL[5] }>>(),
            r.register_material::<$material<{ $crate::shader::filter::ALL[6] }>>(),
            r.register_material::<$material<{ $crate::shader::filter::ALL[7] }>>(),
            r.register_material::<$material<{ $crate::shader::filter::ALL[8] }>>(),
            r.register_material::<$material<{ $crate::shader::filter::ALL[9] }>>(),
            r.register_material::<$material<{ $crate::shader::filter::ALL[10] }>>(),
            r.register_material::<$material<{ $crate::shader::filter::ALL[11] }>>(),
        ]
    }};
}

/// Unreal-style texel dithering, as version 1 did it: each pixel's texture coordinates move
/// by `-e` in u and `+e` in v, with `e` from this matrix at `[x & 3][y & 3]` (screen x and
/// y), in 1/65536 of a texel (-1/2 to 7/16 of a texel), before the nearest texel is read.
/// Across 4x4 pixels the reads spread over the neighboring texels about as bilinear
/// weighting would.
pub const TEXEL_DITHER: [[i32; 4]; 4] = [
    [-32768, 0, -28672, 4096],
    [16384, -16384, 20480, -12288],
    [-20480, 12288, -24576, 8192],
    [28672, -4096, 24576, -8192],
];

/// Ordered dithering between two mip levels, as in version 1's `dev/dither.html`: a pixel
/// reads the next level down when `MIP_DITHER[y & 3][x & 3] + t >= 16`, where `t` is the
/// level of detail's fraction in sixteenths (rounded; 0 to 16). So a fraction of `k / 16`
/// sends `k` of every 16 pixels to the next level.
pub const MIP_DITHER: [[i32; 4]; 4] = [
    [7, 12, 4, 14],
    [11, 2, 9, 1],
    [5, 15, 6, 13],
    [8, 0, 10, 3],
];

/// Each lane's [`TEXEL_DITHER`] offset.
#[inline(always)]
fn texel_dither(at: Pixels) -> I32s {
    let x = at.x_lanes().to_array();
    let y = (at.y & 3) as usize;
    I32s::from(std::array::from_fn::<i32, LANES, _>(|i| {
        TEXEL_DITHER[(x[i] & 3) as usize][y]
    }))
}

/// Each lane's [`MIP_DITHER`] threshold.
#[inline(always)]
fn mip_dither(at: Pixels) -> I32s {
    let x = at.x_lanes().to_array();
    let row = MIP_DITHER[(at.y & 3) as usize];
    I32s::from(std::array::from_fn::<i32, LANES, _>(|i| row[(x[i] & 3) as usize]))
}

/// The nearest texel of one mip level for each lane, wrapping (the texture tiles): `u` and
/// `v` are 16.16 fixed point with 1.0 across the whole texture, `v` down from its top row.
/// The level must be at most 65536 texels on a side.
#[inline(always)]
pub fn level_nearest(tex: &MipLevel, u: I32s, v: I32s) -> U32s {
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
/// [`level_nearest`]; the weights have 8-bit precision.
#[inline(always)]
pub fn level_bilinear(tex: &MipLevel, u: I32s, v: I32s) -> U32s {
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

/// Unreal-style dithered texels of one mip level for each lane (see [`TEXEL_DITHER`]):
/// the nearest texel after moving the point by the pixel's offset, in this level's texels.
/// Coordinates as for [`level_nearest`].
#[inline(always)]
pub fn level_dithered(tex: &MipLevel, u: I32s, v: I32s, at: Pixels) -> U32s {
    let e = texel_dither(at);
    level_nearest(
        tex,
        u - (e >> tex.width_log2 as i32),
        v + (e >> tex.height_log2 as i32),
    )
}

/// One mip level read with texel method `method` (see [`filter`]).
#[inline(always)]
fn read_level(method: u8, tex: &MipLevel, u: I32s, v: I32s, at: Pixels) -> U32s {
    match method {
        filter::NEAREST => level_nearest(tex, u, v),
        filter::BILINEAR => level_bilinear(tex, u, v),
        _ => level_dithered(tex, u, v, at),
    }
}

/// Reads all lanes from one mip level: what [`mip_transition`] picks levels for. Readers
/// are types, not closures, so that each read is always inlined and its sampler folds to
/// constants.
trait LevelRead {
    fn read(&self, level: usize) -> U32s;
}

/// A texture's level, read with sampler `FILTER`'s texel method.
struct TextureRead<'a, const FILTER: u8> {
    tex: &'a Texture,
    u: I32s,
    v: I32s,
    at: Pixels,
}

impl<const FILTER: u8> LevelRead for TextureRead<'_, FILTER> {
    #[inline(always)]
    fn read(&self, level: usize) -> U32s {
        read_level(
            filter::method(FILTER),
            &self.tex.levels[level],
            self.u,
            self.v,
            self.at,
        )
    }
}

/// A level blended toward the next by each lane's `fraction` (of 256), except the last
/// level, or where no lane has a fraction.
struct Blended<'a, R> {
    read: &'a R,
    fraction: U32s,
    whole: bool,
    last: usize,
}

impl<R: LevelRead> LevelRead for Blended<'_, R> {
    #[inline(always)]
    fn read(&self, level: usize) -> U32s {
        let near = self.read.read(level);
        if level < self.last && !self.whole {
            lerp_texels(near, self.read.read(level + 1), self.fraction)
        } else {
            near // the smallest level: nothing further to blend
        }
    }
}

/// The mip transition `mip` (see [`filter`]): the color at level of detail `lod` (8.8
/// fixed point, log2 of the texels per pixel, per lane), from a texture with `levels`
/// levels. Below 0 is the full-size level; past the last level, the last.
///
/// A block's lanes nearly always share a level; where they straddle a level boundary (or
/// dithering spreads them over two), each level they use is read for all lanes, and each
/// lane keeps its own.
#[inline(always)]
fn mip_transition(mip: u8, levels: usize, lod: I32s, at: Pixels, read: &impl LevelRead) -> U32s {
    if mip == filter::MIPMAP_NONE {
        return read.read(0);
    }
    let last = (levels - 1) as i32;
    let lod = lod.max(I32s::fill(0)).min(I32s::fill(last << 8));
    match mip {
        filter::MIPMAP_NEAREST => {
            let level: I32s = (lod + I32s::fill(128)) >> 8;
            per_lane_level(level.min(I32s::fill(last)), read)
        }
        filter::MIPMAP_LINEAR => {
            let fraction: U32s = wide::bytemuck::cast(lod & I32s::fill(255));
            let blended = Blended {
                read,
                fraction,
                // Magnified (or exactly on a level) across the block: the next level has
                // no weight.
                whole: fraction == U32s::fill(0),
                last: last as usize,
            };
            per_lane_level(lod >> 8, &blended)
        }
        _ => {
            // The fraction in sixteenths, rounded; at the last level it is 0.
            let t: I32s = ((lod & I32s::fill(255)) + I32s::fill(8)) >> 4;
            let next: I32s = (mip_dither(at) + t) >> 4;
            let level: I32s = lod >> 8;
            per_lane_level(level + next, read)
        }
    }
}

/// `read` at each lane's own `level`: every level any lane uses is read for all lanes, and
/// each lane keeps its own.
#[inline(always)]
fn per_lane_level(level: I32s, read: &impl LevelRead) -> U32s {
    let levels = level.to_array();
    let (lo, hi) = (*levels.iter().min().unwrap(), *levels.iter().max().unwrap());
    if lo == hi {
        return read.read(lo as usize);
    }
    let mut out = U32s::fill(0);
    for l in lo..=hi {
        let this: U32s = wide::bytemuck::cast(level.simd_eq(I32s::fill(l)));
        if this == U32s::fill(0) {
            continue;
        }
        out = (read.read(l as usize) & this) | (out & !this);
    }
    out
}

/// A texture's color for each lane with sampler `FILTER` (see [`filter`]): `u` and `v` as
/// for [`level_nearest`], `lod` the level of detail (8.8 fixed point, log2 of full-size
/// texels per pixel; see [`LOD`]), `at` the lanes' screen pixels (for the dithered
/// samplers). The `sample_*` functions name each sampler.
#[inline(always)]
pub fn sample_texture<const FILTER: u8>(
    tex: &Texture,
    u: I32s,
    v: I32s,
    lod: I16s,
    at: Pixels,
) -> U32s {
    let read = TextureRead::<FILTER> { tex, u, v, at };
    mip_transition(
        filter::mip(FILTER),
        tex.levels.len(),
        I32s::from_i16x8(lod),
        at,
        &read,
    )
}

/// The nearest texel of the full-size level.
pub fn sample_nearest_mipmap_none(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::NEAREST_MIPMAP_NONE }>(tex, u, v, lod, at)
}
/// Bilinear on the full-size level.
pub fn sample_bilinear_mipmap_none(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::BILINEAR_MIPMAP_NONE }>(tex, u, v, lod, at)
}
/// Dithered texels of the full-size level.
pub fn sample_dithered_mipmap_none(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::DITHERED_MIPMAP_NONE }>(tex, u, v, lod, at)
}
/// The nearest texel of the nearest level.
pub fn sample_nearest_mipmap_nearest(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::NEAREST_MIPMAP_NEAREST }>(tex, u, v, lod, at)
}
/// The nearest texels of the two levels around the level of detail, blended.
pub fn sample_nearest_mipmap_linear(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::NEAREST_MIPMAP_LINEAR }>(tex, u, v, lod, at)
}
/// The nearest texel of one of the two levels around the level of detail, dithered.
pub fn sample_nearest_mipmap_dithered(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::NEAREST_MIPMAP_DITHERED }>(tex, u, v, lod, at)
}
/// Bilinear on the nearest level.
pub fn sample_bilinear_mipmap_nearest(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::BILINEAR_MIPMAP_NEAREST }>(tex, u, v, lod, at)
}
/// Bilinear on the two levels around the level of detail, blended (trilinear).
pub fn sample_bilinear_mipmap_linear(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::BILINEAR_MIPMAP_LINEAR }>(tex, u, v, lod, at)
}
/// Bilinear on one of the two levels around the level of detail, dithered.
pub fn sample_bilinear_mipmap_dithered(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::BILINEAR_MIPMAP_DITHERED }>(tex, u, v, lod, at)
}
/// Dithered texels of the nearest level.
pub fn sample_dithered_mipmap_nearest(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::DITHERED_MIPMAP_NEAREST }>(tex, u, v, lod, at)
}
/// Dithered texels of the two levels around the level of detail, blended.
pub fn sample_dithered_mipmap_linear(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::DITHERED_MIPMAP_LINEAR }>(tex, u, v, lod, at)
}
/// Dithered texels of one of the two levels around the level of detail, dithered.
pub fn sample_dithered_mipmap_dithered(tex: &Texture, u: I32s, v: I32s, lod: I16s, at: Pixels) -> U32s {
    sample_texture::<{ filter::DITHERED_MIPMAP_DITHERED }>(tex, u, v, lod, at)
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

/// One level of a cube map ([`Texture::cube`]) at face coordinates from
/// [`cube_face_coords`], read with texel method `method` (see [`filter`]), clamped to the
/// face (it does not blend across into the next).
#[inline(always)]
pub fn cube_level(
    method: u8,
    level: &MipLevel,
    face: I32s,
    u: F32s,
    v: F32s,
    at: Pixels,
) -> U32s {
    let size = level.width as i32;
    // Texel coordinates with an 8-bit fraction, texel centers on whole numbers:
    // ((c + 1) / 2 * size - 0.5) * 256.
    let (scale, offset) = (size as f32 * 128.0, size as f32 * 128.0 - 128.0);
    let last = I32s::fill(size - 1);
    let max = I32s::fill((size - 1) << 8);
    let to_texels = |c: F32s| (c * F32s::fill(scale) + F32s::fill(offset)).round_int();
    let (u, v) = (to_texels(u), to_texels(v));
    let row = level.width_log2 as i32;
    let base = face << (2 * row);
    let at_index = |i: I32s| {
        let i = i.to_array();
        U32s::from(std::array::from_fn::<u32, LANES, _>(|k| {
            level.texels[i[k] as usize]
        }))
    };
    let zero = I32s::fill(0);
    if method != filter::BILINEAR {
        // The texel whose square holds the point (half a texel on from its center), moved
        // first by the pixel's dither offset (in 1/65536 texels, so >> 8 for 8.8).
        let (du, dv) = if method == filter::DITHERED {
            let e: I32s = texel_dither(at) >> 8;
            (-e, e)
        } else {
            (zero, zero)
        };
        let half = I32s::fill(128);
        let (x, y): (I32s, I32s) = ((u + half + du) >> 8, (v + half + dv) >> 8);
        let (x, y) = (x.max(zero).min(last), y.max(zero).min(last));
        return at_index(base + (y << row) + x);
    }
    let (u, v) = (u.max(zero).min(max), v.max(zero).min(max));
    let (x0, y0): (I32s, I32s) = (u >> 8, v >> 8);
    let (x1, y1) = (
        (x0 + I32s::fill(1)).min(last),
        (y0 + I32s::fill(1)).min(last),
    );
    let (y0, y1) = (base + (y0 << row), base + (y1 << row));
    let byte = I32s::fill(255);
    let (fx, fy): (U32s, U32s) = (
        wide::bytemuck::cast(u & byte),
        wide::bytemuck::cast(v & byte),
    );
    let top = lerp_texels(at_index(y0 + x0), at_index(y0 + x1), fx);
    let bottom = lerp_texels(at_index(y1 + x0), at_index(y1 + x1), fx);
    lerp_texels(top, bottom, fy)
}

/// A cube map's ([`Texture::cube`]) color in each lane's direction `d` (any length), with
/// sampler `FILTER` (see [`filter`]) at level of detail `lod` (log2 of face texels per
/// pixel, the same for every lane); `at` is the lanes' screen pixels.
#[inline(always)]
pub fn sample_cube<const FILTER: u8>(tex: &Texture, d: [F32s; 3], lod: f32, at: Pixels) -> U32s {
    let (face, u, v) = cube_face_coords(d);
    let lod = I32s::fill((lod.clamp(-64.0, 64.0) * 256.0) as i32);
    let read = CubeRead::<FILTER> {
        tex,
        face,
        u,
        v,
        at,
    };
    mip_transition(filter::mip(FILTER), tex.levels.len(), lod, at, &read)
}

/// A cube map's level, read with sampler `FILTER`'s texel method.
struct CubeRead<'a, const FILTER: u8> {
    tex: &'a Texture,
    face: I32s,
    u: F32s,
    v: F32s,
    at: Pixels,
}

impl<const FILTER: u8> LevelRead for CubeRead<'_, FILTER> {
    #[inline(always)]
    fn read(&self, level: usize) -> U32s {
        cube_level(
            filter::method(FILTER),
            &self.tex.levels[level],
            self.face,
            self.u,
            self.v,
            self.at,
        )
    }
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

/// Most textures a polygon can bind (see [`TextureSet`]).
pub const MAX_TEXTURES: usize = 2;

/// The textures a polygon is drawn with, as its shader sees them: `[0]` is its main texture
/// (the one [`LOD`] and the other footprint varyings measure), `[1]` a second one for shaders
/// that want it (a height map, say). Unbound slots hold a 1x1 opaque white texture.
pub type TextureSet<'a> = [&'a Texture; MAX_TEXTURES];

/// A material's constants for one surface (F0, a fade range, ...), set by the app when it
/// picks the surface's material. What the engine knows (the eye, the object's transform)
/// reaches materials through their stage contexts instead.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Params {
    pub values: [f32; 16],
}

impl Params {
    /// `values` first, the rest 0.
    pub fn new(values: &[f32]) -> Self {
        let mut u = Self::default();
        u.values[..values.len()].copy_from_slice(values);
        u
    }
}

/// Element types of the three groups, and their [`LANES`]-wide vectors.
pub trait Elem: Copy + Default {
    type Lanes: Copy + Default + std::fmt::Debug;
    /// Converts a true (f32) value to this interpolation format.
    fn from_f32(v: f32) -> Self;
    /// `v` in every lane.
    fn splat(v: Self) -> Self::Lanes;
    /// Lane `j` holds `base + max(offset + j * stride, 0) * step` (`base` and `step` hold one
    /// value in every lane): the value that many pixels along a line from `base`, never
    /// before it (a half-rate pair can start a pixel before its interval). In fixed point
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
        let j = if offset < 0 { j.max(I32s::fill(0)) } else { j };
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
        let j = if offset < 0 { j.max(I16s::fill(0)) } else { j };
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
        let j = if offset < 0 {
            j.max(F32s::fill(0.0))
        } else {
            j
        };
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
    /// The lines with values `base` at an interval's first pixel and `step` per pixel (this
    /// group's, in layout order).
    fn line(base: &[f32], step: &[f32]) -> Self::Line;
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
    pub fn new(base: &[f32], step: &[f32]) -> Self {
        let mut line = Self {
            base: [E::Lanes::default(); N],
            step: [E::Lanes::default(); N],
        };
        for k in 0..N {
            line.base[k] = E::splat(E::from_f32(base[k]));
            line.step[k] = E::splat(E::from_f32(step[k]));
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
            fn line(base: &[f32], step: &[f32]) -> Self::Line {
                $crate::shader::Line::new(base, step)
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

/// Declares a struct of f32 arrays (one per named value) and its [`Values`] impl. Used by
/// [`material_io!`](crate::material_io).
#[macro_export]
macro_rules! value_struct {
    ($name:ident; $($f:ident: $n:literal),* $(,)?) => {
        #[derive(Clone, Copy, Default, Debug, PartialEq)]
        pub struct $name {
            $(pub $f: [f32; $n]),*
        }

        impl $crate::shader::Values for $name {
            const LEN: usize = 0 $(+ $n)*;

            #[inline(always)]
            #[allow(unused_variables, unused_mut, unused_assignments)]
            fn from_flat(flat: &[f32]) -> Self {
                let mut i = 0;
                Self {
                    $($f: {
                        let a: [f32; $n] = std::array::from_fn(|k| flat[i + k]);
                        i += $n;
                        a
                    }),*
                }
            }

            #[inline(always)]
            #[allow(unused_variables, unused_mut, unused_assignments)]
            fn write_flat(&self, out: &mut [f32]) {
                let mut i = 0;
                $(
                    out[i..i + $n].copy_from_slice(&self.$f);
                    i += $n;
                )*
            }
        }
    };
}

/// Declares a struct of lane arrays (one per named value, [`LANES`] sample points in each)
/// and its [`LaneValues`] impl. Used by [`material_io!`](crate::material_io).
#[macro_export]
macro_rules! lane_struct {
    ($name:ident; $($f:ident: $n:literal),* $(,)?) => {
        #[derive(Clone, Copy, Default, Debug, PartialEq)]
        pub struct $name {
            $(pub $f: [$crate::shader::F32s; $n]),*
        }

        impl $crate::shader::LaneValues for $name {
            const LEN: usize = 0 $(+ $n)*;

            #[inline(always)]
            #[allow(unused_variables, unused_mut, unused_assignments)]
            fn from_lanes(lanes: &[$crate::shader::F32s]) -> Self {
                let mut i = 0;
                Self {
                    $($f: {
                        let a: [$crate::shader::F32s; $n] = std::array::from_fn(|k| lanes[i + k]);
                        i += $n;
                        a
                    }),*
                }
            }

            #[inline(always)]
            #[allow(unused_variables, unused_mut, unused_assignments)]
            fn write_lanes(&self, out: &mut [$crate::shader::F32s]) {
                let mut i = 0;
                $(
                    out[i..i + $n].copy_from_slice(&self.$f);
                    i += $n;
                )*
            }
        }
    };
}

/// Declares what flows between a material's stages, all from one list, and the
/// [`MaterialIo`] `IO` describing it:
///
/// - `vertex`: what `shade_vertex` reads: mesh attributes by name, or the built-ins
///   [`POSITION`] and [`FACE_NORMAL`]. Struct `Vertex`.
/// - `sampled`: what `shade_vertex` returns (struct `Sampled`) and `shade_sample` reads,
///   exact at each sample point ([`LANES`] points in struct `SampledLanes`). The built-ins
///   [`POSITION`] and [`LOD`] are filled by the engine.
/// - `fixed32`, `fixed16`, `float`: what `shade_sample` returns (struct `Interp`, in this
///   order) and pixels get, interpolated linearly between sample points in 16.16, 8.8 and
///   f32: the groups `Fixed32`, `Fixed16` and `Floats`.
///
/// ```ignore
/// material_io! {
///     vertex { color: 3 }
///     sampled { color: 3 }
///     fixed32 {}
///     fixed16 { color: 3 }
///     float {}
/// }
/// ```
#[macro_export]
macro_rules! material_io {
    (
        vertex { $($v:ident: $vn:literal),* $(,)? }
        sampled { $($s:ident: $sn:literal),* $(,)? }
        fixed32 { $($a:ident: $an:literal),* $(,)? }
        fixed16 { $($b:ident: $bn:literal),* $(,)? }
        float { $($c:ident: $cn:literal),* $(,)? }
    ) => {
        $crate::value_struct!(Vertex; $($v: $vn),*);
        $crate::value_struct!(Sampled; $($s: $sn),*);
        $crate::lane_struct!(SampledLanes; $($s: $sn),*);
        $crate::lane_struct!(Interp; $($a: $an,)* $($b: $bn,)* $($c: $cn),*);
        $crate::varying_group!(Fixed32, i32; $($a: $an),*);
        $crate::varying_group!(Fixed16, i16; $($b: $bn),*);
        $crate::varying_group!(Floats, f32; $($c: $cn),*);

        pub const IO: $crate::shader::MaterialIo = $crate::shader::MaterialIo {
            vertex: &[
                $($crate::shader::AttribDesc { name: stringify!($v), format: $crate::shader::Format::Float, count: $vn },)*
            ],
            sampled: &[
                $($crate::shader::AttribDesc { name: stringify!($s), format: $crate::shader::Format::Float, count: $sn },)*
            ],
            interp: &[
                $($crate::shader::AttribDesc { name: stringify!($a), format: $crate::shader::Format::Fixed32, count: $an },)*
                $($crate::shader::AttribDesc { name: stringify!($b), format: $crate::shader::Format::Fixed16, count: $bn },)*
                $($crate::shader::AttribDesc { name: stringify!($c), format: $crate::shader::Format::Float, count: $cn },)*
            ],
        };
    };
}

/// The associated types and `IO` of a [`Material`] impl, from the names
/// [`material_io!`](crate::material_io) declares in the same module.
#[macro_export]
macro_rules! material_types {
    () => {
        type Vertex = Vertex;
        type Sampled = Sampled;
        type SampledLanes = SampledLanes;
        type Interp = Interp;
        type Fixed32 = Fixed32;
        type Fixed16 = Fixed16;
        type Floats = Floats;
        const IO: $crate::shader::MaterialIo = IO;
    };
}

/// Name of the built-in world position (three values): as a `vertex` value, the source
/// vertex's; as a `sampled` value, the point's (from `ViewGeometry::world_positions`: the
/// real surface point, even in a mirror). A mesh attribute cannot supply it.
pub const POSITION: &str = "position";

/// Name of the built-in `vertex` value holding the source polygon's plane normal in world
/// space (three values). A mesh attribute cannot supply it.
pub const FACE_NORMAL: &str = "face_normal";

/// Name of the `sampled` value the level of detail built-in ([`LOD`]) is measured from:
/// two texture coordinates for texture slot 0.
pub const UV: &str = "uv";

/// Name of the built-in `sampled` value holding the texture level of detail (one value,
/// usually passed on to an 8.8 varying): log2 of the polygon's texture's full-size texels
/// per screen pixel. It is computed for each vertex when the polygon is set up, from the
/// polygon's exact screen-space derivatives of the `sampled` value [`UV`], and
/// interpolated across it like any value; see [`sample_texture`].
pub const LOD: &str = "lod";

/// A material's values of one kind as a struct of f32 arrays, generated by
/// [`material_io!`](crate::material_io): `Vertex` and `Sampled`.
pub trait Values: Copy + Default {
    /// Number of f32 values.
    const LEN: usize;
    /// From `LEN` values in declaration order.
    fn from_flat(flat: &[f32]) -> Self;
    fn write_flat(&self, out: &mut [f32]);
}

/// A material's values of one kind for [`LANES`] sample points, generated by
/// [`material_io!`](crate::material_io): `SampledLanes` and `Interp`.
pub trait LaneValues: Copy + Default {
    /// Number of values (each [`LANES`] wide).
    const LEN: usize;
    fn from_lanes(lanes: &[F32s]) -> Self;
    fn write_lanes(&self, out: &mut [F32s]);
}

/// What flows between a material's stages, by name: see [`material_io!`](crate::material_io).
#[derive(Clone, Copy, Debug)]
pub struct MaterialIo {
    pub vertex: &'static [AttribDesc],
    pub sampled: &'static [AttribDesc],
    pub interp: &'static [AttribDesc],
}

/// What `shade_vertex` sees besides the vertex: nothing about the eye (vertex outputs are
/// the same seen directly and in every mirror).
pub struct VertexContext<'a> {
    /// Where the mesh is in the world ([`Object::IDENTITY`] for level geometry).
    pub object: &'a Object,
    pub params: &'a Params,
}

/// What `shade_sample` sees besides its sample points.
pub struct SampleContext<'a> {
    /// The eye the polygon is seen from: the camera, or for a polygon seen in a mirror, the
    /// mirror's eye.
    pub eye: Vec3,
    /// Pixels per unit of height at depth 1.
    pub focal: f32,
    pub object: &'a Object,
    pub params: &'a Params,
    /// The lights that reach the polygon (those in range of it and in front of it), and
    /// the light that reaches everything (linear RGB).
    pub lights: &'a [Light],
    pub ambient: Vec3,
    /// Per light, [`NO_SPLIT`], or for a light whose shadow covers part of the polygon,
    /// which of `split` gets its strength instead of the total getting its light: the
    /// engine adds its light (its color times that) back pixel by pixel, as much as the
    /// shadow there lets through (see [`PixelContext::light`]).
    pub light_split: &'a [u8],
    pub split: &'a [Cell<F32s>],
    /// Where there are split lights: all the light, theirs included (linear), for pixels
    /// all of theirs reaches.
    pub total: &'a Cell<[F32s; 3]>,
}

/// Outputs the engine adds to a polygon's sample points for its `splits` split lights:
/// each one's strength (its light is its color times that), then all the light.
pub const fn split_outputs(splits: usize) -> usize {
    if splits == 0 { 0 } else { splits + 3 }
}

/// In `SampleContext::light_split`: the light is added to the total.
pub const NO_SPLIT: u8 = u8::MAX;

/// Most lights of one polygon whose shadows are applied pixel by pixel (lights whose
/// shadow covers part of it; each adds three values to its sample points' outputs).
pub const MAX_SPLIT: usize = 4;

/// The lights whose shadows cover part of a polygon, at [`LANES`] pixels: each one's light
/// (linear) and how much of it reaches each pixel (0 to 1).
#[derive(Clone, Copy, Default)]
pub struct Split {
    pub count: usize,
    pub light: [[F32s; 3]; MAX_SPLIT],
    pub reaches: [F32s; MAX_SPLIT],
    /// All the light, theirs included, encoded like a `light` output (1 being 1).
    pub total: [F32s; 3],
    /// Blend partly shadowed pixels by the fast approximation (see
    /// [`PixelContext::light`]).
    pub fast: bool,
}

/// What `shade_pixel` sees besides its interpolated values.
pub struct PixelContext<'a> {
    /// The screen position of the [`LANES`] pixels.
    pub at: Pixels,
    pub textures: &'a TextureSet<'a>,
    pub params: &'a Params,
    pub focal: f32,
    /// Lights whose shadows cover part of the polygon; see [`PixelContext::light`].
    pub split: Split,
}

impl PixelContext<'_> {
    /// The light at the pixels: a `light` output (16.16, gamma-encoded, from the lights
    /// the sample stage added up) with the lights whose shadows cover part of the polygon
    /// added back, each as much as its shadow lets through at each pixel. Added in linear
    /// terms: decoded, summed with them, and encoded again.
    #[inline(always)]
    pub fn light(&self, light: &[I32s; 3]) -> [I32s; 3] {
        if self.split.count == 0 {
            return *light;
        }
        // Where all of every split light reaches, all the light, as worked out at the
        // sample points.
        let all = (0..self.split.count)
            .fold(F32s::fill(1.0), |m, j| m.min(self.split.reaches[j]));
        if all.simd_ge(F32s::fill(1.0)).all() {
            return self.split.total.map(|t| (t * F32s::fill(65536.0)).round_int());
        }
        // Where none of any reaches (outside a beam), the rest of the light.
        let any = (0..self.split.count)
            .fold(F32s::fill(0.0), |m, j| m.max(self.split.reaches[j]));
        if any.simd_le(F32s::fill(0.0)).all() {
            return *light;
        }
        // Where each pixel gets all of every split light or none of any (at a hard beam's
        // edge, or a dithered one's fade), all the light or the rest, pixel by pixel.
        let (full, none) = (all.simd_ge(F32s::fill(1.0)), any.simd_le(F32s::fill(0.0)));
        if (full | none).all() {
            return std::array::from_fn(|c| {
                let total = self.split.total[c] * F32s::fill(65536.0);
                wide::Select::select(full, total, light[c].round_float()).round_int()
            });
        }
        let scale = F32s::fill(1.0 / 65536.0);
        if self.split.fast {
            // Between the two ends, the rest `e0` and all of it `e1` (both encoded, exact),
            // in gamma-2 terms: `sqrt(e0^2 + (e1^2 - e0^2) f)`, where `f` is how much of the
            // split lights' light reaches (their light, as much as reaches of each, over
            // all of it). Exact where all of it reaches and where none does.
            return std::array::from_fn(|c| {
                let (mut all, mut reaching) = (F32s::fill(0.0), F32s::fill(0.0));
                for j in 0..self.split.count {
                    all += self.split.light[j][c];
                    reaching += self.split.reaches[j] * self.split.light[j][c];
                }
                let f = reaching / all.max(F32s::fill(1e-12));
                let e0 = light[c].round_float() * scale;
                let e1 = self.split.total[c];
                let e = (e0 * e0 + (e1 * e1 - e0 * e0) * f).max(F32s::fill(0.0)).sqrt();
                (e * F32s::fill(65536.0)).round_int()
            });
        }
        std::array::from_fn(|c| {
            let mut sum = crate::shaders::decode_light(light[c].round_float() * scale);
            for j in 0..self.split.count {
                sum += self.split.reaches[j] * self.split.light[j][c];
            }
            (crate::shaders::encode_light(sum) * F32s::fill(65536.0)).round_int()
        })
    }
}

/// What a translucent pixel is drawn over, for `shade_over`: `w` is the surface's
/// `1 / depth` at each pixel, `behind_w` that of the opaque surface behind it (both on the
/// same ray through the pixel, so Euclidean distances along it scale as `1 / w`); lanes past
/// the end of a run have `behind_w` 0. `row` holds the finished colors and `w` the run is
/// drawn over, for materials that blend over them themselves (returning alpha 255), reading
/// them somewhere else along the row.
pub struct Over<'a> {
    pub w: F32s,
    pub behind_w: F32s,
    pub row: RowBehind<'a>,
}

/// A material program: three stages, and what flows between them (declared with
/// [`material_io!`](crate::material_io), filled in with
/// [`material_types!`](crate::material_types)).
///
/// - `shade_vertex` runs on each vertex of a source polygon, before clipping; clipped
///   vertices get the same weighted sums of its outputs that clipping made of positions.
/// - `shade_sample` runs on exact, perspective-correct `sampled` values at sample points,
///   [`LANES`] points at once: the place for anything costly or not linear on screen (view
///   vectors, Fresnel, lighting). Its outputs are interpolated linearly to pixels.
/// - `shade_pixel` runs on [`LANES`] consecutive pixels (lanes past the end of a run are
///   computed too, and discarded) and returns their colors.
///
/// Opaque materials return XRGB (the top byte is ignored). Translucent ones
/// (`TRANSLUCENT = true`) return ARGB: they are drawn after all opaque geometry, back to
/// front, and blended as `src * a + dst * (1 - a)`. A translucent material with
/// `READS_BEHIND = true` is shaded with `shade_over`, which also sees what is behind.
pub trait Material: 'static {
    type Vertex: Values;
    type Sampled: Values;
    type SampledLanes: LaneValues;
    type Interp: LaneValues;
    type Fixed32: Group<Elem = i32>;
    type Fixed16: Group<Elem = i16>;
    type Floats: Group<Elem = f32>;
    const IO: MaterialIo;
    /// Most pixels between sample points this material wants, in both directions;
    /// perspective may ask for less.
    const SAMPLE_SPACING: i32 = MAX_STEP;
    const TRANSLUCENT: bool = false;
    /// Shade with `shade_over`. Translucent materials only.
    const READS_BEHIND: bool = false;

    fn shade_vertex(v: &Self::Vertex, ctx: &VertexContext) -> Self::Sampled;

    fn shade_sample(s: &Self::SampledLanes, ctx: &SampleContext) -> Self::Interp;

    fn shade_pixel(
        a: &Self::Fixed32,
        b: &Self::Fixed16,
        c: &Self::Floats,
        ctx: &PixelContext,
    ) -> U32s;

    #[inline(always)]
    fn shade_over(
        a: &Self::Fixed32,
        b: &Self::Fixed16,
        c: &Self::Floats,
        ctx: &PixelContext,
        _over: &Over,
    ) -> U32s {
        Self::shade_pixel(a, b, c, ctx)
    }
}

/// Most values (as f32) of one kind any material may declare.
pub const MAX_VARYINGS: usize = 32;

/// Everything needed to shade one run of pixels of one polygon on one row, its values
/// already worked out at the row's sample points.
pub struct SpanJob<'a> {
    /// The polygon's exact left and right edge crossings on this row, and w there.
    pub x_left: f32,
    pub x_right: f32,
    pub w_left: f32,
    pub w_right: f32,
    /// The row's sample points covering the run, left to right: their screen x (pixel
    /// centers) and their outputs, output by output (`outs[k * xs.len() + i]` is output `k`
    /// of point `i`, declaration order). A pixel is shaded from the point at or before its
    /// center and the next one; the last point shades only its own pixel.
    pub xs: &'a [f32],
    pub outs: &'a [f32],
    /// First pixel of the run; the run covers `x0..x0 + color.len()`, inside the row.
    pub x0: i32,
    /// Lights whose shadows cover part of the polygon: their strength (1 value each, their
    /// light being their color, `split_colors`, times it) follows the material's outputs at
    /// every point in `outs`, then all the light (3 values, encoded, theirs included), and
    /// how much of each reaches every pixel of the run is in `reaches` (run by run:
    /// `reaches[j * len + i]` for light `j` at pixel `x0 + i`).
    pub splits: usize,
    pub split_colors: [Vec3; MAX_SPLIT],
    /// Blend partly shadowed pixels by the fast approximation (`RasterConfig::fast_blend`).
    pub fast_blend: bool,
    pub reaches: &'a [f32],
    /// The framebuffer row.
    pub row: i32,
    /// Shade every other pixel and repeat it in the next: set for polygons seen in a mirror.
    /// Pairs start at even pixels counted from `vx`, the viewport's left edge, wherever
    /// the sample points are.
    pub half_rate: bool,
    pub vx: i32,
}

/// What a translucent run is drawn over, pixel for pixel (`x0..x0 + len` of the run): the
/// opaque `w` and the finished color. Empty for opaque runs.
#[derive(Clone, Copy, Default)]
pub struct Behind<'a> {
    pub w: &'a [f32],
    pub colors: &'a [u32],
}

/// What a translucent run is drawn over, for reading along the row (see
/// [`Shader::shade_over`]): pixel `x0 + i` has the finished color `colors[i]` and the opaque
/// `w[i]` (0 over the background).
#[derive(Clone, Copy)]
pub struct RowBehind<'a> {
    colors: &'a [u32],
    w: &'a [f32],
    x0: i32,
}

impl<'a> RowBehind<'a> {
    /// `colors` and `w` of the same pixels, from `x0`.
    pub fn new(colors: &'a [u32], w: &'a [f32], x0: i32) -> Self {
        debug_assert_eq!(colors.len(), w.len());
        Self { colors, w, x0 }
    }

    /// Nothing: [`at`](Self::at) reads 0.
    pub const NONE: RowBehind<'static> = RowBehind {
        colors: &[],
        w: &[],
        x0: 0,
    };

    /// Each lane's color and opaque `w` at pixel `x`, read together so they always agree,
    /// clamped to the run, so a read never reaches the pixels of surfaces in front of it.
    #[inline(always)]
    pub fn at(&self, x: I32s) -> (U32s, F32s) {
        let Some(last) = self.colors.len().checked_sub(1) else {
            return (U32s::fill(0), F32s::fill(0.0));
        };
        let i = (x - I32s::fill(self.x0))
            .max(I32s::fill(0))
            .min(I32s::fill(last as i32))
            .to_array();
        (
            U32s::from(std::array::from_fn::<u32, LANES, _>(|k| {
                self.colors[i[k] as usize]
            })),
            F32s::from(std::array::from_fn::<f32, LANES, _>(|k| {
                self.w[i[k] as usize]
            })),
        )
    }
}

/// What a run is drawn with: the surface's params and textures, and the engine's values
/// for its stage contexts.
pub struct Draw<'a> {
    pub params: &'a Params,
    pub textures: &'a TextureSet<'a>,
    pub eye: Vec3,
    pub focal: f32,
    pub object: &'a Object,
}

/// Shades a run into `color`, over `behind` for translucent materials.
pub type DrawSpanFn = fn(job: &SpanJob, color: &mut [u32], behind: Behind, draw: &Draw);

/// Runs `shade_vertex` on one vertex: `input` holds its `vertex` values, `out` gets its
/// `sampled` values, both in declaration order.
pub type VertexFn = fn(input: &[f32], ctx: &VertexContext, out: &mut [f32]);

/// A registered material, type-erased.
#[derive(Clone, Copy)]
pub struct MaterialEntry {
    pub io: MaterialIo,
    pub vertex: VertexFn,
    pub sample: SampleFn,
    pub draw_span: DrawSpanFn,
    /// `draw_span` for half-rate spans (see [`SpanJob::half_rate`]).
    pub draw_span_half: DrawSpanFn,
    pub translucent: bool,
    pub reads_behind: bool,
    pub sample_spacing: i32,
}

impl MaterialEntry {
    pub fn of<M: Material>() -> Self {
        const {
            assert!(
                M::TRANSLUCENT || !M::READS_BEHIND,
                "only translucent materials can read what is behind them"
            )
        };
        Self {
            io: M::IO,
            vertex: vertex_stage::<M>,
            sample: sample_stage::<M>,
            draw_span: span::<M, 1>,
            draw_span_half: span::<M, 2>,
            translucent: M::TRANSLUCENT,
            reads_behind: M::READS_BEHIND,
            sample_spacing: M::SAMPLE_SPACING,
        }
    }
}

fn vertex_stage<M: Material>(input: &[f32], ctx: &VertexContext, out: &mut [f32]) {
    M::shade_vertex(&M::Vertex::from_flat(input), ctx).write_flat(out);
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

/// Handle to a registered material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaterialId(pub u16);

/// Number of f32 values in a layout.
pub fn layout_len(layout: &[AttribDesc]) -> usize {
    layout.iter().map(|a| a.count as usize).sum()
}

/// Largest sample interval: 8.8 values reset at least every 32 pixels, keeping their
/// accumulated stepping error under one level.
pub const MAX_STEP: i32 = 32;

/// Shades a run of a polygon's row from the row's sample points (see [`SpanJob`]): between
/// two points every value is linear in x, so each interval's pixels are shaded in blocks of
/// [`LANES`] counted from its first pixel, each block's values evaluated at all its pixels
/// at once (`base + offset * step`, in each group's own format), then shaded at once. The
/// points belong to the whole row, so every pixel gets the same value however the row is
/// split into runs.
pub fn draw_span<M: Material>(job: &SpanJob, color: &mut [u32], behind: Behind, draw: &Draw) {
    if job.half_rate {
        span::<M, 2>(job, color, behind, draw);
    } else {
        span::<M, 1>(job, color, behind, draw);
    }
}

/// [`draw_span`], shading every `STRIDE`th pixel counted from the viewport's left edge
/// (each block's [`LANES`] lanes cover `LANES * STRIDE` pixels, every lane's color repeated
/// `STRIDE` times, from the interval holding the pixel it is repeated in).
pub fn span<M: Material, const STRIDE: i32>(
    job: &SpanJob,
    color: &mut [u32],
    behind: Behind,
    draw: &Draw,
) {
    let (x0, x1) = (job.x0, job.x0 + color.len() as i32);
    let n_material = M::Interp::LEN;
    let n_out = n_material + split_outputs(job.splits);
    let n = job.xs.len();
    debug_assert_eq!(job.outs.len(), n * n_out);
    let mut run = Run {
        job,
        x0,
        color,
        behind,
        draw,
    };
    let (mut base, mut step) = ([0.0f32; MAX_VARYINGS], [0.0f32; MAX_VARYINGS]);
    // A half-rate pair shows its first pixel's values, or the row's first pixel's where the
    // pair starts before the row. An interval shades the pixels whose shown pixel it holds:
    // from the first shown at or after `a`.
    let row_first = moose_view::pixel_edge(job.x_left);
    let pixels_from = |a: i32| {
        if a <= row_first {
            row_first
        } else {
            a + (a - job.vx).rem_euclid(STRIDE) * (STRIDE - 1)
        }
    };
    for i in 0..n {
        let xa = job.xs[i];
        // The pixels showing pixels whose centers lie in [xa, xb), or the last point's own.
        let first = moose_view::pixel_edge(xa);
        let (end, slope) = match job.xs.get(i + 1) {
            Some(&xb) => (moose_view::pixel_edge(xb), 1.0 / (xb - xa)),
            None => (first + 1, 0.0),
        };
        let (from, to) = (pixels_from(first).max(x0), pixels_from(end).min(x1));
        if from >= to {
            if pixels_from(first) >= x1 {
                break;
            }
            continue;
        }
        let start = first.max(row_first);
        let offset = start as f32 + 0.5 - xa;
        for k in 0..n_out {
            let a = job.outs[k * n + i];
            step[k] = if i + 1 < n {
                (job.outs[k * n + i + 1] - a) * slope
            } else {
                0.0
            };
            base[k] = a + step[k] * offset;
        }
        let lines = lines::<M>(&base[..n_material], &step[..n_material]);
        let split = (&base[n_material..n_out], &step[n_material..n_out]);
        run.interval::<M, STRIDE>(&lines, split, start, from, to);
    }
}

/// Runs `shade_sample` on [`LANES`] sample points: `inputs` holds their `sampled` values
/// (one lane array per value, declaration order), `out` gets their outputs (`Interp`, one
/// lane array per output).
pub type SampleFn = fn(inputs: &[F32s], ctx: &SampleContext, out: &mut [F32s]);

fn sample_stage<M: Material>(inputs: &[F32s], ctx: &SampleContext, out: &mut [F32s]) {
    M::shade_sample(&M::SampledLanes::from_lanes(inputs), ctx).write_lanes(out);
}

/// The lines of all three groups within one sample interval.
type Lines<M> = (
    <<M as Material>::Fixed32 as Group>::Line,
    <<M as Material>::Fixed16 as Group>::Line,
    <<M as Material>::Floats as Group>::Line,
);

/// The lines with values `base` at the interval's first pixel and `step` per pixel
/// (declaration order).
#[inline(always)]
fn lines<M: Material>(base: &[f32], step: &[f32]) -> Lines<M> {
    #[inline(always)]
    fn split(v: &[f32], f32s: usize, f16s: usize) -> (&[f32], &[f32], &[f32]) {
        (&v[..f32s], &v[f32s..f32s + f16s], &v[f32s + f16s..])
    }
    let (f32s, f16s) = (M::Fixed32::LEN, M::Fixed16::LEN);
    let ((b32, b16, bf), (s32, s16, sf)) = (split(base, f32s, f16s), split(step, f32s, f16s));
    (
        M::Fixed32::line(b32, s32),
        M::Fixed16::line(b16, s16),
        M::Floats::line(bf, sf),
    )
}

/// One run being shaded: its pixels `x0..x0 + color.len()`.
struct Run<'a> {
    job: &'a SpanJob<'a>,
    x0: i32,
    color: &'a mut [u32],
    behind: Behind<'a>,
    draw: &'a Draw<'a>,
}

impl Run<'_> {
    /// Shades pixels `from..to` of the interval whose first pixel is `start` (where
    /// `lines` begin), in blocks of `LANES * STRIDE` pixels, the first at the pair holding
    /// `from`.
    #[inline(always)]
    fn interval<M: Material, const STRIDE: i32>(
        &mut self,
        lines: &Lines<M>,
        split: (&[f32], &[f32]),
        start: i32,
        from: i32,
        to: i32,
    ) {
        let width = LANES as i32 * STRIDE;
        let mut block = from - (from - self.job.vx).rem_euclid(STRIDE);
        while block < to {
            let pixels = block.max(from)..(block + width).min(to);
            self.block::<M, STRIDE>(lines, split, block - start, block, pixels);
            block += width;
        }
    }

    /// Shades the block of `LANES * STRIDE` pixels starting at pixel `first`, `offset`
    /// pixels along `lines` (one lane every `STRIDE` pixels), and stores the ones in
    /// `pixels`.
    #[inline(always)]
    fn block<M: Material, const STRIDE: i32>(
        &mut self,
        lines: &Lines<M>,
        (split_base, split_step): (&[f32], &[f32]),
        offset: i32,
        first: i32,
        pixels: Range<i32>,
    ) {
        let ga = M::Fixed32::lanes(&lines.0, offset, STRIDE);
        let gb = M::Fixed16::lanes(&lines.1, offset, STRIDE);
        let gc = M::Floats::lanes(&lines.2, offset, STRIDE);
        // The split lights at the pixels, and how much of each reaches them (each lane's
        // pixel, clamped to the run).
        let mut split = Split {
            count: self.job.splits,
            fast: self.job.fast_blend,
            ..Split::default()
        };
        let len = self.color.len();
        for j in 0..split.count {
            let strength = f32::lanes(F32s::fill(split_base[j]), F32s::fill(split_step[j]), offset, STRIDE);
            let color = self.job.split_colors[j].to_array();
            split.light[j] = color.map(|c| F32s::fill(c) * strength);
            let reaches = &self.job.reaches[j * len..(j + 1) * len];
            let start = first - self.x0;
            split.reaches[j] = if STRIDE == 1 && start >= 0 && start as usize + LANES <= len {
                // Side by side and all in the run: read at once.
                let at = start as usize;
                F32s::from(<[f32; LANES]>::try_from(&reaches[at..at + LANES]).unwrap())
            } else {
                F32s::from(std::array::from_fn::<f32, LANES, _>(|i| {
                    let px = start + i as i32 * STRIDE;
                    reaches[px.clamp(0, len as i32 - 1) as usize]
                }))
            };
        }
        if split.count > 0 {
            let t = split.count;
            split.total = std::array::from_fn(|c| {
                f32::lanes(F32s::fill(split_base[t + c]), F32s::fill(split_step[t + c]), offset, STRIDE)
            });
        }
        let ctx = PixelContext {
            at: Pixels {
                x: first,
                y: self.job.row,
                stride: STRIDE,
            },
            textures: self.draw.textures,
            params: self.draw.params,
            focal: self.draw.focal,
            split,
        };
        let out = if M::READS_BEHIND {
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
                    *b = self.behind.w[(q - self.x0) as usize];
                }
            }
            let over = Over {
                w,
                behind_w: F32s::from(behind),
                row: RowBehind::new(self.behind.colors, self.behind.w, self.x0),
            };
            M::shade_over(&ga, &gb, &gc, &ctx, &over)
        } else {
            M::shade_pixel(&ga, &gb, &gc, &ctx)
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
    use moose_view::pixel_edge;

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
            let d = [lane(0), lane(1), lane(2)];
            for got in [
                sample_cube::<{ filter::BILINEAR_MIPMAP_LINEAR }>(&tex, d, 0.0, PIXELS),
                sample_cube::<{ filter::NEAREST_MIPMAP_NONE }>(&tex, d, 0.0, PIXELS),
            ] {
                for (i, &(d, want)) in chunk.iter().enumerate() {
                    assert_eq!(got.to_array()[i], want, "direction {d:?}");
                }
            }
        }
    }

    #[test]
    fn dithered_cube_texels_move_by_the_pattern() {
        // Through each texel's center, the dither moves the point by -e in u and +e in v
        // (e from -1/2 to 7/16 of a texel): v stays in the texel, and u crosses into the
        // next one only where e is -1/2 (clamped at the face's edge).
        let size = 4;
        let faces: [Vec<u32>; 6] = std::array::from_fn(|f| {
            (0..size * size)
                .map(|i| ((f as u32) << 20) | (i << 4))
                .collect()
        });
        let tex = Texture::cube("cube", size, faces).unwrap();
        for face in 0..6 {
            for ty in 0..size {
                for tx in 0..size {
                    let d = cube_texel_direction(face, tx, ty, size).map(F32s::fill);
                    for y in 0..4 {
                        let at = Pixels { x: 0, y, stride: 1 };
                        let got = sample_cube::<{ filter::DITHERED_MIPMAP_NONE }>(&tex, d, 0.0, at)
                            .to_array();
                        for (i, &g) in got.iter().enumerate() {
                            let e = TEXEL_DITHER[i & 3][y as usize];
                            let x = if e == -32768 { (tx + 1).min(size - 1) } else { tx };
                            let want = ((face as u32) << 20) | ((ty * size + x) << 4);
                            assert_eq!(g, want, "face {face} texel ({tx}, {ty}) pixel ({i}, {y})");
                        }
                    }
                }
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
        let linear = |lod| sample_cube::<{ filter::BILINEAR_MIPMAP_LINEAR }>(&tex, d, lod, PIXELS);
        assert_eq!(linear(0.0), U32s::fill(0));
        assert_eq!(linear(0.5), U32s::fill(0x003F_3F3F));
        assert_eq!(linear(9.0), U32s::fill(0x007F_7F7F));
    }

    /// Test shader writing its raw fixed-point varyings as the color: one 8.8 value in the
    /// low half, the low 16 bits of one 16.16 value in the high half.
    mod raw {
        use super::super::{
            Fill, I32s, Material, PixelContext, SampleContext, U32s, VertexContext,
        };
        crate::material_io! {
            vertex {}
            sampled { u: 1, c: 1 }
            fixed32 { u: 1 }
            fixed16 { c: 1 }
            float {}
        }
        pub struct Raw;
        impl Material for Raw {
            crate::material_types!();
            fn shade_vertex(_: &Vertex, _: &VertexContext) -> Sampled {
                Sampled::default()
            }
            fn shade_sample(s: &SampledLanes, _: &SampleContext) -> Interp {
                Interp { u: s.u, c: s.c }
            }
            fn shade_pixel(a: &Fixed32, b: &Fixed16, _: &Floats, _: &PixelContext) -> U32s {
                let c: U32s = wide::bytemuck::cast(I32s::from_i16x8(b.c[0]));
                let u: U32s = wide::bytemuck::cast(a.u[0]);
                (c & U32s::fill(0xFFFF)) | (u << 16)
            }
        }
    }

    /// One pixel's value, the way the span shading works it out before SIMD: it shows its
    /// own pixel, or at half rate the even one of its pair (counted from pixel 0, but not
    /// before the row's first pixel); from the point at or before that one (outputs stored
    /// output by output), the step to the next point per pixel in fixed point (none after
    /// the last), stepped there with wrapping arithmetic.
    fn stepped(xs: &[f32], outs: &[f32], half_rate: bool, row_first: i32, px: i32) -> u32 {
        let n = xs.len();
        let shown = if half_rate {
            (px - px.rem_euclid(2)).max(row_first)
        } else {
            px
        };
        let i = (0..n)
            .rev()
            .find(|&i| pixel_edge(xs[i]) <= shown)
            .unwrap();
        let xa = xs[i];
        let first = pixel_edge(xa);
        let a = [outs[i], outs[n + i]];
        let step = match xs.get(i + 1) {
            Some(&xb) => {
                let slope = 1.0 / (xb - xa);
                [(outs[i + 1] - a[0]) * slope, (outs[n + i + 1] - a[1]) * slope]
            }
            None => [0.0, 0.0],
        };
        let start = first.max(row_first);
        let offset = start as f32 + 0.5 - xa;
        let base = [a[0] + step[0] * offset, a[1] + step[1] * offset];
        let (du, dc) = (i32::from_f32(step[0]), i16::from_f32(step[1]));
        let k = (shown - start).max(0);
        let u = i32::from_f32(base[0]).wrapping_add(du.wrapping_mul(k));
        let c = i16::from_f32(base[1]).wrapping_add(dc.wrapping_mul(k as i16));
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
        let blank = Texture::solid("blank", 0);
        for round in 0..4000 {
            // A row's pixels and the grid columns covering them (from the column at or
            // before its first pixel to the one at or after its last).
            let row_x0 = rnd(0.0, 50.0) as i32;
            let row_x1 = row_x0 + 1 + rnd(0.0, 200.0) as i32;
            let nx = [2, 4, 8, 16, 32][rnd(0.0, 4.99) as usize];
            let c0 = row_x0.div_euclid(nx) * nx;
            let c1 = (row_x1 - 1 + nx - 1).div_euclid(nx) * nx;
            let xs: Vec<f32> = (c0..=c1)
                .step_by(nx as usize)
                .map(|c| c as f32 + 0.5)
                .collect();
            let n = xs.len();
            let outs: Vec<f32> = (0..2 * n)
                .map(|k| if k < n { rnd(-3.0, 3.0) } else { rnd(0.0, 255.0) })
                .collect();
            // Half the rows at half rate: each pixel then shows its pair's even pixel.
            let half_rate = round % 2 == 1;
            // The row split into random runs, as visibility would, each given only the
            // points around it.
            let mut x = row_x0;
            while x < row_x1 {
                let end = (x + 1 + rnd(0.0, 40.0) as i32).min(row_x1);
                let shown = if half_rate { (x - x % 2).max(row_x0) } else { x };
                let (i0, i1) = (((shown - c0) / nx) as usize, ((end - 1 - c0 + nx - 1) / nx) as usize);
                let window_outs: Vec<f32> = (0..2)
                    .flat_map(|k| outs[k * n + i0..=k * n + i1].to_vec())
                    .collect();
                let job = SpanJob {
                    x_left: row_x0 as f32,
                    x_right: row_x1 as f32,
                    w_left: 1.0,
                    w_right: 1.0,
                    xs: &xs[i0..=i1],
                    outs: &window_outs,
                    x0: x,
                    splits: 0,
                    split_colors: [Vec3::ZERO; MAX_SPLIT],
                    fast_blend: false,
                    reaches: &[],
                    row: 0,
                    half_rate,
                    vx: 0,
                };
                let mut color = vec![0u32; (end - x) as usize];
                draw_span::<raw::Raw>(
                    &job,
                    &mut color,
                    Behind::default(),
                    &Draw {
                        params: &Params::default(),
                        textures: &[&blank, &blank],
                        eye: Vec3::ZERO,
                        focal: 1.0,
                        object: &Object::IDENTITY,
                    },
                );
                for (i, &got) in color.iter().enumerate() {
                    let px = x + i as i32;
                    assert_eq!(
                        got,
                        stepped(&xs, &outs, half_rate, row_x0, px),
                        "pixel {px} of row {row_x0}..{row_x1} (half rate {half_rate})"
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
                let got = level_bilinear(
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
        let got = level_bilinear(
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

    /// The screen pixels of lanes that don't depend on them.
    const PIXELS: Pixels = Pixels {
        x: 0,
        y: 0,
        stride: 1,
    };

    /// A 64x64 texture of pseudo-random texels, and uv for eight lanes spread across it.
    fn random_texture() -> (Texture, I32s, I32s) {
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
        (tex, u, v)
    }

    /// Each sampler and its named function.
    type SampleFn = fn(&Texture, I32s, I32s, I16s, Pixels) -> U32s;
    const SAMPLERS: [(u8, SampleFn); 12] = [
        (filter::NEAREST_MIPMAP_NONE, sample_nearest_mipmap_none),
        (filter::BILINEAR_MIPMAP_NONE, sample_bilinear_mipmap_none),
        (filter::DITHERED_MIPMAP_NONE, sample_dithered_mipmap_none),
        (filter::NEAREST_MIPMAP_NEAREST, sample_nearest_mipmap_nearest),
        (filter::NEAREST_MIPMAP_LINEAR, sample_nearest_mipmap_linear),
        (filter::NEAREST_MIPMAP_DITHERED, sample_nearest_mipmap_dithered),
        (filter::BILINEAR_MIPMAP_NEAREST, sample_bilinear_mipmap_nearest),
        (filter::BILINEAR_MIPMAP_LINEAR, sample_bilinear_mipmap_linear),
        (filter::BILINEAR_MIPMAP_DITHERED, sample_bilinear_mipmap_dithered),
        (filter::DITHERED_MIPMAP_NEAREST, sample_dithered_mipmap_nearest),
        (filter::DITHERED_MIPMAP_LINEAR, sample_dithered_mipmap_linear),
        (filter::DITHERED_MIPMAP_DITHERED, sample_dithered_mipmap_dithered),
    ];

    #[test]
    fn samplers_are_named_and_listed_once() {
        assert_eq!(filter::ALL.map(filter::name), SAMPLERS.map(|(f, _)| filter::name(f)));
        for (f, _) in SAMPLERS {
            assert_eq!(filter::named(filter::name(f)), Some(f));
        }
        let mut all = filter::ALL.to_vec();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 12);
    }

    #[test]
    fn whole_levels_read_that_level_with_the_texel_method() {
        // At a whole level of detail every mip transition reads exactly that level (none:
        // the full-size one), with its sampler's texel method, at every screen pixel.
        let (tex, u, v) = random_texture();
        for (f, sample) in SAMPLERS {
            for level in 0..tex.levels.len() {
                for (x, y) in [(0, 0), (1, 2), (6, 3), (3, 7)] {
                    let at = Pixels { x, y, stride: 1 + (x & 1) };
                    let read = if filter::mip(f) == filter::MIPMAP_NONE { 0 } else { level };
                    let want = read_level(filter::method(f), &tex.levels[read], u, v, at);
                    let got = sample(&tex, u, v, I16s::fill(level as i16 * 256), at);
                    assert_eq!(got, want, "{} at level {level}, pixel ({x}, {y})", filter::name(f));
                }
            }
        }
    }

    #[test]
    fn dithered_texels_follow_version_one() {
        // Version 1's scalar read: the offset from the matrix at [x & 3][y & 3], shifted to
        // the level's size, taken from u and added to v, then the nearest texel.
        let (tex, _, _) = random_texture();
        let level = &tex.levels[1];
        let mut seed = 3u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as i32
        };
        for _ in 0..500 {
            let (u, v) = (next() - (1 << 30), next() - (1 << 30));
            let (x, y) = (next() & 1023, next() & 1023);
            let at = Pixels { x, y, stride: 1 };
            let got = level_dithered(level, I32s::fill(u), I32s::fill(v), at).to_array();
            for (i, &g) in got.iter().enumerate() {
                let e = TEXEL_DITHER[((x + i as i32) & 3) as usize][(y & 3) as usize];
                let coord = |s: i32, size: u32| ((s & 65535) * size as i32) >> 16;
                let tx = coord(u - (e >> level.width_log2), level.width);
                let ty = coord(v + (e >> level.height_log2), level.height);
                assert_eq!(g, level.texels[(ty * level.width as i32 + tx) as usize], "lane {i}");
            }
        }
    }

    #[test]
    fn mipmap_nearest_rounds_the_level_of_detail() {
        let (tex, u, v) = random_texture();
        let level = |l: usize| level_bilinear(&tex.levels[l], u, v);
        let at = |lod: i16| sample_bilinear_mipmap_nearest(&tex, u, v, I16s::fill(lod), PIXELS);
        assert_eq!(at(2 * 256 + 127), level(2));
        assert_eq!(at(2 * 256 + 128), level(3));
        assert_eq!(at(-700), level(0));
        assert_eq!(at(40 * 256), level(6));
    }

    #[test]
    fn mipmap_linear_blends_the_two_levels_around_the_lod() {
        let (tex, u, v) = random_texture();
        let at = |level: usize| level_bilinear(&tex.levels[level], u, v).to_array();
        let sample = |lod: I16s| sample_bilinear_mipmap_linear(&tex, u, v, lod, PIXELS).to_array();
        let channels_near = |a: u32, b: u32, tol: u32| {
            (0..4).all(|k| ((a >> (8 * k)) & 255).abs_diff((b >> (8 * k)) & 255) <= tol)
        };
        // Below 0 is level 0; past the last is the last.
        assert_eq!(sample(I16s::fill(-700)), at(0));
        assert_eq!(sample(I16s::fill(40 * 256)), at(6));
        // Fractions blend toward the next level.
        let got = sample(I16s::fill(2 * 256 + 64));
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
        let got = sample(I16s::from(lods));
        for (i, &lod) in lods.iter().enumerate() {
            assert_eq!(got[i], sample(I16s::fill(lod))[i], "lane {i} at lod {lod}");
        }
    }

    #[test]
    fn mipmap_dithered_picks_levels_by_the_ordered_pattern() {
        // As dev/dither.html: a fraction of k/16 sends the pixels whose pattern value is at
        // least 16 - k (k of every 16) to the next level, the rest to this one.
        let (tex, u, v) = random_texture();
        let (this, next) = (
            level_nearest(&tex.levels[2], u, v).to_array(),
            level_nearest(&tex.levels[3], u, v).to_array(),
        );
        for k in 0..=16 {
            // A fraction that rounds to k sixteenths.
            let lod = I16s::fill((2 * 256 + (k * 16).min(255) - (k > 0 && k < 16) as i32 * 3) as i16);
            let mut to_next = 0;
            for y in 0..4 {
                for x0 in [0, 4] {
                    let at = Pixels { x: x0, y, stride: 1 };
                    let got = sample_nearest_mipmap_dithered(&tex, u, v, lod, at).to_array();
                    for i in 0..LANES {
                        let x = (x0 as usize + i) & 3;
                        let goes = MIP_DITHER[y as usize][x] + k >= 16;
                        to_next += goes as i32;
                        let want = if goes { next[i] } else { this[i] };
                        assert_eq!(got[i], want, "k {k}, pixel ({}, {y})", x0 as usize + i);
                    }
                }
            }
            // Each pattern cell appears four times across the pixels (16 per row, 4 rows).
            assert_eq!(to_next, 4 * k, "k {k}");
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
