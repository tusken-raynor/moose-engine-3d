use glam::Vec3;

/// Relative tolerance for planarity, convexity and containment tests, scaled by object size.
pub(crate) const TOLERANCE: f32 = 1e-4;

/// A plane through a polygon. The normal points out of the polygon's front face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub d: f32,
}

impl Plane {
    /// Signed distance from the plane: positive on the side the polygon faces.
    pub fn distance(&self, p: Vec3) -> f32 {
        self.normal.dot(p) + self.d
    }
}

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// Bounds of `points`, or a zero box at the origin if there are none.
    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Aabb {
        let mut it = points.into_iter();
        let Some(first) = it.next() else {
            return Aabb {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            };
        };
        it.fold(
            Aabb {
                min: first,
                max: first,
            },
            |b, p| Aabb {
                min: b.min.min(p),
                max: b.max.max(p),
            },
        )
    }

    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }
}

/// Returns the plane of a planar, convex polygon whose points run counter-clockwise
/// as seen from its front. Collinear points are allowed; repeated points,
/// zero-area polygons and self-overlapping outlines are not.
pub(crate) fn convex_polygon_plane(points: &[Vec3]) -> Result<Plane, String> {
    let n = points.len();
    if n < 3 {
        return Err(format!("polygon has {n} vertices, needs at least 3"));
    }
    let centroid = points.iter().copied().sum::<Vec3>() / n as f32;
    let size = points
        .iter()
        .map(|p| p.distance(centroid))
        .fold(0.0, f32::max);
    let tol = TOLERANCE * size.max(1.0);

    // Newell's method, relative to the centroid for precision far from the origin.
    let mut normal = Vec3::ZERO;
    for (i, &p) in points.iter().enumerate() {
        let (p, q) = (p - centroid, points[(i + 1) % n] - centroid);
        normal += Vec3::new(
            (p.y - q.y) * (p.z + q.z),
            (p.z - q.z) * (p.x + q.x),
            (p.x - q.x) * (p.y + q.y),
        );
    }
    let area2 = normal.length();
    if area2.is_nan() || area2 <= TOLERANCE * size * size {
        return Err("polygon has no area".into());
    }
    let normal = normal / area2;

    if points.iter().any(|&p| normal.dot(p - centroid).abs() > tol) {
        return Err("polygon is not planar".into());
    }

    // Every turn must be left (or straight) around the normal, and the turns must
    // add up to exactly one revolution; that rules out reflex corners, spikes and stars.
    let mut turning = 0.0;
    for i in 0..n {
        let (a, b, c) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
        let (e1, e2) = (b - a, c - b);
        if e2.length() <= tol {
            return Err(format!("polygon repeats vertex {i}"));
        }
        let angle = e1.cross(e2).dot(normal).atan2(e1.dot(e2));
        if angle < -1e-3 {
            return Err(format!(
                "polygon is not convex (reflex corner at vertex {i})"
            ));
        }
        turning += angle;
    }
    if (turning - std::f32::consts::TAU).abs() > 1e-3 {
        return Err("polygon is not convex (its outline doubles back or overlaps itself)".into());
    }

    Ok(Plane {
        normal,
        d: -normal.dot(centroid),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, y, z)
    }

    #[test]
    fn square_faces_up_when_ccw_from_above() {
        // Seen from +Y with -Z up the page, this runs counter-clockwise.
        let p = convex_polygon_plane(&[v(0., 0., 0.), v(0., 0., 1.), v(1., 0., 1.), v(1., 0., 0.)])
            .unwrap();
        assert!((p.normal - Vec3::Y).length() < 1e-6);
        assert!(p.distance(v(0.5, 2.0, 0.5)) > 0.0);
    }

    #[test]
    fn collinear_points_allowed() {
        let pts = [
            v(0., 0., 0.),
            v(1., 0., 0.),
            v(2., 0., 0.),
            v(2., 1., 0.),
            v(0., 1., 0.),
        ];
        assert!(convex_polygon_plane(&pts).is_ok());
    }

    #[test]
    fn rejects_bad_polygons() {
        let err = |pts: &[Vec3]| convex_polygon_plane(pts).unwrap_err();
        assert!(err(&[v(0., 0., 0.), v(1., 0., 0.)]).contains("at least 3"));
        assert!(err(&[v(0., 0., 0.), v(1., 0., 0.), v(2., 0., 0.)]).contains("no area"));
        assert!(
            err(&[v(0., 0., 0.), v(1., 0., 0.), v(1., 1., 0.3), v(0., 1., 0.)])
                .contains("not planar")
        );
        // Arrowhead with a reflex corner.
        assert!(
            err(&[
                v(0., 0., 0.),
                v(2., 0., 0.),
                v(1., 0.5, 0.),
                v(2., 2., 0.),
                v(0., 2., 0.)
            ])
            .contains("reflex")
        );
        assert!(
            err(&[v(0., 0., 0.), v(1., 0., 0.), v(1., 0., 0.), v(0., 1., 0.)]).contains("repeats")
        );
        // Pentagram: all left turns, but it winds twice.
        let star: Vec<Vec3> = (0..5)
            .map(|i| {
                let a = (i * 2) as f32 * std::f32::consts::TAU / 5.0;
                v(a.cos(), a.sin(), 0.0)
            })
            .collect();
        assert!(err(&star).contains("overlaps"));
    }
}
