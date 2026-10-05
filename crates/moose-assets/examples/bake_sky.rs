//! Bakes a sky for the sky box shader (`moose_raster::shaders::sky_box`): six faces of a
//! cube map, `sky_px.png`, `sky_nx.png`, `sky_py.png`, `sky_ny.png`, `sky_pz.png` and
//! `sky_nz.png` (the order of `moose_assets::CUBE_FACES`), 512 texels square. A blue
//! gradient from a pale horizon to a deep zenith, haze below the horizon, and clouds from 3D
//! noise. No sun: the shader draws it, toward the level's sun.
//!
//! The faces are HDR, stored RGBM: a color is its red, green and blue times its alpha times
//! `RANGE`, all over 255 (display-encoded, as the framebuffer is), so values up to `RANGE`
//! fit: cloud tops in sunlight go past white. Run from the repository:
//! `cargo run --release -p moose-assets --example bake_sky`.

use std::path::Path;

use moose_assets::CUBE_FACES;

/// Texels across a face.
const SIZE: u32 = 512;
/// The most an RGBM texel can hold (see the module); the shader decodes with the same.
const RANGE: f32 = 4.0;
const NAMES: [&str; 6] = ["px", "nx", "py", "ny", "pz", "nz"];

fn main() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/textures");
    for (face, axes) in CUBE_FACES.iter().enumerate() {
        let [forward, right, up] = axes.map(|a| glam::Vec3::from_array(a));
        let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        for y in 0..SIZE {
            for x in 0..SIZE {
                // Image rows run down the face, against its up axis.
                let u = 2.0 * (x as f32 + 0.5) / SIZE as f32 - 1.0;
                let v = 2.0 * (y as f32 + 0.5) / SIZE as f32 - 1.0;
                let d = (forward + right * u - up * v).normalize();
                data.extend_from_slice(&rgbm(sky(d)));
            }
        }
        let path = root.join(format!("sky_{}.png", NAMES[face]));
        let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), SIZE, SIZE);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&data).map_err(|e| e.to_string())?;
    }
    println!("baked sky_*.png: six {SIZE}x{SIZE} faces, RGBM up to {RANGE}");
    Ok(())
}

/// The sky's color (display-encoded, may pass 1) in direction `d` (unit length).
fn sky(d: glam::Vec3) -> glam::Vec3 {
    use glam::Vec3;
    let horizon = Vec3::new(0.80, 0.87, 0.96);
    let zenith = Vec3::new(0.20, 0.40, 0.80);
    let haze = Vec3::new(0.48, 0.50, 0.53);
    let e = d.y;
    if e < 0.0 {
        return horizon.lerp(haze, smoothstep(0.0, 0.25, -e));
    }
    let mut c = horizon.lerp(zenith, e.powf(0.45));
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
