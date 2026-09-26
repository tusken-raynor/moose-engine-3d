//! The span buffer rasterizer for the Moose engine (see the span buffer module spec).
//!
//! Each frame runs in two phases. Phase 1 sets up every polygon of a [`ViewGeometry`]
//! (moose_view) and bins a reference to it into each row band it touches, split across
//! threads by polygon. Phase 2 hands bands of rows to threads; each row resolves world
//! spans (never overlapping), props by sorted insertion with an exact two-point test, and
//! per-pixel actors in a visibility buffer, then shades each visible pixel once, in runs.

pub mod shader;
pub mod shaders;

mod render;

pub use render::{LayoutError, RasterConfig, RasterPath, RenderStats, Renderer, Surface, Target};
pub use shader::{
    AttribDesc, F32s, Fill, Format, I16s, I32s, LANES, Pixels, Shader, ShaderId, U32s, Uniforms,
    high_byte, widen,
};
/// The portable SIMD crate the lane types come from, for shaders written elsewhere.
pub use wide;
