//! Clouds for the sky (`@clouds`): a layer of them, as a tiling field of evolving noise, for
//! the sky box shader to read where its view meets the layer.
//!
//! The field is fractal value noise in three dimensions, two of them the layer (tiling: the
//! noise's lattice wraps) and the third time, so clouds form, change and fade as time goes
//! on; the seed picks which clouds. How much of it is cloud (the cover) isn't in the field:
//! the shader cuts the field at a level the cover sets, so the cover changes smoothly every
//! frame. Wind is the shader's too (it slides where it reads).
//!
//! The field is made in generations, one each [`PERIOD`] seconds of cloud time, and the
//! shader blends the last two by how far between them the time is, so it changes smoothly
//! with only a generation's work every period. That work is spread over the period: each
//! [`Clouds::advance_to`] makes the share of the next generation the time has come to, so
//! no frame does it all. A generation's texels are its field (0 to 255) and how lit each is
//! from the sun's side (brighter where the field falls away toward the sun, darker where it
//! rises), each texel holding both generations: red and green the older, blue and alpha the
//! newer.

use glam::{Vec2, Vec3};

use crate::Texture;

/// Seconds of cloud time between generations.
pub const PERIOD: f64 = 0.5;
/// Texels across the field (a power of two).
pub const CLOUD_SIZE: usize = 128;
/// How fast the field changes: noise units a second along its time axis.
const EVOLVE: f64 = 0.02;
/// Lattice cells across the field in the coarsest octave, and the octaves.
const CELLS: usize = 4;
const OCTAVES: usize = 4;

/// One generation: per texel, its field and how lit it is, 0 to 255.
#[derive(Clone)]
struct Generation {
    index: i64,
    field: Vec<f32>,
    lit: Vec<u8>,
}

/// The cloud field, its generations, and the one being made.
pub struct Clouds {
    seed: u32,
    /// The older and newer generations shown, and the next (made a share at a time: how
    /// many of its rows are done).
    older: Generation,
    newer: Generation,
    next: Generation,
    rows_done: usize,
    /// The sun's direction across the layer (unit, or zero), for lighting the field.
    sun: Vec2,
}

impl Clouds {
    /// The field for `seed` at cloud time `time`, lit from `sun` (toward the sun).
    pub fn new(seed: u32, time: f64, sun: Vec3) -> Clouds {
        let k = (time / PERIOD).floor() as i64;
        let sun = sun_across(sun);
        Clouds {
            seed,
            older: generation(seed, k, sun),
            newer: generation(seed, k + 1, sun),
            next: Generation { index: k + 2, field: vec![0.0; CLOUD_SIZE * CLOUD_SIZE], lit: vec![0; CLOUD_SIZE * CLOUD_SIZE] },
            rows_done: 0,
            sun,
        }
    }

    /// How far from the older generation to the newer the shader blends at `time`, 0 to 1.
    pub fn blend(&self, time: f64) -> f32 {
        ((time / PERIOD) - self.older.index as f64).clamp(0.0, 1.0) as f32
    }

    /// Moves to cloud time `time`, lit from `sun`: makes the share of the next generation
    /// due, and moves the generations on when a period passes (a jump of more than one is
    /// made whole). Returns whether the texture changed (see [`Clouds::texture`]).
    pub fn advance_to(&mut self, time: f64, sun: Vec3) -> bool {
        let k = (time / PERIOD).floor() as i64;
        self.sun = sun_across(sun);
        if k == self.older.index {
            // The share of the next generation the time has come to.
            let due = (((time / PERIOD) - k as f64) * CLOUD_SIZE as f64).ceil() as usize;
            let due = due.min(CLOUD_SIZE);
            if due > self.rows_done {
                fill_rows(&mut self.next, self.seed, self.rows_done..due);
                self.rows_done = due;
            }
            return false;
        }
        if k == self.older.index + 1 {
            // A period on: the next is shown, finished first if it isn't.
            fill_rows(&mut self.next, self.seed, self.rows_done..CLOUD_SIZE);
            light(&mut self.next, self.sun);
            let fresh = Generation { index: k + 2, field: std::mem::take(&mut self.older.field), lit: std::mem::take(&mut self.older.lit) };
            self.older = std::mem::replace(&mut self.newer, std::mem::replace(&mut self.next, fresh));
        } else {
            // A jump: made whole.
            self.older = generation(self.seed, k, self.sun);
            self.newer = generation(self.seed, k + 1, self.sun);
            self.next.index = k + 2;
        }
        self.rows_done = 0;
        true
    }

    /// The texture for the shader: per texel, red and green the older generation's field and
    /// light, blue and alpha the newer's.
    pub fn texture(&self, name: &str) -> Texture {
        let texels = (0..CLOUD_SIZE * CLOUD_SIZE)
            .map(|i| {
                let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
                (self.newer.lit[i] as u32) << 24
                    | byte(self.older.field[i]) << 16
                    | (self.older.lit[i] as u32) << 8
                    | byte(self.newer.field[i])
            })
            .collect();
        Texture::new(name, CLOUD_SIZE as u32, CLOUD_SIZE as u32, texels).expect("the cloud field is a power of two")
    }

    /// How much cloud is at `at` on the layer (in fields across: it tiles) at `time`, under
    /// `cover` (see [`density`]): for dimming the sun behind a cloud.
    pub fn density_at(&self, at: Vec2, time: f64, cover: f32) -> f32 {
        let t = self.blend(time);
        // Bilinear, as the shader reads it (texel centers at halves), wrapping.
        let read = |g: &Generation| {
            let n = CLOUD_SIZE as isize;
            let (x, y) = (at.x.rem_euclid(1.0) * CLOUD_SIZE as f32 - 0.5, at.y.rem_euclid(1.0) * CLOUD_SIZE as f32 - 0.5);
            let (x0, y0) = (x.floor(), y.floor());
            let (fx, fy) = (x - x0, y - y0);
            let texel = |dx: isize, dy: isize| {
                let (sx, sy) = ((x0 as isize + dx).rem_euclid(n), (y0 as isize + dy).rem_euclid(n));
                g.field[(sy * n + sx) as usize]
            };
            let top = texel(0, 0) + (texel(1, 0) - texel(0, 0)) * fx;
            let bottom = texel(0, 1) + (texel(1, 1) - texel(0, 1)) * fx;
            top + (bottom - top) * fy
        };
        density(read(&self.older) + (read(&self.newer) - read(&self.older)) * t, cover)
    }
}

/// The field's level the cover cuts it at (see [`density`]): from most of it clear (cover 0)
/// to all of it cloud (1).
pub fn cut(cover: f32) -> f32 {
    0.78 + (-0.25 - 0.78) * cover.clamp(0.0, 1.0)
}

/// How soft clouds' edges are: the field's range from none to whole.
pub const SOFTNESS: f32 = 0.22;

/// How much cloud a field value is under `cover`: none below the cut, whole a
/// [`SOFTNESS`] above it, smoothstepped (the shader does the same).
pub fn density(field: f32, cover: f32) -> f32 {
    let t = ((field - cut(cover)) / SOFTNESS).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The sun's way across the layer (`toward` the sun, flattened onto it), unit, or zero
/// straight overhead.
fn sun_across(toward: Vec3) -> Vec2 {
    Vec2::new(toward.x, toward.z).normalize_or_zero()
}

/// Generation `k` of the field for `seed`, made and lit whole.
fn generation(seed: u32, k: i64, sun: Vec2) -> Generation {
    let mut g = Generation { index: k, field: vec![0.0; CLOUD_SIZE * CLOUD_SIZE], lit: vec![0; CLOUD_SIZE * CLOUD_SIZE] };
    fill_rows(&mut g, seed, 0..CLOUD_SIZE);
    light(&mut g, sun);
    g
}

/// Makes rows `rows` of generation `g`'s field: fractal value noise, tiling across the
/// field, at its cloud time.
fn fill_rows(g: &mut Generation, seed: u32, rows: std::ops::Range<usize>) {
    let z = (g.index as f64 * PERIOD * EVOLVE) as f32;
    for y in rows {
        for x in 0..CLOUD_SIZE {
            let (u, v) = (x as f32 / CLOUD_SIZE as f32, y as f32 / CLOUD_SIZE as f32);
            let (mut sum, mut weight, mut amp, mut cells) = (0.0, 0.0, 1.0, CELLS);
            for octave in 0..OCTAVES {
                sum += amp * noise(seed.wrapping_add(octave as u32 * 7919), u * cells as f32, v * cells as f32, z * (1 << octave) as f32, cells);
                weight += amp;
                amp *= 0.5;
                cells *= 2;
            }
            g.field[y * CLOUD_SIZE + x] = sum / weight;
        }
    }
}

/// Lights generation `g` from the sun's side (`sun`, across the layer): a texel whose field
/// falls away toward the sun is on a cloud's sunlit edge, brighter; one it rises toward is in
/// its shade. Overhead (`sun` zero), all evenly.
fn light(g: &mut Generation, sun: Vec2) {
    let step = sun * 2.0;
    for y in 0..CLOUD_SIZE {
        for x in 0..CLOUD_SIZE {
            let at = |dx: f32, dy: f32| {
                let (sx, sy) = ((x as f32 + dx).round() as isize, (y as f32 + dy).round() as isize);
                let n = CLOUD_SIZE as isize;
                g.field[(sy.rem_euclid(n) * n + sx.rem_euclid(n)) as usize]
            };
            let here = g.field[y * CLOUD_SIZE + x];
            let lit = (0.6 + 4.0 * (here - at(step.x, step.y))).clamp(0.0, 1.0);
            g.lit[y * CLOUD_SIZE + x] = (lit * 255.0).round() as u8;
        }
    }
}

/// Smooth value noise at `(x, y, z)`, its lattice wrapping every `period` cells across x and
/// y (so it tiles), from 0 to 1.
fn noise(seed: u32, x: f32, y: f32, z: f32, period: usize) -> f32 {
    let (x0, y0, z0) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (x - x0, y - y0, z - z0);
    let ease = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy, sz) = (ease(fx), ease(fy), ease(fz));
    let p = period as i64;
    let corner = |dx: i64, dy: i64, dz: i64| {
        let cx = (x0 as i64 + dx).rem_euclid(p);
        let cy = (y0 as i64 + dy).rem_euclid(p);
        let cz = z0 as i64 + dz;
        hash(seed, cx as u32, cy as u32, cz as u32)
    };
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let plane = |dz: i64| {
        lerp(lerp(corner(0, 0, dz), corner(1, 0, dz), sx), lerp(corner(0, 1, dz), corner(1, 1, dz), sx), sy)
    };
    lerp(plane(0), plane(1), sz)
}

/// A lattice corner's value, 0 to 1.
fn hash(seed: u32, x: u32, y: u32, z: u32) -> f32 {
    let mut h = seed ^ x.wrapping_mul(0x8DA6_B343) ^ y.wrapping_mul(0xD816_3841) ^ z.wrapping_mul(0xCB1A_B31F);
    h = (h ^ (h >> 13)).wrapping_mul(0x5BD1_E995);
    h ^= h >> 15;
    (h & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_field_tiles_changes_with_time_and_follows_its_seed() {
        let sun = Vec3::new(0.5, 0.6, 0.3);
        let a = Clouds::new(7, 0.0, sun);
        // Tiling: the left and right edges meet as neighbors do.
        let row = |g: &Generation, y: usize| (g.field[y * CLOUD_SIZE], g.field[y * CLOUD_SIZE + CLOUD_SIZE - 1]);
        let (left, right) = row(&a.older, 10);
        assert!((left - right).abs() < 0.1, "{left} {right}");
        // Another seed, other clouds; later, the field has moved on.
        let b = Clouds::new(8, 0.0, sun);
        assert_ne!(a.older.field, b.older.field);
        let later = Clouds::new(7, 60.0, sun);
        assert_ne!(a.older.field, later.older.field);
        // The same seed and time make the same clouds.
        assert_eq!(a.older.field, Clouds::new(7, 0.0, sun).older.field);
    }

    #[test]
    fn generations_are_made_a_share_at_a_time_and_match_one_made_whole() {
        let sun = Vec3::new(0.3, 0.8, -0.5);
        let mut c = Clouds::new(3, 0.0, sun);
        // A quarter of a period in, a quarter of the next generation is made.
        assert!(!c.advance_to(PERIOD * 0.25, sun));
        assert_eq!(c.rows_done, CLOUD_SIZE / 4);
        assert!(!c.advance_to(PERIOD * 0.9, sun));
        // On to the next period: it shows, as one made whole would.
        assert!(c.advance_to(PERIOD * 1.1, sun));
        let whole = Clouds::new(3, PERIOD * 1.1, sun);
        assert_eq!(c.older.field, whole.older.field);
        assert_eq!(c.newer.field, whole.newer.field);
        assert!((c.blend(PERIOD * 1.5) - 0.5).abs() < 1e-5);
        // A jump is made whole too.
        assert!(c.advance_to(PERIOD * 40.0, sun));
        assert_eq!(c.newer.field, Clouds::new(3, PERIOD * 40.0, sun).newer.field);
    }

    #[test]
    fn the_cover_goes_from_clear_to_overcast() {
        let c = Clouds::new(11, 0.0, Vec3::Y);
        let mean = |cover: f32| {
            (0..CLOUD_SIZE * CLOUD_SIZE).map(|i| density(c.older.field[i], cover)).sum::<f32>() / (CLOUD_SIZE * CLOUD_SIZE) as f32
        };
        assert!(mean(0.0) < 0.05, "{}", mean(0.0));
        assert!(mean(1.0) > 0.98, "{}", mean(1.0));
        assert!(mean(0.3) < mean(0.5) && mean(0.5) < mean(0.7));
        let t = c.texture("clouds");
        assert_eq!((t.base().width, t.base().height), (CLOUD_SIZE as u32, CLOUD_SIZE as u32));
    }
}
