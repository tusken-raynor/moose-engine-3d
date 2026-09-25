//! Asset loading for the Moose engine: in-memory mesh and level types, the
//! [`Assets`] store that owns them, and loaders for `.obj` models and `.mmp` levels.
//!
//! Meshes keep their positions as authored, never merged (each transformed once per frame), and
//! polygons that own their vertices. Each polygon vertex is a position index plus
//! its own values for every named attribute. Polygons are planar, convex n-gons
//! and are never triangulated.

mod error;
mod geom;
mod half;
mod level;
mod mesh;
mod mmp;
mod obj;
mod store;
mod text;

pub use error::LoadError;
pub use geom::{Aabb, Plane};
pub use level::{EntityKind, EntitySpawn, Level, Portal, PortalFlags, Sector};
pub use mesh::{Attrib, AttribData, Mesh, PolyFlags, Polygon, StorageFormat};
pub use obj::parse_obj;
pub use store::{Assets, MeshId};

pub use glam;
