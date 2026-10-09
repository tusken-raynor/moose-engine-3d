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

/// Where lit colors start to bend toward white (see [`shoulder`]): the brightest channel's
/// value, 0 to 255.
const KNEE: f32 = 204.0;
/// How far very bright light washes toward white, at most (see [`shoulder`]).
const WASH: f32 = 0.6;
/// How far past 1 the light goes before the shoulder is fully on: it eases in from 1, so
/// nothing lit at most fully changes.
const SHOULDER_RAMP: f32 = 0.25;

/// XRGB from lit channel values (0 up, 255 being white, any past it) under `light` (16.16,
/// encoded), with a soft shoulder instead of clipping where the light goes past 1: past
/// [`KNEE`], the brightest channel `m` comes in toward white,
/// `KNEE + (m - KNEE) / (1 + (m - KNEE) / (255 - KNEE))` (smooth at the knee, never reaching
/// 255), and the others scale with it, so a bright light keeps a surface's texture and hue
/// rather than flattening it. As much as it is brought in, it also washes toward white (up
/// to [`WASH`]), as very bright light does on film. It eases in as the light goes from 1 to
/// `1 + SHOULDER_RAMP`; under light of 1 or less colors are clipped as ever (they can't pass
/// white), so a white texel in full light stays white. Blocks where no lane is lit past 1 or
/// past the knee (most of them) are packed as they are.
#[inline(always)]
fn shoulder(ch: [U32s; 3], light: &[I32s; 3]) -> U32s {
    let byte = U32s::fill(255);
    let m: I32s = wide::bytemuck::cast(ch[0].max(ch[1]).max(ch[2]));
    let l = light[0].max(light[1]).max(light[2]);
    if l.simd_le(I32s::fill(65536)).all() || m.simd_le(I32s::fill(KNEE as i32)).all() {
        return ch[0].min(byte) << 16_u32 | ch[1].min(byte) << 8_u32 | ch[2].min(byte);
    }
    let (scale, wash) = shoulder_lanes(m.round_float(), l.round_float() * F32s::fill(1.0 / 65536.0));
    let pack = |c: U32s| -> U32s {
        let c: I32s = wide::bytemuck::cast(c);
        let c = c.round_float();
        let v = c * scale;
        let v = v + (F32s::fill(255.0) - v) * wash;
        // Eased in from the plain clip.
        wide::bytemuck::cast(v.round_int().max(I32s::fill(0)).min(I32s::fill(255)))
    };
    pack(ch[0]) << 16_u32 | pack(ch[1]) << 8_u32 | pack(ch[2])
}

/// [`shoulder`]'s scale on every channel and wash toward white for lanes whose brightest
/// channel is `m` (0 to 255 being white) under light `l` (encoded, 1 being 1); where it is
/// off, a scale clipping `m` at 255 and no wash.
#[inline(always)]
fn shoulder_lanes(m: F32s, l: F32s) -> (F32s, F32s) {
    let one = F32s::fill(1.0);
    let mf = m.max(one);
    let over = (mf - F32s::fill(KNEE)).max(F32s::fill(0.0));
    let bent = F32s::fill(KNEE) + over / (one + over * F32s::fill(1.0 / (255.0 - KNEE)));
    let clipped = mf.min(F32s::fill(255.0));
    let t = ((l - one) * F32s::fill(1.0 / SHOULDER_RAMP)).max(F32s::fill(0.0)).min(one);
    let t = t * t * (F32s::fill(3.0) - t - t);
    // The brightest channel's new value, between clipped and bent, over its old one.
    let target = clipped + (bent - clipped) * t;
    let scale = mf.simd_gt(F32s::fill(KNEE)).select(target / mf, one);
    let wash = (one - bent / mf).max(F32s::fill(0.0)) * F32s::fill(WASH) * t;
    (scale, mf.simd_gt(F32s::fill(KNEE)).select(wash, F32s::fill(0.0)))
}

/// One lit color as [`shoulder`] packs it, for references in tests: channel values (255
/// being white, any past it) under light whose brightest channel is `light` (encoded).
pub fn shoulder_reference(ch: [f32; 3], light: f32) -> [u32; 3] {
    let m = ch[0].max(ch[1]).max(ch[2]);
    if light <= 1.0 || m <= KNEE {
        return ch.map(|c| (c as u32).min(255));
    }
    let (scale, wash) = shoulder_lanes(F32s::fill(m.floor()), F32s::fill(light));
    let (scale, wash) = (scale.to_array()[0], wash.to_array()[0]);
    ch.map(|c| {
        let v = c.floor() * scale;
        (v + (255.0 - v) * wash).round().clamp(0.0, 255.0) as u32
    })
}

/// XRGB from a `color` output (three 8.8 channels, 0-255) under the 16.16 `light` (see
/// [`light_output`]), past white brought in by [`shoulder`]. Color and light are
/// interpolated apart and multiplied per pixel: clamping lit colors at sample points would
/// bend them near edges, where sample points past the polygon carry values beyond 255
/// (which the wrapping 8.8 stepping brings back in range inside it).
#[inline(always)]
fn lit_rgb(color: &[I16s; 3], light: &[I32s; 3]) -> U32s {
    use crate::shader::high_byte;
    let channel = |k: usize| {
        let l: U32s = wide::bytemuck::cast(light[k]);
        (high_byte(color[k]) * l) >> 16_u32
    };
    shoulder([channel(0), channel(1), channel(2)], light)
}

/// `texel`'s color channels under the 16.16 `light` (see [`light_output`]; 65536 is 1: a
/// light of exactly 1 leaves the texel as it is), past white brought in by [`shoulder`];
/// its alpha is kept.
#[inline(always)]
fn lit_texel(texel: U32s, light: &[I32s; 3]) -> U32s {
    let byte = U32s::fill(255);
    let channel = |shift: u32, k: usize| {
        let l: U32s = wide::bytemuck::cast(light[k]);
        (((texel >> shift) & byte) * l) >> 16_u32
    };
    (texel & U32s::fill(0xFF00_0000)) | shoulder([channel(16, 0), channel(8, 1), channel(0, 2)], light)
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
                (c * l) >> 16_u32
            };
            super::shoulder([channel(16, 0), channel(8, 1), channel(0, 2)], &light)
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
            let channel = |k: usize| {
                let c = (high_byte(b.color[k]) * scale) >> 8_u32;
                let l: U32s = wide::bytemuck::cast(light[k]);
                (c * l) >> 16_u32
            };
            super::shoulder([channel(0), channel(1), channel(2)], &light)
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
/// sample points, for one channel of the lights that reach it, each weighted by its
/// brightness (the luminance of its color times its falloff and cone, as [`diffuse`] has
/// them): in tangent space (`tangent`, `bitangent`, `normal`: x along the texture's u, y
/// along its v, z out), over the channel's brightness on a flat surface, so that a flat
/// texel's factor is exactly 1.
#[derive(Clone, Copy)]
struct Bumps {
    /// The ambient light's share of the flat brightness (the lights that always reach
    /// only: the split lights' channels have none).
    ambient: F32s,
    /// The sum of the lights' directions (toward them, unit length) times their share of
    /// the flat brightness: lights in front of the surface only.
    toward: [F32s; 3],
    /// The lights' color (linear) where they reach, before any surface's cosine: lights in
    /// front of the surface only. A highlight's.
    color: [F32s; 3],
    /// The sum of the directions of the lights behind the surface (within its back light
    /// angle, `SampleContext::back`) times their share of the flat brightness, each eased
    /// from all of it at the surface's plane to none at the angle. None of them lights the
    /// flat surface; a texel's normal tilted toward them catches them (see [`bump_factor`]).
    /// The lights that always reach's channel only.
    behind: [F32s; 3],
}

/// The bump channels (see [`bumps`]): the lights that always reach, the shadowed lamps,
/// the flashlight.
pub(crate) const BUMP_CHANNELS: usize = 3;

/// Bump values to interpolate (`bump` outputs): the lights that always reach's ambient
/// share and summed direction, the shadowed lamps' summed direction, the flashlight's
/// direction, and the lights behind's summed direction (a share of the first channel's
/// brightness).
pub(crate) const BUMP_LANES: usize = 13;

#[inline(always)]
fn bump_lanes([rest, lamps, flash]: &[Bumps; BUMP_CHANNELS]) -> [F32s; BUMP_LANES] {
    [
        rest.ambient, rest.toward[0], rest.toward[1], rest.toward[2],
        lamps.toward[0], lamps.toward[1], lamps.toward[2],
        flash.toward[0], flash.toward[1], flash.toward[2],
        rest.behind[0], rest.behind[1], rest.behind[2],
    ]
}

/// Whether there is any light from one way in `bump` for the lights that always reach
/// (not the ambient light only), in front of the surface or behind it.
#[inline(always)]
fn bump_has_rest(bump: &[F32s; BUMP_LANES]) -> bool {
    let len2 = |k: usize| bump[k] * bump[k] + bump[k + 1] * bump[k + 1] + bump[k + 2] * bump[k + 2];
    (len2(1) + len2(10)).simd_ge(F32s::fill(1e-4)).any()
}

/// A texel's bumps' factor on channel `g`: 0, the lights that always reach (its ambient
/// share, its normal's cosine with their summed direction, and with the lights behind the
/// surface's, which only a normal tilted toward them catches); 1, the shadowed lamps (its
/// normal's cosine with theirs); 2, the flashlight (with its).
#[inline(always)]
fn bump_factor(normal: &[F32s; 3], bump: &[F32s; BUMP_LANES], g: usize) -> F32s {
    let zero = F32s::fill(0.0);
    let cos = |k: usize| (normal[0] * bump[k] + normal[1] * bump[k + 1] + normal[2] * bump[k + 2]).max(zero);
    match g {
        0 => bump[0] + cos(1) + cos(10),
        1 => cos(4),
        _ => cos(7),
    }
}

/// [`Bumps`] per channel of the lights reaching a polygon: those that always reach its
/// pixels (the ambient light and all but its split lights); its split lights but the
/// flashlight, the shadowed lamps (pooled, as the first are); and the player's flashlight,
/// when it is split (its shadow or its beam's cone cuts it pixel by pixel; see
/// `SampleContext::light_split`). Pixels bump each channel apart and add them, each split
/// light as much as it reaches (see `PixelContext::light_scaled`): a light that doesn't
/// reach a pixel doesn't bump it, and the others still do. The flashlight, the light most
/// unlike the others (low, close, often grazing), never steers their bumps, nor they its.
#[inline(always)]
fn bumps(ctx: &SampleContext, position: &[F32s; 3], frame: [&[F32s; 3]; 3]) -> [Bumps; BUMP_CHANNELS] {
    let (zero, one) = (F32s::fill(0.0), F32s::fill(1.0));
    let luma = |c: glam::Vec3| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
    let ambient = F32s::fill(luma(ctx.ambient));
    // Per channel: the flat brightness, the summed directions, the color.
    let mut flat = [ambient, zero, zero];
    let mut toward = [[zero; 3]; BUMP_CHANNELS];
    let mut color = [[zero; 3]; BUMP_CHANNELS];
    let mut behind = [zero; 3];
    let back = F32s::fill(ctx.back.max(1e-6));
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
        let lit = front.simd_gt(zero).select(one, zero);
        // Its channel: a split light (as `diffuse` tells them) is a lamp's or the
        // flashlight's.
        let split = ctx.light_split.get(i).is_some_and(|&j| (j as usize) < ctx.split.len());
        let g = match split {
            false => 0,
            true if l.id == moose_assets::FLASHLIGHT_ID => 2,
            true => 1,
        };
        flat[g] += k * front;
        for (sum, v) in toward[g].iter_mut().zip(to) {
            *sum += k * lit * v;
        }
        for (sum, c) in color[g].iter_mut().zip([l.color.x, l.color.y, l.color.z]) {
            *sum += reach * lit * F32s::fill(c);
        }
        // Behind the surface, within its back light angle: eased out to none at it.
        if g == 0 && ctx.back > 0.0 {
            let w = ((to[2] + back) / back).max(zero).min(one);
            let w = lit.simd_gt(zero).select(zero, w * w * (F32s::fill(3.0) - w - w));
            for (sum, v) in behind.iter_mut().zip(to) {
                *sum += k * w * v;
            }
        }
    }
    std::array::from_fn(|g| {
        let over = flat[g].max(F32s::fill(1e-6)).recip();
        Bumps {
            ambient: if g == 0 { ambient * over } else { zero },
            toward: toward[g].map(|v| v * over),
            color: color[g],
            behind: if g == 0 { behind.map(|v| v * over) } else { [zero; 3] },
        }
    })
}

/// A normal map texel's normal (`normal`, tangent space), bent toward flat by how much of
/// the bumps there are (`f`, 0 to 1), at unit length: always, as the bump factors take it
/// so (a flat texel's factor exactly 1). A normal map's normals are only nearly unit, and
/// filtered ones shorter (about 0.9 past the first mip level of cheese_1's gravel): once
/// only where the bumps were fading, so they were dimmer where they weren't, and the fade's
/// start showed as a hard edge, in steps a block of pixels wide.
#[inline(always)]
fn unit_normal(normal: [F32s; 3], f: F32s) -> [F32s; 3] {
    let one = F32s::fill(1.0);
    let bent = [normal[0] * f, normal[1] * f, one + (normal[2] - one) * f];
    let inv = (bent[0] * bent[0] + bent[1] * bent[1] + bent[2] * bent[2]).max(F32s::fill(1e-12)).recip_sqrt();
    bent.map(|c| c * inv)
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
/// Bumps fade to flat between 4 and 6 meters from the eye (past that the normal map isn't
/// read). The normal map is read with texture 1's sampler. Params: the highlight's strength
/// (`values[0]`) and its power's log2 (`values[1]`: 5 is a power of 32).
pub mod textured_lit {
    use crate::shader::{F32s, Fill, I32s, Material, PixelContext, SampleContext, U32s, VertexContext};

    /// Where [`TexturedLit`]'s bumps come from: none, or a normal map in texture 1's
    /// color.
    pub const FLAT: u8 = 0;
    pub const NORMAL_MAP: u8 = 1;

    // Seen in a mirror, no bumps or highlights: their values aren't carried.
    crate::material_io! {
        vertex { uv: 2, face_normal: 3, face_tangent: 3, face_bitangent: 3 }
        sampled { uv: 2, lod: 1, normal: 3, tangent: 3 if !MIRRORED, bitangent: 3 if !MIRRORED, position: 3 }
        fixed32 { uv: 2, light: 3 }
        fixed16 { lod: 1, fade: 1 if !MIRRORED }
        float { bump: 13 if !MIRRORED, spec: 6 if !MIRRORED }
    }

    /// Textured, bumpy and shiny as `BUMP` and `SPECULAR` say (see the module); seen in a
    /// mirror (`MIRRORED`), neither: textured and lit.
    pub struct TexturedLit<const BUMP: u8, const SPECULAR: bool, const MIRRORED: bool = false>;

    /// The configurations the app uses.
    pub type TexturedNormal = TexturedLit<NORMAL_MAP, false>;
    pub type TexturedSpecular = TexturedLit<FLAT, true>;
    pub type TexturedNormalSpecular = TexturedLit<NORMAL_MAP, true>;
    /// Seen in a mirror (the same whatever the bumps and highlight).
    pub type TexturedLitMirrored = TexturedLit<FLAT, false, true>;

    /// Bumps are whole out to `BUMPS_WHOLE` meters from the eye, and fade (smoothly) to
    /// flat by `BUMPS_GONE`, past which the normal map isn't read.
    const BUMPS_WHOLE: f32 = 4.0;
    const BUMPS_GONE: f32 = 6.0;

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

    impl<const BUMP: u8, const SPECULAR: bool, const MIRRORED: bool> Material for TexturedLit<BUMP, SPECULAR, MIRRORED> {
        const MIRRORED: bool = MIRRORED;
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
            if MIRRORED {
                return Interp {
                    uv: s.uv,
                    lod: s.lod,
                    light: super::light_output(super::diffuse(ctx, &s.position, &s.normal)),
                    ..Default::default()
                };
            }
            let frame = [&s.tangent, &s.bitangent, &s.normal];
            let channels = super::bumps(ctx, &s.position, frame);
            let zero = F32s::fill(0.0);
            // The channels' directions: the bumps', and the highlights' (their half vectors
            // are made at the pixels).
            let bump = match (BUMP, SPECULAR) {
                (NORMAL_MAP, _) | (_, true) => super::bump_lanes(&channels),
                _ => [zero; super::BUMP_LANES],
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
                // The way to the eye, and the color of the lights that always reach (the
                // split lights' are the pixels': see `Split`).
                let rest = &channels[0].color;
                [eye[0], eye[1], eye[2], rest[0], rest[1], rest[2]]
            } else {
                [zero; 6]
            };
            // How much of the bumps there are, by distance: 1 within `BUMPS_WHOLE`, 0 past
            // `BUMPS_GONE`.
            let distance = (0..3)
                .map(|k| {
                    let d = F32s::fill(ctx.eye[k]) - s.position[k];
                    d * d
                })
                .fold(zero, |a, b| a + b)
                .sqrt();
            let t = ((F32s::fill(BUMPS_GONE) - distance) * F32s::fill(1.0 / (BUMPS_GONE - BUMPS_WHOLE)))
                .max(zero)
                .min(F32s::fill(1.0));
            Interp {
                uv: s.uv,
                lod: s.lod,
                fade: [t * t * (F32s::fill(3.0) - t - t)],
                light: super::light_output(super::diffuse(ctx, &s.position, &s.normal)),
                bump,
                spec,
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, c: &Floats, ctx: &PixelContext) -> U32s {
            let texel = ctx.texel(0, &a.uv, b.lod[0]);
            if MIRRORED {
                return super::lit_texel(texel, &ctx.light(&a.light)) & U32s::fill(0xFF_FFFF);
            }
            let one = F32s::fill(1.0);
            // How much of the bumps there are (8.8): none at all, far away, reads nothing.
            let fade = I32s::from_i16x8(b.fade[0]);
            let far = fade.simd_le(I32s::fill(0)).all();
            // Which lights bump it: those that always reach, if any has a direction (not
            // the ambient light only), and the split ones, if any reaches.
            let rest = match BUMP {
                NORMAL_MAP => super::bump_has_rest(&c.bump),
                _ => false,
            };
            let (lamps, flash) = (ctx.split_reaches(false), ctx.split_reaches(true));
            let bumpy = BUMP != FLAT && !far && (rest || lamps || flash || SPECULAR);
            // The texel's normal (tangent space; z out), and its bumps' factors on each
            // channel: the lights that always reach, the shadowed lamps, the flashlight.
            let (mut normal, mut factors) = ([F32s::fill(0.0), F32s::fill(0.0), one], [one; 3]);
            if bumpy {
                let lod = b.lod[0];
                let n = ctx.texel(1, &a.uv, lod);
                let k = F32s::fill(1.0 / 127.5);
                normal = [16, 8, 0].map(|shift| (super::channel(n, shift) - F32s::fill(127.5)) * k);
                // Toward flat as the bumps fade with distance, then unit length.
                let f = (fade.round_float() * F32s::fill(1.0 / 256.0)).min(one);
                normal = super::unit_normal(normal, f);
                for (g, on) in [rest, lamps, flash].into_iter().enumerate() {
                    if on {
                        factors[g] = super::bump_factor(&normal, &c.bump, g);
                    }
                }
            }
            let light = if bumpy {
                let encode = |f: F32s| f.max(F32s::fill(0.0)).sqrt();
                ctx.light_scaled(&a.light, encode(factors[0]), encode(factors[1]), encode(factors[2]))
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
            // Channel `g`'s: its half vector, between its lights' direction (the bumps') and the
            // eye's, both normalized.
            let eye = {
                let e = [c.spec[0], c.spec[1], c.spec[2]];
                let inv = (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).max(F32s::fill(1e-12)).recip_sqrt();
                e.map(|v| v * inv)
            };
            let power = |g: usize| {
                let h = half(&[c.bump[1 + 3 * g], c.bump[2 + 3 * g], c.bump[3 + 3 * g]], &eye);
                let mut p = ((normal[0] * h[0] + normal[1] * h[1] + normal[2] * h[2]) * n_len).max(F32s::fill(0.0));
                for _ in 0..squarings {
                    p = p * p;
                }
                p
            };
            // The lights that always reach's, then each split channel's: its lights' light
            // as much as each reaches the pixel.
            let p = power(0);
            let mut shine = [c.spec[3] * p, c.spec[4] * p, c.spec[5] * p];
            for (g, flashlight, on) in [(1, false, lamps), (2, true, flash)] {
                if on {
                    let p = power(g);
                    for (j, (light, reach)) in ctx.split.light.iter().zip(ctx.split.reaches).enumerate().take(ctx.split.count) {
                        if (ctx.split.flashlight == Some(j)) == flashlight {
                            for k in 0..3 {
                                shine[k] += light[k] * reach * p;
                            }
                        }
                    }
                }
            }
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

pub use textured_lit::{TexturedLit, TexturedLitMirrored, TexturedNormal, TexturedNormalSpecular, TexturedSpecular};

/// A road's shader: [`Textured`], lit, with bumps from a normal map (texture 1's color,
/// `(n + 1) / 2`, read with texture 1's sampler) and nothing else: no highlight, no detail.
/// The bumps are a factor on the light, as [`TexturedLit`]'s are (the texel's normal's
/// cosine with the lights' directions summed into one), whole out to `short` meters from
/// the eye (`values[0]`) and fading smoothly to flat by `far` (`values[1]`), past which the
/// normal map isn't read.
///
/// `MIRRORED` is its copy for polygons seen in a mirror, which draws no bumps and carries
/// none of their values; the app draws with it where bumps are off too.
pub mod basic_bumpy {
    use crate::shader::{F32s, Fill, I32s, Material, PixelContext, SampleContext, U32s, VertexContext};

    crate::material_io! {
        vertex { uv: 2, face_normal: 3, face_tangent: 3, face_bitangent: 3 }
        sampled { uv: 2, lod: 1, normal: 3, tangent: 3 if !MIRRORED, bitangent: 3 if !MIRRORED, position: 3 }
        fixed32 { uv: 2, light: 3 }
        fixed16 { lod: 1, fade: 1 if !MIRRORED }
        float { bump: 13 if !MIRRORED }
    }

    pub struct BasicBumpy<const MIRRORED: bool = false>;

    /// Seen in a mirror (or with bumps off): textured and lit.
    pub type BasicBumpyMirrored = BasicBumpy<true>;

    impl<const MIRRORED: bool> Material for BasicBumpy<MIRRORED> {
        const MIRRORED: bool = MIRRORED;
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
            let light = super::light_output(super::diffuse(ctx, &s.position, &s.normal));
            if MIRRORED {
                return Interp { uv: s.uv, lod: s.lod, light, ..Default::default() };
            }
            let channels = super::bumps(ctx, &s.position, [&s.tangent, &s.bitangent, &s.normal]);
            Interp {
                uv: s.uv,
                lod: s.lod,
                light,
                // How much of the bumps there are, by distance: 1 within `short`, 0 past `far`.
                fade: [bump_fade(ctx, &s.position, ctx.params.values[0], ctx.params.values[1])],
                bump: super::bump_lanes(&channels),
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, c: &Floats, ctx: &PixelContext) -> U32s {
            let texel = ctx.texel(0, &a.uv, b.lod[0]);
            let light = if MIRRORED {
                ctx.light(&a.light)
            } else {
                bumped_light(ctx, &a.light, b.fade[0], &c.bump, || ctx.texel(1, &a.uv, b.lod[0]))
            };
            super::lit_texel(texel, &light) & U32s::fill(0xFF_FFFF)
        }
    }

    /// The light at the pixels (an encoded `light` output) with the bumps of a normal map
    /// on it: `fade` how much of the bumps there are (8.8; none reads nothing), `bump` the
    /// lights' directions in tangent space (as `super::bumps` gives them: the ones that
    /// always reach, then all), and `normal_map` the normal map's texel, read only if it is
    /// needed. Shared by the programs bumped like [`BasicBumpy`].
    #[inline(always)]
    pub(super) fn bumped_light(
        ctx: &PixelContext,
        light: &[I32s; 3],
        fade: crate::shader::I16s,
        bump: &[F32s; super::BUMP_LANES],
        normal_map: impl FnOnce() -> U32s,
    ) -> [I32s; 3] {
        // None at all (far away, or no light from any one way): flat, the normal map
        // unread.
        let one = F32s::fill(1.0);
        let fade = I32s::from_i16x8(fade);
        let rest = super::bump_has_rest(bump);
        let (lamps, flash) = (ctx.split_reaches(false), ctx.split_reaches(true));
        if fade.simd_le(I32s::fill(0)).all() || !(rest || lamps || flash) {
            return ctx.light(light);
        }
        // The texel's normal (tangent space; z out), toward flat as the bumps fade.
        let n = normal_map();
        let k = F32s::fill(1.0 / 127.5);
        let normal = [16, 8, 0].map(|shift| (super::channel(n, shift) - F32s::fill(127.5)) * k);
        let f = (fade.round_float() * F32s::fill(1.0 / 256.0)).min(one);
        let normal = super::unit_normal(normal, f);
        // Its factor on each channel that has light here.
        let factor = |g: usize, on: bool| if on { super::bump_factor(&normal, bump, g) } else { one };
        let factors = (factor(0, rest), factor(1, lamps), factor(2, flash));
        let encode = |f: F32s| f.max(F32s::fill(0.0)).sqrt();
        ctx.light_scaled(light, encode(factors.0), encode(factors.1), encode(factors.2))
    }

    /// How much of the bumps there are at sample points `position` (smoothly 1 within
    /// `short` meters of the eye to 0 by `far`).
    #[inline(always)]
    pub(super) fn bump_fade(ctx: &SampleContext, position: &[F32s; 3], short: f32, far: f32) -> F32s {
        let zero = F32s::fill(0.0);
        let distance = (0..3)
            .map(|k| {
                let d = F32s::fill(ctx.eye[k]) - position[k];
                d * d
            })
            .fold(zero, |a, b| a + b)
            .sqrt();
        let t = ((F32s::fill(far) - distance) * F32s::fill(1.0 / (far - short).max(1e-3))).max(zero).min(F32s::fill(1.0));
        t * t * (F32s::fill(3.0) - t - t)
    }
}

pub use basic_bumpy::{BasicBumpy, BasicBumpyMirrored};

/// [`BasicBumpy`] for a texture of four sections side by side that tile with each other in
/// any order (a sidewalk's slabs, say), which surfaces map one section to a whole repeat
/// of their `uv` (1:1). Each repeat (cell: the whole parts of `uv`) gets one of the four,
/// picked by a hash of the cell and `seed` (`values[2]`, a whole number), so they don't
/// show the same four in a row: the texture is read at u = (section + u's fraction) / 4,
/// in 16.16, `(section << 16 | fraction) >> 2`. The normal map is laid out the same way.
///
/// The texture's level of detail is measured from u / 4 (the texture is four repeats
/// wide), so it is as sharp as the plain one would be. Inputs: `short`, `far` (as
/// [`BasicBumpy`]'s) and `seed`. `MIRRORED` is its copy seen in a mirror (or with bumps
/// off): the same sections, no bumps.
pub mod random_sections {
    use super::basic_bumpy::{bump_fade, bumped_light};
    use crate::shader::{Fill, I32s, Material, PixelContext, SampleContext, U32s, VertexContext};

    // `uv` is u / 4 (what the level of detail is measured from); `cell` the surface's own.
    crate::material_io! {
        vertex { uv: 2, face_normal: 3, face_tangent: 3, face_bitangent: 3 }
        sampled { uv: 2, lod: 1, cell: 2, normal: 3, tangent: 3 if !MIRRORED, bitangent: 3 if !MIRRORED, position: 3 }
        fixed32 { cell: 2, light: 3 }
        fixed16 { lod: 1, fade: 1 if !MIRRORED }
        float { bump: 13 if !MIRRORED }
    }

    pub struct RandomSections<const MIRRORED: bool = false>;

    /// Seen in a mirror (or with bumps off): the same sections, textured and lit.
    pub type RandomSectionsMirrored = RandomSections<true>;

    /// One of four (0 to 3) for each cell `(x, y)` and `seed`: an integer hash's top bits.
    #[inline(always)]
    pub fn section(x: I32s, y: I32s, seed: i32) -> I32s {
        let u = |v: I32s| -> U32s { wide::bytemuck::cast(v) };
        let mut h = (u(x) * U32s::fill(0x8DA6_B343)) ^ (u(y) * U32s::fill(0xD816_3841)) ^ U32s::fill((seed as u32).wrapping_mul(0xCB1A_B31F));
        h ^= h >> 15;
        h *= U32s::fill(0x2C1B_3C6D);
        h ^= h >> 12;
        h *= U32s::fill(0x297A_2D39);
        h ^= h >> 15;
        wide::bytemuck::cast(h >> 30)
    }

    impl<const MIRRORED: bool> Material for RandomSections<MIRRORED> {
        const MIRRORED: bool = MIRRORED;
        crate::material_types!();

        #[inline(always)]
        fn shade_vertex(v: &Vertex, _: &VertexContext) -> Sampled {
            Sampled {
                uv: [v.uv[0] / 4.0, v.uv[1]],
                cell: v.uv,
                normal: v.face_normal,
                tangent: v.face_tangent,
                bitangent: v.face_bitangent,
                ..Default::default()
            }
        }

        #[inline(always)]
        fn shade_sample(s: &SampledLanes, ctx: &SampleContext) -> Interp {
            let light = super::light_output(super::diffuse(ctx, &s.position, &s.normal));
            if MIRRORED {
                return Interp { cell: s.cell, lod: s.lod, light, ..Default::default() };
            }
            let channels = super::bumps(ctx, &s.position, [&s.tangent, &s.bitangent, &s.normal]);
            Interp {
                cell: s.cell,
                lod: s.lod,
                light,
                fade: [bump_fade(ctx, &s.position, ctx.params.values[0], ctx.params.values[1])],
                bump: super::bump_lanes(&channels),
            }
        }

        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, c: &Floats, ctx: &PixelContext) -> U32s {
            // The cell (whole parts: the shift floors, negatives too), its section, and where
            // in that section the pixel is.
            let [cu, cv] = a.cell;
            let k = section(cu >> 16, cv >> 16, ctx.params.values[2] as i32);
            let uv = [((k << 16) | (cu & I32s::fill(0xFFFF))) >> 2, cv];
            let texel = ctx.texel(0, &uv, b.lod[0]);
            let light = if MIRRORED {
                ctx.light(&a.light)
            } else {
                bumped_light(ctx, &a.light, b.fade[0], &c.bump, || ctx.texel(1, &uv, b.lod[0]))
            };
            super::lit_texel(texel, &light) & U32s::fill(0xFF_FFFF)
        }
    }
}

pub use random_sections::{RandomSections, RandomSectionsMirrored};

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
        over_with(texel, b.facing[0], c.distance[0], params.values[0], params.values[1], w, behind_w)
    }

    /// [`over`] from its values: the surface's `facing` and `distance` (as
    /// `super::facing_and_distance` gives them), its `reflectance` (F0, 0 to 1) and fade
    /// `range` (meters; 0, none).
    #[inline(always)]
    pub fn over_with(
        texel: U32s,
        facing: I16s,
        distance: F32s,
        reflectance: f32,
        range: f32,
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
        let scale = ((1.0 - reflectance.clamp(0.0, 1.0)) * ONE as f32).round() as i16;
        let fresnel = I16s::fill(ONE) - keep(facing).mul_scale_round(I16s::fill(scale));
        let mut reflected = fresnel.mul_scale_round(smooth);
        if range > 0.0 {
            // What is left `bounce` meters past the surface: (1 - t)^2, t = bounce / range
            // (as a Q15 fraction). No fade where nothing is behind (lanes past the run).
            let one = F32s::fill(1.0);
            let bounce = distance * (w / behind_w - one);
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

/// [`BasicBumpy`] with puddles: texture 0's alpha says where it is wet (255 water, 0 dry,
/// between them the shore), and there it reflects what is above it by Fresnel, as
/// [`TexturedFresnel`] does, drawn over the reflection the engine draws under a reflective
/// surface. Where it is wet, as much as it is:
///
/// - The ground under the water wobbles: texture 0 is read again with its `uv` moved by
///   two crossing waves of `uv` and time (smoothed triangle waves: no texture of its own),
///   the move scaled by the wetness read where the pixel really is, so the shore stays
///   still.
/// - The bumps fade out: water over the ground is flat.
/// - The reflection is shifted along the row by texture 2, the turbulence (its blue byte
///   from 128, as [`Water`]'s heights; a grey noise or `@water_heights`), slid across the
///   surface with time.
///
/// It reads texture 0 where the pixel is, again where it's wet, the normal map (texture 1)
/// where the bumps are, and the turbulence where it's wet and over a reflection.
///
/// Params (`values`): `short` and `far`, the bumps' fade with distance (as
/// [`BasicBumpy`]'s; both 0, no bumps); the reflectance (F0) and fade (meters; 0, none) of
/// [`TexturedFresnel`]; `shift`, the reflection's shift per turbulence step at a meter (as
/// [`Water`]'s); `ripple`, how far the ground moves (in repeats of its texture);
/// `ripple_size`, a wave's length (repeats); `ripple_speed`, waves a second; and `drift`,
/// repeats a second the turbulence slides.
///
/// Its copies: `REFLECTED`, translucent over its reflection; not, opaque (no reflection
/// under it: reflections off, or a surface not marked reflective), with its puddles but
/// no reflection; `MIRRORED`, seen in a mirror, textured and lit only.
pub mod reflective_bumpy {
    use super::basic_bumpy::{bump_fade, bumped_light};
    use crate::shader::{
        F32s, Fill, I16s, I32s, Material, Over, PixelContext, RowBehind, SampleContext, U32s, VertexContext,
        blend_lanes,
    };

    crate::material_io! {
        vertex { uv: 2, face_normal: 3, face_tangent: 3, face_bitangent: 3 }
        sampled { uv: 2, lod: 1, normal: 3, tangent: 3 if !MIRRORED, bitangent: 3 if !MIRRORED, position: 3 }
        fixed32 { uv: 2, light: 3 }
        fixed16 { lod: 1, facing: 1, fade: 1 if !MIRRORED }
        float { distance: 1, bump: 13 if !MIRRORED }
    }

    const SHORT: usize = 0;
    const FAR: usize = 1;
    const REFLECTANCE: usize = 2;
    const FADE: usize = 3;
    const SHIFT: usize = 4;
    const RIPPLE: usize = 5;
    const RIPPLE_SIZE: usize = 6;
    const RIPPLE_SPEED: usize = 7;
    const DRIFT: usize = 8;
    /// Largest shift of the reflection, in pixels.
    const MAX_SHIFT: i32 = 32;

    pub struct ReflectiveBumpy<const REFLECTED: bool = false, const MIRRORED: bool = false>;

    /// Over its reflection.
    pub type ReflectiveBumpyReflected = ReflectiveBumpy<true, false>;
    /// Seen in a mirror: textured and lit.
    pub type ReflectiveBumpyMirrored = ReflectiveBumpy<false, true>;

    /// A smooth wave of period 1 in `x`, from -1 to 1: a triangle wave, smoothstepped.
    #[inline(always)]
    pub(crate) fn wave(x: F32s) -> F32s {
        let f = x - x.floor();
        let g = (f + f - F32s::fill(1.0)).abs();
        let s = g * g * (F32s::fill(3.0) - g - g);
        s + s - F32s::fill(1.0)
    }

    /// `rate` a second at `time`, wrapped to one turn: a phase of period 1 that keeps its
    /// fraction as time grows.
    #[inline(always)]
    pub(crate) fn turns(time: f32, rate: f32) -> f32 {
        (time as f64 * rate as f64).rem_euclid(1.0) as f32
    }

    impl<const REFLECTED: bool, const MIRRORED: bool> Material for ReflectiveBumpy<REFLECTED, MIRRORED> {
        const MIRRORED: bool = MIRRORED;
        const TRANSLUCENT: bool = REFLECTED;
        const READS_BEHIND: bool = REFLECTED;
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
            let light = super::light_output(super::diffuse(ctx, &s.position, &s.normal));
            let (facing, distance) = super::facing_and_distance(ctx.eye, &s.position, &s.normal);
            if MIRRORED {
                return Interp { uv: s.uv, lod: s.lod, light, facing: [facing], distance: [distance], ..Default::default() };
            }
            let channels = super::bumps(ctx, &s.position, [&s.tangent, &s.bitangent, &s.normal]);
            let p = &ctx.params.values;
            Interp {
                uv: s.uv,
                lod: s.lod,
                light,
                facing: [facing],
                distance: [distance],
                fade: [bump_fade(ctx, &s.position, p[SHORT], p[FAR])],
                bump: super::bump_lanes(&channels),
            }
        }

        /// Without what is behind: no reflection.
        #[inline(always)]
        fn shade_pixel(a: &Fixed32, b: &Fixed16, c: &Floats, ctx: &PixelContext) -> U32s {
            let zero = F32s::fill(0.0);
            let over = Over { w: zero, behind_w: zero, row: RowBehind::NONE };
            Self::shade_over(a, b, c, ctx, &over)
        }

        #[inline(always)]
        fn shade_over(a: &Fixed32, b: &Fixed16, c: &Floats, ctx: &PixelContext, over: &Over) -> U32s {
            let opaque = U32s::fill(0xFF00_0000);
            let rgb = U32s::fill(0xFF_FFFF);
            let lod = b.lod[0];
            let ground = ctx.texel(0, &a.uv, lod);
            if MIRRORED {
                return super::lit_texel(ground, &ctx.light(&a.light)) & rgb;
            }
            let p = &ctx.params.values;
            // How wet, where the pixel really is: 0 to 255, and 0 to 1.
            let wet: I32s = wide::bytemuck::cast(ground >> 24_u32);
            let any_wet = wet.simd_gt(I32s::fill(0)).any();
            let wetness = wet.round_float() * F32s::fill(1.0 / 255.0);
            // The ground under the water, moved by two crossing waves as much as it's wet.
            let color = if any_wet && p[RIPPLE] != 0.0 {
                let k = F32s::fill(1.0 / p[RIPPLE_SIZE].max(1e-3));
                let repeat = F32s::fill(1.0 / 65536.0);
                let (u, v) = (a.uv[0].round_float() * repeat, a.uv[1].round_float() * repeat);
                let (one, two) = (turns(ctx.time, p[RIPPLE_SPEED]), turns(ctx.time, p[RIPPLE_SPEED] * 0.77));
                let reach = wetness * F32s::fill(p[RIPPLE] * 65536.0);
                let du = wave(v * k + F32s::fill(one)) * reach;
                let dv = wave(u * k * F32s::fill(0.83) - F32s::fill(two) + F32s::fill(0.25)) * reach;
                let moved = [a.uv[0] + du.round_int(), a.uv[1] + dv.round_int()];
                ctx.texel(0, &moved, lod)
            } else {
                ground
            };
            // The bumps, less as it's wetter (8.8 times 0 to 256, over 256).
            let dry = I32s::fill(255) - wet;
            let fade = (I32s::from_i16x8(b.fade[0]) * (dry + (dry >> 7_i32))) >> 8_i32;
            let fade = I16s::from_i32x8_truncate(fade);
            let light = bumped_light(ctx, &a.light, fade, &c.bump, || ctx.texel(1, &a.uv, lod));
            let lit = super::lit_texel(color, &light) & rgb;
            if !REFLECTED {
                return lit;
            }
            if !any_wet {
                return lit | opaque;
            }
            // The reflection, shifted along the row by the turbulence (as much as it's wet,
            // and by depth: w is 1 / depth), let through by Fresnel where it's wet.
            let (w, behind_w) = (over.w, over.behind_w);
            let ripple = p[SHIFT] * ctx.focal;
            let x = ctx.at.x_lanes();
            let (under, under_w) = if ripple != 0.0 {
                let slide = |turn: f32| I32s::fill((turn * 65536.0) as i32);
                let at = [
                    a.uv[0] + slide(turns(ctx.time, p[DRIFT])),
                    a.uv[1] + slide(turns(ctx.time, p[DRIFT] * 0.61)),
                ];
                let h: I32s = wide::bytemuck::cast(ctx.texel(2, &at, lod) & U32s::fill(0xFF));
                let shift = ((h - I32s::fill(128)).round_float() * w * wetness * F32s::fill(ripple * 0.25))
                    .round_int()
                    .max(I32s::fill(-MAX_SHIFT))
                    .min(I32s::fill(MAX_SHIFT));
                over.row.at(x + shift)
            } else {
                over.row.at(x)
            };
            // Lanes past the run keep nothing behind.
            let under_w = behind_w.simd_gt(F32s::fill(0.0)).select(under_w, behind_w);
            // Its alpha as `over` reads it, how rough: 255 less how wet.
            let rough: U32s = wide::bytemuck::cast(I32s::fill(255) - wet);
            let surface = super::textured_fresnel::over_with(
                lit | (rough << 24_u32),
                b.facing[0],
                c.distance[0],
                p[REFLECTANCE],
                p[FADE],
                w,
                under_w,
            );
            // Blended here, so the renderer's own blend (alpha 255) keeps it as it is.
            blend_lanes(surface, under) | opaque
        }
    }
}

pub use reflective_bumpy::{ReflectiveBumpy, ReflectiveBumpyMirrored, ReflectiveBumpyReflected};


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

/// A sky: a cube map seen from where the eye is (texture 0, RGBM: see [`SKY_RANGE`]), so it
/// shows the same whichever surfaces it is drawn on, with a sun in it: a disk and its glare
/// toward the level's sun, past white (an HDR material: how far is in the top byte, for a
/// post pass to make glow; see `Material::HDR`).
///
/// Params: the way toward the sun (`values[0..3]`, unit length), its tint (`values[3..6]`),
/// its angular radius (`values[6]`, radians), how bright its disk and glare are
/// (`values[7]`, 0 for no sun), whether to write how far past white (`values[8]`, 1 or 0),
/// and the cube map's face size (`values[9]`, texels).
pub mod sky_box {
    use crate::shader::{
        F32s, Fill, HDR_RANGE, I32s, Material, PixelContext, SampleContext, U32s, VertexContext, sample_cube_by,
    };

    crate::material_io! {
        vertex {}
        sampled { position: 3 }
        fixed32 {}
        fixed16 {}
        float { dir: 3, lod: 1 }
    }

    /// What an RGBM texel holds at most: its color is its red, green and blue times its
    /// alpha times this, all over 255 (as `bake_sky` writes them).
    pub const SKY_RANGE: f32 = 4.0;

    pub struct SkyBox;

    impl Material for SkyBox {
        crate::material_types!();
        const HDR: bool = true;

        #[inline(always)]
        fn shade_vertex(_: &Vertex, _: &VertexContext) -> Sampled {
            Sampled::default()
        }

        /// The way from the eye (affine on the polygon, so exact between sample points),
        /// and the level of detail: face texels per pixel (a face spans pi / 2 of view, a
        /// pixel about 1 / focal).
        #[inline(always)]
        fn shade_sample(s: &SampledLanes, ctx: &SampleContext) -> Interp {
            let p = &s.position;
            let size = ctx.params.values[9];
            let lod = (2.0 * size / (std::f32::consts::PI * ctx.focal)).log2();
            Interp {
                dir: [
                    p[0] - F32s::fill(ctx.eye.x),
                    p[1] - F32s::fill(ctx.eye.y),
                    p[2] - F32s::fill(ctx.eye.z),
                ],
                lod: [F32s::fill(lod)],
            }
        }

        #[inline(always)]
        fn shade_pixel(_: &Fixed32, _: &Fixed16, c: &Floats, ctx: &PixelContext) -> U32s {
            let v = &ctx.params.values;
            let texel = sample_cube_by(ctx.textures[0], c.dir, c.lod[0].to_array()[0], ctx.at, ctx.filters[0]);
            let m = super::channel(texel, 24) * F32s::fill(SKY_RANGE / (255.0 * 255.0));
            let mut color = [16, 8, 0].map(|shift| super::channel(texel, shift) * m);
            if v[7] > 0.0 {
                // The sun: a disk (its edge over a pixel) and glare around it, by the angle
                // from it (from 1 - cos, which is half its square near the sun).
                let d = &c.dir;
                let inv = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).max(F32s::fill(1e-12)).recip_sqrt();
                let cos = (d[0] * F32s::fill(v[0]) + d[1] * F32s::fill(v[1]) + d[2] * F32s::fill(v[2])) * inv;
                let angle2 = ((F32s::fill(1.0) - cos) * F32s::fill(2.0)).max(F32s::fill(0.0));
                let pixel = 1.0 / ctx.focal;
                let disk = ((F32s::fill(v[6]) - angle2.sqrt()) * F32s::fill(1.0 / pixel) + F32s::fill(0.5))
                    .max(F32s::fill(0.0))
                    .min(F32s::fill(1.0));
                // Glare: a tight core and a wide halo (each 1 / (1 + (angle / width)^2)).
                let glare = |strength: f32, width: f32| {
                    F32s::fill(strength) / (F32s::fill(1.0) + angle2 * F32s::fill(1.0 / (width * width)))
                };
                let sun = (disk * F32s::fill(12.0) + glare(1.5, 0.02) + glare(0.25, 0.25)) * F32s::fill(v[7]);
                for (k, ch) in color.iter_mut().enumerate() {
                    *ch += sun * F32s::fill(v[3 + k]);
                }
            }
            // White at most in the color, and how far past white in the top byte.
            let peak = color[0].max(color[1]).max(color[2]);
            let over = if v[8] > 0.0 {
                let m = ((peak - F32s::fill(1.0)) * F32s::fill(255.0 / HDR_RANGE))
                    .max(F32s::fill(0.0))
                    .min(F32s::fill(255.0));
                let m: U32s = wide::bytemuck::cast(m.round_int());
                m << 24_u32
            } else {
                U32s::fill(0)
            };
            let byte = |c: F32s| -> U32s {
                let b: I32s = (c.min(F32s::fill(1.0)) * F32s::fill(255.0)).round_int().max(I32s::fill(0));
                wide::bytemuck::cast(b)
            };
            over | byte(color[0]) << 16_u32 | byte(color[1]) << 8_u32 | byte(color[2])
        }
    }
}

pub use sky_box::SkyBox;

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

    #[test]
    fn a_light_just_behind_bumps_only_texels_tilted_toward_it_within_the_back_light_angle() {
        use crate::shader::Fill;
        use moose_assets::Light;
        let params = Params::default();
        let f = |v: f32| F32s::fill(v);
        // A surface at the origin facing +z, its texture's u along x and v along y.
        let (position, frame) = ([f(0.0); 3], [[f(1.0), f(0.0), f(0.0)], [f(0.0), f(1.0), f(0.0)], [f(0.0), f(0.0), f(1.0)]]);
        let flat = [f(0.0), f(0.0), f(1.0)];
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let tilted = [f(0.0), f(s), f(s)];
        let factors = |light_z: f32, back_deg: f32| {
            let lights = [Light::point(0, Vec3::new(0.0, 1.0, light_z), Vec3::ONE, 10.0)];
            let ctx = SampleContext {
                eye: Vec3::new(0.0, 0.0, 5.0),
                focal: 360.0,
                object: &Object::IDENTITY,
                params: &params,
                lights: &lights,
                ambient: Vec3::splat(0.1),
                light_split: &[],
                split: &[],
                back: back_deg.to_radians().sin(),
            };
            let channels = super::bumps(&ctx, &position, [&frame[0], &frame[1], &frame[2]]);
            let lanes = super::bump_lanes(&channels);
            let at = |n: &[F32s; 3], g| super::bump_factor(n, &lanes, g).to_array()[0];
            (at(&flat, 0), at(&tilted, 0), at(&tilted, 1), super::bump_has_rest(&lanes))
        };
        // Just behind (about 6 degrees), with no back light angle: no bumps, only ambient.
        let (flat0, tilted0, _, rest0) = factors(-0.1, 0.0);
        assert!(!rest0 && (flat0 - 1.0).abs() < 1e-5 && (tilted0 - 1.0).abs() < 1e-5);
        // With 20 degrees: a flat texel is as before (the light gives a flat face nothing),
        // one tilted toward the light catches it; the shadowed lamps' channel has nothing (it
        // isn't split).
        let (flat20, tilted20, split20, rest20) = factors(-0.1, 20.0);
        assert!(rest20 && (flat20 - 1.0).abs() < 1e-5);
        assert!(tilted20 > 2.0, "{tilted20}");
        assert_eq!(split20, 0.0);
        // It eases out toward the angle, and past it is gone.
        let (_, tilted_near_edge, _, _) = factors(-0.3, 20.0);
        assert!(1.0 < tilted_near_edge && tilted_near_edge < tilted20, "{tilted_near_edge}");
        let (_, past, _, rest_past) = factors(-0.5, 20.0);
        assert!(!rest_past && (past - 1.0).abs() < 1e-5);
        // A light in front is bumped the same with or without the angle.
        assert_eq!(factors(0.2, 0.0), factors(0.2, 20.0));
    }

    #[test]
    fn the_flashlight_and_the_shadowed_lamps_bump_apart() {
        use crate::shader::{Fill, Split};
        use moose_assets::{FLASHLIGHT_ID, Light};
        use std::cell::Cell;
        // A surface at the origin facing +z. A lamp above it and the flashlight off to the
        // side, low and bright, aimed away: both split (a shadow, a beam). Each channel's
        // direction is its own (to it over its cosine): the lamp's (0, 1, 1), the
        // flashlight's low along -x; the lights that always reach have the ambient only.
        let params = Params::default();
        let f = |v: f32| F32s::fill(v);
        let (position, frame) = ([f(0.0); 3], [[f(1.0), f(0.0), f(0.0)], [f(0.0), f(1.0), f(0.0)], [f(0.0), f(0.0), f(1.0)]]);
        let split: [Cell<F32s>; 2] = Default::default();
        let lamp = Light::point(0, Vec3::new(0.0, 1.0, 1.0), Vec3::splat(0.5), 10.0);
        let mut flashlight = Light::spot(0, Vec3::new(-5.0, 0.0, 0.3), Vec3::splat(3.4), 16.0, Vec3::NEG_X, 6.0, 20.0);
        (flashlight.beam, flashlight.id) = (true, FLASHLIGHT_ID);
        let lights = [lamp, flashlight];
        let ctx = SampleContext {
            eye: Vec3::new(0.0, 0.0, 5.0),
            focal: 360.0,
            object: &Object::IDENTITY,
            params: &params,
            lights: &lights,
            ambient: Vec3::splat(0.05),
            light_split: &[0, 1],
            split: &split,
            back: 0.0,
        };
        let [rest, lamps, flash] = super::bumps(&ctx, &position, [&frame[0], &frame[1], &frame[2]]);
        let first = |b: &super::Bumps| b.toward.map(|v| v.to_array()[0]);
        assert_eq!(first(&rest), [0.0; 3]);
        let (l, fl) = (first(&lamps), first(&flash));
        assert!(l[0].abs() < 1e-4 && (l[1] - 1.0).abs() < 1e-4 && (l[2] - 1.0).abs() < 1e-4, "{l:?}");
        assert!(fl[0] < -10.0 && (fl[2] - 1.0).abs() < 1e-4, "{fl:?}");
        // At pixels the lamp reaches and the flashlight doesn't, the light is the ambient
        // light by its factor and the lamp's by its own: the flashlight's factor, however
        // big, changes nothing.
        let texture = Texture::solid("t", 0);
        let textures = slots(&[&texture]);
        let mut px = pixel_ctx(&textures, &params, 0, 360.0);
        px.split = Split {
            count: 2,
            light: [[f(0.4); 3], [f(2.0); 3], [f(0.0); 3], [f(0.0); 3]],
            reaches: [f(1.0), f(0.0), f(0.0), f(0.0)],
            flashlight: Some(1),
        };
        let light = [I32s::fill(((0.05f32 + 0.4 + 2.0).sqrt() * 65536.0).round() as i32); 3];
        let shade = |flash: f32| px.light_scaled(&light, f(1.0), f(1.5f32.sqrt()), f(flash.sqrt()))[1].to_array()[0];
        let want = ((0.05f32 + 0.4 * 1.5).sqrt() * 65536.0) as i32;
        assert!((shade(1.0) - want).abs() < 64 && shade(9.0) == shade(1.0), "{} {}", shade(1.0), want);
    }

    #[test]
    fn a_short_normal_bumps_the_same_whether_the_bumps_are_fading_or_not() {
        use crate::shader::Fill;
        // A filtered normal map texel's normal is short (0.9 here). Its factor must not
        // jump between where the bumps are whole and just past where they start fading.
        let f = |v: f32| F32s::fill(v);
        let short = [f(0.3 * 0.9), f(0.1 * 0.9), f((1.0f32 - 0.09 - 0.01).sqrt() * 0.9)];
        let whole = super::unit_normal(short, f(1.0));
        let fading = super::unit_normal(short, f(0.999));
        let len = |n: &[F32s; 3]| (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().to_array()[0];
        assert!((len(&whole) - 1.0).abs() < 1e-5 && (len(&fading) - 1.0).abs() < 1e-5);
        for k in 0..3 {
            assert!((whole[k] - fading[k]).abs().to_array()[0] < 1e-3, "{k}");
        }
        // Faded out, flat.
        let flat = super::unit_normal(short, f(0.0));
        assert_eq!(flat.map(|c| c.to_array()[0]), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn sections_are_picked_evenly_and_the_same_every_time() {
        use super::random_sections::section;
        use crate::shader::{Fill, I32s};
        let pick = |x: i32, y: i32, seed: i32| section(I32s::fill(x), I32s::fill(y), seed).to_array()[0];
        let mut counts = [0usize; 4];
        let (mut same_as_neighbor, mut moved_by_seed) = (0, 0);
        for y in -32..32 {
            for x in -32..32 {
                let k = pick(x, y, 0);
                assert!((0..4).contains(&k));
                assert_eq!(k, pick(x, y, 0));
                counts[k as usize] += 1;
                same_as_neighbor += (k == pick(x + 1, y, 0)) as usize;
                moved_by_seed += (k != pick(x, y, 7)) as usize;
            }
        }
        // 4096 cells: about 1024 each, a quarter alike side by side, three quarters moved.
        assert!(counts.iter().all(|&n| (900..1150).contains(&n)), "{counts:?}");
        assert!((850..1200).contains(&same_as_neighbor), "{same_as_neighbor}");
        assert!((2900..3250).contains(&moved_by_seed), "{moved_by_seed}");
        // Lanes are cells of their own.
        let lanes = section(I32s::from([0, 1, 2, 3, 4, 5, 6, 7]), I32s::fill(3), 0).to_array();
        assert_eq!(lanes, std::array::from_fn(|i| pick(i as i32, 3, 0)));
    }

    #[test]
    fn a_mirrored_copy_carries_only_its_own_values() {
        use super::{TexturedLitMirrored, TexturedNormalSpecular};
        use crate::shader::{Fill, LaneValues, layout_len};
        let (full, mirrored) = (TexturedNormalSpecular::IO, TexturedLitMirrored::IO);
        // uv, lod, light; no tangent frame, bump fade, bumps or highlight.
        assert_eq!(layout_len(mirrored.interp), 2 + 3 + 1);
        assert_eq!(layout_len(full.interp), 2 + 3 + 1 + 1 + 13 + 6);
        assert_eq!(layout_len(mirrored.sampled), layout_len(full.sampled) - 6);
        assert_eq!(<TexturedLitMirrored as Material>::Interp::len(true), 6);
        // Its values round-trip through the engine's shorter layout.
        let values: Vec<F32s> = (0..6).map(|k| F32s::fill(k as f32)).collect();
        let interp = <TexturedLitMirrored as Material>::Interp::from_lanes(&values, true);
        let mut back = vec![F32s::fill(-1.0); 6];
        interp.write_lanes(&mut back, true);
        assert_eq!(back, values);
        assert_eq!(interp.bump, [F32s::fill(0.0); 13]);
    }

    /// A texture set with `given` in its first slots, the first again in the rest.
    fn slots<'a>(given: &[&'a Texture]) -> TextureSet<'a> {
        std::array::from_fn(|k| given.get(k).copied().unwrap_or(given[0]))
    }

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
            time: 0.0,
            split: Default::default(),
        }
    }

    /// A 16.16 `light` output of exactly 1: surfaces lit as they are.
    fn full_light() -> [I32s; 3] {
        [I32s::from([65536; LANES]); 3]
    }

    #[test]
    fn a_bumped_pixel_gets_the_color_of_the_split_light_that_reaches_it() {
        use crate::shader::{Fill, Split};
        // A dim ambient, a red light whose shadow covers part of the polygon, and a bright
        // white one (a beam) also split: at these pixels the red one reaches and the white
        // one doesn't. Unbumped (factors 1), the light is the ambient and the red one's,
        // as an unbumped pixel's is, not the two split lights' summed color let through.
        let (ambient, red, white) = ([0.01f32; 3], [0.5f32, 0.05, 0.02], [1.0f32, 1.05, 1.2]);
        let f = |v: f32| F32s::fill(v);
        let texture = Texture::solid("t", 0);
        let (textures, params) = (slots(&[&texture]), Params::default());
        let mut ctx = pixel_ctx(&textures, &params, 0, 360.0);
        ctx.split = Split {
            count: 2,
            light: [red.map(f), white.map(f), [f(0.0); 3], [f(0.0); 3]],
            reaches: [f(1.0), f(0.0), f(0.0), f(0.0)],
            flashlight: None,
        };
        let light: [I32s; 3] = std::array::from_fn(|c| {
            I32s::fill(((ambient[c] + red[c] + white[c]).sqrt() * 65536.0).round() as i32)
        });
        let flat = ctx.light(&light);
        let bumped = ctx.light_scaled(&light, f(1.0), f(1.0), f(1.0));
        for c in 0..3 {
            let want = (ambient[c] + red[c]).sqrt() * 65536.0;
            let (a, b) = (flat[c].to_array()[0] as f32, bumped[c].to_array()[0] as f32);
            assert!((a - want).abs() < 64.0 && (b - want).abs() < 64.0, "channel {c}: {a}, {b}, want {want}");
        }
        // Bumped (factors on the encoded light): the ambient light by its own, the red one by
        // the shadowed lamps' (the white one, which doesn't reach, by nothing).
        let bumped = ctx.light_scaled(&light, f(0.5f32.sqrt()), f(2.0f32.sqrt()), f(1.0));
        for c in 0..3 {
            let want = (ambient[c] * 0.5 + red[c] * 2.0).sqrt() * 65536.0;
            let b = bumped[c].to_array()[0] as f32;
            assert!((b - want).abs() < 64.0, "channel {c}: {b}, want {want}");
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
        let textures = slots(&[&tex, &map]);
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
    fn a_puddle_reflects_and_moves_only_where_it_is_wet() {
        use super::filter::NEAREST_MIPMAP_NONE;
        use super::reflective_bumpy::{self as rb, ReflectiveBumpy, ReflectiveBumpyReflected};
        use crate::shader::Fill;
        // Eight texels, one a lane: the first four dry (alpha 0), the last four wet (255),
        // each its own color. A flat turbulence 12 steps up shifts the reflection 3 pixels
        // here (as `Water`'s: at w 1/2, focal 1, `shift` 2). A full mirror (F0 1, no fade).
        let colors: Vec<u32> = (0..8u32).map(|i| (if i < 4 { 0 } else { 255 << 24 }) | (0x10_20_30 + i * 0x08_08_08)).collect();
        let tex = Texture::new("t", 8, 1, colors.clone()).unwrap();
        let flat = Texture::solid("n", 0xFF80_80FF);
        let turbulence = Texture::solid("h", 140 * 0x01_01_01);
        let textures = slots(&[&tex, &flat, &turbulence]);
        let behind: Vec<u32> = (0..64).map(|i| 0x01_02_03 * i).collect();
        let depths = vec![0.25f32; 64];
        let u = I32s::from(std::array::from_fn(|i| ((2 * i as i32 + 1) << 16) / 16));
        let a = rb::Fixed32 { uv: [u, I32s::from([0; LANES])], light: full_light() };
        let (b, c) = (rb::Fixed16::default(), rb::Floats::default());
        let shade = |time: f32, ripple: f32, reflected: bool| {
            let params = Params::new(&[0.0, 0.0, 1.0, 0.0, 2.0, ripple, 1.0, 1.0, 0.0]);
            let ctx = PixelContext {
                filters: [NEAREST_MIPMAP_NONE; crate::shader::MAX_TEXTURES],
                time,
                ..pixel_ctx(&textures, &params, 1020, 1.0)
            };
            let over = Over { w: F32s::fill(0.5), behind_w: F32s::fill(0.25), row: RowBehind::new(&behind, &depths, 1000) };
            match reflected {
                true => <ReflectiveBumpyReflected>::shade_over(&a, &b, &c, &ctx, &over),
                false => <ReflectiveBumpy>::shade_pixel(&a, &b, &c, &ctx),
            }
            .to_array()
        };
        let (opaque, over) = (shade(0.0, 0.0, false), shade(0.0, 0.0, true));
        for i in 0..LANES {
            if i < 4 {
                // Dry: the ground, lit, the same in both copies (opaque over the reflection).
                assert_eq!(opaque[i], colors[i] & 0xFF_FFFF, "lane {i}");
                assert_eq!(over[i], 0xFF00_0000 | opaque[i], "lane {i}");
            } else {
                // Wet: the reflection, from 3 pixels along.
                assert_eq!(over[i], 0xFF00_0000 | behind[20 + i + 3], "lane {i}");
            }
        }
        // Rippling, only the wet ground moves: a quarter repeat at most, two texels here.
        let (then, later) = (shade(0.0, 0.25, false), shade(0.37, 0.25, false));
        assert_eq!(then[..4], opaque[..4]);
        assert_eq!(later[..4], opaque[..4]);
        assert!((4..LANES).any(|i| then[i] != later[i]), "{then:x?} {later:x?}");
        // The waves are smooth, from -1 to 1, a turn long.
        let x = F32s::from(std::array::from_fn(|i| i as f32 / 8.0));
        let (w0, w1) = (rb::wave(x).to_array(), rb::wave(x + F32s::fill(1.0)).to_array());
        assert_eq!(w0, w1);
        assert!(w0.iter().all(|v| (-1.0..=1.0).contains(v)) && w0[0] == 1.0 && w0[4] == -1.0, "{w0:?}");
        assert!((rb::turns(15502.7, 0.5) - 0.35).abs() < 1e-3);
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
        let textures = slots(&[&tex, &map]);
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
            back: 0.0,
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
        let textures = slots(&[&tex]);
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
        let textures = slots(&[&tex]);
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
        let (textures, params) = (slots(&[&blank]), Params::new(&[]));
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
