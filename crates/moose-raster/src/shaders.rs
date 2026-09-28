//! Standard materials.

use crate::shader::{F32s, Fill, I16s, U32s};

/// XRGB from a `color` output of three 8.8 channels.
#[inline(always)]
fn rgb(c: &[I16s; 3]) -> U32s {
    use crate::shader::high_byte;
    high_byte(c[0]) << 16 | high_byte(c[1]) << 8 | high_byte(c[2])
}

/// The facing term of the Fresnel materials at [`LANES`](crate::shader::LANES) sample
/// points, as [`vertex_color_fresnel::facing`] stores it, from the cosine between the unit
/// `normal` and the view vector from each point to the eye. Also the distance to the eye.
#[inline(always)]
fn facing_and_distance(eye: glam::Vec3, position: &[F32s; 3], normal: &[F32s; 3]) -> (F32s, F32s) {
    let (dx, dy, dz) = (
        F32s::fill(eye.x) - position[0],
        F32s::fill(eye.y) - position[1],
        F32s::fill(eye.z) - position[2],
    );
    let distance = (dx * dx + dy * dy + dz * dz).sqrt();
    let cos = (normal[0] * dx + normal[1] * dy + normal[2] * dz) / distance;
    let cos = distance
        .simd_gt(F32s::fill(0.0))
        .select(cos, F32s::fill(1.0));
    (vertex_color_fresnel::facing_lanes(cos), distance)
}

/// Unlit per-vertex color: the mesh attribute `color` (three 0-255 values), interpolated in
/// 8.8 fixed point.
pub mod vertex_color {
    use crate::shader::{Material, PixelContext, SampleContext, U32s, VertexContext};

    crate::material_io! {
        vertex { color: 3 }
        sampled { color: 3 }
        fixed32 {}
        fixed16 { color: 3 }
        float {}
    }

    pub struct VertexColor;

    impl Material for VertexColor {
        crate::material_types!();

        #[inline(always)]
        fn shade_vertex(v: &Vertex, _: &VertexContext) -> Sampled {
            Sampled { color: v.color }
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, _: &SampleContext) -> Interp {
            Interp { color: s.color }
        }

        #[inline(always)]
        fn shade_pixel(_: &Fixed32, b: &Fixed16, _: &Floats, _: &PixelContext) -> U32s {
            super::rgb(&b.color)
        }
    }
}

pub use vertex_color::VertexColor;

/// Per-vertex color (the mesh attribute `color`, three 0-255 values) drawn translucent,
/// with one opacity for the whole surface: `params.values[0]`, from 0 (invisible) to 1.
pub mod vertex_color_translucent {
    use crate::shader::{Fill, Material, PixelContext, SampleContext, U32s, VertexContext};

    crate::material_io! {
        vertex { color: 3 }
        sampled { color: 3 }
        fixed32 {}
        fixed16 { color: 3 }
        float {}
    }

    pub struct VertexColorTranslucent;

    impl Material for VertexColorTranslucent {
        crate::material_types!();
        const TRANSLUCENT: bool = true;

        #[inline(always)]
        fn shade_vertex(v: &Vertex, _: &VertexContext) -> Sampled {
            Sampled { color: v.color }
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, _: &SampleContext) -> Interp {
            Interp { color: s.color }
        }

        #[inline(always)]
        fn shade_pixel(_: &Fixed32, b: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
            let alpha = (ctx.params.values[0].clamp(0.0, 1.0) * 255.0).round() as u32;
            U32s::fill(alpha << 24) | super::rgb(&b.color)
        }
    }
}

pub use vertex_color_translucent::VertexColorTranslucent;

/// Per-vertex color (the mesh attribute `color`) drawn over a reflection with a Fresnel
/// falloff: nearly opaque seen head-on, and fading to show the reflection at grazing
/// angles. For reflective surfaces seen directly; their reflection is already drawn
/// behind them.
///
/// Params: the surface's reflectance seen head-on, F0 (`values[0]`, 0 to 1).
///
/// Schlick's approximation gives the reflected share `F = F0 + (1 - F0)(1 - cos)^5`, with
/// cos the angle between the view vector and the surface normal; the surface's own color
/// covers the rest, so its alpha is `1 - F`. `1 - cos` comes from the exact view vector at
/// each sample point, is interpolated per pixel as a 16-bit fraction (Q15: 32767 is 1),
/// and `(1 - cos)^5` is four rounding fraction multiplies for all lanes at once.
pub mod vertex_color_fresnel {
    use crate::shader::{
        F32s, Fill, I16s, Material, PixelContext, SampleContext, U32s, VertexContext, widen,
    };

    crate::material_io! {
        vertex { color: 3, face_normal: 3 }
        sampled { color: 3, normal: 3, position: 3 }
        fixed32 {}
        fixed16 { color: 3, facing: 1 }
        float {}
    }

    /// The Fresnel exponent: Schlick's physical value.
    pub const FALLOFF: i32 = 5;

    /// 1 as a Q15 fraction.
    pub(crate) const ONE: i16 = 32767;

    /// `1 - (1 - cos)^FALLOFF` (Q15) from the `facing` output (`1 - cos`, Q15): the share
    /// of `1 - F0` that the surface's own color keeps.
    #[inline(always)]
    pub(crate) fn keep(facing: I16s) -> I16s {
        let mut power = facing;
        for _ in 1..FALLOFF {
            power = power.mul_scale_round(facing);
        }
        I16s::fill(ONE) - power
    }

    /// The value stored in `facing` for a given cos: `1 - cos` as Q15 bits once converted
    /// to 8.8.
    #[cfg(test)]
    pub(crate) fn facing(cos: f32) -> f32 {
        (1.0 - cos.clamp(0.0, 1.0)) * (ONE as f32 / 256.0)
    }

    /// [`facing`] for [`LANES`](crate::shader::LANES) values.
    #[inline(always)]
    pub(crate) fn facing_lanes(cos: F32s) -> F32s {
        (F32s::fill(1.0) - cos.max(F32s::fill(0.0)).min(F32s::fill(1.0)))
            * F32s::fill(ONE as f32 / 256.0)
    }

    pub struct VertexColorFresnel;

    impl Material for VertexColorFresnel {
        crate::material_types!();
        const SAMPLE_SPACING: i32 = 16;
        const TRANSLUCENT: bool = true;

        #[inline(always)]
        fn shade_vertex(v: &Vertex, _: &VertexContext) -> Sampled {
            Sampled {
                color: v.color,
                normal: v.face_normal,
                ..Default::default()
            }
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, ctx: &SampleContext) -> Interp {
            let (facing, _) = super::facing_and_distance(ctx.eye, &s.position, &s.normal);
            Interp {
                color: s.color,
                facing: [facing],
            }
        }

        #[inline(always)]
        fn shade_pixel(_: &Fixed32, b: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
            // alpha = (1 - F0) * keep, in 1/64 steps of a 0-255 alpha, then rounded.
            let f0 = ctx.params.values[0].clamp(0.0, 1.0);
            let scale = ((1.0 - f0) * 255.0 * 64.0).round() as i16;
            let alpha = keep(b.facing[0]).mul_scale_round(I16s::fill(scale));
            let alpha = widen(alpha + I16s::fill(32)) >> 6;
            (alpha << 24) | super::rgb(&b.color)
        }
    }
}

pub use vertex_color_fresnel::VertexColorFresnel;

/// [`VertexColorFresnel`], plus the reflection dispersing with distance: what is farther
/// past the surface reflects less, as a slightly rough surface scatters it.
///
/// Params: F0 (`values[0]`, 0 to 1), and the reflection's fade range in meters
/// (`values[1]`, 0 for no fade).
///
/// The Fresnel term `F` is computed as in [`VertexColorFresnel`]. The reflection also fades
/// with the distance the reflected ray travels past the surface, as a slightly rough
/// surface blurs (and so washes out) what is farther from it: the reflected share is
/// `F * (1 - t)^2` with `t` = distance / range, clamped to 1. Things touching the surface
/// reflect fully; things a whole range away not at all. The distance needs no extra buffer:
/// the surface point and the reflected point behind it lie on the same ray (the reflection
/// is drawn at its virtual position), so with `d` the eye's distance to the surface point,
/// it is `d * (w / behind_w - 1)`.
///
/// The surface's own color covers the rest, so its alpha is `1 - F * fade`.
pub mod vertex_color_fresnel_disperse {
    use super::vertex_color_fresnel::{ONE, keep};
    use crate::shader::{
        F32s, Fill, I32s, Material, Over, PixelContext, RowBehind, SampleContext, U32s,
        VertexContext,
    };

    crate::material_io! {
        vertex { color: 3, face_normal: 3 }
        sampled { color: 3, normal: 3, position: 3 }
        fixed32 {}
        fixed16 { color: 3, facing: 1 }
        float { distance: 1 }
    }

    pub struct VertexColorFresnelDisperse;

    impl Material for VertexColorFresnelDisperse {
        crate::material_types!();
        const SAMPLE_SPACING: i32 = 16;
        const TRANSLUCENT: bool = true;
        const READS_BEHIND: bool = true;

        #[inline(always)]
        fn shade_vertex(v: &Vertex, _: &VertexContext) -> Sampled {
            Sampled {
                color: v.color,
                normal: v.face_normal,
                ..Default::default()
            }
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, ctx: &SampleContext) -> Interp {
            let (facing, distance) = super::facing_and_distance(ctx.eye, &s.position, &s.normal);
            Interp {
                color: s.color,
                facing: [facing],
                distance: [distance],
            }
        }

        /// Without what is behind: Fresnel only, no fade.
        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, c: &Floats, ctx: &PixelContext) -> U32s {
            let zero = F32s::fill(0.0);
            let over = Over {
                w: zero,
                behind_w: zero,
                row: RowBehind::NONE,
            };
            Self::shade_over(a, b, c, ctx, &over)
        }

        #[inline(always)]
        fn shade_over(
            _: &Fixed32,
            b: &Fixed16,
            c: &Floats,
            ctx: &PixelContext,
            over: &Over,
        ) -> U32s {
            let (w, behind_w) = (over.w, over.behind_w);
            let one = F32s::fill(1.0);
            let scale = F32s::fill((1.0 - ctx.params.values[0].clamp(0.0, 1.0)) / ONE as f32);
            let keep = F32s::from_i32x8(I32s::from_i16x8(keep(b.facing[0])));
            let reflected = one - scale * keep;
            let range = ctx.params.values[1];
            let faded = if range > 0.0 {
                // What is left `bounce` meters past the surface: (1 - t)^2, t = bounce / range.
                let bounce = c.distance[0] * (w / behind_w - one);
                let t = one
                    - (bounce * F32s::fill(1.0 / range))
                        .max(F32s::fill(0.0))
                        .min(one);
                // No fade where nothing is behind (lanes past the run).
                behind_w
                    .simd_gt(F32s::fill(0.0))
                    .select(reflected * t * t, reflected)
            } else {
                reflected
            };
            let alpha = ((one - faded) * F32s::fill(255.0) + F32s::fill(0.5)).trunc_int();
            let alpha: U32s = wide::bytemuck::cast(alpha);
            (alpha << 24) | super::rgb(&b.color)
        }
    }
}

pub use vertex_color_fresnel_disperse::VertexColorFresnelDisperse;

/// Texture filtering for the textured materials, as their `FILTER` parameter.
pub mod filter {
    /// The nearest texel of the full-size level.
    pub const NEAREST: u8 = 0;
    /// Bilinear on the full-size level.
    pub const BILINEAR: u8 = 1;
    /// Bilinear on the two mip levels around the level of detail, blended.
    pub const TRILINEAR: u8 = 2;
    /// Two trilinear probes along the long axis of each pixel's footprint (2x anisotropic).
    pub const ANISOTROPIC: u8 = 3;
}

/// The texture's color at `uv` with filtering `FILTER` (see [`filter`]), from the built-in
/// footprint values ([`LOD`](crate::shader::LOD), [`ANISO`](crate::shader::ANISO),
/// [`ANISO_LOD`](crate::shader::ANISO_LOD)).
#[inline(always)]
fn texel<const FILTER: u8>(
    tex: &moose_assets::Texture,
    uv: &[crate::shader::I32s; 2],
    aniso: &[crate::shader::I32s; 2],
    lod: crate::shader::I16s,
    aniso_lod: crate::shader::I16s,
) -> crate::shader::U32s {
    use crate::shader::{sample, sample_anisotropic, sample_bilinear, sample_trilinear};
    match FILTER {
        filter::NEAREST => sample(tex.base(), uv[0], uv[1]),
        filter::BILINEAR => sample_bilinear(tex.base(), uv[0], uv[1]),
        filter::TRILINEAR => sample_trilinear(tex, uv[0], uv[1], lod),
        _ => sample_anisotropic(tex, uv[0], uv[1], *aniso, lod, aniso_lod),
    }
}

/// Texture color from the mesh attribute `uv` (two coordinates, 1.0 across the texture,
/// which tiles), trilinearly filtered with mipmaps (or as [`TexturedAnisotropic`],
/// [`TexturedBilinear`] and [`TexturedNearest`]), opaque. The texture's alpha is ignored.
pub mod textured {
    use super::filter::TRILINEAR;
    use crate::shader::{Fill, Material, PixelContext, SampleContext, U32s, VertexContext};

    crate::material_io! {
        vertex { uv: 2 }
        sampled { uv: 2, lod: 1, aniso: 2, aniso_lod: 1 }
        fixed32 { uv: 2, aniso: 2 }
        fixed16 { lod: 1, aniso_lod: 1 }
        float {}
    }

    /// Filtering `FILTER`, from [`super::filter`].
    pub struct Textured<const FILTER: u8 = TRILINEAR>;

    impl<const FILTER: u8> Material for Textured<FILTER> {
        crate::material_types!();

        #[inline(always)]
        fn shade_vertex(v: &Vertex, _: &VertexContext) -> Sampled {
            Sampled {
                uv: v.uv,
                ..Default::default()
            }
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, _: &SampleContext) -> Interp {
            Interp {
                uv: s.uv,
                aniso: s.aniso,
                lod: s.lod,
                aniso_lod: s.aniso_lod,
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
            super::texel::<FILTER>(ctx.textures[0], &a.uv, &a.aniso, b.lod[0], b.aniso_lod[0])
                & U32s::fill(0xFF_FFFF)
        }
    }
}

pub use textured::Textured;
/// [`Textured`] with 2x anisotropic filtering.
pub type TexturedAnisotropic = textured::Textured<{ filter::ANISOTROPIC }>;
/// [`Textured`] with bilinear filtering of the full-size level, no mipmaps.
pub type TexturedBilinear = textured::Textured<{ filter::BILINEAR }>;
/// [`Textured`] with the nearest texel of the full-size level.
pub type TexturedNearest = textured::Textured<{ filter::NEAREST }>;

/// A textured reflective surface: the texture's color drawn over the reflection with a
/// Fresnel falloff, as [`VertexColorFresnel`], with the texture's alpha as roughness: 128
/// (or less) is fully smooth, 255 fully rough, and the reflected share `F` is scaled by
/// the smoothness `(255 - alpha) / 127`. The reflection also fades with how far past the
/// surface it is, as [`VertexColorFresnelDisperse`], except that Fresnel limits the fade:
/// `fade + (1 - fade) F`.
///
/// Params: F0 (`values[0]`, 0 to 1) and the fade range in meters (`values[1]`, 0 for none).
pub mod textured_fresnel {
    use super::filter::TRILINEAR;
    use super::vertex_color_fresnel::{ONE, keep};
    use crate::shader::{
        F32s, Fill, I16s, I32s, Material, Over, Params, PixelContext, RowBehind, SampleContext,
        U32s, VertexContext, widen,
    };

    crate::material_io! {
        vertex { uv: 2, face_normal: 3 }
        sampled { uv: 2, normal: 3, position: 3, lod: 1, aniso: 2, aniso_lod: 1 }
        fixed32 { uv: 2, aniso: 2 }
        fixed16 { facing: 1, lod: 1, aniso_lod: 1 }
        float { distance: 1 }
    }

    /// Filtering `FILTER`, from [`super::filter`].
    pub struct TexturedFresnel<const FILTER: u8 = TRILINEAR>;

    impl<const FILTER: u8> Material for TexturedFresnel<FILTER> {
        crate::material_types!();
        const SAMPLE_SPACING: i32 = 16;
        const TRANSLUCENT: bool = true;
        const READS_BEHIND: bool = true;

        #[inline(always)]
        fn shade_vertex(v: &Vertex, _: &VertexContext) -> Sampled {
            Sampled {
                uv: v.uv,
                normal: v.face_normal,
                ..Default::default()
            }
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, ctx: &SampleContext) -> Interp {
            let (facing, distance) = super::facing_and_distance(ctx.eye, &s.position, &s.normal);
            Interp {
                uv: s.uv,
                aniso: s.aniso,
                facing: [facing],
                lod: s.lod,
                aniso_lod: s.aniso_lod,
                distance: [distance],
            }
        }

        /// Without what is behind: no fade.
        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, c: &Floats, ctx: &PixelContext) -> U32s {
            let zero = F32s::fill(0.0);
            let over = Over {
                w: zero,
                behind_w: zero,
                row: RowBehind::NONE,
            };
            Self::shade_over(a, b, c, ctx, &over)
        }

        #[inline(always)]
        fn shade_over(
            a: &Fixed32,
            b: &Fixed16,
            c: &Floats,
            ctx: &PixelContext,
            over: &Over,
        ) -> U32s {
            let texel =
                super::texel::<FILTER>(ctx.textures[0], &a.uv, &a.aniso, b.lod[0], b.aniso_lod[0]);
            self::over(texel, b, c, ctx.params, over.w, over.behind_w)
        }
    }

    /// `texel`'s color with the alpha that lets the reflection through: Fresnel, less by the
    /// texel's roughness (its alpha), faded with the distance behind (see
    /// [`TexturedFresnel`]).
    #[inline(always)]
    pub fn over(
        texel: U32s,
        b: &Fixed16,
        c: &Floats,
        params: &Params,
        w: F32s,
        behind_w: F32s,
    ) -> U32s {
        // Smoothness (1 - roughness) as a Q15 fraction: (255 - alpha) / 127, at most 1
        // (32767 / 127 is 258.0).
        let rough: U32s = texel >> 24_u32;
        let smooth: U32s = (U32s::fill(255) - rough) * U32s::fill(258);
        let smooth = smooth.min(U32s::fill(ONE as u32));
        let smooth = I16s::from_i32x8_truncate(wide::bytemuck::cast::<U32s, I32s>(smooth));
        // F = F0 + (1 - F0)(1 - cos)^5 = 1 - (1 - F0) * keep, then less by roughness.
        let scale = ((1.0 - params.values[0].clamp(0.0, 1.0)) * ONE as f32).round() as i16;
        let fresnel = I16s::fill(ONE) - keep(b.facing[0]).mul_scale_round(I16s::fill(scale));
        let mut reflected = fresnel.mul_scale_round(smooth);
        let range = params.values[1];
        if range > 0.0 {
            // What is left `bounce` meters past the surface: (1 - t)^2, t = bounce / range
            // (as a Q15 fraction). No fade where nothing is behind (lanes past the run).
            let one = F32s::fill(1.0);
            let bounce = c.distance[0] * (w / behind_w - one);
            let t = one
                - (bounce * F32s::fill(1.0 / range))
                    .max(F32s::fill(0.0))
                    .min(one);
            let fade = behind_w.simd_gt(F32s::fill(0.0)).select(t * t, one);
            let fade = (fade * F32s::fill(ONE as f32)).round_int();
            let fade = I16s::from_i32x8_truncate(fade);
            // Fresnel limits how much the fade takes away: fade + (1 - fade) F. At grazing
            // angles, where F is strong and what is reflected tends to be far, the
            // reflection holds.
            let one = I16s::fill(ONE);
            let fade = one - (one - fade).mul_scale_round(one - fresnel);
            reflected = reflected.mul_scale_round(fade);
        }
        // alpha = 1 - reflected, in 1/64 steps of a 0-255 alpha, then rounded.
        let alpha = (I16s::fill(ONE) - reflected).mul_scale_round(I16s::fill(255 * 64));
        let alpha = widen(alpha + I16s::fill(32)) >> 6;
        (alpha << 24) | (texel & U32s::fill(0xFF_FFFF))
    }
}

pub use textured_fresnel::TexturedFresnel;
/// [`TexturedFresnel`] with 2x anisotropic filtering.
pub type TexturedFresnelAnisotropic = textured_fresnel::TexturedFresnel<{ filter::ANISOTROPIC }>;
/// [`TexturedFresnel`] with bilinear filtering of the full-size level, no mipmaps.
pub type TexturedFresnelBilinear = textured_fresnel::TexturedFresnel<{ filter::BILINEAR }>;
/// [`TexturedFresnel`] with the nearest texel of the full-size level.
pub type TexturedFresnelNearest = textured_fresnel::TexturedFresnel<{ filter::NEAREST }>;

/// Water, for shiny floors: [`TexturedFresnel`] with the reflection seen through it rippling
/// from side to side with the water's own ripples. Textures: `[0]` the rippled water
/// ([`moose_assets::Ripples::texture`], Half-Life's software water), `[1]` its height map
/// ([`moose_assets::Ripples::heights`], the same size as `[0]`, whose footprint it shares),
/// which must be bound. Each pixel reads how far its texel was moved, filtered like the
/// water, and shifts what it sees through the surface as far along its row, rounded to a
/// whole pixel (it can only reach its own row): the reflection moves where and as the
/// texture does, smoothly across texels.
///
/// Params: as [`TexturedFresnel`] (`[F0, fade range]`), then the size in meters of one
/// texel of shift (0 for no shift).
pub mod water {
    use super::filter::TRILINEAR;
    use super::textured_fresnel::{
        Fixed16, Fixed32, Floats, Interp, Sampled, SampledLanes, TexturedFresnel, Vertex, over,
    };
    use crate::shader::{
        F32s, Fill, I32s, Material, MaterialIo, Over, PixelContext, RowBehind, SampleContext, U32s,
        VertexContext, blend_lanes,
    };

    /// Largest shift, in pixels.
    const MAX_SHIFT: i32 = 32;

    /// The translucent surface, over its reflection. Filtering `FILTER`, from
    /// [`super::filter`].
    pub struct Water<const FILTER: u8 = TRILINEAR>;

    impl<const FILTER: u8> Material for Water<FILTER> {
        type Vertex = Vertex;
        type Sampled = Sampled;
        type SampledLanes = SampledLanes;
        type Interp = Interp;
        type Fixed32 = Fixed32;
        type Fixed16 = Fixed16;
        type Floats = Floats;
        const IO: MaterialIo = super::textured_fresnel::IO;
        const SAMPLE_SPACING: i32 = 16;
        const TRANSLUCENT: bool = true;
        const READS_BEHIND: bool = true;

        #[inline(always)]
        fn shade_vertex(v: &Vertex, ctx: &VertexContext) -> Sampled {
            TexturedFresnel::<FILTER>::shade_vertex(v, ctx)
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, ctx: &SampleContext) -> Interp {
            TexturedFresnel::<FILTER>::shade_sample(s, ctx)
        }

        /// Without what is behind: no fade and no ripples.
        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, c: &Floats, ctx: &PixelContext) -> U32s {
            let zero = F32s::fill(0.0);
            let over = Over {
                w: zero,
                behind_w: zero,
                row: RowBehind::NONE,
            };
            Self::shade_over(a, b, c, ctx, &over)
        }

        #[inline(always)]
        fn shade_over(
            a: &Fixed32,
            b: &Fixed16,
            c: &Floats,
            ctx: &PixelContext,
            over: &Over,
        ) -> U32s {
            let (w, behind_w) = (over.w, over.behind_w);
            let tex = ctx.textures;
            let texel = super::texel::<FILTER>(tex[0], &a.uv, &a.aniso, b.lod[0], b.aniso_lod[0]);
            // Pixels one meter away per texel of shift.
            let ripple = ctx.params.values[2] * ctx.focal;
            if ripple == 0.0 {
                return self::over(texel, b, c, ctx.params, w, behind_w);
            }
            // How far this pixel's texel was moved (in quarter texels, filtered like the
            // water, so a fraction of a texel), and the same distance on screen at this depth
            // (w is 1 / depth), rounded to a pixel. The color and the depth behind both come
            // from there, so the fade moves with what it fades.
            let height = super::texel::<FILTER>(tex[1], &a.uv, &a.aniso, b.lod[0], b.aniso_lod[0])
                & U32s::fill(0xFF);
            let h = wide::bytemuck::cast::<U32s, I32s>(height) - I32s::fill(128);
            let shift = (h.round_float() * w * F32s::fill(ripple * 0.25))
                .round_int()
                .max(I32s::fill(-MAX_SHIFT))
                .min(I32s::fill(MAX_SHIFT));
            let (under, under_w) = over.row.at(ctx.at.x_lanes() + shift);
            // Lanes past the run keep nothing behind.
            let has_behind = behind_w.simd_gt(F32s::fill(0.0));
            let under_w = has_behind.select(under_w, behind_w);
            let surface = self::over(texel, b, c, ctx.params, w, under_w);
            // Blended here, so the renderer's own blend (alpha 255) keeps it as it is.
            blend_lanes(surface, under) | U32s::fill(0xFF00_0000)
        }
    }
}

pub use water::Water;

/// A mirror finish from a cube map (texture `[0]`, made with
/// [`Texture::cube`](moose_assets::Texture::cube)), opaque: the color the cube map holds in
/// the direction the view reflects off the surface. The mesh attribute `normal` (three
/// values, any length, in the mesh's own space) is turned with the object and
/// interpolated, so a smooth mesh reflects smoothly, however it is turned.
///
/// Params: the object's radius in meters (`values[0]`) and the cube map's face size in
/// texels (`values[1]`), for its level of detail.
pub mod cube_reflection {
    use crate::shader::{
        F32s, Fill, Material, PixelContext, SampleContext, U32s, VertexContext, sample_cube,
    };

    crate::material_io! {
        vertex { normal: 3 }
        sampled { normal: 3, position: 3 }
        fixed32 {}
        fixed16 {}
        float { reflect: 3, lod: 1 }
    }

    pub struct CubeReflection;

    impl Material for CubeReflection {
        crate::material_types!();
        /// The reflection turns fast across a curved mesh's polygons.
        const SAMPLE_SPACING: i32 = 8;

        #[inline(always)]
        fn shade_vertex(v: &Vertex, ctx: &VertexContext) -> Sampled {
            let n = ctx.object.rotation * glam::Vec3::from_array(v.normal);
            Sampled {
                normal: n.to_array(),
                ..Default::default()
            }
        }

        /// The view direction `d` reflected off the normal `n`, scaled by `n·n` so that
        /// neither needs normalizing (no square root): `d (n·n) - 2 n (n·d)`. Only its
        /// direction matters to the lookup, and between samples it is interpolated
        /// linearly, which keeps the direction exact on a flat polygon.
        ///
        /// The level of detail is face texels per pixel at the middle of the object, where
        /// the view turns least across it: a pixel spans 1 / r of the normal's angle (r
        /// the radius in pixels), so 2 / r of the reflection's, and a face texel spans
        /// about (pi / 2) / size.
        #[inline(always)]
        fn shade_sample(s: &SampledLanes, ctx: &SampleContext) -> Interp {
            let (p, n) = (&s.position, &s.normal);
            let d = [
                p[0] - F32s::fill(ctx.eye.x),
                p[1] - F32s::fill(ctx.eye.y),
                p[2] - F32s::fill(ctx.eye.z),
            ];
            let nn = n[0] * n[0] + n[1] * n[1] + n[2] * n[2];
            let nd2 = F32s::fill(2.0) * (n[0] * d[0] + n[1] * d[1] + n[2] * d[2]);
            let reflect = std::array::from_fn(|k| d[k] * nn - n[k] * nd2);
            let (radius, size) = (ctx.params.values[0], ctx.params.values[1]);
            let distance = ctx.object.position.distance(ctx.eye);
            let r = ctx.focal * radius / distance.max(1e-3);
            let lod = (4.0 * size / (std::f32::consts::PI * r)).log2();
            Interp {
                reflect,
                lod: [F32s::fill(lod)],
            }
        }

        #[inline(always)]
        fn shade_pixel(_: &Fixed32, _: &Fixed16, c: &Floats, ctx: &PixelContext) -> U32s {
            sample_cube(ctx.textures[0], c.reflect, c.lod[0].to_array()[0])
        }
    }
}

pub use cube_reflection::CubeReflection;

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use moose_assets::Texture;
    use moose_view::Object;

    use super::textured_fresnel::{self, TexturedFresnel};
    use crate::shader::{
        F32s, I16s, I32s, LANES, Material, Over, Params, PixelContext, Pixels, RowBehind,
        SampleContext, TextureSet, blend,
    };

    fn pixel_ctx<'a>(
        textures: &'a TextureSet<'a>,
        params: &'a Params,
        x: i32,
        focal: f32,
    ) -> PixelContext<'a> {
        PixelContext {
            at: Pixels { x, y: 0, stride: 1 },
            textures,
            params,
            focal,
        }
    }

    #[test]
    fn row_reads_stay_in_the_run() {
        let colors = [10u32, 11, 12, 13];
        let w = [0.5f32, 0.25, 0.0, 1.0];
        let row = RowBehind::new(&colors, &w, 100);
        let x = I32s::from([90, 99, 100, 101, 103, 104, 200, -5]);
        let (c, got_w) = row.at(x);
        assert_eq!(c.to_array(), [10, 10, 10, 11, 13, 13, 13, 10]);
        assert_eq!(got_w.to_array(), [0.5, 0.5, 0.5, 0.25, 1.0, 1.0, 1.0, 0.5]);
        let (c, got_w) = RowBehind::NONE.at(x);
        assert_eq!((c.to_array(), got_w.to_array()), ([0; LANES], [0.0; LANES]));
    }

    #[test]
    fn water_shows_the_row_shifted_by_its_ripples() {
        use super::water::Water;
        // A full mirror (F0 1, a smooth texel) over a row of distinct colors and depths, each
        // lane at the center of its own cell of the height map (in quarter texels): each pixel
        // shows the row's color as many pixels along as its cell says (at this depth and
        // scale, one pixel per texel of shift), faded by the depth there, opaque.
        let texel = 128 << 24 | 0x40_40_40;
        let tex = Texture::new("t", 1, 1, vec![texel]).unwrap();
        let hs = [0i32, 3, -2, 5, -7, 1, 0, 4];
        let map = Texture::new(
            "h",
            8,
            1,
            hs.map(|h| (128 + 4 * h) as u32 * 0x01_01_01).into(),
        )
        .unwrap();
        let colors: Vec<u32> = (0..64).map(|i| 0x01_02_03 * i).collect();
        let depths: Vec<f32> = (0..64).map(|i| 1.0 / (2.5 + (i % 7) as f32)).collect();
        let row = RowBehind::new(&colors, &depths, 1000);
        // Two pixels a meter away per texel of shift, at w 1/2: one pixel.
        let (focal, w) = (1.0f32, 0.5f32);
        let params = Params::new(&[1.0, 5.0, 2.0]);
        let u = I32s::from(std::array::from_fn(|i| ((2 * i as i32 + 1) << 16) / 16));
        let floats = textured_fresnel::Floats {
            distance: [F32s::from([2.0; LANES])],
        };
        let textures = [&tex, &map];
        let out = <Water>::shade_over(
            &textured_fresnel::Fixed32 {
                uv: [u, I32s::from([0; LANES])],
                aniso: [I32s::from([0; LANES]); 2],
            },
            &textured_fresnel::Fixed16::default(),
            &floats,
            &pixel_ctx(&textures, &params, 1020, focal),
            &Over {
                w: F32s::from([w; LANES]),
                behind_w: F32s::from([w / 2.0; LANES]),
                row,
            },
        )
        .to_array();
        for (i, &h) in hs.iter().enumerate() {
            let at = (20 + i as i32 + h) as usize;
            let surface = textured_fresnel::over(
                crate::shader::U32s::from([texel; LANES]),
                &textured_fresnel::Fixed16::default(),
                &floats,
                &params,
                F32s::from([w; LANES]),
                F32s::from([depths[at]; LANES]),
            );
            let want = blend(surface.to_array()[i], colors[at]);
            assert_eq!(out[i], 0xFF00_0000 | want, "lane {i}, shift {h}");
        }
    }

    #[test]
    fn filtered_heights_shift_by_fractions_rounded() {
        use super::filter::BILINEAR;
        use super::water::Water;
        // Two cells, shifts of 0 and 6 texels (one pixel each here), and pixels spread
        // between their centers: filtered, the shift climbs through the pixels between
        // instead of jumping from 0 to 6 halfway.
        let tex = Texture::new("t", 1, 1, vec![128 << 24]).unwrap();
        let map = Texture::new("h", 2, 1, vec![128 * 0x01_01_01, 152 * 0x01_01_01]).unwrap();
        let colors: Vec<u32> = (0..64).collect();
        let depths = vec![0.25f32; 64];
        let row = RowBehind::new(&colors, &depths, 0);
        let params = Params::new(&[1.0, 0.0, 2.0]);
        // u from the first cell's center (1/4) to the second's (3/4).
        let u = I32s::from(std::array::from_fn(|i| (1 << 14) + ((i as i32) << 15) / 7));
        let textures = [&tex, &map];
        let out = Water::<BILINEAR>::shade_over(
            &textured_fresnel::Fixed32 {
                uv: [u, I32s::from([0; LANES])],
                aniso: [I32s::from([0; LANES]); 2],
            },
            &textured_fresnel::Fixed16::default(),
            &textured_fresnel::Floats {
                distance: [F32s::from([2.0; LANES])],
            },
            &pixel_ctx(&textures, &params, 20, 1.0),
            &Over {
                w: F32s::from([0.5; LANES]),
                behind_w: F32s::from([0.25; LANES]),
                row,
            },
        )
        .to_array();
        // A full mirror: each pixel is exactly the row color it reads.
        let shifts: Vec<i32> = (0..LANES)
            .map(|i| (out[i] & 0xFF_FFFF) as i32 - (20 + i as i32))
            .collect();
        assert_eq!((shifts[0], shifts[LANES - 1]), (0, 6), "{shifts:?}");
        assert!(
            shifts.windows(2).all(|p| p[0] <= p[1] && p[1] - p[0] <= 2),
            "{shifts:?}"
        );
    }

    #[test]
    fn cube_reflections_need_no_unit_normal() {
        use super::CubeReflection;
        use super::cube_reflection::SampledLanes;
        // Eye at the origin, a point below it and ahead; normals of any length give the
        // mirror direction, scaled.
        let params = Params::new(&[0.25, 128.0]);
        let ctx = SampleContext {
            eye: Vec3::ZERO,
            focal: 360.0,
            object: &Object::IDENTITY,
            params: &params,
        };
        let (d, n) = ([1.0f32, -2.0, 0.5], [0.3f32, 0.9, -0.1]);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let unit = n.map(|c| c / len);
        let dn = d[0] * unit[0] + d[1] * unit[1] + d[2] * unit[2];
        let want: Vec<f32> = (0..3).map(|k| d[k] - 2.0 * dn * unit[k]).collect();
        for scale in [0.2f32, 1.0, 5.0] {
            let s = SampledLanes {
                normal: n.map(|c| F32s::from([c * scale; LANES])),
                position: d.map(|c| F32s::from([c; LANES])),
            };
            let out = CubeReflection::shade_sample(&s, &ctx);
            let got: Vec<f32> = out.reflect.iter().map(|l| l.to_array()[0]).collect();
            let k = got[0] / want[0];
            assert!(k > 0.0);
            for i in 0..3 {
                assert!(
                    (got[i] - want[i] * k).abs() < 1e-4 * k,
                    "{got:?} vs {want:?}"
                );
            }
        }
    }

    #[test]
    fn roughness_takes_reflection_away() {
        // Four texels in a row, one per roughness case: fully shiny, halfway, fully rough, and
        // below 128 (counted as shiny).
        let colors = [0x10_20_30, 0x40_50_60, 0x70_80_90, 0xA0_B0_C0];
        let alphas = [128u32, 191, 255, 50];
        let texels: Vec<u32> = (0..4).map(|i| alphas[i] << 24 | colors[i]).collect();
        let tex = Texture::new("t", 4, 1, texels).unwrap();
        let textures = [&tex, &tex];
        for f0 in [0.04f32, 0.15, 1.0] {
            let params = Params::new(&[f0, 0.0]);
            for cos in [0.0f32, 0.3, 0.7, 1.0] {
                // Lane i samples texel i % 4 (u at its center).
                let u: [i32; LANES] =
                    std::array::from_fn(|i| ((i % 4) as f32 * 0.25 * 65536.0) as i32 + 8192);
                let a = textured_fresnel::Fixed32 {
                    uv: [I32s::from(u), I32s::from([0; LANES])],
                    aniso: [I32s::from([0; LANES]); 2],
                };
                // The sample stage's value, converted to 8.8 as the renderer does.
                let bits = (textured_fresnel_facing(cos) * 256.0).round() as i16;
                let b = textured_fresnel::Fixed16 {
                    facing: [I16s::from([bits; LANES])],
                    lod: [I16s::from([0; LANES])],
                    aniso_lod: [I16s::from([0; LANES])],
                };
                let out = <TexturedFresnel>::shade_pixel(
                    &a,
                    &b,
                    &Default::default(),
                    &pixel_ctx(&textures, &params, 0, 1.0),
                )
                .to_array();
                for (i, &c) in out.iter().enumerate() {
                    let t = i % 4;
                    assert_eq!(c & 0xFF_FFFF, colors[t], "color from the texture");
                    let rough = ((alphas[t] as f32 - 128.0) / 127.0).clamp(0.0, 1.0);
                    let fresnel = f0 + (1.0 - f0) * (1.0 - cos).powi(5);
                    let want = (1.0 - fresnel * (1.0 - rough)) * 255.0;
                    let got = (c >> 24) as f32;
                    assert!(
                        (got - want).abs() <= 1.5,
                        "F0 {f0} cos {cos} alpha {}: got {got}, want {want}",
                        alphas[t]
                    );
                }
            }
        }
    }

    #[test]
    fn the_reflection_fades_with_distance_past_the_surface() {
        // A smooth texel (alpha 128), seen at cos 0.5 from 2 m away (w = 1/2), with the
        // reflected points behind it 0 to 7 m past the surface; fade range 5 m.
        let tex = Texture::new("t", 1, 1, vec![128 << 24 | 0x40_40_40]).unwrap();
        let textures = [&tex, &tex];
        let (f0, cos, range, distance) = (0.15f32, 0.5f32, 5.0f32, 2.0f32);
        let params = Params::new(&[f0, range]);
        let bounces = [0.0f32, 0.5, 1.0, 2.5, 4.0, 5.0, 7.0, 0.0];
        let w = 1.0 / distance;
        // bounce = distance * (w / behind_w - 1); the last lane has nothing behind.
        let behind: [f32; LANES] = std::array::from_fn(|i| {
            if i == LANES - 1 {
                0.0
            } else {
                w / (1.0 + bounces[i] / distance)
            }
        });
        let bits = (textured_fresnel_facing(cos) * 256.0).round() as i16;
        let out = <TexturedFresnel>::shade_over(
            &textured_fresnel::Fixed32::default(),
            &textured_fresnel::Fixed16 {
                facing: [I16s::from([bits; LANES])],
                lod: [I16s::from([0; LANES])],
                aniso_lod: [I16s::from([0; LANES])],
            },
            &textured_fresnel::Floats {
                distance: [F32s::from([distance; LANES])],
            },
            &pixel_ctx(&textures, &params, 0, 1.0),
            &Over {
                w: F32s::from([w; LANES]),
                behind_w: F32s::from(behind),
                row: RowBehind::NONE,
            },
        )
        .to_array();
        let fresnel = f0 + (1.0 - f0) * (1.0 - cos).powi(5);
        for (i, &c) in out.iter().enumerate() {
            let fade = if i == LANES - 1 {
                1.0
            } else {
                (1.0 - (bounces[i] / range).min(1.0)).powi(2)
            };
            // Fresnel limits the fade.
            let fade = fade + (1.0 - fade) * fresnel;
            let want = (1.0 - fresnel * fade) * 255.0;
            let got = (c >> 24) as f32;
            assert!(
                (got - want).abs() <= 1.5,
                "bounce {}: got {got}, want {want}",
                bounces[i]
            );
        }
    }

    #[test]
    fn facing_lanes_match_the_scalar_facing() {
        use super::vertex_color_fresnel::{facing, facing_lanes};
        for i in 0..=200 {
            let cos = i as f32 / 100.0 - 0.5;
            assert_eq!(
                facing_lanes(F32s::from([cos; LANES])).to_array()[0],
                facing(cos)
            );
        }
    }

    fn textured_fresnel_facing(cos: f32) -> f32 {
        super::vertex_color_fresnel::facing(cos)
    }
}
