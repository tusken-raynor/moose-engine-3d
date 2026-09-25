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
