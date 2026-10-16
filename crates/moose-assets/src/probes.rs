//! Ambient light probes: a grid of [`Ambient`] cubes over a level, each the light that
//! reaches its point from every way, read anywhere between them by blending the nearest.

use glam::Vec3;

use crate::level::Ambient;

/// How [`AmbientProbes::sample`] blends the probes around a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeBlend {
    /// The 8 corners of the grid cell the point is in. (A blend of the 4 corners of the
    /// tetrahedron the point is in, of 6 per cell, was dropped: it showed blocks where a
    /// bounce light is close, and blended for a sample group's points at once, cost what
    /// this does.)
    Trilinear,
    /// The 27 probes around the one nearest the point, by a quadratic B-spline: the light's
    /// slope changes smoothly, where the others' turn at every cell's faces (soft bands in
    /// motion, a cell wide). It doesn't pass through the probes: a probe's own light is
    /// three quarters of what it gives at its point along each axis, its neighbors' the rest.
    Smooth,
}

impl ProbeBlend {
    /// A quadratic B-spline's weights for the probes before, at and after the nearest one,
    /// `t` (-0.5 to 0.5) from it.
    pub fn spline(t: f32) -> [f32; 3] {
        [0.5 * (0.5 - t) * (0.5 - t), 0.75 - t * t, 0.5 * (0.5 + t) * (0.5 + t)]
    }
}

/// A grid of ambient light probes: `dims` probes along x, y and z, `spacing` apart, the first
/// at `origin`. Each holds the light reaching its point from every way (see [`Ambient`]); a
/// point between them takes a blend of the nearest (see [`AmbientProbes::sample`]). Points
/// outside the grid take the nearest point on its edge.
#[derive(Clone, Debug, PartialEq)]
pub struct AmbientProbes {
    pub origin: Vec3,
    pub spacing: f32,
    pub dims: [usize; 3],
    /// x fastest, then y, then z.
    pub probes: Vec<Ambient>,
    /// The ambient light when they were made: the probes' light goes up and down with it.
    pub ambient: Vec3,
}

impl AmbientProbes {
    /// A grid covering the box `min`..`max` with probes `spacing` apart (one more than fits
    /// along each axis, so the box's far side is covered), all dark.
    pub fn covering(min: Vec3, max: Vec3, spacing: f32) -> AmbientProbes {
        let dims = ((max - min) / spacing).ceil().max(Vec3::ZERO).to_array().map(|n| n as usize + 1);
        AmbientProbes {
            origin: min,
            spacing,
            dims,
            probes: vec![Ambient::default(); dims[0] * dims[1] * dims[2]],
            ambient: Vec3::ONE,
        }
    }

    pub fn index(&self, [i, j, k]: [usize; 3]) -> usize {
        i + self.dims[0] * (j + self.dims[1] * k)
    }

    /// Where probe `[i, j, k]` is.
    pub fn position(&self, [i, j, k]: [usize; 3]) -> Vec3 {
        self.origin + Vec3::new(i as f32, j as f32, k as f32) * self.spacing
    }

    /// Every probe's grid coordinates, in the order of `probes`.
    pub fn cells(&self) -> impl Iterator<Item = [usize; 3]> + '_ {
        let [nx, ny, nz] = self.dims;
        (0..nz).flat_map(move |k| (0..ny).flat_map(move |j| (0..nx).map(move |i| [i, j, k])))
    }

    /// The ambient light at `p`, blended from the probes around it as `blend` says.
    pub fn sample(&self, p: Vec3, blend: ProbeBlend) -> Ambient {
        if blend == ProbeBlend::Smooth {
            return self.sample_smooth(p);
        }
        // The cell `p` is in (its lowest corner), and where in it, 0 to 1 along each axis.
        let mut cell = [0usize; 3];
        let mut f = [0.0f32; 3];
        let local = ((p - self.origin) / self.spacing).to_array();
        for a in 0..3 {
            let last = self.dims[a].saturating_sub(1);
            if last == 0 {
                continue;
            }
            let x = local[a].clamp(0.0, last as f32);
            let c = (x.floor() as usize).min(last - 1);
            cell[a] = c;
            f[a] = x - c as f32;
        }
        let at = |d: [usize; 3]| {
            let corner = std::array::from_fn(|a| (cell[a] + d[a]).min(self.dims[a] - 1));
            &self.probes[self.index(corner)]
        };
        let mut faces = [Vec3::ZERO; 6];
        let mut add = |d: [usize; 3], w: f32| {
            if w > 0.0 {
                for (sum, c) in faces.iter_mut().zip(at(d).faces) {
                    *sum += c * w;
                }
            }
        };
        for corner in 0..8 {
            let d = [corner & 1, corner >> 1 & 1, corner >> 2 & 1];
            let w = (0..3).map(|a| if d[a] == 1 { f[a] } else { 1.0 - f[a] }).product();
            add(d, w);
        }
        Ambient { faces }
    }

    /// [`ProbeBlend::Smooth`]'s blend at `p`.
    fn sample_smooth(&self, p: Vec3) -> Ambient {
        let local = ((p - self.origin) / self.spacing).to_array();
        let mut center = [0i64; 3];
        let mut weights = [[0.0f32; 3]; 3];
        for a in 0..3 {
            let last = self.dims[a].saturating_sub(1) as f32;
            let x = local[a].clamp(0.0, last);
            let c = x.round();
            center[a] = c as i64;
            weights[a] = ProbeBlend::spline(x - c);
        }
        let mut faces = [Vec3::ZERO; 6];
        for corner in 0..27 {
            let d = [corner % 3, corner / 3 % 3, corner / 9];
            let w = weights[0][d[0]] * weights[1][d[1]] * weights[2][d[2]];
            let at = std::array::from_fn(|a| (center[a] + d[a] as i64 - 1).clamp(0, self.dims[a] as i64 - 1) as usize);
            for (sum, c) in faces.iter_mut().zip(self.probes[self.index(at)].faces) {
                *sum += c * w;
            }
        }
        Ambient { faces }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid whose probes' colors are a linear function of where they are.
    fn linear() -> (AmbientProbes, impl Fn(Vec3) -> Vec3) {
        let field = |p: Vec3| Vec3::new(0.5 + 0.1 * p.x, 0.2 + 0.05 * p.y - 0.02 * p.z, 1.0 + 0.03 * p.z);
        let mut g = AmbientProbes::covering(Vec3::new(-2.0, 0.0, -3.0), Vec3::new(4.0, 3.0, 3.0), 1.5);
        for cell in g.cells().collect::<Vec<_>>() {
            let i = g.index(cell);
            g.probes[i] = Ambient::uniform(field(g.position(cell)));
        }
        (g, field)
    }

    #[test]
    fn a_trilinear_blend_is_exact_on_a_linear_field_and_at_probes() {
        let (g, field) = linear();
        assert_eq!(g.dims, [5, 3, 5]);
        let blend = ProbeBlend::Trilinear;
        for p in [Vec3::new(0.3, 1.1, -0.4), Vec3::new(3.9, 0.1, 2.6), Vec3::new(-1.0, 2.9, 0.0)] {
            let got = g.sample(p, blend).faces[0];
            assert!((got - field(p)).length() < 1e-5, "at {p}: {got} against {}", field(p));
        }
        // At a probe, that probe.
        let cell = [2, 1, 3];
        assert_eq!(g.sample(g.position(cell), blend), g.probes[g.index(cell)]);
    }

    #[test]
    fn a_smooth_blend_is_exact_on_a_linear_field_and_smooth_across_cells() {
        let (g, field) = linear();
        // Inside the grid (a probe in from its edges, where the spline's neighbors are all
        // there), exact on a linear field: the spline keeps straight lines straight.
        for p in [Vec3::new(0.3, 1.1, -0.4), Vec3::new(1.2, 1.4, 1.1)] {
            let got = g.sample(p, ProbeBlend::Smooth).faces[0];
            assert!((got - field(p)).length() < 1e-5, "{p}: {got} against {}", field(p));
        }
        // A probe sticking out: the slope along x, just either side of a cell's face (x =
        // -0.5 + 1.5 k, the midpoints between probes are where the nearest one changes),
        // is the same; the trilinear blend's jumps.
        let mut g = g;
        let spike = g.index([2, 1, 2]);
        g.probes[spike] = Ambient::uniform(Vec3::splat(5.0));
        let at = |x: f32, blend| g.sample(Vec3::new(x, 1.5, 0.0), blend).faces[0].x;
        let slope = |x: f32, blend| (at(x + 1e-3, blend) - at(x - 1e-3, blend)) / 2e-3;
        let face = g.position([2, 1, 2]).x + 0.75;
        let (before, after) = (slope(face - 0.01, ProbeBlend::Smooth), slope(face + 0.01, ProbeBlend::Smooth));
        assert!((before - after).abs() < 0.1 * before.abs().max(1.0), "{before} {after}");
        let probe = g.position([2, 1, 2]).x;
        let (before, after) = (slope(probe - 0.01, ProbeBlend::Trilinear), slope(probe + 0.01, ProbeBlend::Trilinear));
        assert!((before - after).abs() > 1.0, "{before} {after}");
    }

    #[test]
    fn outside_the_grid_takes_its_nearest_edge() {
        let (g, _) = linear();
        let inside = g.sample(Vec3::new(1.0, 1.5, 0.5), ProbeBlend::Trilinear);
        assert_eq!(g.sample(Vec3::new(1.0, 50.0, 0.5), ProbeBlend::Trilinear), g.sample(Vec3::new(1.0, 3.0, 0.5), ProbeBlend::Trilinear));
        assert_ne!(inside, g.sample(Vec3::new(1.0, 3.0, 0.5), ProbeBlend::Trilinear));
        // A grid one probe thick along an axis.
        let flat = AmbientProbes::covering(Vec3::ZERO, Vec3::new(2.0, 0.0, 2.0), 1.0);
        assert_eq!(flat.dims, [3, 1, 3]);
        assert_eq!(flat.sample(Vec3::new(0.5, 7.0, 0.5), ProbeBlend::Trilinear), Ambient::default());
    }
}
