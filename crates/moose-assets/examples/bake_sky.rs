//! Bakes skies for the sky box shader (`moose_raster::shaders::sky_box`): six faces of a
//! cube map each, `NAME_px.png`, `NAME_nx.png`, `NAME_py.png`, `NAME_ny.png`, `NAME_pz.png`
//! and `NAME_nz.png` (the order of `moose_assets::CUBE_FACES`), 512 texels square:
//!
//! - `sky`: a blue gradient from a pale horizon to a deep zenith, haze below the horizon,
//!   and clouds from 3D noise;
//! - `sky_day`: the same with no clouds, for a day and night cycle, whose clouds are drawn
//!   live (`@clouds`);
//! - `sky_night`: a night, a dark gradient with a faint band of the Milky Way and a few
//!   thousand stars of different brightness and color.
//!
//! No sun: the shader draws it, toward the level's sun. The faces are HDR, stored RGBM: a
//! color is its red, green and blue times its alpha times `RANGE`, all over 255
//! (display-encoded, as the framebuffer is), so values up to `RANGE` fit: cloud tops in
//! sunlight go past white. Run from the repository, naming the skies (all three without):
//! `cargo run --release -p moose-assets --example bake_sky -- sky_day sky_night`.

use std::path::Path;

use glam::Vec3;
use moose_assets::CUBE_FACES;

/// Texels across a face.
const SIZE: u32 = 512;
/// The most an RGBM texel can hold (see the module); the shader decodes with the same.
const RANGE: f32 = 4.0;
const NAMES: [&str; 6] = ["px", "nx", "py", "ny", "pz", "nz"];
/// Stars in the night sky.
const STARS: usize = 4000;

fn main() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/textures");
    let asked: Vec<String> = std::env::args().skip(1).collect();
    for sky in ["sky", "sky_day", "sky_night"] {
        if !asked.is_empty() && !asked.iter().any(|a| a == sky) {
            continue;
        }
        let color: fn(Vec3) -> Vec3 = match sky {
            "sky" => |d| day_sky(d, true),
            "sky_day" => |d| day_sky(d, false),
            _ => night_sky,
        };
        let mut faces: Vec<Vec<Vec3>> = CUBE_FACES
            .iter()
            .map(|axes| {
                let [forward, right, up] = axes.map(Vec3::from_array);
                (0..SIZE * SIZE)
                    .map(|k| {
                        let (x, y) = (k % SIZE, k / SIZE);
                        // Image rows run down the face, against its up axis.
                        let u = 2.0 * (x as f32 + 0.5) / SIZE as f32 - 1.0;
                        let v = 2.0 * (y as f32 + 0.5) / SIZE as f32 - 1.0;
                        color((forward + right * u - up * v).normalize())
                    })
                    .collect()
            })
            .collect();
        if sky == "sky_night" {
            add_stars(&mut faces);
        }
        for (face, texels) in faces.iter().enumerate() {
            let data: Vec<u8> = texels.iter().flat_map(|&c| rgbm(c)).collect();
            let path = root.join(format!("{sky}_{}.png", NAMES[face]));
            let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), SIZE, SIZE);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
            writer.write_image_data(&data).map_err(|e| e.to_string())?;
        }
        println!("baked {sky}_*.png: six {SIZE}x{SIZE} faces, RGBM up to {RANGE}");
    }
    Ok(())
}

/// The night's color (display-encoded) in direction `d` (unit length): a dark gradient,
/// darkest overhead, a faint glow at the horizon and haze below it, and the Milky Way, a
/// band of faint mottled light across the sky.
fn night_sky(d: Vec3) -> Vec3 {
    let horizon = Vec3::new(0.045, 0.055, 0.09);
    let zenith = Vec3::new(0.006, 0.009, 0.022);
    let haze = Vec3::new(0.025, 0.027, 0.035);
    let e = d.y;
    if e < 0.0 {
        return horizon.lerp(haze, smoothstep(0.0, 0.2, -e));
    }
    let mut c = horizon.lerp(zenith, e.powf(0.35));
    // The Milky Way: about a great circle, tilted.
    let band_normal = Vec3::new(0.35, 0.45, 0.82).normalize();
    let across = band_normal.dot(d);
    let band = (-(across * across) / (2.0 * 0.12 * 0.12)).exp();
    let mottle = fbm(d * 9.0, 5);
    c += Vec3::new(0.045, 0.045, 0.055) * band * smoothstep(0.35, 0.75, mottle) * smoothstep(0.0, 0.15, e);
    c
}

/// Splats the night's stars onto `faces`: random directions (seeded, so it bakes the same
/// each time), most faint and a few bright, white to a little blue or warm, each a point on
/// the face it falls on (spread over the four nearest texels).
fn add_stars(faces: &mut [Vec<Vec3>]) {
    for k in 0..STARS as i32 {
        // A direction from two hashes (even over the sphere).
        let z = hash(k, 1, 7) * 2.0 - 1.0;
        let a = hash(k, 2, 7) * std::f32::consts::TAU;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let d = Vec3::new(r * a.cos(), z, r * a.sin());
        // Brightness: most faint, a few bright (a steep power).
        let b = 0.25 + 3.2 * hash(k, 3, 7).powf(9.0);
        let tint = match hash(k, 4, 7) {
            t if t < 0.15 => Vec3::new(0.75, 0.85, 1.0),
            t if t > 0.88 => Vec3::new(1.0, 0.85, 0.65),
            _ => Vec3::ONE,
        };
        // Its face: the one whose forward it's most along.
        let (face, axes) = CUBE_FACES
            .iter()
            .enumerate()
            .max_by(|a, b| Vec3::from_array(a.1[0]).dot(d).total_cmp(&Vec3::from_array(b.1[0]).dot(d)))
            .unwrap();
        let [forward, right, up] = axes.map(Vec3::from_array);
        let f = d.dot(forward);
        let (u, v) = (d.dot(right) / f, -d.dot(up) / f);
        let (x, y) = ((u + 1.0) / 2.0 * SIZE as f32 - 0.5, (v + 1.0) / 2.0 * SIZE as f32 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        for (dx, dy, w) in [(0, 0, (1.0 - fx) * (1.0 - fy)), (1, 0, fx * (1.0 - fy)), (0, 1, (1.0 - fx) * fy), (1, 1, fx * fy)] {
            let (px, py) = (x0 as i32 + dx, y0 as i32 + dy);
            if px >= 0 && py >= 0 && (px as u32) < SIZE && (py as u32) < SIZE {
                faces[face][(py as u32 * SIZE + px as u32) as usize] += tint * b * w;
            }
        }
    }
}

/// The day's color (display-encoded, may pass 1) in direction `d` (unit length), with its
/// clouds or without.
fn day_sky(d: Vec3, clouds: bool) -> Vec3 {
    let horizon = Vec3::new(0.80, 0.87, 0.96);
    let zenith = Vec3::new(0.20, 0.40, 0.80);
    let haze = Vec3::new(0.48, 0.50, 0.53);
    let e = d.y;
    if e < 0.0 {
        return horizon.lerp(haze, smoothstep(0.0, 0.25, -e));
    }
    let mut c = horizon.lerp(zenith, e.powf(0.45));
    if !clouds {
        return c;
    }
    // Clouds: a band of cumulus, thinning toward the horizon (where they'd be far and flat)
    // and the zenith.
    let n = fbm(d * 3.0, 5);
    let cover = smoothstep(0.52, 0.72, n) * smoothstep(0.03, 0.18, e) * (1.0 - 0.5 * smoothstep(0.6, 1.0, e));
    if cover > 0.0 {
        // Lit from above: brighter where the noise rises toward the zenith.
        let lift = fbm(d * 3.0 + Vec3::new(0.0, 0.04, 0.0), 5) - n;
        let shade = (0.95 + lift * 6.0).clamp(0.62, 1.35);
        let cloud = Vec3::new(1.0, 0.98, 0.95) * shade;
        c = c.lerp(cloud, cover);
    }
    c
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hash(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343) ^ (y as u32).wrapping_mul(0xD816_3841) ^ (z as u32).wrapping_mul(0xCB1A_B31F);
    h = (h ^ (h >> 16)).wrapping_mul(0x7FEB_352D);
    h = (h ^ (h >> 15)).wrapping_mul(0x846C_A68B);
    (h >> 8) as f32 / (1 << 24) as f32
}

/// Smooth value noise, 0 to 1.
fn noise(p: glam::Vec3) -> f32 {
    let i = p.floor();
    let f = p - i;
    let s = f * f * (glam::Vec3::splat(3.0) - 2.0 * f);
    let (x, y, z) = (i.x as i32, i.y as i32, i.z as i32);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let face = |z: i32| {
        lerp(
            lerp(hash(x, y, z), hash(x + 1, y, z), s.x),
            lerp(hash(x, y + 1, z), hash(x + 1, y + 1, z), s.x),
            s.y,
        )
    };
    lerp(face(z), face(z + 1), s.z)
}

/// Octaves of noise, each twice as fine and half as strong, normalized to 0 to 1.
fn fbm(p: glam::Vec3, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut total, mut q) = (0.0, 1.0, 0.0, p);
    for _ in 0..octaves {
        sum += noise(q) * amp;
        total += amp;
        amp *= 0.5;
        q *= 2.0;
    }
    sum / total
}

/// RGBM bytes for `c` (see the module).
fn rgbm(c: glam::Vec3) -> [u8; 4] {
    let peak = c.max_element().max(1e-4);
    let m = ((peak / RANGE * 255.0).ceil()).clamp(1.0, 255.0);
    let scale = m / 255.0 * RANGE;
    let byte = |v: f32| (v / scale * 255.0).round().clamp(0.0, 255.0) as u8;
    [byte(c.x), byte(c.y), byte(c.z), m as u8]
}
