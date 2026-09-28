//! Shadow maps: what a spot light sees, as depth, rendered from its position before the
//! frame and looked up wherever the light is evaluated (at sample points, by the standard
//! lighting). A light with one names it by index (`Light::shadow`) in
//! [`Renderer::shadow_maps`](crate::Renderer::shadow_maps).

use glam::Vec3;
use moose_assets::{Assets, Light};
use moose_scene::World;

use crate::shader::{F32s, Fill, LANES};

/// How far a point is moved off its surface (along the surface's normal) before it is looked
/// up, in texels at its depth, so a surface doesn't shadow itself where the map's texels are
/// coarser than its slope.
const NORMAL_OFFSET: f32 = 1.5;
/// How much nearer than a point the map's depth must be to shadow it, in texels at its depth.
const DEPTH_BIAS: f32 = 1.0;
/// Nearest depth rendered, in meters.
const NEAR: f32 = 0.05;
/// The widest half-angle a map covers, in degrees (wider cones are cut off there).
const MAX_HALF_ANGLE: f32 = 80.0;

/// Depth seen from a spot light, over a square covering its cone: per texel, 1 / the
/// distance along the light's direction to the nearest surface (0 where there is none).
/// Affine across a polygon's image, so drawing it needs no division, and nearer is larger.
pub struct ShadowMap {
    size: u32,
    /// The light it was rendered for (see [`ShadowMap::render`]).
    light: Option<Light>,
    origin: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
    /// Texels per unit of `x / z` from the middle.
    scale: f32,
    depth: Vec<f32>,
}

impl ShadowMap {
    /// An empty map, `size` texels square.
    pub fn new(size: u32) -> ShadowMap {
        let size = size.max(2);
        ShadowMap {
            size,
            light: None,
            origin: Vec3::ZERO,
            right: Vec3::X,
            up: Vec3::Y,
            forward: Vec3::NEG_Z,
            scale: 1.0,
            depth: vec![0.0; (size * size) as usize],
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// Renders what the world's light `index` (a spot light) sees: the level's polygons in
    /// the sectors it reaches, and the entities touching them. Nothing is drawn again if the
    /// light is the one it was last rendered for; `force` renders anyway (for when entities
    /// moved).
    pub fn render(&mut self, world: &World, assets: &Assets, index: u32, force: bool) {
        let light = world.lights()[index as usize];
        if !force && self.light.is_some_and(|l| same_view(&l, &light)) {
            return;
        }
        self.aim(&light);

        let reaches: Vec<bool> = (0..world.sectors.len() as u32)
            .map(|s| world.sector_lights(s).contains(&index))
            .collect();
        let mut points = Vec::new();
        let level = assets.mesh(world.geometry);
        for (s, sector) in world.sectors.iter().enumerate() {
            if !reaches[s] {
                continue;
            }
            for p in sector.polygons.clone() {
                let polygon = &level.polygons[p as usize];
                points.clear();
                points.extend(level.polygon_points(polygon));
                self.draw_polygon(&points);
            }
        }
        for entity in &world.entities {
            if !entity.sectors.iter().any(|&s| reaches[s as usize]) {
                continue;
            }
            let mesh = assets.mesh(entity.mesh);
            let transform = entity.transform();
            for polygon in &mesh.polygons {
                points.clear();
                points.extend(
                    mesh.polygon_points(polygon)
                        .map(|p| transform.transform_point3(p)),
                );
                self.draw_polygon(&points);
            }
        }
    }

    /// Points the map along `light` and clears it.
    fn aim(&mut self, light: &Light) {
        self.light = Some(*light);
        self.origin = light.position;
        self.forward = light.direction;
        let helper = if light.direction.y.abs() < 0.99 { Vec3::Y } else { Vec3::Z };
        self.right = light.direction.cross(helper).normalize();
        self.up = self.right.cross(light.direction);
        let half = light
            .cos_outer
            .clamp(-1.0, 1.0)
            .acos()
            .min(MAX_HALF_ANGLE.to_radians());
        // The cone's edge a texel inside the square's.
        let size = self.size as f32;
        self.scale = (size / 2.0 - 1.0) / half.tan().max(1e-3);
        self.depth.fill(0.0);
    }

    /// Draws a world-space polygon's depth (counter-clockwise from its front), clipped to the
    /// near plane. Polygons facing away from the light are left out: sectors and models are
    /// closed, so whatever is behind one is behind a polygon facing the light too.
    fn draw_polygon(&mut self, points: &[Vec3]) {
        const MAX: usize = 64;
        if points.len() < 3 {
            return;
        }
        let normal = (1..points.len() - 1)
            .map(|i| (points[i] - points[0]).cross(points[i + 1] - points[0]))
            .sum::<Vec3>();
        if normal.dot(self.origin - points[0]) <= 0.0 {
            return;
        }
        let mut view = [Vec3::ZERO; MAX];
        let n = points.len().min(MAX - 1);
        for (v, &p) in view.iter_mut().zip(&points[..n]) {
            let d = p - self.origin;
            *v = Vec3::new(d.dot(self.right), d.dot(self.up), d.dot(self.forward));
        }
        // Clip to z >= NEAR.
        let mut clipped = [Vec3::ZERO; MAX];
        let mut m = 0;
        for i in 0..n {
            let (a, b) = (view[i], view[(i + 1) % n]);
            if a.z >= NEAR {
                clipped[m] = a;
                m += 1;
            }
            if (a.z >= NEAR) != (b.z >= NEAR) && m < MAX {
                let t = (NEAR - a.z) / (b.z - a.z);
                clipped[m] = a + (b - a) * t;
                m += 1;
            }
        }
        if m < 3 {
            return;
        }
        // To the map: texel coordinates (x right, y down), and 1 / depth, which is affine
        // there.
        let half = self.size as f32 / 2.0;
        let screen: [Vec3; MAX] = std::array::from_fn(|i| {
            let v = clipped[i];
            if i >= m {
                return Vec3::ZERO;
            }
            Vec3::new(half + v.x / v.z * self.scale, half - v.y / v.z * self.scale, 1.0 / v.z)
        });
        for i in 1..m - 1 {
            self.draw_triangle(screen[0], screen[i], screen[i + 1]);
        }
    }

    /// Draws a triangle given in texel coordinates and 1 / depth, keeping the nearest (the
    /// largest 1 / depth) at each texel whose center it covers.
    fn draw_triangle(&mut self, a: Vec3, b: Vec3, c: Vec3) {
        let area = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y);
        if area.abs() < 1e-6 {
            return;
        }
        // 1 / depth as a plane: a.z + gx (x - a.x) + gy (y - a.y).
        let gx = ((b.z - a.z) * (c.y - a.y) - (c.z - a.z) * (b.y - a.y)) / area;
        let gy = ((c.z - a.z) * (b.x - a.x) - (b.z - a.z) * (c.x - a.x)) / area;
        // Edges as e(x, y) = ex * x + ey * y + e0, positive inside.
        let sign = area.signum();
        let edge = |p: Vec3, q: Vec3| {
            let (ex, ey) = (-(q.y - p.y) * sign, (q.x - p.x) * sign);
            (ex, ey, -(ex * p.x + ey * p.y))
        };
        let edges = [edge(a, b), edge(b, c), edge(c, a)];
        let size = self.size as i32;
        let y0 = (a.y.min(b.y).min(c.y) - 0.5).ceil().max(0.0) as i32;
        let y1 = ((a.y.max(b.y).max(c.y) - 0.5).floor() as i32).min(size - 1);
        for row in y0..=y1 {
            let y = row as f32 + 0.5;
            let (mut xl, mut xr) = (0.0f32, size as f32);
            for &(ex, ey, e0) in &edges {
                let rest = ey * y + e0;
                if ex > 0.0 {
                    xl = xl.max(-rest / ex);
                } else if ex < 0.0 {
                    xr = xr.min(-rest / ex);
                } else if rest < 0.0 {
                    xr = -1.0;
                }
            }
            let x0 = (xl - 0.5).ceil().max(0.0) as i32;
            let x1 = ((xr - 0.5).floor() as i32).min(size - 1);
            if x0 > x1 {
                continue;
            }
            let line = &mut self.depth[(row * size) as usize..][x0 as usize..=x1 as usize];
            let start = a.z + gy * (y - a.y) + gx * (x0 as f32 + 0.5 - a.x);
            for (x, depth) in line.iter_mut().enumerate() {
                *depth = depth.max(start + gx * x as f32);
            }
        }
    }

    /// How much of the light reaches [`LANES`] points on surfaces facing `normal`: 1 where
    /// nothing is nearer the light, 0 in shadow, and in between near a shadow's edge (the
    /// four nearest texels' tests, blended bilinearly). Points outside the map, or behind
    /// the light, are lit (the cone leaves them dark anyway).
    #[inline]
    pub(crate) fn visibility(&self, position: &[F32s; 3], normal: &[F32s; 3]) -> F32s {
        let fill = F32s::fill;
        let d = [
            position[0] - fill(self.origin.x),
            position[1] - fill(self.origin.y),
            position[2] - fill(self.origin.z),
        ];
        let dot = |d: &[F32s; 3], v: Vec3| d[0] * fill(v.x) + d[1] * fill(v.y) + d[2] * fill(v.z);
        // A texel's width at the point's depth.
        let texel = dot(&d, self.forward).max(fill(NEAR)) * fill(1.0 / self.scale);
        let offset = texel * fill(NORMAL_OFFSET);
        let d = [
            d[0] + normal[0] * offset,
            d[1] + normal[1] * offset,
            d[2] + normal[2] * offset,
        ];
        let z = dot(&d, self.forward);
        let inv = fill(1.0) / z.max(fill(NEAR));
        let half = self.size as f32 / 2.0;
        // Texel coordinates, relative to texel centers.
        let u = (dot(&d, self.right) * inv * fill(self.scale) + fill(half - 0.5)).to_array();
        let v = (fill(half - 0.5) - dot(&d, self.up) * inv * fill(self.scale)).to_array();
        // Shadowed where the map is nearer than the point, less the bias: in 1 / depth.
        let test = (fill(1.0) / (z - texel * fill(DEPTH_BIAS)).max(fill(NEAR))).to_array();
        let z = z.to_array();
        let size = self.size as i32;
        let lit = |x: i32, y: i32, inv: f32| {
            if x < 0 || y < 0 || x >= size || y >= size {
                return 1.0;
            }
            if inv >= self.depth[(y * size + x) as usize] { 1.0 } else { 0.0 }
        };
        F32s::from(std::array::from_fn::<f32, LANES, _>(|k| {
            if z[k] <= NEAR || !(u[k].is_finite() && v[k].is_finite()) {
                return 1.0;
            }
            let (fu, fv) = (u[k].floor(), v[k].floor());
            let (x, y) = (fu as i32, fv as i32);
            let (s, t) = (u[k] - fu, v[k] - fv);
            let top = lit(x, y, test[k]) * (1.0 - s) + lit(x + 1, y, test[k]) * s;
            let bottom = lit(x, y + 1, test[k]) * (1.0 - s) + lit(x + 1, y + 1, test[k]) * s;
            top * (1.0 - t) + bottom * t
        }))
    }
}

/// Whether two lights see the same view (position, direction and cone).
fn same_view(a: &Light, b: &Light) -> bool {
    a.position == b.position && a.direction == b.direction && a.cos_outer == b.cos_outer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_occluder_shadows_what_is_behind_it_but_not_itself() {
        let light = Light::spot(0, Vec3::ZERO, Vec3::ONE, 20.0, Vec3::NEG_Z, 20.0, 30.0);
        let mut map = ShadowMap::new(256);
        map.aim(&light);
        // A 1 m square 2 m ahead, facing the light.
        map.draw_polygon(&[
            Vec3::new(-0.5, -0.5, -2.0),
            Vec3::new(0.5, -0.5, -2.0),
            Vec3::new(0.5, 0.5, -2.0),
            Vec3::new(-0.5, 0.5, -2.0),
        ]);
        let at = |x: f32, y: f32, z: f32| {
            let p = [F32s::fill(x), F32s::fill(y), F32s::fill(z)];
            let n = [F32s::fill(0.0), F32s::fill(0.0), F32s::fill(1.0)];
            map.visibility(&p, &n).to_array()[0]
        };
        // Its shadow on a wall 4 m ahead spans 2 m across.
        assert_eq!(at(0.0, 0.0, -4.0), 0.0);
        assert_eq!(at(0.9, -0.5, -4.0), 0.0);
        assert_eq!(at(1.2, 0.0, -4.0), 1.0);
        assert_eq!(at(0.0, -1.3, -4.0), 1.0);
        // The square itself is lit.
        assert_eq!(at(0.0, 0.0, -2.0), 1.0);
        assert_eq!(at(0.4, 0.3, -2.0), 1.0);
        // Seen from behind, it is left out: from the other side it would shadow nothing.
        let mut back = ShadowMap::new(64);
        back.aim(&light);
        back.draw_polygon(&[
            Vec3::new(-0.5, -0.5, -2.0),
            Vec3::new(-0.5, 0.5, -2.0),
            Vec3::new(0.5, 0.5, -2.0),
            Vec3::new(0.5, -0.5, -2.0),
        ]);
        assert!(back.depth.iter().all(|&d| d == 0.0));
    }
}
