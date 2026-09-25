//! Runtime state for the Moose engine.
//!
//! A [`World`] holds one loaded level and everything placed in it, shared by all
//! players. Each player owns a [`Camera`]; every frame it produces an immutable
//! [`View`] that the renderer reads. Splitscreen is several cameras with different
//! viewports rendering the same world.

mod camera;
mod world;

pub use camera::{Camera, View, Viewport};
pub use world::{Entity, PORTAL_CLEARANCE, SpawnPoint, Trace, World};
