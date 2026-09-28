//! Half-Life's software water: a texture that ripples as raindrops fall on it.
//!
//! A wrapping height field is stepped with the classic ripple equation (Hugo Elias's
//! "2D water"), random drops disturb it, and the texture is redrawn from the field: each
//! texel copies the source texel the height pushes it to. The same field covers every
//! repeat of the texture, so every tile ripples alike. After the recreation of the effect in
//! Xash3D FWGS (`r_ripple`, ref/gl/gl_warp.c), which follows the original's constants.

use crate::texture::{MipLevel, Texture};

/// Side of the square height field, in cells.
pub const RIPPLE_SIZE: usize = 128;
/// Seconds between simulation steps (20 a second).
pub const RIPPLE_STEP: f64 = 0.05;
/// A new drop falls every this many steps (every 0.1 s).
const STEPS_PER_DROP: u64 = 2;
/// A stall longer than this many steps skips ahead instead of catching up.
const MAX_CATCH_UP: u64 = 200;

const CELLS: usize = RIPPLE_SIZE * RIPPLE_SIZE;
const MASK: usize = CELLS - 1;

/// The ripple simulation. Deterministic for a given seed and time, so a screenshot at a
/// given time always shows the same water.
#[derive(Clone, Debug)]
pub struct Ripples {
    /// The field now, and one step before. Each step overwrites the older with the next.
    cur: Vec<i16>,
    old: Vec<i16>,
    steps: u64,
    rng: u32,
}

impl Ripples {
    /// Still water.
    pub fn new(seed: u32) -> Self {
        Self {
            cur: vec![0; CELLS],
            old: vec![0; CELLS],
            steps: 0,
            rng: seed,
        }
    }

    /// Runs the simulation up to `time` seconds from the start, one step every
    /// [`RIPPLE_STEP`]. Returns whether the water changed.
    pub fn advance_to(&mut self, time: f64) -> bool {
        let target = (time.max(0.0) / RIPPLE_STEP) as u64;
        if target <= self.steps {
            return false;
        }
        self.steps = self.steps.max(target.saturating_sub(MAX_CATCH_UP));
        while self.steps < target {
            self.step();
        }
        true
    }

    /// One step: the newest field becomes the older one, maybe takes a drop, and the next
    /// is made from the two.
    fn step(&mut self) {
        std::mem::swap(&mut self.cur, &mut self.old);
        self.steps += 1;
        if self.steps.is_multiple_of(STEPS_PER_DROP) {
            // Like C's rand(): 15 bits.
            let (x, y, strength) = (self.rand(), self.rand(), self.rand() & 0x3FF);
            self.drop(x as usize, y as usize, strength as i16);
        }
        propagate(&self.old, &mut self.cur);
    }

    /// A drop of `strength` at cell `(x, y)` (wrapping), a quarter as strong on the four
    /// cells around it.
    fn drop(&mut self, x: usize, y: usize, strength: i16) {
        let at = |x: usize, y: usize| (x & (RIPPLE_SIZE - 1)) + ((y & (RIPPLE_SIZE - 1)) << 7);
        let old = &mut self.old;
        old[at(x, y)] = old[at(x, y)].wrapping_add(strength);
        let side = strength >> 2;
        for (dx, dy) in [(1, 0), (RIPPLE_SIZE - 1, 0), (0, 1), (0, RIPPLE_SIZE - 1)] {
            let i = at(x + dx, y + dy);
            old[i] = old[i].wrapping_add(side);
        }
    }

    fn rand(&mut self) -> u32 {
        self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (self.rng >> 16) & 0x7FFF
    }

    /// The field's height at cell `(x, y)`, wrapping.
    pub fn height(&self, x: usize, y: usize) -> i16 {
        self.cur[(x % RIPPLE_SIZE) + (y % RIPPLE_SIZE) * RIPPLE_SIZE]
    }

    /// `source` rippled, with its mip chain: [`RIPPLE_SIZE`] texels along its longer side
    /// (keeping its shape). Texel `(x, y)` is the source's at `(x + h, y - h)` (wrapping,
    /// in this texture's texels, nearest), `h` a sixteenth of the field's height there.
    pub fn texture(&self, name: &str, source: &MipLevel) -> Texture {
        let (sw, sh) = (source.width as usize, source.height as usize);
        let (width, height) = if sw >= sh {
            (RIPPLE_SIZE, RIPPLE_SIZE * sh / sw)
        } else {
            (RIPPLE_SIZE * sw / sh, RIPPLE_SIZE)
        };
        let mut texels = Vec::with_capacity(width * height);
        for y in 0..height {
            let ry = y * RIPPLE_SIZE / height;
            for x in 0..width {
                let rx = x * RIPPLE_SIZE / width;
                let h = self.cur[ry * RIPPLE_SIZE + rx] as i32 / 16;
                let px = (x as i32 + h).rem_euclid(width as i32) as usize * sw / width;
                let py = (y as i32 - h).rem_euclid(height as i32) as usize * sh / height;
                texels.push(source.texels[py * sw + px]);
            }
        }
        Texture::new(name, width as u32, height as u32, texels)
            .expect("powers of two in, powers of two out")
    }
}

/// The next field from the last (`old`) and the one before it (`cur`, overwritten): half
/// the four neighbors, less the value two steps ago, then damped by 1/64. The field wraps
/// as one long line, as the original's does.
fn propagate(old: &[i16], cur: &mut [i16]) {
    let w = RIPPLE_SIZE;
    for (c, h) in cur.iter_mut().enumerate() {
        let around = old[c.wrapping_sub(w) & MASK] as i32
            + old[c.wrapping_sub(1) & MASK] as i32
            + old[(c + 1) & MASK] as i32
            + old[(c + w) & MASK] as i32;
        let v = ((around >> 1) - *h as i32) as i16;
        *h = (v as i32 - (v >> 6) as i32) as i16;
    }
}

impl Ripples {
    /// The shift [`texture`](Self::texture) gives each texel, as a map: [`RIPPLE_SIZE`]
    /// texels square (one per cell), grey, each channel `128 + 4h` for a shift of `h`
    /// texels, in quarter texels so that filtering it gives fractions of a texel (clamped
    /// to -127..=127, about 32 texels either way). Sampled at a texture coordinate of a
    /// rippled texture of the same shape, it tells how far that texel was moved.
    pub fn heights(&self, name: &str) -> Texture {
        let texels = self
            .cur
            .iter()
            .map(|&h| {
                let v = (128 + (h as i32 / 4).clamp(-127, 127)) as u32;
                0xFF00_0000 | (v * 0x01_01_01)
            })
            .collect();
        Texture::new(name, RIPPLE_SIZE as u32, RIPPLE_SIZE as u32, texels).expect("a power of two")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Texture {
        Texture::new("t", 64, 64, (0..64 * 64).collect()).unwrap()
    }

    #[test]
    fn still_water_is_the_source_scaled_up() {
        let t = Ripples::new(1).texture("w", source().base());
        assert_eq!((t.width(), t.height()), (128, 128));
        for y in 0..128 {
            for x in 0..128 {
                assert_eq!(t.texel(x, y), (y / 2) * 64 + x / 2);
            }
        }
    }

    #[test]
    fn heights_push_texels_across_and_up() {
        let mut r = Ripples::new(1);
        r.cur[5 * RIPPLE_SIZE + 10] = 3 * 16; // h = 3 at (10, 5)
        r.cur[RIPPLE_SIZE - 1] = -16; // h = -1 at (127, 0), wrapping both ways
        let t = r.texture("w", source().base());
        let src = |x: u32, y: u32| (y / 2) * 64 + x / 2;
        assert_eq!(t.texel(10, 5), src(13, 2));
        assert_eq!(t.texel(127, 0), src(126, 1));
        assert_eq!(t.texel(11, 5), src(11, 5), "its neighbors stay");
        let map = r.heights("h");
        assert_eq!(map.texel(10, 5), 0xFF_8C_8C_8C, "128 + 4 * 3");
        assert_eq!(map.texel(127, 0), 0xFF_7C_7C_7C, "128 - 4 * 1");
        assert_eq!(map.texel(11, 5), 0xFF_80_80_80, "0");
    }

    #[test]
    fn a_drop_spreads_in_a_ring_and_dies_down() {
        let mut r = Ripples::new(1);
        r.drop(64, 64, 1000);
        for _ in 0..20 {
            // Steps with no new drops.
            std::mem::swap(&mut r.cur, &mut r.old);
            propagate(&r.old, &mut r.cur);
        }
        // Symmetric about the drop, and it has reached 10 cells out.
        for d in 1..20 {
            assert_eq!(r.height(64 + d, 64), r.height(64 - d, 64), "x {d}");
            assert_eq!(r.height(64, 64 + d), r.height(64, 64 - d), "y {d}");
            assert_eq!(r.height(64 + d, 64), r.height(64, 64 + d), "x and y {d}");
        }
        assert!((10..20).any(|d| r.height(64 + d, 64) != 0));
        // The whole simulation settles: drops keep falling, but the damping holds the field
        // in bounds.
        let mut r = Ripples::new(7);
        r.advance_to(60.0);
        let peak = r.cur.iter().map(|h| h.unsigned_abs()).max().unwrap();
        assert!(peak < 4096, "peak {peak}");
    }

    #[test]
    fn the_same_time_shows_the_same_water() {
        let (mut a, mut b) = (Ripples::new(3), Ripples::new(3));
        assert!(a.advance_to(2.5));
        for t in [0.3, 1.0, 1.7, 2.5] {
            b.advance_to(t);
        }
        assert!(!b.advance_to(2.5), "nothing new");
        assert_eq!(a.cur, b.cur);
        assert_ne!(a.cur, Ripples::new(3).cur, "it moved");
    }
}
