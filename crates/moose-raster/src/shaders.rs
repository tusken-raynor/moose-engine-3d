//! Standard materials.

use crate::shader::{F32s, Fill, I16s, I32s, SampleContext, U32s};

/// The light reaching [`LANES`](crate::shader::LANES) sample points on a surface facing
/// `normal` (unit length), in linear RGB (1 shows a surface's full color): the ambient light,
/// plus each light that reaches the polygon, by Lambert's cosine law, a smooth falloff to
/// nothing at its range, `(1 - d^2 / range^2)^2`, and a spot light's cone (smoothstep from
/// its outer half-angle to its inner; a point light's cone is whole).
///
/// Evaluated at sample points only, and interpolated to pixels in between.
#[inline(always)]
fn diffuse(ctx: &SampleContext, position: &[F32s; 3], normal: &[F32s; 3]) -> [F32s; 3] {
    let a = ctx.ambient;
    let mut light = [F32s::fill(a.x), F32s::fill(a.y), F32s::fill(a.z)];
    let (zero, one) = (F32s::fill(0.0), F32s::fill(1.0));
    for (i, l) in ctx.lights.iter().enumerate() {
        let d = [
            F32s::fill(l.position.x) - position[0],
            F32s::fill(l.position.y) - position[1],
            F32s::fill(l.position.z) - position[2],
        ];
        let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        let t = (one - d2 * F32s::fill(1.0 / (l.range * l.range))).max(zero);
        // cos = n.d / |d|: one reciprocal square root, no division.
        let n_dot_d = normal[0] * d[0] + normal[1] * d[1] + normal[2] * d[2];
        let inv_len = d2.max(F32s::fill(1e-8)).recip_sqrt();
        let cos = (n_dot_d * inv_len).max(zero);
        // The cone: 1 within the inner half-angle, easing to 0 at the outer (always 1 for a
        // point light, whose cone is whole). The way from the light to the point is -d.
        let (scale, offset) = l.sample_cone();
        let d_dot_dir =
            d[0] * F32s::fill(l.direction.x) + d[1] * F32s::fill(l.direction.y) + d[2] * F32s::fill(l.direction.z);
        let c = (F32s::fill(offset) - d_dot_dir * inv_len * F32s::fill(scale))
            .max(zero)
            .min(one);
        let cone = c * c * (F32s::fill(3.0) - c - c);
        let k = t * t * cos * cone;
        // A light whose shadow covers part of the polygon is in the sum like any other,
        // and its strength goes to the engine too, which takes it back out pixel by pixel
        // as much as the shadow covers it.
        if let Some(&j) = ctx.light_split.get(i)
            && let Some(split) = ctx.split.get(j as usize)
        {
            split.set(k);
        }
        light[0] += F32s::fill(l.color.x) * k;
        light[1] += F32s::fill(l.color.y) * k;
        light[2] += F32s::fill(l.color.z) * k;
    }
    light
}

/// The display's gamma: textures, vertex colors and the framebuffer hold gamma-encoded
/// values, `linear^(1 / GAMMA)`.
#[cfg(test)]
pub(crate) const GAMMA: f32 = 2.2;

/// Linear `light` encoded for a 16.16 `light` output: its square root, gamma 2 (close to the
/// display's 2.2). A gamma-encoded color times the encoded light is the gamma-encoded color
/// of the lit surface (a power law commutes with products), so pixels need no conversion;
/// and the light is interpolated between sample points in nearly the display's own terms,
/// where its steps are even to the eye. Gamma 2 exactly (not 2.2) so that pixels can take a
/// light back out of the sum exactly, by squaring, subtracting and taking the root again
/// (see `PixelContext::light`).
#[inline(always)]
fn light_output(light: [F32s; 3]) -> [F32s; 3] {
    light.map(encode_light)
}

/// Linear light encoded (see [`light_output`]), one channel.
#[inline(always)]
pub(crate) fn encode_light(l: F32s) -> F32s {
    l.max(F32s::fill(0.0)).sqrt()
}

/// XRGB from a `color` output (three 8.8 channels, 0-255) under the 16.16 `light` (see
/// [`light_output`]), each channel at most 255. Color and light are interpolated apart and
/// multiplied per pixel: clamping lit colors at sample points would bend them near edges,
/// where sample points past the polygon carry values beyond 255 (which the wrapping 8.8
/// stepping brings back in range inside it).
#[inline(always)]
fn lit_rgb(color: &[I16s; 3], light: &[I32s; 3]) -> U32s {
    use crate::shader::high_byte;
    let byte = U32s::fill(255);
    let channel = |k: usize| {
        let l: U32s = wide::bytemuck::cast(light[k]);
        ((high_byte(color[k]) * l) >> 16_u32).min(byte)
    };
    channel(0) << 16 | channel(1) << 8 | channel(2)
}

/// `texel`'s color channels under the 16.16 `light` (see [`light_output`]; 65536 is 1: a
/// light of exactly 1 leaves the texel as it is), each at most 255; its alpha is kept.
#[inline(always)]
fn lit_texel(texel: U32s, light: &[I32s; 3]) -> U32s {
    let byte = U32s::fill(255);
    let channel = |shift: u32, k: usize| {
        let l: U32s = wide::bytemuck::cast(light[k]);
        (((((texel >> shift) & byte) * l) >> 16_u32).min(byte)) << shift
    };
    (texel & U32s::fill(0xFF00_0000)) | channel(16, 0) | channel(8, 1) | channel(0, 2)
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

/// Per-vertex color: the mesh attribute `color` (three 0-255 values), lit at sample points
/// (see [`diffuse`]) and interpolated in 8.8 fixed point.
pub mod vertex_color {
    use crate::shader::{Material, PixelContext, SampleContext, U32s, VertexContext};

    crate::material_io! {
        vertex { color: 3, face_normal: 3 }
        sampled { color: 3, normal: 3, position: 3 }
        fixed32 { light: 3 }
        fixed16 { color: 3 }
        float {}
    }

    pub struct VertexColor;

    impl Material for VertexColor {
        crate::material_types!();

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
            let light = super::diffuse(ctx, &s.position, &s.normal);
            Interp {
                color: s.color,
                light: super::light_output(light),
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
            super::lit_rgb(&b.color, &ctx.light(&a.light))
        }
    }
}

pub use vertex_color::VertexColor;

/// Per-vertex color times a detail texture, smoothly lit: for terrain. The mesh's `color`
/// (three 0-255 values) is multiplied by texture 0 (from `uv`) at twice its value, so a
/// texel of 128 leaves the color as it is and the texture adds grain around it. Lighting
/// uses the mesh's own `normal`s, interpolated to the sample points (Gouraud shading),
/// rather than the polygon's.
pub mod vertex_color_detail {
    use crate::shader::{F32s, Fill, Material, PixelContext, SampleContext, U32s, VertexContext, high_byte};

    crate::material_io! {
        vertex { color: 3, uv: 2, normal: 3 }
        sampled { color: 3, uv: 2, lod: 1, normal: 3, position: 3 }
        fixed32 { uv: 2, light: 3 }
        fixed16 { color: 3, lod: 1 }
        float {}
    }

    pub struct VertexColorDetail;

    impl Material for VertexColorDetail {
        crate::material_types!();

        #[inline(always)]
        fn shade_vertex(v: &Vertex, ctx: &VertexContext) -> Sampled {
            let n = ctx.object.rotation * glam::Vec3::from_array(v.normal);
            Sampled {
                color: v.color,
                uv: v.uv,
                normal: n.to_array(),
                ..Default::default()
            }
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, ctx: &SampleContext) -> Interp {
            // Interpolated normals are shorter between the vertices.
            let n = &s.normal;
            let inv = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).max(F32s::fill(1e-12)).recip_sqrt();
            let light = super::diffuse(ctx, &s.position, &n.map(|c| c * inv));
            Interp {
                uv: s.uv,
                lod: s.lod,
                color: s.color,
                light: super::light_output(light),
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
            let texel = ctx.texel(0, &a.uv, b.lod[0]);
            let light = ctx.light(&a.light);
            let byte = U32s::fill(255);
            let channel = |shift: u32, k: usize| {
                // Color times twice the texel (at most 510), then the light.
                let c = (high_byte(b.color[k]) * ((texel >> shift) & byte)) >> 7_u32;
                let l: U32s = wide::bytemuck::cast(light[k]);
                ((c * l) >> 16_u32).min(byte) << shift
            };
            channel(16, 0) | channel(8, 1) | channel(0, 2)
        }
    }
}

pub use vertex_color_detail::VertexColorDetail;

/// [`VertexColorDetail`] with its grain from 3D noise instead of a texture: smooth value
/// noise in world space (random values at the corners of a grid of cubes, blended
/// between), a few octaves of it, so it needs no texture coordinates, has no seams where
/// polygons meet, and doesn't stretch on steep slopes. For things that never move (it's
/// anchored to the world, not the mesh).
///
/// Each octave fades out where a pixel covers more than half of its cells, from how
/// far away the surface is and how steeply it's seen (worked out at the sample points), so
/// far ground doesn't sparkle.
///
/// Params: strength (`values[0]`: how far the grain takes colors either way, about; 0
/// for none).
pub mod vertex_color_noise {
    use crate::shader::{F32s, Fill, I32s, Material, PixelContext, SampleContext, U32s, VertexContext, high_byte};

    crate::material_io! {
        vertex { color: 3, normal: 3 }
        sampled { color: 3, normal: 3, position: 3 }
        fixed32 { position: 3, light: 3 }
        fixed16 { color: 3, footprint: 1 }
        float {}
    }

    /// Octaves, and the coarsest's cells (log2 of their size in meters: 1 m); each next
    /// one's are half the size, and its strength `PERSISTENCE` times.
    const OCTAVES: i32 = 5;
    const COARSEST: i32 = 0;
    const PERSISTENCE: f32 = 0.7;

    pub struct VertexColorNoise;

    impl Material for VertexColorNoise {
        crate::material_types!();

        #[inline(always)]
        fn shade_vertex(v: &Vertex, ctx: &VertexContext) -> Sampled {
            let n = ctx.object.rotation * glam::Vec3::from_array(v.normal);
            Sampled {
                color: v.color,
                normal: n.to_array(),
                ..Default::default()
            }
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, ctx: &SampleContext) -> Interp {
            let n = &s.normal;
            let inv = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).max(F32s::fill(1e-12)).recip_sqrt();
            let n = n.map(|c| c * inv);
            let light = super::diffuse(ctx, &s.position, &n);
            // A pixel's size on the surface, in meters (log2): its size at that distance,
            // stretched as the surface turns away (at most twice).
            let to_eye = [
                F32s::fill(ctx.eye.x) - s.position[0],
                F32s::fill(ctx.eye.y) - s.position[1],
                F32s::fill(ctx.eye.z) - s.position[2],
            ];
            let d2 = to_eye[0] * to_eye[0] + to_eye[1] * to_eye[1] + to_eye[2] * to_eye[2];
            let distance = d2.max(F32s::fill(1e-12)).sqrt();
            let cos = ((n[0] * to_eye[0] + n[1] * to_eye[1] + n[2] * to_eye[2]) / distance).abs();
            let size = distance / (F32s::fill(ctx.focal) * cos.max(F32s::fill(0.5)));
            let footprint = size.max(F32s::fill(1e-6)).ln() * F32s::fill(std::f32::consts::LOG2_E);
            Interp {
                color: s.color,
                footprint: [footprint.max(F32s::fill(-100.0)).min(F32s::fill(100.0))],
                position: s.position,
                light: super::light_output(light),
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
            let footprint = I32s::from_i16x8(b.footprint[0]);
            // Each lane's cell of an octave (cells `2^log2` meters) along one axis, and where
            // in it the point is (0 to 256, smoothstepped), from 16.16 meters.
            let cell = |c: I32s, log2: i32| {
                let s = 16 + log2;
                let cell: U32s = wide::bytemuck::cast(c >> s);
                let f: U32s = wide::bytemuck::cast(((c & I32s::fill((1 << s) - 1)) << 8) >> s);
                (cell, (f * f * (U32s::fill(768) - f - f)) >> 16)
            };
            // A hash of a cell corner, 0 to 255: one round of mixing, enough after the
            // corner's coordinates are spread by large odd keys.
            const K: [u32; 3] = [0x8DA6_B343, 0xD816_3841, 0xCB1A_B31F];
            let hash = |h: U32s| {
                let h: I32s = wide::bytemuck::cast(((h ^ (h >> 15)) * U32s::fill(0x846C_A68B)) >> 24);
                h
            };
            // Between `a` and `b` (0 to 255) by `f` (0 to 256): one multiply.
            let lerp = |a: I32s, b: I32s, f: I32s| a + (((b - a) * f) >> 8_i32);
            let mut sum = I32s::fill(0);
            let mut amount = ctx.params.values[0].clamp(0.0, 1.0);
            for k in 0..OCTAVES {
                let log2 = COARSEST - k;
                // Faded out (0) where a pixel covers a cell, in (256) by half of one. Each
                // octave fades before the coarser one, so once no lane has this one, none
                // has the rest.
                let fade = (I32s::fill(log2 << 8) - footprint).max(I32s::fill(0)).min(I32s::fill(256));
                if fade == I32s::fill(0) {
                    break;
                }
                let (x, fx) = cell(a.position[0], log2);
                let (y, fy) = cell(a.position[1], log2);
                let (z, fz) = cell(a.position[2], log2);
                let f = |f: U32s| -> I32s { wide::bytemuck::cast(f) };
                let (fx, fy, fz) = (f(fx), f(fy), f(fz));
                let seed = U32s::fill(k as u32 * 0x9E37_79B9);
                let (x0, y0, z0) = (x * U32s::fill(K[0]), y * U32s::fill(K[1]), (z * U32s::fill(K[2])) ^ seed);
                let (x1, y1) = (x0 + U32s::fill(K[0]), y0 + U32s::fill(K[1]));
                let z1 = ((z * U32s::fill(K[2])) + U32s::fill(K[2])) ^ seed;
                let (c00, c10, c01, c11) = (x0 ^ y0, x1 ^ y0, x0 ^ y1, x1 ^ y1);
                let face = |z: U32s| {
                    let near = lerp(hash(c00 ^ z), hash(c10 ^ z), fx);
                    let far = lerp(hash(c01 ^ z), hash(c11 ^ z), fx);
                    lerp(near, far, fy)
                };
                let noise = lerp(face(z0), face(z1), fz) - I32s::fill(128);
                sum += noise * fade * I32s::fill((amount * 4096.0).round() as i32);
                amount *= PERSISTENCE;
            }
            // Brightness scale, 8.8: 1 + the sum (noise / 128 * fade / 256 * strength).
            let scaled: I32s = I32s::fill(256) + (sum >> 19_i32);
            let scale: U32s = wide::bytemuck::cast(scaled.max(I32s::fill(0)));
            let light = ctx.light(&a.light);
            let byte = U32s::fill(255);
            let channel = |shift: u32, k: usize| {
                let c = (high_byte(b.color[k]) * scale) >> 8_u32;
                let l: U32s = wide::bytemuck::cast(light[k]);
                ((c * l) >> 16_u32).min(byte) << shift
            };
            channel(16, 0) | channel(8, 1) | channel(0, 2)
        }
    }
}

pub use vertex_color_noise::VertexColorNoise;

/// Per-vertex color (the mesh attribute `color`, three 0-255 values), unlit: shown as it
/// is, whatever lights there are (a sky, say).
pub mod unlit_color {
    use crate::shader::{Material, PixelContext, SampleContext, U32s, VertexContext, high_byte};

    crate::material_io! {
        vertex { color: 3 }
        sampled { color: 3 }
        fixed32 {}
        fixed16 { color: 3 }
        float {}
    }

    pub struct UnlitColor;

    impl Material for UnlitColor {
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
            high_byte(b.color[0]) << 16 | high_byte(b.color[1]) << 8 | high_byte(b.color[2])
        }
    }
}

pub use unlit_color::UnlitColor;

/// Per-vertex color (the mesh attribute `color`, three 0-255 values, lit as
/// [`VertexColor`]) drawn translucent, with one opacity for the whole surface:
/// `params.values[0]`, from 0 (invisible) to 1.
pub mod vertex_color_translucent {
    use crate::shader::{Fill, Material, PixelContext, SampleContext, U32s, VertexContext};

    crate::material_io! {
        vertex { color: 3, face_normal: 3 }
        sampled { color: 3, normal: 3, position: 3 }
        fixed32 { light: 3 }
        fixed16 { color: 3 }
        float {}
    }

    pub struct VertexColorTranslucent;

    impl Material for VertexColorTranslucent {
        crate::material_types!();
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
            let light = super::diffuse(ctx, &s.position, &s.normal);
            Interp {
                color: s.color,
                light: super::light_output(light),
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
            let alpha = (ctx.params.values[0].clamp(0.0, 1.0) * 255.0).round() as u32;
            U32s::fill(alpha << 24) | super::lit_rgb(&b.color, &ctx.light(&a.light))
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
        fixed32 { light: 3 }
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
            let light = super::diffuse(ctx, &s.position, &s.normal);
            Interp {
                color: s.color,
                facing: [facing],
                light: super::light_output(light),
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
            // alpha = (1 - F0) * keep, in 1/64 steps of a 0-255 alpha, then rounded.
            let f0 = ctx.params.values[0].clamp(0.0, 1.0);
            let scale = ((1.0 - f0) * 255.0 * 64.0).round() as i16;
            let alpha = keep(b.facing[0]).mul_scale_round(I16s::fill(scale));
            let alpha = widen(alpha + I16s::fill(32)) >> 6;
            (alpha << 24) | super::lit_rgb(&b.color, &ctx.light(&a.light))
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
        fixed32 { light: 3 }
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
            let light = super::diffuse(ctx, &s.position, &s.normal);
            Interp {
                color: s.color,
                facing: [facing],
                light: super::light_output(light),
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
            a: &Fixed32,
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
            (alpha << 24) | super::lit_rgb(&b.color, &ctx.light(&a.light))
        }
    }
}

pub use vertex_color_fresnel_disperse::VertexColorFresnelDisperse;

/// Texture samplers for the textured materials, as their `FILTER` parameter.
pub use crate::shader::filter;


/// The coarsest detail noise's cells across one of the texture's full-size texels (each
/// way), as log2.
pub const DETAIL_LOG2: i32 = 3;
/// Octaves of detail noise, each with cells half the size of the one before: the finest
/// has 2^(`DETAIL_LOG2` + `DETAIL_OCTAVES` - 1) across a texel.
pub const DETAIL_OCTAVES: i32 = 4;
/// Each octave's strength over the one before's.
const DETAIL_PERSISTENCE: f32 = 0.7;

/// `texel` (texture `tex`'s color at `uv`, level of detail `lod`) with procedural detail
/// where the texture is magnified: its brightness scaled by fractal value noise anchored to
/// the texture, so it moves with the surface. Each octave ([`DETAIL_OCTAVES`], the first
/// [`DETAIL_LOG2`] finer than the texels) fades in from where its cells are one pixel across
/// to where they are two, so the closer the view, the finer the grain it adds, and none of
/// it shimmers. The first octave scales brightness by up to `strength` either way (1 is from
/// black to double), each finer one by [`DETAIL_PERSISTENCE`] as much. Each octave's values
/// are blended across its cells with smoothstep, so its cells show no edges. `mask` (0 to
/// 255 per lane) scales it all: 0 for none, 255 for full. Octaves no lane of a block is close
/// enough for cost one compare. Alpha is kept.
#[inline(always)]
pub fn detail(
    texel: crate::shader::U32s,
    tex: &moose_assets::Texture,
    uv: &[crate::shader::I32s; 2],
    lod: crate::shader::I16s,
    strength: f32,
    mask: crate::shader::U32s,
) -> crate::shader::U32s {
    use crate::shader::{Fill, I32s, U32s};
    if strength <= 0.0 || mask == U32s::fill(0) {
        return texel;
    }
    let lod = I32s::from_i16x8(lod);
    let base = &tex.levels[0];
    // Each lane's cell of octave `k` and where in it the point is (0 to 256, smoothstepped),
    // from 16.16 `uv` with 1.0 across the texture.
    let cell = |c: I32s, log2: u32, k: i32| {
        let s = (16 - (log2 as i32 + DETAIL_LOG2 + k)).max(1);
        let cell: U32s = wide::bytemuck::cast(c >> s);
        let f: U32s = wide::bytemuck::cast(((c & I32s::fill((1 << s) - 1)) << 8) >> s);
        (cell, (f * f * (U32s::fill(768) - f - f)) >> 16)
    };
    // A hash of a cell corner (lowbias32's mixing, after scrambling x and y apart), its top
    // byte: 0 to 255.
    const KX: u32 = 0x8DA6_B343;
    const KY: u32 = 0xD816_3841;
    let hash = |h: U32s| {
        let h = (h ^ (h >> 16)) * U32s::fill(0x7FEB_352D);
        ((h ^ (h >> 15)) * U32s::fill(0x846C_A68B)) >> 24
    };
    let lerp = |a: U32s, b: U32s, f: U32s| (a * (U32s::fill(256) - f) + b * f) >> 8;
    // The octaves' sum: noise (-128 to 127) times fade (0 to 256) times strength (Q12).
    let mut sum = I32s::fill(0);
    let mut amount = strength.min(1.0);
    for k in 0..DETAIL_OCTAVES {
        // How far faded in, 0 to 256 (the level of detail is 8.8). Each octave fades in
        // closer than the one before, so once no lane has this one, none has the rest.
        let fade = (I32s::fill(-((DETAIL_LOG2 + k) << 8)) - lod)
            .max(I32s::fill(0))
            .min(I32s::fill(256));
        if fade == I32s::fill(0) {
            break;
        }
        let (x, fx) = cell(uv[0], base.width_log2, k);
        let (y, fy) = cell(uv[1], base.height_log2, k);
        let (x0, y0) = (x * U32s::fill(KX), y * U32s::fill(KY));
        // Each octave its own values (seeded after stepping, so neighboring cells share
        // corners).
        let seed = U32s::fill(k as u32 * 0x9E37_79B9);
        let (x1, y1) = (x0 + U32s::fill(KX), (y0 + U32s::fill(KY)) ^ seed);
        let y0 = y0 ^ seed;
        let top = lerp(hash(x0 ^ y0), hash(x1 ^ y0), fx);
        let bottom = lerp(hash(x0 ^ y1), hash(x1 ^ y1), fx);
        let noise = wide::bytemuck::cast::<U32s, I32s>(lerp(top, bottom, fy)) - I32s::fill(128);
        sum += noise * fade * I32s::fill((amount * 4096.0).round() as i32);
        amount *= DETAIL_PERSISTENCE;
    }
    if sum == I32s::fill(0) {
        return texel;
    }
    // Brightness scale, 8.8: 1 + the sum (noise / 128 * fade / 256 * strength / 4096), by
    // the mask (255 as a whole 1).
    let mask = wide::bytemuck::cast::<U32s, I32s>(mask + (mask >> 7));
    let scale: U32s = wide::bytemuck::cast(I32s::fill(256) + (((sum >> 19) * mask) >> 8));
    let channel = |shift: u32| -> U32s {
        let c: U32s = ((texel >> shift) & U32s::fill(255)) * scale;
        (c >> 8_u32).min(U32s::fill(255)) << shift
    };
    (texel & U32s::fill(0xFF00_0000)) | channel(16) | channel(8) | channel(0)
}

/// Texture color from the mesh attribute `uv` (two coordinates, 1.0 across the texture,
/// which tiles), read with sampler `FILTER` (see [`filter`]; bilinear with blended mip
/// levels by default), opaque. The texture's alpha masks the detail noise (see [`detail`]):
/// 0 for none, 255 for full.
///
/// Params: detail strength (`values[0]`, 0 for none).
pub mod textured {
    use crate::shader::{Fill, Material, PixelContext, SampleContext, U32s, VertexContext};

    crate::material_io! {
        vertex { uv: 2, face_normal: 3 }
        sampled { uv: 2, lod: 1, normal: 3, position: 3 }
        fixed32 { uv: 2, light: 3 }
        fixed16 { lod: 1 }
        float {}
    }

    /// Sampler `FILTER`, from [`super::filter`].
    pub struct Textured;

    impl Material for Textured {
        crate::material_types!();

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
            let light = super::diffuse(ctx, &s.position, &s.normal);
            Interp {
                uv: s.uv,
                lod: s.lod,
                light: super::light_output(light),
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
            let tex = ctx.textures[0];
            let texel = ctx.texel(0, &a.uv, b.lod[0]);
            let strength = ctx.params.values[0];
            let detailed = super::detail(texel, tex, &a.uv, b.lod[0], strength, texel >> 24_u32);
            super::lit_texel(detailed, &ctx.light(&a.light)) & U32s::fill(0xFF_FFFF)
        }
    }
}

pub use textured::Textured;


/// How a bumpy surface's light differs from a flat one's at [`LANES`](crate::shader::LANES)
/// sample points, from a group of the lights that reach it, each weighted by its
/// brightness (the luminance of its color times its falloff and cone, as [`diffuse`] has
/// them): in tangent space (`tangent`, `bitangent`, `normal`: x along the texture's u, y
/// along its v, z out), over the group's brightness on a flat surface (ambient included),
/// so that a flat texel's factor is exactly 1.
#[derive(Clone, Copy)]
struct Bumps {
    /// The ambient light's share of the flat brightness.
    ambient: F32s,
    /// The sum of the lights' directions (toward them, unit length) times their share of
    /// the flat brightness: lights in front of the surface only.
    toward: [F32s; 3],
    /// The lights' color (linear) where they reach, before any surface's cosine: lights in
    /// front of the surface only. A highlight's.
    color: [F32s; 3],
}

/// [`Bumps`] of the lights that always reach the polygon's pixels (`rest`: all but its
/// split lights, which shadows or a beam's cone cut pixel by pixel; see
/// `SampleContext::light_split`), and of all of them (`all`). Pixels bump the two lights
/// apart, then blend them as much as the split lights reach (see
/// `PixelContext::light_scaled`): a light that doesn't reach a pixel doesn't bump it, and
/// the others still do.
#[inline(always)]
fn bumps(ctx: &SampleContext, position: &[F32s; 3], frame: [&[F32s; 3]; 3]) -> (Bumps, Bumps) {
    let (zero, one) = (F32s::fill(0.0), F32s::fill(1.0));
    let luma = |c: glam::Vec3| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
    let ambient = F32s::fill(luma(ctx.ambient));
    // Per group (rest, all): the flat brightness, the summed directions, the color.
    let mut flat = [ambient; 2];
    let mut toward = [[zero; 3]; 2];
    let mut color = [[zero; 3]; 2];
    let dot = |a: &[F32s; 3], b: &[F32s; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    for (i, l) in ctx.lights.iter().enumerate() {
        let d = [
            F32s::fill(l.position.x) - position[0],
            F32s::fill(l.position.y) - position[1],
            F32s::fill(l.position.z) - position[2],
        ];
        let d2 = dot(&d, &d);
        let t = (one - d2 * F32s::fill(1.0 / (l.range * l.range))).max(zero);
        let inv_len = d2.max(F32s::fill(1e-8)).recip_sqrt();
        let (scale, offset) = l.sample_cone();
        let dir = [F32s::fill(l.direction.x), F32s::fill(l.direction.y), F32s::fill(l.direction.z)];
        let c = (F32s::fill(offset) - dot(&d, &dir) * inv_len * F32s::fill(scale)).max(zero).min(one);
        let reach = t * t * c * c * (F32s::fill(3.0) - c - c);
        let k = reach * F32s::fill(luma(l.color));
        // Toward the light, in tangent space.
        let to = frame.map(|axis| dot(axis, &d) * inv_len);
        let front = to[2].max(zero);
        let lit = k * front.simd_gt(zero).select(one, zero);
        // A split light (as `diffuse` tells them) is in all only.
        let split = ctx.light_split.get(i).is_some_and(|&j| (j as usize) < ctx.split.len());
        for g in if split { 1..2 } else { 0..2 } {
            flat[g] += k * front;
            for (sum, v) in toward[g].iter_mut().zip(to) {
                *sum += lit * v;
            }
            let shines = reach * front.simd_gt(zero).select(one, zero);
            for (sum, l) in color[g].iter_mut().zip([l.color.x, l.color.y, l.color.z]) {
                *sum += shines * F32s::fill(l);
            }
        }
    }
    let group = |g: usize| {
        let over = flat[g].max(F32s::fill(1e-6)).recip();
        Bumps {
            ambient: ambient * over,
            toward: toward[g].map(|v| v * over),
            color: color[g],
        }
    };
    (group(0), group(1))
}

/// A texel's byte `shift` bits up as a float, 0 to 255.
#[inline(always)]
fn channel(texel: U32s, shift: u32) -> F32s {
    let b: I32s = wide::bytemuck::cast((texel >> shift) & U32s::fill(255));
    b.round_float()
}

/// [`Textured`] with bumps and specular highlights, in any combination: `BUMP` is where
/// the bumps come from ([`FLAT`] or [`NORMAL_MAP`]) and `SPECULAR` adds a highlight. Each light's direction is kept apart from the summed light at the sample
/// points (see [`Bumps`]), for the lights that always reach and for all of them, and
/// pixels weigh those by their texel's bumps:
///
/// - Bumps are a factor on the light, 1 on a flat texel. With a normal map (texture 1's
///   color, `(n + 1) / 2`), the texel's normal's cosine with the lights' directions summed
///   into one.
/// - The highlight is scaled by texture 0's alpha (255 full, 0 none), so parts of a
///   texture, such as the mortar between bricks, can be dull.
/// - The highlight is Blinn-Phong's, from the lights' summed direction (the dominant one):
///   the texel's normal's cosine with the half vector between it and the eye's direction,
///   to a power, times the lights' color and a strength, added to the lit color. One
///   highlight per group of lights, whatever their number.
///
/// The normal map is read with texture 1's sampler. Params: the highlight's strength
/// (`values[0]`) and its power's log2 (`values[1]`: 5 is a power of 32).
pub mod textured_lit {
    use crate::shader::{F32s, Fill, I32s, Material, PixelContext, SampleContext, U32s, VertexContext};

    /// Where [`TexturedLit`]'s bumps come from: none, or a normal map in texture 1's
    /// color.
    pub const FLAT: u8 = 0;
    pub const NORMAL_MAP: u8 = 1;

    crate::material_io! {
        vertex { uv: 2, face_normal: 3, face_tangent: 3, face_bitangent: 3 }
        sampled { uv: 2, lod: 1, normal: 3, tangent: 3, bitangent: 3, position: 3 }
        fixed32 { uv: 2, light: 3 }
        fixed16 { lod: 1 }
        float { bump: 8, spec: 12 }
    }

    /// Textured, bumpy and shiny as `BUMP` and `SPECULAR` say (see the module).
    pub struct TexturedLit<const BUMP: u8, const SPECULAR: bool>;

    /// The configurations the app uses.
    pub type TexturedNormal = TexturedLit<NORMAL_MAP, false>;
    pub type TexturedSpecular = TexturedLit<FLAT, true>;
    pub type TexturedNormalSpecular = TexturedLit<NORMAL_MAP, true>;

    /// Bumps past this level of detail (8.8: textures shrunk to a third or less) are
    /// smaller than pixels: those pixels are lit flat.
    const FAR: i32 = 400;

    /// The half vector between the lights' summed direction (`toward`, any length) and
    /// the way to the eye (`eye`, unit length), unit length: or the way to the eye, where
    /// no light comes from any one way.
    #[inline(always)]
    fn half(toward: &[F32s; 3], eye: &[F32s; 3]) -> [F32s; 3] {
        let len2 = toward[0] * toward[0] + toward[1] * toward[1] + toward[2] * toward[2];
        let inv = len2.max(F32s::fill(1e-12)).recip_sqrt();
        let lit = len2.simd_gt(F32s::fill(1e-12));
        let h: [F32s; 3] = std::array::from_fn(|c| eye[c] + lit.select(toward[c] * inv, F32s::fill(0.0)));
        let inv = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).max(F32s::fill(1e-12)).recip_sqrt();
        h.map(|v| v * inv)
    }

    impl<const BUMP: u8, const SPECULAR: bool> Material for TexturedLit<BUMP, SPECULAR> {
        crate::material_types!();

        #[inline(always)]
        fn shade_vertex(v: &Vertex, _: &VertexContext) -> Sampled {
            Sampled {
                uv: v.uv,
                normal: v.face_normal,
                tangent: v.face_tangent,
                bitangent: v.face_bitangent,
                ..Default::default()
            }
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, ctx: &SampleContext) -> Interp {
            let frame = [&s.tangent, &s.bitangent, &s.normal];
            let (rest, all) = super::bumps(ctx, &s.position, frame);
            let zero = F32s::fill(0.0);
            let bump = match BUMP {
                NORMAL_MAP => [
                    rest.ambient, rest.toward[0], rest.toward[1], rest.toward[2],
                    all.ambient, all.toward[0], all.toward[1], all.toward[2],
                ],
                _ => [zero; 8],
            };
            let spec = if SPECULAR {
                // The way to the eye, in tangent space.
                let to_eye = [
                    F32s::fill(ctx.eye.x) - s.position[0],
                    F32s::fill(ctx.eye.y) - s.position[1],
                    F32s::fill(ctx.eye.z) - s.position[2],
                ];
                let inv = (to_eye[0] * to_eye[0] + to_eye[1] * to_eye[1] + to_eye[2] * to_eye[2])
                    .max(F32s::fill(1e-12))
                    .recip_sqrt();
                let eye = frame.map(|axis| (axis[0] * to_eye[0] + axis[1] * to_eye[1] + axis[2] * to_eye[2]) * inv);
                let (hr, ha) = (half(&rest.toward, &eye), half(&all.toward, &eye));
                [hr[0], hr[1], hr[2], rest.color[0], rest.color[1], rest.color[2], ha[0], ha[1], ha[2], all.color[0], all.color[1], all.color[2]]
            } else {
                [zero; 12]
            };
            Interp {
                uv: s.uv,
                lod: s.lod,
                light: super::light_output(super::diffuse(ctx, &s.position, &s.normal)),
                bump,
                spec,
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, c: &Floats, ctx: &PixelContext) -> U32s {
            let texel = ctx.texel(0, &a.uv, b.lod[0]);
            let one = F32s::fill(1.0);
            let far = I32s::from_i16x8(b.lod[0]).simd_gt(I32s::fill(FAR)).all();
            // Which lights bump it: those that always reach, if any has a direction (not
            // the ambient light only), and the split ones, if any reaches.
            let rest = match BUMP {
                NORMAL_MAP => (c.bump[1] * c.bump[1] + c.bump[2] * c.bump[2] + c.bump[3] * c.bump[3])
                    .simd_ge(F32s::fill(1e-4))
                    .any(),
                _ => false,
            };
            let all = ctx.split.count > 0 && (0..ctx.split.count).any(|j| ctx.split.reaches[j].simd_gt(F32s::fill(0.0)).any());
            let bumpy = BUMP != FLAT && !far && (rest || all || SPECULAR);
            // The texel's normal (tangent space; z out), and its bumps' factors on the
            // lights that always reach and on all of them.
            let (mut normal, mut factors) = ([F32s::fill(0.0), F32s::fill(0.0), one], (one, one));
            if bumpy {
                let lod = b.lod[0];
                let n = ctx.texel(1, &a.uv, lod);
                let k = F32s::fill(1.0 / 127.5);
                normal = [16, 8, 0].map(|shift| (super::channel(n, shift) - F32s::fill(127.5)) * k);
                let factor = |g: usize| {
                    let cos = normal[0] * c.bump[g + 1] + normal[1] * c.bump[g + 2] + normal[2] * c.bump[g + 3];
                    c.bump[g] + cos.max(F32s::fill(0.0))
                };
                let rested = if rest { factor(0) } else { one };
                factors = (rested, if all { factor(4) } else { rested });
            }
            let light = if bumpy {
                let encode = |f: F32s| f.max(F32s::fill(0.0)).sqrt();
                ctx.light_scaled(&a.light, encode(factors.0), encode(factors.1))
            } else {
                ctx.light(&a.light)
            };
            let lit = super::lit_texel(texel, &light) & U32s::fill(0xFF_FFFF);
            if !SPECULAR {
                return lit;
            }
            // The highlight: per group, its color times the normal's cosine with its half
            // vector (both normalized: interpolated, they shorten between sample points,
            // which would show the sample grid in a sharp highlight) to the power.
            // Texture 0's alpha scales it: the mortar (0) between bricks (255) isn't shiny.
            let gloss = texel >> 24_u32;
            let strength = ctx.params.values[0];
            if strength <= 0.0 || gloss == U32s::fill(0) {
                return lit;
            }
            let squarings = ctx.params.values[1].clamp(0.0, 8.0) as u32;
            let n_len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2])
                .max(F32s::fill(1e-12))
                .recip_sqrt();
            let shine = |g: usize| {
                let h = &c.spec[g..g + 3];
                let h_len = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).max(F32s::fill(1e-12)).recip_sqrt();
                let mut p = ((normal[0] * h[0] + normal[1] * h[1] + normal[2] * h[2]) * n_len * h_len).max(F32s::fill(0.0));
                for _ in 0..squarings {
                    p = p * p;
                }
                [c.spec[g + 3] * p, c.spec[g + 4] * p, c.spec[g + 5] * p]
            };
            let rest_shine = shine(0);
            let shine = if ctx.split.count > 0 {
                let (all_shine, f) = (shine(6), ctx.split_reach());
                std::array::from_fn(|k| rest_shine[k] + (all_shine[k] - rest_shine[k]) * f)
            } else {
                rest_shine
            };
            // Added to the gamma-encoded color as it is (a cheat: light adds in linear
            // terms), each channel at most 255.
            let scale = super::channel(texel, 24) * F32s::fill(strength);
            let add = shine.map(|v| (v * scale).min(F32s::fill(255.0)).round_int());
            let channel = |shift: u32, k: usize| {
                let base: I32s = wide::bytemuck::cast((lit >> shift) & U32s::fill(255));
                let sum: U32s = wide::bytemuck::cast((base + add[k]).min(I32s::fill(255)));
                sum << shift
            };
            channel(16, 0) | channel(8, 1) | channel(0, 2)
        }
    }
}

pub use textured_lit::{TexturedLit, TexturedNormal, TexturedNormalSpecular, TexturedSpecular};

/// [`Textured`], translucent: the texture's color at a uniform opacity, blended over what
/// is behind.
///
/// Params: opacity (`values[0]`, 0 to 1).
pub mod textured_translucent {
    use crate::shader::{Fill, Material, PixelContext, SampleContext, U32s, VertexContext};

    crate::material_io! {
        vertex { uv: 2, face_normal: 3 }
        sampled { uv: 2, lod: 1, normal: 3, position: 3 }
        fixed32 { uv: 2, light: 3 }
        fixed16 { lod: 1 }
        float {}
    }

    /// Sampler `FILTER`, from [`super::filter`].
    pub struct TexturedTranslucent;

    impl Material for TexturedTranslucent {
        crate::material_types!();
        const TRANSLUCENT: bool = true;

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
            let light = super::diffuse(ctx, &s.position, &s.normal);
            Interp {
                uv: s.uv,
                lod: s.lod,
                light: super::light_output(light),
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
            let alpha = (ctx.params.values[0].clamp(0.0, 1.0) * 255.0).round() as u32;
            let texel = super::lit_texel(
                ctx.texel(0, &a.uv, b.lod[0]),
                &ctx.light(&a.light),
            );
            (texel & U32s::fill(0xFF_FFFF)) | U32s::fill(alpha << 24)
        }
    }
}

pub use textured_translucent::TexturedTranslucent;

/// A textured reflective surface: the texture's color drawn over the reflection with a
/// Fresnel falloff, as [`VertexColorFresnel`], with the texture's alpha as roughness: 128
/// (or less) is fully smooth, 255 fully rough, and the reflected share `F` is scaled by
/// the smoothness `(255 - alpha) / 127`. The reflection also fades with how far past the
/// surface it is, as [`VertexColorFresnelDisperse`], except that Fresnel limits the fade:
/// `fade + (1 - fade) F`.
///
/// Params: F0 (`values[0]`, 0 to 1) and the fade range in meters (`values[1]`, 0 for none).
pub mod textured_fresnel {
    use super::vertex_color_fresnel::{ONE, keep};
    use crate::shader::{
        F32s, Fill, I16s, I32s, Material, Over, Params, PixelContext, RowBehind, SampleContext,
        U32s, VertexContext, widen,
    };

    crate::material_io! {
        vertex { uv: 2, face_normal: 3 }
        sampled { uv: 2, normal: 3, position: 3, lod: 1 }
        fixed32 { uv: 2, light: 3 }
        fixed16 { facing: 1, lod: 1 }
        float { distance: 1 }
    }

    /// Sampler `FILTER`, from [`super::filter`].
    pub struct TexturedFresnel;

    impl Material for TexturedFresnel {
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
            let light = super::diffuse(ctx, &s.position, &s.normal);
            Interp {
                uv: s.uv,
                facing: [facing],
                lod: s.lod,
                light: super::light_output(light),
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
            let texel = super::lit_texel(
                ctx.texel(0, &a.uv, b.lod[0]),
                &ctx.light(&a.light),
            );
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
    use super::textured_fresnel::{
        Fixed16, Fixed32, Floats, Interp, Sampled, SampledLanes, TexturedFresnel, Vertex, over,
    };
    use crate::shader::{
        F32s, Fill, I32s, Material, MaterialIo, Over, PixelContext, RowBehind, SampleContext, U32s,
        VertexContext, blend_lanes,
    };

    /// Largest shift, in pixels.
    const MAX_SHIFT: i32 = 32;

    /// The translucent surface, over its reflection. Sampler `FILTER`, from
    /// [`super::filter`].
    pub struct Water;

    impl Material for Water {
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
            TexturedFresnel::shade_vertex(v, ctx)
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, ctx: &SampleContext) -> Interp {
            TexturedFresnel::shade_sample(s, ctx)
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
            let texel = super::lit_texel(
                ctx.texel(0, &a.uv, b.lod[0]),
                &ctx.light(&a.light),
            );
            // Pixels one meter away per texel of shift.
            let ripple = ctx.params.values[2] * ctx.focal;
            if ripple == 0.0 {
                return self::over(texel, b, c, ctx.params, w, behind_w);
            }
            // How far this pixel's texel was moved (in quarter texels, filtered like the
            // water, so a fraction of a texel), and the same distance on screen at this depth
            // (w is 1 / depth), rounded to a pixel. The color and the depth behind both come
            // from there, so the fade moves with what it fades.
            let height = ctx.texel(1, &a.uv, b.lod[0])
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
        F32s, Fill, Material, PixelContext, SampleContext, U32s, VertexContext, sample_cube_by,
    };

    crate::material_io! {
        vertex { normal: 3 }
        sampled { normal: 3, position: 3 }
        fixed32 {}
        fixed16 {}
        float { reflect: 3, lod: 1 }
    }

    /// Sampler `FILTER`, from [`super::filter`].
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
            sample_cube_by(ctx.textures[0], c.reflect, c.lod[0].to_array()[0], ctx.at, ctx.filters[0])
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
            filters: [super::filter::BILINEAR_MIPMAP_LINEAR; crate::shader::MAX_TEXTURES],
            params,
            focal,
            split: Default::default(),
        }
    }

    /// A 16.16 `light` output of exactly 1: surfaces lit as they are.
    fn full_light() -> [I32s; 3] {
        [I32s::from([65536; LANES]); 3]
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
                light: full_light(),
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
        use super::filter::BILINEAR_MIPMAP_NONE;
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
        let ctx = PixelContext {
            filters: [BILINEAR_MIPMAP_NONE; crate::shader::MAX_TEXTURES],
            ..pixel_ctx(&textures, &params, 20, 1.0)
        };
        let out = Water::shade_over(
            &textured_fresnel::Fixed32 {
                uv: [u, I32s::from([0; LANES])],
                light: full_light(),
            },
            &textured_fresnel::Fixed16::default(),
            &textured_fresnel::Floats {
                distance: [F32s::from([2.0; LANES])],
            },
            &ctx,
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
            lights: &[],
            ambient: Vec3::ONE,
            light_split: &[],
            split: &[],
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
            let out = <CubeReflection>::shade_sample(&s, &ctx);
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
                    light: full_light(),
                };
                // The sample stage's value, converted to 8.8 as the renderer does.
                let bits = (textured_fresnel_facing(cos) * 256.0).round() as i16;
                let b = textured_fresnel::Fixed16 {
                    facing: [I16s::from([bits; LANES])],
                    lod: [I16s::from([0; LANES])],
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
            &textured_fresnel::Fixed32 {
                light: full_light(),
                ..Default::default()
            },
            &textured_fresnel::Fixed16 {
                facing: [I16s::from([bits; LANES])],
                lod: [I16s::from([0; LANES])],
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

    #[test]
    fn split_lights_come_out_of_the_sum_as_their_shadow_covers_them() {
        use crate::shader::{Fill, Split};
        let blank = Texture::solid("t", 0);
        let (textures, params) = ([&blank; 2], Params::new(&[]));
        let mut ctx = pixel_ctx(&textures, &params, 0, 1.0);
        // No split lights: the light as it is.
        assert_eq!(ctx.light(&full_light()), full_light());
        // The sum is one split light of 1 (linear; encoded 1 too): all, half and none of it
        // reaching. Halfway, sqrt(0.5) = 0.71, exactly half the light in gamma-2 terms (and
        // close to the display's 0.5^(1 / GAMMA) = 0.73).
        let mut split = Split {
            count: 1,
            ..Split::default()
        };
        split.light[0] = [F32s::fill(1.0); 3];
        split.reaches[0] = F32s::from([1.0, 0.5, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        ctx.split = split;
        let close = |got: i32, want: f32| (got as f32 - want * 65536.0).abs() <= 65536.0 * 1e-3;
        let lit = ctx.light(&full_light())[0].to_array();
        assert!(close(lit[0], 1.0), "{}", lit[0]);
        assert!(close(lit[1], 0.5f32.sqrt()), "{}", lit[1]);
        assert!((lit[1] as f32 / 65536.0 - 0.5f32.powf(1.0 / super::GAMMA)).abs() < 0.03);
        assert!(lit[2] < 10, "{}", lit[2]);
        // All of it everywhere: the sum, exactly.
        ctx.split.reaches[0] = F32s::fill(1.0);
        assert_eq!(ctx.light(&full_light()), full_light());
        // Over another light of the same strength, the sum is 2 (linear, encoded sqrt(2)):
        // where none of the split one reaches, exactly the other's 1 is left.
        let sum = [I32s::fill((2.0f32.sqrt() * 65536.0).round() as i32); 3];
        ctx.split.reaches[0] = F32s::fill(0.0);
        let rest = ctx.light(&sum)[0].to_array();
        assert!(close(rest[0], 1.0), "{}", rest[0]);
    }
}
