//! Post passes over a finished frame.

use rayon::prelude::*;

use crate::shader::HDR_RANGE;

/// How a frame's bright light spills into a glow around it (see [`Bloom`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BloomConfig {
    /// How much of the glow is added (1: as bright as the light that makes it, before
    /// blurring).
    pub strength: f32,
    /// Light past white (an HDR material's top byte) always glows. Below 1, so does the
    /// part of any pixel's brightest channel past this (a bright pass, as 2000s games
    /// bloomed ordinary bright pixels); 1 or more, only past white.
    pub threshold: f32,
}

impl Default for BloomConfig {
    fn default() -> Self {
        Self { strength: 0.6, threshold: 1.0 }
    }
}

/// Pixels per side of the grid the glow is blurred on.
const SCALE: usize = 8;
/// The blur's reach, in grid cells each way, and its spread.
const RADIUS: usize = 6;
const SIGMA: f32 = 2.2;

/// Bloom: the light past white in a frame (and, with a threshold below 1, past that) spread
/// into a glow and added back. It is worked out on a grid an eighth the frame's size each
/// way: each cell the mean of its 8 x 8 pixels' bright light, blurred by a Gaussian (each
/// way in turn), then blended back up between cells and added to the pixels. Rows with no
/// glow near them are left alone. The frame's top bytes (an HDR material's past-white
/// amounts) are cleared. Reuse one: its buffers keep their size.
#[derive(Default)]
pub struct Bloom {
    grid: Vec<[f32; 3]>,
    across: Vec<[f32; 3]>,
    /// Per grid row, whether any of its cells glow.
    lit: Vec<bool>,
    /// Per row of pixels, whether any is past white.
    hdr_rows: Vec<bool>,
    /// Per grid cell, whether it glows enough to add anything.
    glows: Vec<bool>,
    /// Per column of pixels, its two grid columns and how far between them.
    columns: Vec<(u32, u32, f32)>,
}

impl Bloom {
    /// Adds the glow to `pixels` (`width` x `height`, XRGB). Returns whether anything glowed.
    pub fn apply(&mut self, pixels: &mut [u32], width: usize, height: usize, config: &BloomConfig) -> bool {
        let (cols, rows) = (width.div_ceil(SCALE), height.div_ceil(SCALE));
        self.grid.clear();
        self.grid.resize(cols * rows, [0.0; 3]);
        let threshold = config.threshold;
        let over = HDR_RANGE / 255.0;
        // A pixel is bright if it is past white (its top byte), or its brightest channel is
        // past the threshold (a whole-number test first, so most pixels cost a compare).
        let floor = if threshold < 1.0 { (threshold * 255.0).floor() as u32 } else { u32::MAX };
        let bright = |c: u32| -> Option<[f32; 3]> {
            let (r, g, b) = ((c >> 16) & 255, (c >> 8) & 255, c & 255);
            let top = c >> 24;
            let peak = r.max(g).max(b);
            if top == 0 && peak <= floor {
                return None;
            }
            let mut k = top as f32 * over;
            if threshold < 1.0 {
                k += (peak as f32 / 255.0 - threshold).max(0.0) / (1.0 - threshold);
            }
            Some([r, g, b].map(|v| v as f32 / 255.0 * k))
        };
        // Per row of pixels, whether any is past white, found as each grid row is filled:
        // with nothing past white in its rows (most, past white only), a grid row is passed
        // over after one sweep.
        self.hdr_rows.clear();
        self.hdr_rows.resize(height, false);
        self.grid
            .par_chunks_mut(cols)
            .zip(self.hdr_rows.par_chunks_mut(SCALE))
            .enumerate()
            .for_each(|(gy, (row, hdr))| {
            let y0 = gy * SCALE;
            let ys = y0..(y0 + SCALE).min(height);
            for (h, y) in hdr.iter_mut().zip(ys.clone()) {
                *h = pixels[y * width..(y + 1) * width].iter().fold(0, |a, &c| a | c) >> 24 != 0;
            }
            if threshold >= 1.0 && !hdr.iter().any(|&h| h) {
                return;
            }
            let n_rows = ys.len();
            for (gx, cell) in row.iter_mut().enumerate() {
                let x0 = gx * SCALE;
                let xs = x0..(x0 + SCALE).min(width);
                let mut sum = [0.0f32; 3];
                for y in ys.clone() {
                    for &c in &pixels[y * width + xs.start..y * width + xs.end] {
                        if let Some(b) = bright(c) {
                            for k in 0..3 {
                                sum[k] += b[k];
                            }
                        }
                    }
                }
                let n = (n_rows * xs.len()) as f32;
                *cell = sum.map(|v| v / n);
            }
        });
        self.lit.clear();
        self.lit.extend(self.grid.chunks(cols).map(|r| r.iter().any(|c| c[0] + c[1] + c[2] > 0.0)));
        if !self.lit.iter().any(|&l| l) {
            // Nothing to add; only past-white amounts to clear.
            let hdr_rows = &self.hdr_rows;
            pixels.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
                if hdr_rows[y] {
                    row.iter_mut().for_each(|c| *c &= 0xFF_FFFF);
                }
            });
            return false;
        }

        // Blurred across, then down.
        let weights: Vec<f32> = {
            let w: Vec<f32> = (0..=RADIUS).map(|i| (-((i * i) as f32) / (2.0 * SIGMA * SIGMA)).exp()).collect();
            let total = w[0] + 2.0 * w[1..].iter().sum::<f32>();
            w.iter().map(|v| v / total).collect()
        };
        self.across.clear();
        self.across.resize(cols * rows, [0.0; 3]);
        let (grid, lit) = (&self.grid, &self.lit);
        self.across.par_chunks_mut(cols).enumerate().for_each(|(gy, out)| {
            if !lit[gy] {
                return;
            }
            let row = &grid[gy * cols..(gy + 1) * cols];
            for (gx, o) in out.iter_mut().enumerate() {
                let mut sum = [0.0f32; 3];
                for (i, &w) in weights.iter().enumerate() {
                    let at = |x: isize| row[x.clamp(0, cols as isize - 1) as usize];
                    let (a, b) = (at(gx as isize - i as isize), at(gx as isize + i as isize));
                    let w = if i == 0 { w * 0.5 } else { w };
                    for k in 0..3 {
                        sum[k] += w * (a[k] + b[k]);
                    }
                }
                *o = sum;
            }
        });
        // Rows the vertical blur reaches from a lit one glow.
        let reach: Vec<bool> = (0..rows)
            .map(|gy| (gy.saturating_sub(RADIUS)..(gy + RADIUS + 1).min(rows)).any(|y| lit[y]))
            .collect();
        let across = &self.across;
        self.grid.par_chunks_mut(cols).enumerate().for_each(|(gy, out)| {
            if !reach[gy] {
                out.fill([0.0; 3]);
                return;
            }
            for (gx, o) in out.iter_mut().enumerate() {
                let mut sum = [0.0f32; 3];
                for (i, &w) in weights.iter().enumerate() {
                    let at = |y: isize| {
                        let y = y.clamp(0, rows as isize - 1) as usize;
                        if lit[y] { across[y * cols + gx] } else { [0.0; 3] }
                    };
                    let (a, b) = (at(gy as isize - i as isize), at(gy as isize + i as isize));
                    let w = if i == 0 { w * 0.5 } else { w };
                    for k in 0..3 {
                        sum[k] += w * (a[k] + b[k]);
                    }
                }
                *o = sum;
            }
        });
        self.lit.clear();
        self.lit.extend_from_slice(&reach);

        // Added back, blended between cell centers. Cells too faint to add a level are
        // passed over.
        let limit = 0.5 / (config.strength * 255.0);
        self.glows.clear();
        self.glows.extend(self.grid.iter().map(|c| c[0].max(c[1]).max(c[2]) > limit));
        // Each column's two cells and how far between them.
        self.columns.clear();
        self.columns.extend((0..width).map(|x| {
            let gx = (x as f32 + 0.5) / SCALE as f32 - 0.5;
            let x0 = (gx.floor().max(0.0) as usize).min(cols - 1);
            (x0 as u32, (x0 + 1).min(cols - 1) as u32, (gx - x0 as f32).clamp(0.0, 1.0))
        }));
        let (grid, lit, glows, hdr_rows, columns) =
            (&self.grid, &self.lit, &self.glows, &self.hdr_rows, &self.columns);
        let strength = config.strength * 255.0;
        pixels.par_chunks_mut(width).enumerate().for_each_init(Vec::new, |line, (y, row)| {
            let gy = (y as f32 + 0.5) / SCALE as f32 - 0.5;
            let g0 = (gy.floor().max(0.0) as usize).min(rows - 1);
            let g1 = (g0 + 1).min(rows - 1);
            if !lit[g0] && !lit[g1] {
                if hdr_rows[y] {
                    row.iter_mut().for_each(|c| *c &= 0xFF_FFFF);
                }
                return;
            }
            // The glow along this row at each cell column, already times the strength, and
            // whether it adds anything there.
            let fy = (gy - g0 as f32).clamp(0.0, 1.0);
            let (r0, r1) = (&grid[g0 * cols..(g0 + 1) * cols], &grid[g1 * cols..(g1 + 1) * cols]);
            let (m0, m1) = (&glows[g0 * cols..(g0 + 1) * cols], &glows[g1 * cols..(g1 + 1) * cols]);
            line.clear();
            line.extend((0..cols).map(|gx| {
                let v: [f32; 3] = std::array::from_fn(|k| (r0[gx][k] + (r1[gx][k] - r0[gx][k]) * fy) * strength);
                (v, m0[gx] || m1[gx])
            }));
            for (c, &(x0, x1, fx)) in row.iter_mut().zip(columns) {
                let (a, b) = (&line[x0 as usize], &line[x1 as usize]);
                let mut out = *c & 0xFF_FFFF;
                if a.1 || b.1 {
                    for (k, shift) in [16u32, 8, 0].into_iter().enumerate() {
                        let glow = a.0[k] + (b.0[k] - a.0[k]) * fx;
                        if glow > 0.5 {
                            let v = ((out >> shift) & 255) as f32 + glow;
                            out = (out & !(255 << shift)) | ((v.min(255.0) as u32) << shift);
                        }
                    }
                }
                *c = out;
            }
        });
        true
    }
}
