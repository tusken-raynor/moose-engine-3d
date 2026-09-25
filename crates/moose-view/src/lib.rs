//! Per-frame view processing for the Moose engine: portal traversal out from the
//! camera's sector, view transform and projection, culling, and clipping. The output is
//! screen-space convex polygons with `w = 1 / depth` and f32 attribute values, ready for
//! the span buffer module.

mod clip;
mod frame;

pub use frame::{
    EdgeLine, MAX_REFLECTIONS, Mirror, PolygonKind, PolygonSource, ScreenVertex, SectorVisit,
    ViewConfig, ViewGeometry, ViewPolygon, ViewStats, pixel_edge,
};
