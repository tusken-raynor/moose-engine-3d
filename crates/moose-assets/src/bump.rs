//! Bump data for textures: heights, and tangent-space normals from them.
//!
//! Tangent space: x along the texture's u, y along its v (down its rows), z out of the
//! surface.

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

#[cfg(test)]
mod tests {
    use super::*;

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
