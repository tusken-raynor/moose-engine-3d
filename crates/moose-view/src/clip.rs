use glam::Vec3;

/// Relative distance within which a vertex counts as lying on a clip plane.
const ON_PLANE: f32 = 1e-6;

/// How a clip point is placed exactly on a frustum plane, as v1 did: after clipping against
/// x = -w, write x = -w. Then x / w is exactly -1 in float, so the vertex lands exactly on
/// the viewport edge. Rounding is symmetric under negation, so interpolating between two
/// points with x = -w gives x = -w again: points stay on a frustum edge through every
/// later clip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Snap {
    /// General plane (portal edge, near plane): no exact form needed.
    None,
    /// On the plane x = s * w (s is -1 for the left edge, +1 for the right).
    X(f32),
    /// On the plane y = s * w (s is -1 for the bottom edge, +1 for the top).
    Y(f32),
}

/// A half-space in homogeneous clip coordinates `(x, y, w)`: keeps points where
/// `normal · p + offset >= 0`.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClipPlane {
    pub normal: Vec3,
    pub offset: f32,
    pub snap: Snap,
}

impl ClipPlane {
    /// A general plane through the eye (the origin).
    pub fn through_eye(normal: Vec3) -> Self {
        Self {
            normal,
            offset: 0.0,
            snap: Snap::None,
        }
    }

    /// The frustum plane x = s * w (s = -1 for the left edge, +1 for the right), keeping
    /// the side toward the view axis.
    pub fn frustum_x(s: f32) -> Self {
        Self {
            normal: Vec3::new(-s, 0.0, 1.0),
            offset: 0.0,
            snap: Snap::X(s),
        }
    }

    /// The frustum plane y = s * w (s = -1 for the bottom edge, +1 for the top).
    pub fn frustum_y(s: f32) -> Self {
        Self {
            normal: Vec3::new(0.0, -s, 1.0),
            offset: 0.0,
            snap: Snap::Y(s),
        }
    }

    /// Keeps points at least `near` deep (w >= near).
    pub fn near(near: f32) -> Self {
        Self {
            normal: Vec3::Z,
            offset: -near,
            snap: Snap::None,
        }
    }

    pub fn distance(&self, p: Vec3) -> f32 {
        self.normal.dot(p) + self.offset
    }

    /// Moves a point that lies on this plane (up to rounding) exactly onto it.
    fn snap(&self, xyw: &mut [f32]) {
        match self.snap {
            Snap::None => {}
            Snap::X(s) => xyw[0] = s * xyw[2],
            Snap::Y(s) => xyw[1] = s * xyw[2],
        }
    }
}

/// Which line a clipped polygon's edge lies on: one of the input polygon's own edges (by
/// index of its starting vertex), or one of the clip planes (by index in the plane list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    Input(u16),
    Plane(u16),
}

/// Sutherland–Hodgman clipping of convex polygons stored as interleaved f32 records:
/// clip-space x, y, w followed by the vertex's attribute values. One routine serves every
/// plane (frustum sides, near, portal edges) and every polygon (level faces, entity faces,
/// portal outlines). For each output vertex it records the line the edge leaving it lies
/// on. Buffers are reused.
///
/// A clip point on one of the input polygon's own edges is always computed from that
/// edge's two original vertices (from the one inside the plane toward the one outside),
/// never from points left by earlier clips. So where an edge crosses a plane depends only
/// on the edge and the plane: an edge shared by two polygons, or a portal edge that is also
/// a wall's edge, gets bit-identical clip points in every polygon, whatever other planes
/// each was clipped against and in whatever order.
#[derive(Default)]
pub(crate) struct Clipper {
    original: Vec<f32>,
    current: Vec<f32>,
    next: Vec<f32>,
    edges: Vec<Edge>,
    next_edges: Vec<Edge>,
    distances: Vec<f32>,
}

impl Clipper {
    /// Clips the polygon in `input` (records of `stride` floats) against every plane and
    /// returns the surviving records, plus the source line of the edge leaving each one.
    /// Empty means the polygon is entirely outside.
    pub fn clip(
        &mut self,
        input: &[f32],
        stride: usize,
        planes: &[ClipPlane],
    ) -> (&[f32], &[Edge]) {
        self.original.clear();
        self.original.extend_from_slice(input);
        let originals = input.len() / stride;
        self.current.clear();
        self.current.extend_from_slice(input);
        self.edges.clear();
        self.edges
            .extend((0..input.len() / stride).map(|i| Edge::Input(i as u16)));
        for (k, plane) in planes.iter().enumerate() {
            let n = self.current.len() / stride;
            if n < 3 {
                break;
            }
            self.distances.clear();
            // Vertices within float error of the plane count as on it: kept (put exactly on
            // it), with no clip point beside them.
            for i in 0..n {
                let record = &mut self.current[i * stride..i * stride + 3];
                let p = Vec3::from_slice(record);
                let mut d = plane.distance(p);
                if d.abs() <= ON_PLANE * (1.0 + p.length()) {
                    if d < 0.0 {
                        plane.snap(record);
                    }
                    d = 0.0;
                }
                self.distances.push(d);
            }
            if self.distances.iter().all(|&d| d >= 0.0) {
                continue;
            }
            if !self.distances.iter().any(|&d| d > 0.0) {
                self.current.clear();
                break;
            }
            let along_plane = Edge::Plane(k as u16);
            self.next.clear();
            self.next_edges.clear();
            for i in 0..n {
                let j = (i + 1) % n;
                let (di, dj) = (self.distances[i], self.distances[j]);
                let edge = self.edges[i];
                if di >= 0.0 {
                    // The outline leaves this vertex along its original edge, unless the
                    // vertex sits on the plane and the edge heads outside: then it follows
                    // the plane.
                    self.next
                        .extend_from_slice(&self.current[i * stride..(i + 1) * stride]);
                    self.next_edges.push(if di == 0.0 && dj < 0.0 {
                        along_plane
                    } else {
                        edge
                    });
                }
                if (di > 0.0 && dj < 0.0) || (di < 0.0 && dj > 0.0) {
                    // Interpolate between the edge's original vertices when it is (part of) an
                    // input edge, otherwise between the current points (an edge made by an
                    // earlier clip, along that clip's plane).
                    let (src, a, b) = match edge {
                        Edge::Input(e) => {
                            (&self.original, e as usize, (e as usize + 1) % originals)
                        }
                        Edge::Plane(_) => (&self.current, i, j),
                    };
                    let da = plane.distance(Vec3::from_slice(&src[a * stride..]));
                    let db = plane.distance(Vec3::from_slice(&src[b * stride..]));
                    let (inside, outside, d_in, d_out) = if da > db {
                        (a, b, da, db)
                    } else {
                        (b, a, db, da)
                    };
                    let t = d_in / (d_in - d_out);
                    let start = self.next.len();
                    for c in 0..stride {
                        let p = src[inside * stride + c];
                        let q = src[outside * stride + c];
                        self.next.push(p + (q - p) * t);
                    }
                    plane.snap(&mut self.next[start..start + 3]);
                    // Exiting: the outline continues along the plane. Entering: along the edge.
                    self.next_edges
                        .push(if di > 0.0 { along_plane } else { edge });
                }
            }
            std::mem::swap(&mut self.current, &mut self.next);
            std::mem::swap(&mut self.edges, &mut self.next_edges);
        }
        if self.current.len() / stride < 3 {
            return (&[], &[]);
        }
        (&self.current, &self.edges)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(z: f32) -> Vec<f32> {
        // x, y, z, one attribute
        [
            [-1.0, -1.0, 0.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 2.0],
            [-1.0, 1.0, 3.0],
        ]
        .iter()
        .flat_map(|&[x, y, a]| [x, y, z, a])
        .collect()
    }

    #[test]
    fn inside_outside_and_split() {
        let mut c = Clipper::default();
        let keep_right = ClipPlane::through_eye(Vec3::X); // x >= 0
        assert_eq!(c.clip(&square(-2.0), 4, &[]).0.len(), 16);
        assert_eq!(
            c.clip(
                &square(-2.0),
                4,
                &[ClipPlane {
                    normal: Vec3::X,
                    offset: 5.0,
                    snap: Snap::None,
                }]
            )
            .0
            .len(),
            16
        );
        assert!(
            c.clip(
                &square(-2.0),
                4,
                &[ClipPlane {
                    normal: Vec3::X,
                    offset: -5.0,
                    snap: Snap::None,
                }]
            )
            .0
            .is_empty()
        );
        let out = c.clip(&square(-2.0), 4, &[keep_right]).0.to_vec();
        assert_eq!(out.len() / 4, 4);
        // The new vertices sit on x = 0 with their attribute interpolated halfway.
        let on_plane: Vec<f32> = out
            .chunks(4)
            .filter(|v| v[0] == 0.0)
            .map(|v| v[3])
            .collect();
        assert_eq!(on_plane, [0.5, 2.5]);
    }

    #[test]
    fn frustum_clips_stay_exactly_on_the_plane() {
        // A polygon crossing the left edge (x = -w) and then the top edge (y = w). Every point
        // on the left edge must have x == -w bit for bit, including the corner made later by
        // the top clip, so x / w is exactly -1 and the vertex projects onto the viewport edge.
        let quad: Vec<f32> = [
            [-3.1f32, 0.3, 1.3],
            [0.9, -0.1, 2.7],
            [0.4, 3.3, 2.2],
            [-2.6, 2.9, 1.9],
        ]
        .concat();
        let planes = [ClipPlane::frustum_x(-1.0), ClipPlane::frustum_y(1.0)];
        let mut c = Clipper::default();
        let out = c.clip(&quad, 3, &planes).0.to_vec();
        let on_left: Vec<&[f32]> = out.chunks(3).filter(|v| v[0] == -v[2]).collect();
        let on_top: Vec<&[f32]> = out.chunks(3).filter(|v| v[1] == v[2]).collect();
        assert_eq!(on_left.len(), 2, "{out:?}");
        assert_eq!(on_top.len(), 2, "{out:?}");
        assert!(
            on_left.iter().any(|v| v[1] == v[2]),
            "the top-left corner is exactly on both edges"
        );
        assert!(on_left.iter().all(|v| v[0] / v[2] == -1.0));
    }

    #[test]
    fn edges_know_their_source() {
        // A square portal straddling the left edge x = -w: the clipped window's edges are
        // portal edges 0, 1, 2 (partly), the left plane, and portal edge 3 (partly).
        let corners = [
            Vec3::new(-2.0, -0.5, 1.0),
            Vec3::new(0.5, -0.5, 1.0),
            Vec3::new(0.5, 0.5, 1.0),
            Vec3::new(-2.0, 0.5, 1.0),
        ];
        let mut c = Clipper::default();
        let flat: Vec<f32> = corners.iter().flat_map(|p| p.to_array()).collect();
        let (records, sources) = c.clip(&flat, 3, &[ClipPlane::frustum_x(-1.0)]);
        let points: Vec<Vec3> = records.chunks(3).map(Vec3::from_slice).collect();
        assert_eq!(points.len(), 4);
        let mut got: Vec<Edge> = sources.to_vec();
        got.sort_by_key(|s| format!("{s:?}"));
        assert_eq!(
            got,
            [
                Edge::Input(0),
                Edge::Input(1),
                Edge::Input(2),
                Edge::Plane(0)
            ]
        );
        // The points on the left plane are exactly on it.
        assert_eq!(points.iter().filter(|p| p.x == -p.z).count(), 2);
    }

    #[test]
    fn shared_edge_clips_identically_both_ways() {
        let mut c = Clipper::default();
        let plane = [ClipPlane {
            normal: Vec3::new(0.3, 0.7, 0.1).normalize(),
            offset: 0.123,
            snap: Snap::None,
        }];
        let (a, b) = ([0.1f32, -3.7, -2.9], [-2.3f32, 1.9, -5.3]);
        // Two triangles sharing edge a-b, walking it in opposite directions.
        let t1: Vec<f32> = [a, b, [3.0, 3.0, -4.0]].concat();
        let t2: Vec<f32> = [b, a, [-3.0, -3.0, -4.0]].concat();
        let p1: Vec<[f32; 3]> = c
            .clip(&t1, 3, &plane)
            .0
            .chunks(3)
            .map(|v| [v[0], v[1], v[2]])
            .collect();
        let p2: Vec<[f32; 3]> = c
            .clip(&t2, 3, &plane)
            .0
            .chunks(3)
            .map(|v| [v[0], v[1], v[2]])
            .collect();
        let shared: Vec<_> = p1
            .iter()
            .filter(|p| p2.contains(p) && **p != a && **p != b)
            .collect();
        assert_eq!(
            shared.len(),
            1,
            "exactly one bit-identical clip point on the shared edge"
        );
    }
}
