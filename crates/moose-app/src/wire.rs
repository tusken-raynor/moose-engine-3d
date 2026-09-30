//! Wireframes for the editor: every edge of the level and its entities, drawn through the
//! 3D view (over the rendered frame) or through a 2D orthographic view (top, front or
//! side) on a grid, as classic level editors show them.

use glam::{Vec2, Vec3};
use moose_assets::Assets;
use moose_scene::{View, World};

use crate::ui::Canvas;

/// Which way a 2D view looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Down, north (-Z) up the screen.
    Top,
    /// Along -Z.
    Front,
    /// Along -X.
    Side,
}

impl Axis {
    /// The world directions of the screen's right and up.
    pub fn basis(self) -> (Vec3, Vec3) {
        match self {
            Axis::Top => (Vec3::X, Vec3::NEG_Z),
            Axis::Front => (Vec3::X, Vec3::Y),
            Axis::Side => (Vec3::NEG_Z, Vec3::Y),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Axis::Top => "top",
            Axis::Front => "front",
            Axis::Side => "side",
        }
    }
}

/// A 2D view: `center` (world) at the middle of a `width` x `height` screen, `scale`
/// pixels per meter.
#[derive(Clone, Copy, Debug)]
pub struct Ortho {
    pub axis: Axis,
    pub center: Vec3,
    pub scale: f32,
    pub width: f32,
    pub height: f32,
}

impl Ortho {
    pub fn to_screen(self, p: Vec3) -> Vec2 {
        let (right, up) = self.axis.basis();
        let d = p - self.center;
        Vec2::new(
            self.width / 2.0 + d.dot(right) * self.scale,
            self.height / 2.0 - d.dot(up) * self.scale,
        )
    }

    /// The world point under screen point `(x, y)`, on the plane through `center` facing
    /// the view.
    pub fn to_world(self, x: f32, y: f32) -> Vec3 {
        let (right, up) = self.axis.basis();
        self.center
            + right * ((x - self.width / 2.0) / self.scale)
            + up * ((self.height / 2.0 - y) / self.scale)
    }
}

/// How lines reach the screen.
pub enum Projection<'a> {
    Perspective(&'a View),
    Ortho(&'a Ortho),
}

impl Projection<'_> {
    /// Segment `a`-`b` on screen, if any of it is in front of the eye (the 3D view clips
    /// it at the near plane).
    pub fn segment(&self, a: Vec3, b: Vec3) -> Option<(Vec2, Vec2)> {
        match self {
            Projection::Ortho(o) => Some((o.to_screen(a), o.to_screen(b))),
            Projection::Perspective(view) => {
                let camera = |p: Vec3| view.rotation.conjugate() * (p - view.position);
                let (mut a, mut b) = (camera(a), camera(b));
                // Depth is -z.
                let near = view.near.max(1e-3);
                let (da, db) = (-a.z, -b.z);
                if da < near && db < near {
                    return None;
                }
                if da < near {
                    a = a + (b - a) * ((near - da) / (db - da));
                } else if db < near {
                    b = b + (a - b) * ((near - db) / (da - db));
                }
                let screen = |v: Vec3| {
                    let z = -v.z;
                    Vec2::new(
                        view.center.x + view.focal * v.x / z,
                        view.center.y - view.focal * v.y / z,
                    )
                };
                Some((screen(a), screen(b)))
            }
        }
    }

    /// Point `p` on screen, if it is in front of the eye.
    pub fn point(&self, p: Vec3) -> Option<Vec2> {
        match self {
            Projection::Ortho(o) => Some(o.to_screen(p)),
            Projection::Perspective(view) => view.project(p).map(|(s, _)| s),
        }
    }

    pub fn line(&self, canvas: &mut Canvas, a: Vec3, b: Vec3, color: u32) {
        if let Some((a, b)) = self.segment(a, b) {
            canvas.line(a.x, a.y, b.x, b.y, color);
        }
    }
}

/// Wire colors.
pub const SURFACE: u32 = 0x9A_9A_9A;
pub const PORTAL: u32 = 0xC0_60_C0;
pub const ENTITY: u32 = 0x60_C0_80;

/// Draws every edge of the level's surfaces (portals in their own color) and of its
/// entities' models.
pub fn draw_level(canvas: &mut Canvas, projection: &Projection, world: &World, assets: &Assets) {
    let geometry = assets.mesh(world.geometry);
    for polygon in &geometry.polygons {
        let points: Vec<Vec3> = geometry.polygon_points(polygon).collect();
        outline(canvas, projection, &points, SURFACE);
    }
    for portal in &world.portals {
        let points: Vec<Vec3> = portal.positions.iter().map(|&i| geometry.positions[i as usize]).collect();
        outline(canvas, projection, &points, PORTAL);
    }
    for entity in &world.entities {
        let mesh = assets.mesh(entity.mesh);
        let transform = entity.transform();
        for polygon in &mesh.polygons {
            let points: Vec<Vec3> = mesh
                .polygon_points(polygon)
                .map(|p| transform.transform_point3(p))
                .collect();
            outline(canvas, projection, &points, ENTITY);
        }
    }
}

/// Draws a closed polygon's edges.
pub fn outline(canvas: &mut Canvas, projection: &Projection, points: &[Vec3], color: u32) {
    for i in 0..points.len() {
        projection.line(canvas, points[i], points[(i + 1) % points.len()], color);
    }
}

/// Fills the screen and draws a 2D view's grid: lines every `step` meters where they are
/// far enough apart to see, brighter every meter, and brightest along the world axes.
pub fn draw_grid(canvas: &mut Canvas, ortho: &Ortho, step: f32) {
    canvas.fill(0, 0, canvas.width, canvas.height, 0x0010_1216);
    let (right, up) = ortho.axis.basis();
    let lo = ortho.to_world(0.0, ortho.height);
    let hi = ortho.to_world(ortho.width, 0.0);
    let span = |axis: Vec3| {
        let (a, b) = (lo.dot(axis), hi.dot(axis));
        (a.min(b), a.max(b))
    };
    for (axis, vertical) in [(right, true), (up, false)] {
        let (a, b) = span(axis);
        let mut step = step;
        while step * ortho.scale < 6.0 {
            step *= 2.0;
        }
        let mut k = (a / step).floor();
        while k * step <= b {
            let v = k * step;
            let color = if v.abs() < 1e-4 {
                0x40_48_60
            } else if (v - v.round()).abs() < 1e-4 {
                0x28_2C_34
            } else {
                0x1A_1D_22
            };
            let p = ortho.center + axis * (v - ortho.center.dot(axis));
            let s = ortho.to_screen(p);
            if vertical {
                canvas.line(s.x, 0.0, s.x, ortho.height, color);
            } else {
                canvas.line(0.0, s.y, ortho.width, s.y, color);
            }
            k += 1.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_d_views_map_the_world_both_ways() {
        for axis in [Axis::Top, Axis::Front, Axis::Side] {
            let o = Ortho {
                axis,
                center: Vec3::new(1.0, 2.0, 3.0),
                scale: 50.0,
                width: 640.0,
                height: 360.0,
            };
            assert_eq!(o.to_screen(o.center), Vec2::new(320.0, 180.0));
            let p = o.to_world(400.0, 100.0);
            assert!((o.to_screen(p) - Vec2::new(400.0, 100.0)).length() < 1e-3);
            // Up the screen is up the view.
            let (_, up) = axis.basis();
            assert!(o.to_screen(o.center + up).y < 180.0);
        }
    }
}
