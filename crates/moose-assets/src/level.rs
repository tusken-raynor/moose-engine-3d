use std::ops::Range;

use glam::{Quat, Vec3};

use crate::geom::{Aabb, Plane};
use crate::store::MeshId;

/// A loaded `.mmp` level: world geometry split into convex sectors joined by portals.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub name: String,
    /// All solid surfaces as one mesh. Each sector's polygons are a contiguous range.
    pub geometry: MeshId,
    pub sectors: Vec<Sector>,
    /// Ordered by sector; each sector's portals are a contiguous range.
    pub portals: Vec<Portal>,
    pub spawns: Vec<EntitySpawn>,
    /// Light that reaches everything, in linear RGB (1 is a surface's full color).
    pub ambient: Vec3,
    pub lights: Vec<Light>,
}

/// A static light: it lights surfaces facing it within `range` of it, fading smoothly to
/// nothing there, and within its cone. A point light's cone is whole: it shines every way.
/// A spot light's shines along `direction`, at full strength within the inner half-angle
/// and fading smoothly to nothing at the outer one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    /// The sector containing it.
    pub sector: u32,
    pub position: Vec3,
    /// Linear RGB at full strength (up close, facing it); 1 is a surface's full color, and
    /// more overbrightens.
    pub color: Vec3,
    /// Where its light ends, in meters.
    pub range: f32,
    /// Where a spot light shines (unit length).
    pub direction: Vec3,
    /// Cosines of the cone's inner and outer half-angles; -1 for a point light (whole).
    pub cos_inner: f32,
    pub cos_outer: f32,
    /// Its shadow map, if it casts shadows: an index into the renderer's shadow maps (a
    /// runtime choice; levels don't set it).
    pub shadow: Option<u16>,
}

impl Light {
    /// A point light.
    pub fn point(sector: u32, position: Vec3, color: Vec3, range: f32) -> Light {
        Light {
            sector,
            position,
            color,
            range,
            direction: Vec3::NEG_Y,
            cos_inner: -1.0,
            cos_outer: -1.0,
            shadow: None,
        }
    }

    /// A spot light shining along `direction` (any length), full within `inner` degrees of
    /// it and gone past `outer`.
    pub fn spot(
        sector: u32,
        position: Vec3,
        color: Vec3,
        range: f32,
        direction: Vec3,
        inner: f32,
        outer: f32,
    ) -> Light {
        Light {
            sector,
            position,
            color,
            range,
            direction: direction.normalize(),
            cos_inner: inner.to_radians().cos(),
            cos_outer: outer.to_radians().cos(),
            shadow: None,
        }
    }

    /// Whether its cone is whole (a point light).
    pub fn is_point(&self) -> bool {
        self.cos_outer <= -1.0
    }

    /// Whether its cone reaches any of the sphere around `center` (conservatively: when a
    /// part of the sphere lies within the outer half-angle, widened by the sphere's own
    /// angular radius seen from the light). Always, for a point light.
    pub fn cone_reaches(&self, center: Vec3, radius: f32) -> bool {
        if self.is_point() {
            return true;
        }
        let to = center - self.position;
        let d = to.length();
        if d <= radius {
            return true;
        }
        let angle = (to.dot(self.direction) / d).clamp(-1.0, 1.0).acos();
        angle <= self.cos_outer.clamp(-1.0, 1.0).acos() + (radius / d).asin()
    }

    /// Its cone as `(scale, offset)`: `clamp(cos * scale + offset, 0, 1)` goes from 0 at
    /// the outer half-angle to 1 at the inner, where `cos` is the cosine of the angle
    /// between `direction` and the way to the lit point. `(0, 1)` for a point light: 1
    /// everywhere.
    pub fn cone(&self) -> (f32, f32) {
        if self.is_point() {
            (0.0, 1.0)
        } else {
            let scale = 1.0 / (self.cos_inner - self.cos_outer).max(1e-4);
            (scale, -self.cos_outer * scale)
        }
    }
}

/// A convex region of the level.
#[derive(Clone, Debug, PartialEq)]
pub struct Sector {
    pub name: String,
    /// Range of `Level::geometry` polygons that bound this sector.
    pub polygons: Range<u32>,
    /// Range of `Level::portals` leading out of this sector.
    pub portals: Range<u32>,
    pub bounds: Aabb,
    /// Mean of the sector's vertices; always inside a convex sector.
    pub center: Vec3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortalFlags(pub u32);

impl PortalFlags {
    pub const RENDER_THROUGH: u32 = 0x1;
    pub const PASSABLE: u32 = 0x2;
    pub const ALL: u32 = Self::RENDER_THROUGH | Self::PASSABLE;

    pub fn render_through(self) -> bool {
        self.0 & Self::RENDER_THROUGH != 0
    }

    pub fn passable(self) -> bool {
        self.0 & Self::PASSABLE != 0
    }
}

/// One side of an opening between two sectors. The polygon is convex and faces
/// into `sector`; its mirror lists the same points in reverse and faces into `target`.
#[derive(Clone, Debug, PartialEq)]
pub struct Portal {
    pub sector: u32,
    pub target: u32,
    /// Index of the portal on the other side, in `Level::portals`.
    pub mirror: u32,
    /// Indices into the geometry mesh's `positions`, shared with the surrounding walls.
    pub positions: Vec<u32>,
    pub plane: Plane,
    pub flags: PortalFlags,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Spawn,
    Prop,
    Actor,
}

/// Placement data for an entity, as read from the level. Creating live
/// entities from these is the scene's job.
#[derive(Clone, Debug, PartialEq)]
pub struct EntitySpawn {
    pub name: String,
    pub kind: EntityKind,
    pub sector: u32,
    /// The model to draw; `None` for spawn points.
    pub mesh: Option<MeshId>,
    pub position: Vec3,
    /// Pitch/yaw/roll applied roll, then pitch, then yaw (R = Ry * Rx * Rz).
    pub rotation: Quat,
    pub scale: f32,
}
