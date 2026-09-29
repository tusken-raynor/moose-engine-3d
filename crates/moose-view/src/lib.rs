//! Per-frame view processing for the Moose engine: portal traversal out from the
//! camera's sector, view transform and projection, culling, and clipping. The output is
//! screen-space convex polygons with `w = 1 / depth`, each vertex weighted over its source
//! polygon's vertices, ready for the span buffer module.

mod carve;
mod clip;
mod frame;

pub use carve::MAX_SHADOW_SLOTS;
pub use frame::{
    EdgeLine, MAX_REFLECTIONS, Mirror, Object, PolygonKind, PolygonSource, ScreenVertex,
    SectorVisit, ViewConfig, ViewGeometry, ViewPolygon, ViewStats, pixel_edge,
};
