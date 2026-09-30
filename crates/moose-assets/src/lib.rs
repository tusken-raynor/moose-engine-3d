//! Asset loading for the Moose engine: in-memory mesh, level and texture types, the
//! [`Assets`] store that owns them, and loaders for `.obj` models, `.mmp` levels and `.png`
//! textures, and Half-Life style rippling water ([`Ripples`]).
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
mod mmp_doc;
mod mmp_edit;
mod mmdl;
mod obj;
mod ripples;
mod skin;
mod store;
mod text;
mod texture;

pub use error::LoadError;
pub use geom::{Aabb, Plane};
pub use level::{
    DirectionalLight, EntityKind, EntitySpawn, Level, Light, NO_SECTOR, Occluder, Oscillation,
    Portal, PortalFlags, Sector,
};
pub use mmp_doc::{
    AdjoinDoc, AttributeDoc, DirectionalDoc, EntityDoc, LevelDoc, LightDoc, SectorDoc, SurfaceDoc,
    number, occluder_value,
};
pub use mesh::{Attrib, AttribData, Mesh, PolyFlags, Polygon, StorageFormat};
pub use obj::parse_obj;
pub use ripples::{RIPPLE_SIZE, RIPPLE_STEP, Ripples};
pub use skin::{Animation, Bone, Pose, Skin};
pub use store::{Assets, MeshId, TextureId};
pub use texture::{CUBE_FACES, MipLevel, Texture, decode_png};

pub use glam;
