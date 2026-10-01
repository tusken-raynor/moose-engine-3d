//! Bump data for textures: heights, tangent-space normals, the radiosity normal mapping
//! basis's weights, and a normal packed into one byte (4 bits each for x and y, z implied).
//!
//! Tangent space: x along the texture's u, y along its v (down its rows), z out of the
//! surface.

use crate::texture::{MipLevel, Texture};

/// The basis radiosity normal mapping lights a surface along (Half-Life 2's), in tangent
/// space: three unit vectors 54.7° from the normal, 120° apart around it, the first
/// leaning toward +u. A normal's weights are its squared cosines with them (each at least
/// 0), over their sum: a flat surface has a third of each.
pub const BASIS: [[f32; 3]; 3] = [
    [0.816_496_6, 0.0, 0.577_350_3],
    [-0.408_248_3, std::f32::consts::FRAC_1_SQRT_2, 0.577_350_3],
    [-0.408_248_3, -std::f32::consts::FRAC_1_SQRT_2, 0.577_350_3],
];

/// A normal's weights in [`BASIS`]: squared cosines (each at least 0), over their sum.
pub fn basis_weights(n: [f32; 3]) -> [f32; 3] {
    let w = BASIS.map(|b| {
        let c = (n[0] * b[0] + n[1] * b[1] + n[2] * b[2]).max(0.0);
        c * c
    });
    let sum = (w[0] + w[1] + w[2]).max(1e-6);
    w.map(|v| v / sum)
}

/// Steps of a packed normal's x and y each way from 0 (see [`pack`]): 15 values, -7 to 7,
/// so that 0 (flat) is one of them.
pub const PACKED_STEPS: i32 = 7;

/// A packed normal's implied z: a normal is `(x / 7, y / 7, PACKED_Z)` normalized, so the
/// steepest leans `atan(1 / PACKED_Z)` (59°) from straight out, and x = y = 0 is straight
/// out.
pub const PACKED_Z: f32 = 0.6;

/// `n` (unit length, facing out) in one byte: x in the high 4 bits and y in the low 4,
/// each `7 + round(7 * PACKED_Z * slope)`, the slope `n.x / n.z` (or `n.y / n.z`) held to
/// what fits. Coarse (a step leans about 13° near flat), but one channel.
pub fn pack(n: [f32; 3]) -> u8 {
    let z = n[2].max(1e-3);
    let step = |c: f32| {
        ((c / z * PACKED_Z * PACKED_STEPS as f32).round() as i32).clamp(-PACKED_STEPS, PACKED_STEPS)
    };
    ((step(n[0]) + PACKED_STEPS) << 4 | (step(n[1]) + PACKED_STEPS)) as u8
}

/// The unit normal packed in `byte` (see [`pack`]); the unused value 15 reads as 14.
pub fn unpack(byte: u8) -> [f32; 3] {
    let step = |b: u8| (b as i32 - PACKED_STEPS).min(PACKED_STEPS) as f32 / PACKED_STEPS as f32;
    let (x, y, z) = (step(byte >> 4), step(byte & 15), PACKED_Z);
    let len = (x * x + y * y + z * z).sqrt();
    [x / len, y / len, z / len]
}

/// The height of a raised area's outermost texels (see [`bevel_heights`]).
pub const EDGE: f32 = 0.3;

/// Heights from a mask (`true` raised, as bricks are; row by row, `width` x `height`,
/// wrapping): 0 off it, and on it rising over `bevel` texels from its edge, from [`EDGE`]
/// to 1 (a smoothstep), so its edges are rounded. Heights are above 0 just where the mask
/// is raised, so the mask can be read back from them.
pub fn bevel_heights(mask: &[bool], width: u32, height: u32, bevel: f32) -> Vec<f32> {
    let (w, h) = (width as i32, height as i32);
    let reach = bevel.ceil() as i32 + 1;
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            if !mask[(y * w + x) as usize] {
                return 0.0;
            }
            // How far its center is from the nearest texel off the mask (within reach).
            let mut near = f32::INFINITY;
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    let (xx, yy) = ((x + dx).rem_euclid(w), (y + dy).rem_euclid(h));
                    if !mask[(yy * w + xx) as usize] {
                        near = near.min(((dx * dx + dy * dy) as f32).sqrt());
                    }
                }
            }
            let t = ((near - 0.5) / bevel).clamp(0.0, 1.0);
            EDGE + (1.0 - EDGE) * t * t * (3.0 - 2.0 * t)
        })
        .collect()
}

/// The tangent-space normal at each texel of a `width`x`height` height field (row by row,
/// wrapping): `(-dh/du, -dh/dv, 1)` normalized, the slopes from central differences times
/// `strength` (texels of height per unit of `heights`).
pub fn normals(heights: &[f32], width: u32, height: u32, strength: f32) -> Vec<[f32; 3]> {
    let (w, h) = (width as i32, height as i32);
    let at = |x: i32, y: i32| heights[(y.rem_euclid(h) * w + x.rem_euclid(w)) as usize];
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let du = (at(x + 1, y) - at(x - 1, y)) * 0.5 * strength;
            let dv = (at(x, y + 1) - at(x, y - 1)) * 0.5 * strength;
            let len = (du * du + dv * dv + 1.0).sqrt();
            [-du / len, -dv / len, 1.0 / len]
        })
        .collect()
}

/// Remakes the alpha of `texture`'s mip levels below the first as packed normals (see
/// [`pack`]): each the normals it covers in the level above, unpacked, averaged and packed
/// again. Averaging the packed bytes, as mip levels are made, mixes their x and y bits.
pub fn repack_mips(texture: &mut Texture) {
    for l in 1..texture.levels.len() {
        let (above, below) = texture.levels.split_at_mut(l);
        let (parent, level): (&MipLevel, &mut MipLevel) = (&above[l - 1], &mut below[0]);
        let (sx, sy) = (parent.width / level.width, parent.height / level.height);
        for y in 0..level.height {
            for x in 0..level.width {
                let mut sum = [0.0f32; 3];
                for dy in 0..sy {
                    for dx in 0..sx {
                        let n = unpack((parent.texel(x * sx + dx, y * sy + dy) >> 24) as u8);
                        for (s, c) in sum.iter_mut().zip(n) {
                            *s += c;
                        }
                    }
                }
                let len = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt().max(1e-6);
                let packed = pack(sum.map(|c| c / len)) as u32;
                let t = &mut level.texels[(y * level.width + x) as usize];
                *t = (*t & 0xFF_FFFF) | packed << 24;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_normal_weighs_the_basis_evenly() {
        let w = basis_weights([0.0, 0.0, 1.0]);
        for v in w {
            assert!((v - 1.0 / 3.0).abs() < 1e-5, "{w:?}");
        }
        // Leaning toward the first basis vector weighs it most.
        let w = basis_weights([0.5, 0.0, 0.866]);
        assert!(w[0] > w[1] && w[0] > w[2], "{w:?}");
    }

    #[test]
    fn packed_normals_keep_flat_exact_and_repack_the_same() {
        assert_eq!(unpack(pack([0.0, 0.0, 1.0])), [0.0, 0.0, 1.0]);
        for x in 0..15u8 {
            for y in 0..15u8 {
                let byte = x << 4 | y;
                assert_eq!(pack(unpack(byte)), byte, "{byte:#x}");
            }
        }
        // Leaning toward +u: x above the middle.
        assert!(pack([0.5, 0.0, 0.866]) >> 4 > PACKED_STEPS as u8);
    }

    #[test]
    fn bevels_rise_from_the_mortar() {
        // A raised block 8 texels across in 12: its middle highest, its edges low.
        let mask: Vec<bool> =
            (0..12 * 12).map(|i| (2..10).contains(&(i % 12)) && (2..10).contains(&(i / 12))).collect();
        let h = bevel_heights(&mask, 12, 12, 2.0);
        assert_eq!(h[0], 0.0);
        let (edge, middle) = (h[6 * 12 + 2], h[6 * 12 + 5]);
        assert!(edge > 0.0 && edge < middle && (middle - 1.0).abs() < 1e-6, "{edge} {middle}");
    }

    #[test]
    fn a_slope_up_along_u_leans_the_normal_back() {
        // Heights rising along u: the normal leans toward -u.
        let heights: Vec<f32> = (0..16).map(|i| (i % 4) as f32 * 0.25).collect();
        let n = normals(&heights, 4, 4, 1.0);
        assert!(n[5][0] < 0.0 && n[5][1].abs() < 1e-6, "{:?}", n[5]);
    }
}
