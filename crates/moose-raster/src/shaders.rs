//! Standard shaders.

/// Unlit per-vertex color: the mesh attribute `color` (three 0-255 values), interpolated in
/// 8.8 fixed point.
pub mod vertex_color {
    use crate::shader::{AttribDesc, Shader, Uniforms};

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
        fn shade(_: &Fixed32, b: &Fixed16, _: &Floats, _: &Uniforms) -> u32 {
            let channel = |v: i16| ((v as u16) >> 8) as u32;
            channel(b.color[0]) << 16 | channel(b.color[1]) << 8 | channel(b.color[2])
        }
    }
}

pub use vertex_color::VertexColor;

/// Per-vertex color (the mesh attribute `color`, three 0-255 values) drawn translucent,
/// with one opacity for the whole surface: `uniforms.values[0]`, from 0 (invisible) to 1.
pub mod vertex_color_translucent {
    use crate::shader::{AttribDesc, Shader, Uniforms};

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
        fn shade(_: &Fixed32, b: &Fixed16, _: &Floats, uni: &Uniforms) -> u32 {
            let channel = |v: i16| ((v as u16) >> 8) as u32;
            let alpha = (uni.values[0].clamp(0.0, 1.0) * 255.0).round() as u32;
            alpha << 24 | channel(b.color[0]) << 16 | channel(b.color[1]) << 8 | channel(b.color[2])
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
/// the rest, so its alpha is `1 - F`. cos comes from the exact view vector at each sample
/// point (`per_sample`, from the built-in `position` varying), is stepped per pixel in
/// 8.8 like a color channel, and indexes a 256-entry table of `(1 - cos)^5`.
pub mod vertex_color_fresnel {
    use crate::shader::{AttribDesc, Shader, Uniforms};

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

    /// `1 - (1 - cos)^FALLOFF` for cos = i / 255, in 8.8 (256 = 1): the share of `1 - F0`
    /// that the surface's own color keeps.
    pub(crate) const KEEP: [u16; 256] = {
        let mut table = [0u16; 256];
        let mut i = 0;
        while i < 256 {
            let c = 1.0 - i as f32 / 255.0;
            let mut power = 1.0;
            let mut k = 0;
            while k < FALLOFF {
                power *= c;
                k += 1;
            }
            table[i] = ((1.0 - power) * 256.0 + 0.5) as u16;
            i += 1;
        }
        table
    };

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
            // Stored as cos * 255 plus a half, so the table index (the integer part) rounds.
            v[FACING] = cos.clamp(0.0, 1.0) * 255.0 + 0.5;
        }

        #[inline(always)]
        fn shade(_: &Fixed32, b: &Fixed16, _: &Floats, uni: &Uniforms) -> u32 {
            let channel = |v: i16| ((v as u16) >> 8) as u32;
            let keep = KEEP[channel(b.facing[0]) as usize] as u32;
            let scale = ((1.0 - uni.values[6].clamp(0.0, 1.0)) * 255.0) as u32;
            let alpha = (scale * keep + 128) >> 8;
            alpha << 24 | channel(b.color[0]) << 16 | channel(b.color[1]) << 8 | channel(b.color[2])
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
    use super::vertex_color_fresnel::KEEP;
    use crate::shader::{AttribDesc, Shader, Uniforms};

    crate::varyings! {
        fixed32 {}
        fixed16 { color: 3, facing: 1 }
        float { position: 3, distance: 1 }
    }

    /// Offsets of `facing`, `position` and `distance` in the layout's values.
    const FACING: usize = 3;
    const POSITION: usize = 4;
    const DISTANCE: usize = 7;

    /// How much of the reflection is left `bounce` meters past the surface, with the fade
    /// range `range` (0 for no fade).
    #[inline(always)]
    pub fn fade(bounce: f32, range: f32) -> f32 {
        if range > 0.0 {
            let t = 1.0 - (bounce / range).clamp(0.0, 1.0);
            t * t
        } else {
            1.0
        }
    }

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
            // Stored as cos * 255 plus a half, so the table index (the integer part) rounds.
            v[FACING] = cos.clamp(0.0, 1.0) * 255.0 + 0.5;
            v[DISTANCE] = distance;
        }

        /// Without what is behind: Fresnel only, no fade.
        #[inline(always)]
        fn shade(a: &Fixed32, b: &Fixed16, c: &Floats, uni: &Uniforms) -> u32 {
            Self::shade_over(a, b, c, uni, 0.0, 0.0)
        }

        #[inline(always)]
        fn shade_over(
            _: &Fixed32,
            b: &Fixed16,
            c: &Floats,
            uni: &Uniforms,
            w: f32,
            behind_w: f32,
        ) -> u32 {
            let channel = |v: i16| ((v as u16) >> 8) as u32;
            let keep = KEEP[channel(b.facing[0]) as usize] as f32 * (1.0 / 256.0);
            let reflected = 1.0 - (1.0 - uni.values[6].clamp(0.0, 1.0)) * keep;
            let faded = if behind_w > 0.0 {
                reflected * fade(c.distance[0] * (w / behind_w - 1.0), uni.values[7])
            } else {
                reflected
            };
            let alpha = ((1.0 - faded) * 255.0 + 0.5) as u32;
            alpha << 24 | channel(b.color[0]) << 16 | channel(b.color[1]) << 8 | channel(b.color[2])
        }
    }
}

pub use vertex_color_fresnel_disperse::VertexColorFresnelDisperse;
