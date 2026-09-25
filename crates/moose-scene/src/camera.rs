use glam::{Affine3A, EulerRot, Quat, Vec2, Vec3};

use crate::world::{SpawnPoint, World};

/// A rectangle of the framebuffer, in pixels. Pixel `(x, y)` covers `[x, x+1) x [y, y+1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// One player's point of view. Each player owns one; nothing about it is global.
///
/// Orientation uses the level conventions: yaw 0 faces -Z, and rotations apply roll,
/// then pitch, then yaw (R = Ry * Rx * Rz). Positive yaw turns left, positive pitch
/// looks up, and positive roll tilts the camera's right side up (the horizon appears
/// to turn clockwise). Angles are radians and are not clamped; that is up to controls.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub position: Vec3,
    /// The sector containing `position`, where portal rendering starts. Keep it current
    /// by moving with [`move_to`](Self::move_to).
    pub sector: u32,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    /// Vertical field of view in radians. Wider viewports see more to the sides.
    pub fov_y: f32,
    /// Distance to the near clipping plane. There is no far plane.
    pub near: f32,
    pub viewport: Viewport,
}

impl Camera {
    /// A camera at a spawn point, with a 90 degree vertical field of view.
    pub fn at_spawn(spawn: &SpawnPoint, viewport: Viewport) -> Camera {
        let (yaw, pitch, roll) = spawn.rotation.to_euler(EulerRot::YXZ);
        Camera {
            position: spawn.position,
            sector: spawn.sector,
            yaw,
            pitch,
            roll,
            fov_y: 90f32.to_radians(),
            near: 0.05,
            viewport,
        }
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, self.roll)
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation() * Vec3::NEG_Z
    }

    pub fn right(&self) -> Vec3 {
        self.rotation() * Vec3::X
    }

    pub fn up(&self) -> Vec3 {
        self.rotation() * Vec3::Y
    }

    /// Moves toward `target`, following portals and updating `sector`.
    /// Returns true if a solid surface stopped the move short.
    pub fn move_to(&mut self, world: &World, target: Vec3) -> bool {
        let trace = world.trace(self.sector, self.position, target);
        self.position = trace.position;
        self.sector = trace.sector;
        trace.blocked
    }

    /// Moves by `delta` in camera space (x right, y up, -z forward).
    pub fn move_local(&mut self, world: &World, delta: Vec3) -> bool {
        let target = self.position + self.rotation() * delta;
        self.move_to(world, target)
    }

    /// An immutable snapshot of this camera for rendering one frame.
    pub fn view(&self) -> View {
        let rotation = self.rotation();
        let half_height = self.viewport.height as f32 / 2.0;
        let focal = half_height / (self.fov_y / 2.0).tan();
        View {
            sector: self.sector,
            position: self.position,
            rotation,
            world_to_view: Affine3A::from_rotation_translation(rotation, self.position).inverse(),
            focal,
            center: Vec2::new(
                self.viewport.x as f32 + self.viewport.width as f32 / 2.0,
                self.viewport.y as f32 + half_height,
            ),
            near: self.near,
            viewport: self.viewport,
        }
    }
}

/// Everything the renderer needs from a camera for one frame. It is `Copy` and never
/// changes during the frame, so every render thread can read it freely.
///
/// View space: the camera sits at the origin looking down -Z, +X right, +Y up.
/// Depth is `z = -view.z`, and the rasterizer's `w = 1 / z` (larger is closer).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub sector: u32,
    pub position: Vec3,
    pub rotation: Quat,
    pub world_to_view: Affine3A,
    /// Focal length in pixels: screen offset = `focal * (x or y) / z`. Pixels are square.
    pub focal: f32,
    /// Framebuffer position of the view axis (the viewport's center).
    pub center: Vec2,
    pub near: f32,
    pub viewport: Viewport,
}

impl View {
    /// Projects a world-space point to framebuffer coordinates (y down) and its `w`.
    /// Returns `None` for points closer than the near plane or behind the camera.
    pub fn project(&self, world: Vec3) -> Option<(Vec2, f32)> {
        // Subtract the eye first: exact for nearby points, unlike the affine form.
        let v = self.rotation.conjugate() * (world - self.position);
        let z = -v.z;
        if z < self.near {
            return None;
        }
        let w = 1.0 / z;
        Some((
            Vec2::new(
                self.center.x + self.focal * v.x * w,
                self.center.y - self.focal * v.y * w,
            ),
            w,
        ))
    }
}
