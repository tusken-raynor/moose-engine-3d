//! Carving a terrain's model by a level's sectors (see [`Terrain`](crate::Terrain)).
//!
//! Every polygon is clipped to each convex sector it crosses, by the sector's planes in
//! turn. Pieces meet exactly: a point where a cut crosses one of the model's own edges is
//! worked out from that edge's two original corners, so the two polygons sharing the edge
//! (in the same sector or on either side of a portal, whose two sides are cut by the same
//! plane) get the bit-identical point, and the pieces share it as one position.

use std::collections::HashMap;
use std::ops::Range;

use glam::{Affine3A, Quat, Vec3};

use crate::geom::{Aabb, Plane};
use crate::mesh::{Mesh, MeshBuilder};

/// Where a vertex of a polygon being clipped comes from: one of the model polygon's
/// corners (its position index), a point on one of its edges (its corners' position
/// indices, the lower first), or inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Origin {
    Corner(u32),
    Edge(u32, u32),
    Inside,
}

#[derive(Clone, Debug)]
struct Vertex {
    p: Vec3,
    /// Every attribute's components, in the model's attribute order.
    values: Vec<f32>,
    origin: Origin,
}

/// One sector: the planes that bound it, each keeping the side where
/// `distance >= 0`, and its box (for a quick test).
pub(crate) struct Region {
    pub planes: Vec<Plane>,
    pub bounds: Aabb,
}

/// The carved model: its pieces in world space (`transform` places the model; `rotation`
/// is its rotation, for a `normal` attribute), grouped by region, and how many slivers were
/// dropped as too thin to keep.
pub(crate) fn carve(
    name: &str,
    model: &Mesh,
    transform: Affine3A,
    rotation: Quat,
    regions: &[Region],
) -> (Mesh, Vec<Range<u32>>, usize) {
    let world: Vec<Vec3> = model.positions.iter().map(|&p| transform.transform_point3(p)).collect();
    let normal_attrib = model.attribs.iter().position(|a| a.name == "normal" && a.count == 3);
    let stride: usize = model.attribs.iter().map(|a| a.count as usize).sum();
    let values_of = |v: usize| -> Vec<f32> {
        let mut out = Vec::with_capacity(stride);
        for (k, a) in model.attribs.iter().enumerate() {
            let n = a.count as usize;
            let start = out.len();
            out.extend((0..n).map(|c| a.data.get_f32(v * n + c)));
            if Some(k) == normal_attrib {
                let turned = rotation * Vec3::from_slice(&out[start..start + 3]);
                out[start..start + 3].copy_from_slice(&turned.to_array());
            }
        }
        out
    };

    // Pieces per region: (corners, flags).
    let mut pieces: Vec<Vec<(Vec<Vertex>, crate::mesh::PolyFlags)>> = vec![Vec::new(); regions.len()];
    let mut corners: Vec<Vertex> = Vec::new();
    let (mut poly, mut next) = (Vec::new(), Vec::new());
    for polygon in &model.polygons {
        if polygon.flags.proxy() {
            continue;
        }
        corners.clear();
        corners.extend(polygon.vertices().map(|v| {
            let index = model.vertex_positions[v];
            Vertex { p: world[index as usize], values: values_of(v), origin: Origin::Corner(index) }
        }));
        let bounds = Aabb::from_points(corners.iter().map(|c| c.p));
        for (r, region) in regions.iter().enumerate() {
            if bounds.max.cmplt(region.bounds.min).any() || bounds.min.cmpgt(region.bounds.max).any() {
                continue;
            }
            poly.clear();
            poly.extend(corners.iter().cloned());
            for plane in &region.planes {
                clip(&poly, plane, &corners, &mut next);
                std::mem::swap(&mut poly, &mut next);
                if poly.len() < 3 {
                    break;
                }
            }
            poly.dedup_by(|a, b| a.p == b.p);
            while poly.len() > 1 && poly[0].p == poly[poly.len() - 1].p {
                poly.pop();
            }
            if poly.len() >= 3 {
                pieces[r].push((poly.clone(), polygon.flags));
            }
        }
    }

    // One position per distinct point.
    let mut positions: Vec<Vec3> = Vec::new();
    let mut index: HashMap<[u32; 3], u32> = HashMap::new();
    let mut indexed: Vec<Vec<(Vec<u32>, usize, usize)>> = Vec::with_capacity(regions.len());
    for (r, list) in pieces.iter().enumerate() {
        let mut out = Vec::with_capacity(list.len());
        for (i, (verts, _)) in list.iter().enumerate() {
            let ids = verts
                .iter()
                .map(|v| {
                    *index.entry(v.p.to_array().map(f32::to_bits)).or_insert_with(|| {
                        positions.push(v.p);
                        positions.len() as u32 - 1
                    })
                })
                .collect();
            out.push((ids, r, i));
        }
        indexed.push(out);
    }

    let decls = model.attribs.iter().map(|a| (a.name.clone(), a.count, a.format()));
    let mut builder = MeshBuilder::new(name, positions, decls);
    let mut ranges = Vec::with_capacity(regions.len());
    let mut dropped = 0;
    for list in &indexed {
        let first = builder.polygon_count();
        for (ids, r, i) in list {
            let (verts, flags) = &pieces[*r][*i];
            // Slivers too thin to have a plane: dropped.
            if builder.push_polygon(ids, *flags).is_err() {
                dropped += 1;
                continue;
            }
            for v in verts {
                let mut at = 0;
                for (k, a) in model.attribs.iter().enumerate() {
                    for c in 0..a.count as usize {
                        builder.attrib_data(k).push_f32(v.values[at + c]);
                    }
                    at += a.count as usize;
                }
            }
        }
        ranges.push(first..builder.polygon_count());
    }
    (builder.finish(&mut []), ranges, dropped)
}

/// Clips convex `poly` (a piece of the polygon with `corners`) to the side of `plane`
/// where `distance >= 0`, into `out`.
fn clip(poly: &[Vertex], plane: &Plane, corners: &[Vertex], out: &mut Vec<Vertex>) {
    out.clear();
    let n = poly.len();
    for i in 0..n {
        let (u, v) = (&poly[i], &poly[(i + 1) % n]);
        let (du, dv) = (plane.distance(u.p), plane.distance(v.p));
        if du >= 0.0 {
            out.push(u.clone());
        }
        if (du >= 0.0) != (dv >= 0.0) {
            out.push(cut(u, v, plane, corners));
        }
    }
}

/// Where `plane` crosses the edge from `u` to `v`. On one of the model polygon's own
/// edges, it is worked out from that edge's corners, whatever part of it `u` and `v` are;
/// otherwise from `u` and `v` in a fixed order (by position), so it never depends on which
/// way the edge is walked.
fn cut(u: &Vertex, v: &Vertex, plane: &Plane, corners: &[Vertex]) -> Vertex {
    let edge = match (u.origin, v.origin) {
        (Origin::Corner(a), Origin::Corner(b)) => Some((a.min(b), a.max(b))),
        (Origin::Corner(c), Origin::Edge(a, b)) | (Origin::Edge(a, b), Origin::Corner(c))
            if c == a || c == b =>
        {
            Some((a, b))
        }
        (Origin::Edge(a, b), Origin::Edge(c, d)) if (a, b) == (c, d) => Some((a, b)),
        _ => None,
    };
    let corner = |i: u32| corners.iter().find(|c| c.origin == Origin::Corner(i));
    let ends = match edge {
        Some((a, b)) => corner(a).zip(corner(b)),
        None => None,
    };
    let (a, b, origin) = match (ends, edge) {
        (Some((a, b)), Some((i, j))) => (a, b, Origin::Edge(i, j)),
        _ => {
            let key = |x: &Vertex| x.p.to_array().map(f32::to_bits);
            let (a, b) = if key(u) <= key(v) { (u, v) } else { (v, u) };
            (a, b, Origin::Inside)
        }
    };
    let (da, db) = (plane.distance(a.p), plane.distance(b.p));
    let t = da / (da - db);
    Vertex {
        p: a.p + (b.p - a.p) * t,
        values: a.values.iter().zip(&b.values).map(|(x, y)| x + (y - x) * t).collect(),
        origin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{PolyFlags, StorageFormat};

    /// Two triangles making a square in y = 0, from x 0 to 4 and z 0 to 4, with a color
    /// that runs from 0 at x = 0 to 200 at x = 4.
    fn square() -> Mesh {
        let positions = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 4.0),
            Vec3::new(4.0, 0.0, 4.0),
            Vec3::new(4.0, 0.0, 0.0),
        ];
        let mut b = MeshBuilder::new("square", positions.clone(), [("color".to_string(), 1, StorageFormat::U8)]);
        for tri in [[0u32, 1, 2], [0, 2, 3]] {
            b.push_polygon(&tri, PolyFlags(0)).unwrap();
            for i in tri {
                b.attrib_data(0).push_f32(positions[i as usize].x * 50.0);
            }
        }
        b.finish(&mut [])
    }

    /// Everything with x between `lo` and `hi`.
    fn slab(lo: f32, hi: f32) -> Region {
        Region {
            planes: vec![
                Plane { normal: Vec3::X, d: -lo },
                Plane { normal: Vec3::NEG_X, d: hi },
            ],
            bounds: Aabb { min: Vec3::new(lo, -10.0, -10.0), max: Vec3::new(hi, 10.0, 10.0) },
        }
    }

    #[test]
    fn pieces_meet_on_shared_points_and_keep_their_values() {
        // Two sectors meeting at x = 1 (the second cut by the first's plane, reversed), and
        // the model sticking out past x = 3, where no sector is.
        let left = slab(-1.0, 1.0);
        let right = Region {
            planes: vec![
                Plane { normal: -left.planes[1].normal, d: -left.planes[1].d },
                Plane { normal: Vec3::NEG_X, d: 3.0 },
            ],
            bounds: slab(1.0, 3.0).bounds,
        };
        let (mesh, ranges, dropped) =
            carve("t", &square(), Affine3A::IDENTITY, Quat::IDENTITY, &[left, right]);
        assert_eq!(dropped, 0);
        assert_eq!(ranges.len(), 2);
        assert!(!ranges[0].is_empty() && !ranges[1].is_empty());
        // Nothing past x = 3.
        assert!(mesh.positions.iter().all(|p| p.x <= 3.0 + 1e-6), "{:?}", mesh.positions);
        // The cut at x = 1 crosses the diagonal and both outer edges: each crossing is one
        // position, used by pieces on both sides.
        let on_cut: Vec<u32> = (0..mesh.positions.len() as u32)
            .filter(|&i| (mesh.positions[i as usize].x - 1.0).abs() < 1e-6)
            .collect();
        assert_eq!(on_cut.len(), 3, "{:?}", mesh.positions);
        let users = |range: &Range<u32>, i: u32| {
            range.clone().any(|p| mesh.vertex_positions[mesh.polygons[p as usize].vertices()].contains(&i))
        };
        for &i in &on_cut {
            assert!(users(&ranges[0], i) && users(&ranges[1], i), "position {i} not shared");
        }
        // Colors are carried along: 50 at x = 1.
        let color = mesh.attrib("color").unwrap();
        for (v, &p) in mesh.vertex_positions.iter().enumerate() {
            let x = mesh.positions[p as usize].x;
            assert!((color.data.get_f32(v) - x * 50.0).abs() <= 0.5, "x {x}");
        }
        // Each sector's pieces cover its part of the square.
        let area = |range: &Range<u32>| -> f32 {
            range
                .clone()
                .map(|p| {
                    let pts: Vec<Vec3> = mesh.polygon_points(&mesh.polygons[p as usize]).collect();
                    (1..pts.len() - 1).map(|k| (pts[k] - pts[0]).cross(pts[k + 1] - pts[0]).length() / 2.0).sum::<f32>()
                })
                .sum()
        };
        assert!((area(&ranges[0]) - 4.0).abs() < 1e-4 && (area(&ranges[1]) - 8.0).abs() < 1e-4);
    }
}
