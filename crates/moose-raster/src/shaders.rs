//! Standard shaders.

/// Unlit per-vertex color: the mesh attribute `color` (three 0-255 values), interpolated in
/// 8.8 fixed point.
pub mod vertex_color {
    use crate::shader::{AttribDesc, Pixels, Shader, U32s, Uniforms};
    use moose_assets::Texture;

    crate::varyings! {
        fixed32 {}
        fixed16 { color: 3 }
        float {}
    }

    pub struct VertexColor;

    impl Shader for VertexColor {
        type Fixed32 = Fixed32;
        type Fixed16 = Fixed16;
        type Floats = Floats;
        const LAYOUT: &'static [AttribDesc] = LAYOUT;

        #[inline(always)]
        fn shade(
            _: &Fixed32,
            b: &Fixed16,
            _: &Floats,
            _: &Uniforms,
            _: &Texture,
            _: Pixels,
        ) -> U32s {
            super::rgb(b)
        }
    }
}

pub use vertex_color::VertexColor;

/// XRGB from a `color` varying of three 8.8 channels.
#[inline(always)]
fn rgb<G: HasColor>(g: &G) -> crate::shader::U32s {
    use crate::shader::high_byte;
    let c = g.color();
    high_byte(c[0]) << 16 | high_byte(c[1]) << 8 | high_byte(c[2])
}

/// A `Fixed16` group with a three-channel `color` varying.
trait HasColor {
    fn color(&self) -> &[crate::shader::I16s; 3];
}

macro_rules! has_color {
    ($($module:ident),*) => {
        $(impl HasColor for $module::Fixed16 {
            fn color(&self) -> &[crate::shader::I16s; 3] {
                &self.color
            }
        })*
    };
}

has_color!(
    vertex_color,
    vertex_color_translucent,
    vertex_color_fresnel,
    vertex_color_fresnel_disperse
);

/// Per-vertex color (the mesh attribute `color`, three 0-255 values) drawn translucent,
/// with one opacity for the whole surface: `uniforms.values[0]`, from 0 (invisible) to 1.
pub mod vertex_color_translucent {
    use crate::shader::{AttribDesc, Fill, Pixels, Shader, U32s, Uniforms};
    use moose_assets::Texture;

    crate::varyings! {
        fixed32 {}
        fixed16 { color: 3 }
        float {}
    }

    pub struct VertexColorTranslucent;

    impl Shader for VertexColorTranslucent {
        type Fixed32 = Fixed32;
        type Fixed16 = Fixed16;
        type Floats = Floats;
        const LAYOUT: &'static [AttribDesc] = LAYOUT;
        const TRANSLUCENT: bool = true;

        #[inline(always)]
        fn shade(
            _: &Fixed32,
            b: &Fixed16,
            _: &Floats,
            uni: &Uniforms,
            _: &Texture,
            _: Pixels,
        ) -> U32s {
            let alpha = (uni.values[0].clamp(0.0, 1.0) * 255.0).round() as u32;
            U32s::fill(alpha << 24) | super::rgb(b)
        }
    }
}

pub use vertex_color_translucent::VertexColorTranslucent;

/// Per-vertex color (the mesh attribute `color`) drawn over a reflection with a Fresnel
/// falloff: nearly opaque seen head-on, and fading to show the reflection at grazing
/// angles. For reflective surfaces seen directly; their reflection is already drawn
/// behind them.
///
/// Uniforms: the eye's world position (`values[0..3]`), the surface's unit normal
/// (`values[3..6]`), and its reflectance seen head-on, F0 (`values[6]`, 0 to 1).
///
/// Schlick's approximation gives the reflected share `F = F0 + (1 - F0)(1 - cos)^5`, with
/// cos the angle between the view vector and the normal; the surface's own color covers
/// the rest, so its alpha is `1 - F`. `1 - cos` comes from the exact view vector at each
/// sample point (`per_sample`, from the built-in `position` varying), is interpolated per
/// pixel as a 16-bit fraction (Q15: 32767 is 1), and `(1 - cos)^5` is four rounding
/// fraction multiplies for all lanes at once.
pub mod vertex_color_fresnel {
    use crate::shader::{AttribDesc, Fill, I16s, Pixels, Shader, U32s, Uniforms, widen};
    use moose_assets::Texture;

    crate::varyings! {
        fixed32 {}
        fixed16 { color: 3, facing: 1 }
        float { position: 3 }
    }

    /// The Fresnel exponent: Schlick's physical value.
    pub const FALLOFF: i32 = 5;

    /// Offsets of `facing` and `position` in the layout's values.
    const FACING: usize = 3;
    const POSITION: usize = 4;

    /// 1 as a Q15 fraction.
    pub(crate) const ONE: i16 = 32767;

    /// `1 - (1 - cos)^FALLOFF` (Q15) from the `facing` varying (`1 - cos`, Q15): the share
    /// of `1 - F0` that the surface's own color keeps.
    #[inline(always)]
    pub(crate) fn keep(facing: I16s) -> I16s {
        let mut power = facing;
        for _ in 1..FALLOFF {
            power = power.mul_scale_round(facing);
        }
        I16s::fill(ONE) - power
    }

    /// The value `per_sample` stores in `facing` for a given cos: `1 - cos` as Q15 bits
    /// once converted to 8.8.
    #[inline(always)]
    pub(crate) fn facing(cos: f32) -> f32 {
        (1.0 - cos.clamp(0.0, 1.0)) * (ONE as f32 / 256.0)
    }

    pub struct VertexColorFresnel;

    impl Shader for VertexColorFresnel {
        type Fixed32 = Fixed32;
        type Fixed16 = Fixed16;
        type Floats = Floats;
        const LAYOUT: &'static [AttribDesc] = LAYOUT;
        const DERIVED: &'static [&'static str] = &["facing"];
        const TRANSLUCENT: bool = true;

        #[inline(always)]
        fn per_sample(v: &mut [f32], uni: &Uniforms) {
            let u = &uni.values;
            let (dx, dy, dz) = (
                u[0] - v[POSITION],
                u[1] - v[POSITION + 1],
                u[2] - v[POSITION + 2],
            );
            let len2 = dx * dx + dy * dy + dz * dz;
            let cos = if len2 > 0.0 {
                (u[3] * dx + u[4] * dy + u[5] * dz) / len2.sqrt()
            } else {
                1.0
            };
            v[FACING] = facing(cos);
        }

        #[inline(always)]
        fn shade(
            _: &Fixed32,
            b: &Fixed16,
            _: &Floats,
            uni: &Uniforms,
            _: &Texture,
            _: Pixels,
        ) -> U32s {
            // alpha = (1 - F0) * keep, in 1/64 steps of a 0-255 alpha, then rounded.
            let scale = ((1.0 - uni.values[6].clamp(0.0, 1.0)) * 255.0 * 64.0).round() as i16;
            let alpha = keep(b.facing[0]).mul_scale_round(I16s::fill(scale));
            let alpha = widen(alpha + I16s::fill(32)) >> 6;
            (alpha << 24) | super::rgb(b)
        }
    }
}

pub use vertex_color_fresnel::VertexColorFresnel;

/// [`VertexColorFresnel`], plus the reflection dispersing with distance: what is farther
/// past the surface reflects less, as a slightly rough surface scatters it.
///
/// Uniforms: the eye's world position (`values[0..3]`), the surface's unit normal
/// (`values[3..6]`), its reflectance seen head-on, F0 (`values[6]`, 0 to 1), and the
/// reflection's fade range in meters (`values[7]`, 0 for no fade).
///
/// The Fresnel term `F` is computed as in [`VertexColorFresnel`]. The reflection also fades with the distance the reflected ray travels past the surface,
/// as a slightly rough surface blurs (and so washes out) what is farther from it: the
/// reflected share is `F * (1 - t)^2` with `t` = distance / range, clamped to 1. Things
/// touching the surface reflect fully; things a whole range away not at all. The distance
/// needs no extra buffer: the surface point and the reflected point behind it lie on the
/// same ray (the reflection is drawn at its virtual position), so with `d` the eye's
/// distance to the surface point, it is `d * (w / behind_w - 1)`.
///
/// The surface's own color covers the rest, so its alpha is `1 - F * fade`.
pub mod vertex_color_fresnel_disperse {
    use super::vertex_color_fresnel::{ONE, facing, keep};
    use crate::shader::{AttribDesc, F32s, Fill, I32s, Pixels, Shader, U32s, Uniforms};
    use moose_assets::Texture;

    crate::varyings! {
        fixed32 {}
        fixed16 { color: 3, facing: 1 }
        float { position: 3, distance: 1 }
    }

    /// Offsets of `facing`, `position` and `distance` in the layout's values.
    const FACING: usize = 3;
    const POSITION: usize = 4;
    const DISTANCE: usize = 7;

    pub struct VertexColorFresnelDisperse;

    impl Shader for VertexColorFresnelDisperse {
        type Fixed32 = Fixed32;
        type Fixed16 = Fixed16;
        type Floats = Floats;
        const LAYOUT: &'static [AttribDesc] = LAYOUT;
        const DERIVED: &'static [&'static str] = &["facing", "distance"];
        const TRANSLUCENT: bool = true;
        const READS_BEHIND: bool = true;

        #[inline(always)]
        fn per_sample(v: &mut [f32], uni: &Uniforms) {
            let u = &uni.values;
            let (dx, dy, dz) = (
                u[0] - v[POSITION],
                u[1] - v[POSITION + 1],
                u[2] - v[POSITION + 2],
            );
            let distance = (dx * dx + dy * dy + dz * dz).sqrt();
            let cos = if distance > 0.0 {
                (u[3] * dx + u[4] * dy + u[5] * dz) / distance
            } else {
                1.0
            };
            v[FACING] = facing(cos);
            v[DISTANCE] = distance;
        }

        /// Without what is behind: Fresnel only, no fade.
        #[inline(always)]
        fn shade(
            a: &Fixed32,
            b: &Fixed16,
            c: &Floats,
            uni: &Uniforms,
            tex: &Texture,
            at: Pixels,
        ) -> U32s {
            let zero = F32s::fill(0.0);
            Self::shade_over(a, b, c, uni, tex, at, zero, zero)
        }

        #[inline(always)]
        fn shade_over(
            _: &Fixed32,
            b: &Fixed16,
            c: &Floats,
            uni: &Uniforms,
            _: &Texture,
            _: Pixels,
            w: F32s,
            behind_w: F32s,
        ) -> U32s {
            let one = F32s::fill(1.0);
            let scale = F32s::fill((1.0 - uni.values[6].clamp(0.0, 1.0)) / ONE as f32);
            let keep = F32s::from_i32x8(I32s::from_i16x8(keep(b.facing[0])));
            let reflected = one - scale * keep;
            let range = uni.values[7];
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
            (alpha << 24) | super::rgb(b)
        }
    }
}

pub use vertex_color_fresnel_disperse::VertexColorFresnelDisperse;

/// Texture filtering for the textured shaders, as their `FILTER` parameter.
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
/// footprint varyings ([`LOD`](crate::shader::LOD), [`ANISO`](crate::shader::ANISO),
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
    use crate::shader::{AttribDesc, Fill, Pixels, Shader, U32s, Uniforms};
    use moose_assets::Texture;

    crate::varyings! {
        fixed32 { uv: 2, aniso: 2 }
        fixed16 { lod: 1, aniso_lod: 1 }
        float {}
    }

    /// Filtering `FILTER`, from [`super::filter`].
    pub struct Textured<const FILTER: u8 = TRILINEAR>;

    impl<const FILTER: u8> Shader for Textured<FILTER> {
        type Fixed32 = Fixed32;
        type Fixed16 = Fixed16;
        type Floats = Floats;
        const LAYOUT: &'static [AttribDesc] = LAYOUT;

        #[inline(always)]
        fn shade(
            a: &Fixed32,
            b: &Fixed16,
            _: &Floats,
            _: &Uniforms,
            tex: &Texture,
            _: Pixels,
        ) -> U32s {
            super::texel::<FILTER>(tex, &a.uv, &a.aniso, b.lod[0], b.aniso_lod[0])
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

/// A textured shiny surface drawn over its reflection: [`VertexColorFresnel`] with the
/// color from a texture (as [`Textured`]: trilinear, or as [`TexturedFresnelAnisotropic`],
/// [`TexturedFresnelBilinear`] and [`TexturedFresnelNearest`]), and the texture's alpha as
/// roughness, which takes reflectivity away.
///
/// Uniforms as [`VertexColorFresnelDisperse`]: the eye's world position (`values[0..3]`),
/// the surface's unit normal (`values[3..6]`), its reflectance seen head-on, F0
/// (`values[6]`), and the reflection's fade range in meters (`values[7]`, 0 for no fade).
///
/// Roughness runs from alpha 128 (smooth: reflects fully, as `F`) to 255 (rough: no
/// reflection). Alpha below 128 counts as smooth; keeping it at 128 or more means no texel
/// is ever fully transparent, which image editors may strip the color from. The reflection
/// also fades with the distance the reflected ray travels past the surface, as in
/// [`VertexColorFresnelDisperse`], but by less where Fresnel is strong: at grazing angles,
/// where reflected things tend to be far away, the fade would otherwise cancel the Fresnel
/// effect. The reflected share is `F * (1 - roughness) * (fade + (1 - fade) * F)`, and the
/// texture's color covers the rest.
pub mod textured_fresnel {
    use super::filter::TRILINEAR;
    use super::vertex_color_fresnel::{ONE, facing, keep};
    use crate::shader::{
        AttribDesc, F32s, Fill, I16s, I32s, Pixels, Shader, U32s, Uniforms, widen,
    };
    use moose_assets::Texture;

    crate::varyings! {
        fixed32 { uv: 2, aniso: 2 }
        fixed16 { facing: 1, lod: 1, aniso_lod: 1 }
        float { position: 3, distance: 1 }
    }

    /// Offsets of `facing`, `position` and `distance` in the layout's values.
    const FACING: usize = 4;
    const POSITION: usize = 7;
    const DISTANCE: usize = 10;

    /// Filtering `FILTER`, from [`super::filter`].
    pub struct TexturedFresnel<const FILTER: u8 = TRILINEAR>;

    impl<const FILTER: u8> Shader for TexturedFresnel<FILTER> {
        type Fixed32 = Fixed32;
        type Fixed16 = Fixed16;
        type Floats = Floats;
        const LAYOUT: &'static [AttribDesc] = LAYOUT;
        const DERIVED: &'static [&'static str] = &["facing", "distance"];
        const TRANSLUCENT: bool = true;
        const READS_BEHIND: bool = true;

        #[inline(always)]
        fn per_sample(v: &mut [f32], uni: &Uniforms) {
            let u = &uni.values;
            let (dx, dy, dz) = (
                u[0] - v[POSITION],
                u[1] - v[POSITION + 1],
                u[2] - v[POSITION + 2],
            );
            let distance = (dx * dx + dy * dy + dz * dz).sqrt();
            let cos = if distance > 0.0 {
                (u[3] * dx + u[4] * dy + u[5] * dz) / distance
            } else {
                1.0
            };
            v[FACING] = facing(cos);
            v[DISTANCE] = distance;
        }

        /// Without what is behind: no fade.
        #[inline(always)]
        fn shade(
            a: &Fixed32,
            b: &Fixed16,
            c: &Floats,
            uni: &Uniforms,
            tex: &Texture,
            at: Pixels,
        ) -> U32s {
            let zero = F32s::fill(0.0);
            Self::shade_over(a, b, c, uni, tex, at, zero, zero)
        }

        #[inline(always)]
        fn shade_over(
            a: &Fixed32,
            b: &Fixed16,
            c: &Floats,
            uni: &Uniforms,
            tex: &Texture,
            _: Pixels,
            w: F32s,
            behind_w: F32s,
        ) -> U32s {
            let texel = super::texel::<FILTER>(tex, &a.uv, &a.aniso, b.lod[0], b.aniso_lod[0]);
            // Smoothness (1 - roughness) as a Q15 fraction: (255 - alpha) / 127, at most 1
            // (32767 / 127 is 258.0).
            let rough: U32s = texel >> 24_u32;
            let smooth: U32s = (U32s::fill(255) - rough) * U32s::fill(258);
            let smooth = smooth.min(U32s::fill(ONE as u32));
            let smooth = I16s::from_i32x8_truncate(wide::bytemuck::cast::<U32s, I32s>(smooth));
            // F = F0 + (1 - F0)(1 - cos)^5 = 1 - (1 - F0) * keep, then less by roughness.
            let scale = ((1.0 - uni.values[6].clamp(0.0, 1.0)) * ONE as f32).round() as i16;
            let fresnel = I16s::fill(ONE) - keep(b.facing[0]).mul_scale_round(I16s::fill(scale));
            let mut reflected = fresnel.mul_scale_round(smooth);
            let range = uni.values[7];
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
}

pub use textured_fresnel::TexturedFresnel;
/// [`TexturedFresnel`] with 2x anisotropic filtering.
pub type TexturedFresnelAnisotropic = textured_fresnel::TexturedFresnel<{ filter::ANISOTROPIC }>;
/// [`TexturedFresnel`] with bilinear filtering of the full-size level, no mipmaps.
pub type TexturedFresnelBilinear = textured_fresnel::TexturedFresnel<{ filter::BILINEAR }>;
/// [`TexturedFresnel`] with the nearest texel of the full-size level.
pub type TexturedFresnelNearest = textured_fresnel::TexturedFresnel<{ filter::NEAREST }>;

/// A mirror finish from a cube map (the polygon's texture, made with
/// [`Texture::cube`](moose_assets::Texture::cube)), opaque: the color the cube map holds in
/// the direction the view reflects off the surface. The mesh attribute `normal` (three
/// values, any length, in world space: the entity unrotated) is interpolated, so a smooth
/// mesh reflects smoothly.
///
/// Uniforms: `[eye x, y, z, lod]`, the eye the surface is seen from (a mirror's eye for
/// polygons seen in it) and the cube map level of detail, log2 of face texels per pixel.
pub mod cube_reflection {
    use crate::shader::{AttribDesc, Pixels, Shader, U32s, Uniforms, sample_cube};
    use moose_assets::Texture;

    crate::varyings! {
        fixed32 {}
        fixed16 {}
        float { position: 3, normal: 3, reflect: 3 }
    }

    /// Offsets of `position`, `normal` and `reflect` in the layout's values.
    const POSITION: usize = 0;
    const NORMAL: usize = 3;
    const REFLECT: usize = 6;

    pub struct CubeReflection;

    impl Shader for CubeReflection {
        type Fixed32 = Fixed32;
        type Fixed16 = Fixed16;
        type Floats = Floats;
        const LAYOUT: &'static [AttribDesc] = LAYOUT;
        const DERIVED: &'static [&'static str] = &["reflect"];

        /// The view direction `d` reflected off the normal `n`, scaled by `n·n` so that
        /// neither needs normalizing (no square root): `d (n·n) - 2 n (n·d)`. Only its
        /// direction matters to the lookup, and between samples it is interpolated
        /// linearly, which keeps the direction exact on a flat polygon.
        #[inline(always)]
        fn per_sample(v: &mut [f32], uni: &Uniforms) {
            let u = &uni.values;
            let d = [
                v[POSITION] - u[0],
                v[POSITION + 1] - u[1],
                v[POSITION + 2] - u[2],
            ];
            let n = [v[NORMAL], v[NORMAL + 1], v[NORMAL + 2]];
            let nn = n[0] * n[0] + n[1] * n[1] + n[2] * n[2];
            let nd2 = 2.0 * (n[0] * d[0] + n[1] * d[1] + n[2] * d[2]);
            for k in 0..3 {
                v[REFLECT + k] = d[k] * nn - n[k] * nd2;
            }
        }

        #[inline(always)]
        fn shade(
            _: &Fixed32,
            _: &Fixed16,
            c: &Floats,
            uni: &Uniforms,
            tex: &Texture,
            _: Pixels,
        ) -> U32s {
            sample_cube(tex, c.reflect, uni.values[3])
        }
    }
}

pub use cube_reflection::CubeReflection;

#[cfg(test)]
mod tests {
    use moose_assets::Texture;

    use super::textured_fresnel::{self, TexturedFresnel};
    use crate::shader::{I16s, I32s, LANES, Pixels, Shader, Uniforms};

    #[test]
    fn cube_reflections_need_no_unit_normal() {
        use super::CubeReflection;
        // Eye at the origin, a point below it and ahead; normals of any length give the
        // mirror direction, scaled.
        let uni = Uniforms {
            values: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        let (d, n) = ([1.0f32, -2.0, 0.5], [0.3f32, 0.9, -0.1]);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let unit = n.map(|c| c / len);
        let dn = d[0] * unit[0] + d[1] * unit[1] + d[2] * unit[2];
        let want: Vec<f32> = (0..3).map(|k| d[k] - 2.0 * dn * unit[k]).collect();
        for scale in [0.2f32, 1.0, 5.0] {
            let mut v = [
                d[0],
                d[1],
                d[2],
                n[0] * scale,
                n[1] * scale,
                n[2] * scale,
                0.0,
                0.0,
                0.0,
            ];
            CubeReflection::per_sample(&mut v, &uni);
            let got = &v[6..9];
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
        for f0 in [0.04f32, 0.15, 1.0] {
            let uni = Uniforms {
                values: [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, f0, 0.0],
            };
            for cos in [0.0f32, 0.3, 0.7, 1.0] {
                // Lane i samples texel i % 4 (u at its center).
                let u: [i32; LANES] =
                    std::array::from_fn(|i| ((i % 4) as f32 * 0.25 * 65536.0) as i32 + 8192);
                let a = textured_fresnel::Fixed32 {
                    uv: [I32s::from(u), I32s::from([0; LANES])],
                    aniso: [I32s::from([0; LANES]); 2],
                };
                // The per-sample value, converted to 8.8 as the renderer does.
                let bits = (textured_fresnel_facing(cos) * 256.0).round() as i16;
                let b = textured_fresnel::Fixed16 {
                    facing: [I16s::from([bits; LANES])],
                    lod: [I16s::from([0; LANES])],
                    aniso_lod: [I16s::from([0; LANES])],
                };
                let out = <TexturedFresnel>::shade(
                    &a,
                    &b,
                    &Default::default(),
                    &uni,
                    &tex,
                    Pixels {
                        x: 0,
                        y: 0,
                        stride: 1,
                    },
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
        use crate::shader::F32s;
        // A smooth texel (alpha 128), seen at cos 0.5 from 2 m away (w = 1/2), with the
        // reflected points behind it 0 to 7 m past the surface; fade range 5 m.
        let tex = Texture::new("t", 1, 1, vec![128 << 24 | 0x40_40_40]).unwrap();
        let (f0, cos, range, distance) = (0.15f32, 0.5f32, 5.0f32, 2.0f32);
        let uni = Uniforms {
            values: [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, f0, range],
        };
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
                position: [F32s::from([0.0; LANES]); 3],
                distance: [F32s::from([distance; LANES])],
            },
            &uni,
            &tex,
            Pixels {
                x: 0,
                y: 0,
                stride: 1,
            },
            F32s::from([w; LANES]),
            F32s::from(behind),
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

    fn textured_fresnel_facing(cos: f32) -> f32 {
        super::vertex_color_fresnel::facing(cos)
    }
}
