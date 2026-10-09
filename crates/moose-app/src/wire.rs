//! Wireframes for the editor: every edge of the level and its entities, drawn through the
//! 3D view (over the rendered frame) or through a 2D orthographic view (top, bottom,
//! front, back, left, right, or turned any way between) on a grid, as classic level
//! editors show them.

use glam::{EulerRot, Quat, Vec2, Vec3};
use moose_assets::Assets;
use moose_scene::{View, World};

use crate::ui::Canvas;

/// Which way a 2D view looks: turned `yaw` degrees about the world's up (left, from
/// looking along -Z) and `pitch` degrees up (-90 looks straight down), as the camera turns.
/// The six views along the world's axes have names (keys 1 to 6); the arrows turn a view
/// from there, for looking at things from an angle (isometric views).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub yaw: f32,
    pub pitch: f32,
}

impl Axis {
    /// Down, north (-Z) up the screen.
    pub const TOP: Axis = Axis { yaw: 0.0, pitch: -90.0 };
    /// Up, south (+Z) up the screen.
    pub const BOTTOM: Axis = Axis { yaw: 0.0, pitch: 90.0 };
    /// Along -Z.
    pub const FRONT: Axis = Axis { yaw: 0.0, pitch: 0.0 };
    /// Along +Z.
    pub const BACK: Axis = Axis { yaw: 180.0, pitch: 0.0 };
    /// Along +X.
    pub const LEFT: Axis = Axis { yaw: -90.0, pitch: 0.0 };
    /// Along -X.
    pub const RIGHT: Axis = Axis { yaw: 90.0, pitch: 0.0 };
    /// The named views, in key order (1 to 6).
    pub const NAMED: [(Axis, &'static str); 6] = [
        (Axis::TOP, "top"),
        (Axis::BOTTOM, "bottom"),
        (Axis::FRONT, "front"),
        (Axis::BACK, "back"),
        (Axis::LEFT, "left"),
        (Axis::RIGHT, "right"),
    ];

    fn rotation(self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw.to_radians(), self.pitch.to_radians(), 0.0)
    }

    /// The world directions of the screen's right and up.
    pub fn basis(self) -> (Vec3, Vec3) {
        let q = self.rotation();
        (q * Vec3::X, q * Vec3::Y)
    }

    /// The way it looks (into the screen).
    pub fn forward(self) -> Vec3 {
        self.rotation() * Vec3::NEG_Z
    }

    /// Turned by `yaw` and `pitch` degrees (pitch kept within straight down and up).
    pub fn turned(self, yaw: f32, pitch: f32) -> Axis {
        Axis { yaw: (self.yaw + yaw).rem_euclid(360.0), pitch: (self.pitch + pitch).clamp(-90.0, 90.0) }
    }

    /// Its name: a named view's, or its angles.
    pub fn name(self) -> String {
        let same = |a: Axis| {
            let d = (a.yaw - self.yaw).rem_euclid(360.0);
            (!(0.01..=359.99).contains(&d) || a.pitch.abs() == 90.0) && (a.pitch - self.pitch).abs() < 0.01
        };
        match Axis::NAMED.iter().find(|(a, _)| same(*a)) {
            Some((_, name)) => (*name).to_string(),
            None => format!("2D {}° {}°", self.yaw.round(), self.pitch.round()),
        }
    }

    /// The named view called `name`.
    pub fn named(name: &str) -> Option<Axis> {
        Axis::NAMED.iter().find(|(_, n)| *n == name).map(|&(a, _)| a)
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
    // On the world plane that faces the view most squarely, through the view's center:
    // the view's own plane when it looks along an axis, the floor seen from an angle.
    let forward = ortho.axis.forward();
    let a = forward.abs();
    let (normal, u, v) = if a.y >= a.x && a.y >= a.z {
        (Vec3::Y, Vec3::X, Vec3::Z)
    } else if a.x >= a.z {
        (Vec3::X, Vec3::Z, Vec3::Y)
    } else {
        (Vec3::Z, Vec3::X, Vec3::Y)
    };
    let facing = forward.dot(normal);
    if facing.abs() < 0.2 {
        // Edge-on: no grid to see.
        return;
    }
    let level = ortho.center.dot(normal);
    // The screen's corners on the plane, along the view.
    let on_plane = |x: f32, y: f32| {
        let p = ortho.to_world(x, y);
        p + forward * ((level - p.dot(normal)) / facing)
    };
    let corners = [on_plane(0.0, 0.0), on_plane(ortho.width, 0.0), on_plane(0.0, ortho.height), on_plane(ortho.width, ortho.height)];
    let span = |axis: Vec3| {
        corners.iter().map(|c| c.dot(axis)).fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), t| (lo.min(t), hi.max(t)))
    };
    let (su, sv) = (span(u), span(v));
    // As far apart on screen as the plane is foreshortened.
    let mut step = step;
    while step * ortho.scale * facing.abs() < 6.0 {
        step *= 2.0;
    }
    for (along, (a, b), across) in [(u, su, sv), (v, sv, su)] {
        let mut k = (a / step).floor();
        while k * step <= b {
            let t = k * step;
            let color = if t.abs() < 1e-4 {
                0x40_48_60
            } else if (t - t.round()).abs() < 1e-4 {
                0x28_2C_34
            } else {
                0x1A_1D_22
            };
            let other = if along == u { v } else { u };
            let base = normal * level + along * t;
            let (from, to) = (ortho.to_screen(base + other * across.0), ortho.to_screen(base + other * across.1));
            canvas.line(from.x, from.y, to.x, to.y, color);
            k += 1.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_d_views_map_the_world_both_ways() {
        for axis in Axis::NAMED.map(|(a, _)| a).into_iter().chain([Axis::FRONT.turned(30.0, -35.0)]) {
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
            // Into the screen is square to both.
            let (right, up) = axis.basis();
            assert!(axis.forward().dot(right).abs() < 1e-5 && axis.forward().dot(up).abs() < 1e-5);
        }
    }

    #[test]
    fn the_named_views_look_along_the_axes_and_turn_from_there() {
        assert!((Axis::TOP.forward() - Vec3::NEG_Y).length() < 1e-5);
        assert!((Axis::TOP.basis().1 - Vec3::NEG_Z).length() < 1e-5);
        assert!((Axis::RIGHT.forward() - Vec3::NEG_X).length() < 1e-5);
        assert!((Axis::LEFT.forward() - Vec3::X).length() < 1e-5);
        assert!((Axis::BACK.forward() - Vec3::Z).length() < 1e-5);
        assert!((Axis::BOTTOM.forward() - Vec3::Y).length() < 1e-5);
        assert_eq!(Axis::FRONT.name(), "front");
        assert_eq!(Axis::FRONT.turned(6.0, 0.0).name(), "2D 6° 0°");
        assert_eq!(Axis::TOP.turned(0.0, -30.0), Axis::TOP);
        assert_eq!(Axis::named("left"), Some(Axis::LEFT));
    }
}
