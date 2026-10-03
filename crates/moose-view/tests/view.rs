use std::f32::consts::{FRAC_PI_2, PI, TAU};

use glam::{Quat, Vec2, Vec3};
use moose_assets::Assets;
use moose_scene::{Camera, PORTAL_CLEARANCE, Viewport, World};
use moose_view::{EdgeLine, PolygonKind, PolygonSource, ViewGeometry, ViewPolygon, pixel_edge};

const SMALL: Viewport = Viewport {
    x: 0,
    y: 0,
    width: 320,
    height: 180,
};

fn world() -> (World, Assets) {
    load("two_rooms.mmp")
}

fn load(file: &str) -> (World, Assets) {
    let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    let level = assets.load_level(file).unwrap();
    (World::new(level, &assets), assets)
}

fn camera(
    world: &World,
    position: Vec3,
    yaw: f32,
    pitch: f32,
    roll: f32,
    viewport: Viewport,
) -> Camera {
    let mut c = Camera::at_spawn(&world.spawn_points[0], viewport);
    c.position = position;
    c.sector = world
        .find_sector(position)
        .expect("camera outside the level");
    (c.yaw, c.pitch, c.roll) = (yaw, pitch, roll);
    c
}

/// Small deterministic generator, so failures reproduce.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// The pixels a polygon covers on `row`, as `[start, end)`: its left and right boundaries'
/// exact x at the row's center, rounded once with `pixel_edge`. An edge covers the rows
/// whose centers lie between its endpoints (`pixel_edge(top)..pixel_edge(bottom)`); its x
/// comes from its carried line if it has one.
fn row_span(points: &[(f32, f32)], lines: &[Option<EdgeLine>], row: i32) -> Option<(i32, i32)> {
    // Front-facing polygons wind so that, on screen (y down), edges heading down form the
    // left boundary and edges heading up the right one. A row whose left boundary is not
    // left of its right one (an edge-on sliver, flipped by float rounding) covers nothing.
    let (mut left, mut right) = (f32::NEG_INFINITY, f32::INFINITY);
    for (i, &a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        let (top, bottom) = (a.1.min(b.1), a.1.max(b.1));
        if row < pixel_edge(top) || row >= pixel_edge(bottom) {
            continue; // horizontal, or its rows don't include this one
        }
        let x = lines[i].unwrap_or(EdgeLine::between(a, b)).x_at_row(row);
        if b.1 > a.1 {
            left = left.max(x)
        } else {
            right = right.min(x)
        }
    }
    if !(left.is_finite() && right.is_finite()) {
        return None; // only one side on this row: a sliver whose outline doubles back
    }
    let (start, end) = (pixel_edge(left), pixel_edge(right));
    (start < end).then_some((start, end))
}

/// A polygon's screen points and the line of each edge.
type Outline = (Vec<(f32, f32)>, Vec<Option<EdgeLine>>);

/// Opaque world polygons: all but reflective polygons whose reflection was drawn, which are
/// drawn over it in the translucent pass.
fn opaque_world(p: &ViewPolygon) -> bool {
    p.kind == PolygonKind::World && p.reflection.is_none()
}

/// Checks that opaque world polygons tile the viewport exactly: every pixel belongs to
/// exactly one, with no tolerance. Returns the number of them.
fn assert_world_tiles_screen(out: &ViewGeometry, viewport: Viewport, what: &str) -> usize {
    let polys: Vec<Outline> = out
        .polygons
        .iter()
        .filter(|p| opaque_world(p))
        .map(|p| {
            let points = out.vertices[p.vertices()]
                .iter()
                .map(|v| (v.x, v.y))
                .collect();
            (points, out.edge_lines[p.vertices()].to_vec())
        })
        .collect();
    let (x0, y0) = (viewport.x as i32, viewport.y as i32);
    let mut counts = vec![0u8; viewport.width as usize];
    for y in y0..y0 + viewport.height as i32 {
        counts.iter_mut().for_each(|c| *c = 0);
        for (points, lines) in &polys {
            if let Some((lo, hi)) = row_span(points, lines, y) {
                assert!(
                    lo >= x0 && hi <= x0 + viewport.width as i32,
                    "{what}: row {y} span {lo}..{hi} outside the viewport"
                );
                for x in lo..hi {
                    counts[(x - x0) as usize] += 1;
                }
            }
        }
        for (i, &c) in counts.iter().enumerate() {
            let x = x0 + i as i32;
            assert!(c >= 1, "{what}: pixel ({x}, {y}) is not covered");
            assert!(
                c <= 1,
                "{what}: pixel ({x}, {y}) is covered by {c} world polygons"
            );
        }
    }
    polys.len()
}

fn assert_invariants(out: &ViewGeometry, cam: &Camera, what: &str) {
    let vp = cam.viewport;
    assert_eq!(out.world_positions.len(), out.vertices.len());
    assert_eq!(out.edge_lines.len(), out.vertices.len());
    // Each vertex shows its world position: projected (reflected first, in a mirror), it
    // lands on the vertex.
    let view = cam.view();
    for p in &out.polygons {
        for i in p.vertices() {
            let world = out.world_positions[i];
            let seen = out.reflect(p.mirror, world);
            let v = out.vertices[i];
            let q = view.rotation.conjugate() * (seen - view.position);
            let (x, y) = (
                view.center.x + view.focal * q.x / -q.z,
                view.center.y - view.focal * q.y / -q.z,
            );
            // Float error of clipping, reflecting and projecting: a hundredth of a pixel.
            let tol = 1e-2 + 1e-5 * x.abs().max(y.abs());
            assert!(
                (x - v.x).abs() < tol
                    && (y - v.y).abs() < tol
                    && (1.0 / -q.z - v.w).abs() < 1e-4 * v.w.max(1.0),
                "{what}: vertex at ({}, {}) w {} shows world {world} at ({x}, {y}) w {}",
                v.x,
                v.y,
                v.w,
                1.0 / -q.z
            );
        }
    }
    for v in &out.vertices {
        assert!(
            v.w > 0.0 && v.w <= 1.0 / cam.near * 1.0001,
            "{what}: w {} outside (0, 1/near]",
            v.w
        );
        assert!(
            v.x >= vp.x as f32
                && v.x <= (vp.x + vp.width) as f32
                && v.y >= vp.y as f32
                && v.y <= (vp.y + vp.height) as f32,
            "{what}: vertex ({}, {}) outside the viewport",
            v.x,
            v.y
        );
    }
    for p in &out.polygons {
        assert!(p.vertex_count >= 3, "{what}: degenerate polygon");
        // Clipping only interpolates: each vertex is a convex combination of the source
        // polygon's vertices.
        let n = p.source_vertices as usize;
        for weights in out.weights[p.weights()].chunks_exact(n) {
            let sum: f32 = weights.iter().sum();
            assert!(
                (sum - 1.0).abs() < 1e-4
                    && weights.iter().all(|&w| (-1e-5..=1.0 + 1e-5).contains(&w)),
                "{what}: weights {weights:?}"
            );
        }
    }
}

#[test]
fn world_polygons_tile_the_screen_from_anywhere() {
    tile_from_anywhere("two_rooms.mmp", 1, 7);
}

#[test]
fn world_polygons_tile_the_screen_with_mirrors() {
    tile_from_anywhere("shiny_rooms.mmp", 1, 11);
}

#[test]
fn world_polygons_tile_the_screen_with_mirrors_in_mirrors() {
    tile_from_anywhere("mirror_rooms.mmp", 3, 13);
}

fn tile_from_anywhere(level: &str, max_reflections: u8, seed: u64) {
    let (world, assets) = load(level);
    let mut out = ViewGeometry::new();
    out.config.max_reflections = max_reflections;
    let mut rng = Lcg(seed);
    // Sector interiors, kept 0.3 m from solid surfaces (there is no collision radius yet).
    let regions = [
        (Vec3::new(-3.7, 0.3, 0.3), Vec3::new(3.7, 3.7, 7.7)),
        (Vec3::new(-0.7, 0.3, -11.7), Vec3::new(0.7, 2.7, -0.3)),
        (Vec3::new(-3.7, 0.3, -19.7), Vec3::new(3.7, 3.7, -12.3)),
    ];
    let mut views = 0;
    let mut reached_room_b_through_two_portals = false;
    let (mut with_mirrors, mut reflected_through_a_portal, mut deepest) = (0, false, 0);
    for round in 0..240 {
        let position = if round % 4 == 3 {
            // In a doorway: at the portal clearance or a few mm off either side of a portal plane.
            let plane_z = if rng.next() < 0.5 { 0.0 } else { -12.0 };
            let offsets = [
                PORTAL_CLEARANCE,
                -PORTAL_CLEARANCE,
                0.004,
                -0.004,
                0.03,
                -0.03,
            ];
            let dz = offsets[(rng.next() * offsets.len() as f32) as usize % offsets.len()];
            Vec3::new(rng.range(-0.7, 0.7), rng.range(0.3, 2.7), plane_z + dz)
        } else {
            let (lo, hi) = regions[round % 3];
            Vec3::new(
                rng.range(lo.x, hi.x),
                rng.range(lo.y, hi.y),
                rng.range(lo.z, hi.z),
            )
        };
        let (yaw, pitch, roll) = (
            rng.range(0.0, TAU),
            rng.range(-1.3, 1.3),
            rng.range(-PI, PI),
        );
        let cam = camera(&world, position, yaw, pitch, roll, SMALL);
        out.build(&world, &assets, &cam.view());
        let what =
            format!("{level} view {round} at {position:?} ypr ({yaw:?}, {pitch:?}, {roll:?})");
        assert_invariants(&out, &cam, &what);
        assert_world_tiles_screen(&out, SMALL, &what);
        reached_room_b_through_two_portals |=
            out.visits.iter().any(|v| v.sector == 2 && v.depth == 2);
        with_mirrors += !out.mirrors.is_empty() as u32;
        deepest = out.mirrors.iter().map(|m| m.depth).fold(deepest, u8::max);
        assert!(
            out.mirrors.iter().all(|m| m.depth <= max_reflections),
            "{what}: mirror deeper than {max_reflections}"
        );
        reflected_through_a_portal |= out
            .visits
            .iter()
            .any(|v| v.mirror.is_some() && v.depth >= 2);
        views += 1;
    }
    assert_eq!(views, 240);
    if level != "two_rooms.mmp" {
        assert_eq!(
            deepest, max_reflections,
            "never saw a mirror {max_reflections} deep"
        );
        assert!(with_mirrors > 60, "only {with_mirrors} views saw a mirror");
        assert!(
            reflected_through_a_portal,
            "no reflection looked through a portal"
        );
    }
    assert!(
        reached_room_b_through_two_portals,
        "no view looked down the hallway into room_b"
    );
}

#[test]
fn eye_exactly_on_a_portal_plane() {
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    for (yaw, pitch) in [
        (0.0, 0.0),
        (FRAC_PI_2, 0.0),
        (FRAC_PI_2, 0.4),
        (-FRAC_PI_2, -0.3),
        (PI, 0.0),
        (0.3, 1.2),
    ] {
        let cam = camera(&world, Vec3::new(0.2, 1.5, 0.0), yaw, pitch, 0.0, SMALL);
        out.build(&world, &assets, &cam.view());
        let what = format!("on the portal plane, yaw {yaw} pitch {pitch}");
        assert_invariants(&out, &cam, &what);
        assert_world_tiles_screen(&out, SMALL, &what);
    }
}

#[test]
fn walking_through_the_doorway() {
    // Real movement: every frame's camera comes from Camera::move_local, facing the door.
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let mut cam = camera(&world, Vec3::new(0.3, 1.6, 0.6), 0.2, -0.1, 0.05, SMALL);
    for step in 0..60 {
        out.build(&world, &assets, &cam.view());
        let what = format!("step {step} at {} in sector {}", cam.position, cam.sector);
        assert_invariants(&out, &cam, &what);
        assert_world_tiles_screen(&out, SMALL, &what);
        cam.move_to(&world, cam.position + Vec3::new(0.0, 0.0, -0.02));
    }
    assert_eq!(cam.sector, 1, "ended in the hallway");
}

#[test]
fn looking_down_the_hallway_from_spawn() {
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let viewport = Viewport {
        x: 0,
        y: 0,
        width: 1280,
        height: 720,
    };
    let cam = camera(&world, Vec3::new(0.0, 1.7, 6.0), 0.0, 0.0, 0.0, viewport);
    out.build(&world, &assets, &cam.view());

    let visits: Vec<(u32, u16, bool)> = out
        .visits
        .iter()
        .map(|v| (v.sector, v.depth, v.near_clipped))
        .collect();
    assert_eq!(visits, [(0, 0, true), (1, 1, false), (2, 2, false)]);

    let drawn = |name: &str| {
        let e = world.entities.iter().position(|e| e.name == name).unwrap() as u32;
        out.polygons.iter().any(|p| {
            p.source
                == PolygonSource::Entity {
                    entity: e,
                    polygon: p_poly(p.source),
                }
        })
    };
    fn p_poly(s: PolygonSource) -> u32 {
        match s {
            PolygonSource::Entity { polygon, .. }
            | PolygonSource::World { polygon, .. }
            | PolygonSource::Terrain { polygon, .. } => polygon,
        }
    }
    // Straight down the hallway: its crate, and room_b's center crate through both portals.
    assert!(drawn("crate_hall"));
    assert!(drawn("crate_b1"));
    // crate_b2 (x = 3, z = -18) sits off to the side of room_b; the line of sight to it
    // crosses z = -12 at x = 2.25, outside the 2 m doorway, so both windows cull it.
    assert!(!drawn("crate_b2"));
    // room_a's crates are ahead of the camera too.
    assert!(drawn("crate_a1") && drawn("crate_a2_stacked"));
    // Turned around: only room_a is reached, and every crate is behind the camera.
    let back = camera(&world, Vec3::new(0.0, 1.7, 6.0), PI, 0.0, 0.0, viewport);
    out.build(&world, &assets, &back.view());
    assert_eq!(out.visits.iter().map(|v| v.sector).collect::<Vec<_>>(), [0]);
    assert_eq!(
        (out.stats.entities_drawn, out.stats.entities_culled),
        (0, 6)
    );
}

#[test]
fn projection_matches_the_view() {
    // Unclipped world polygons keep their vertices, projected exactly as View::project does.
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let cam = camera(
        &world,
        Vec3::new(0.0, 1.7, 6.0),
        0.0,
        0.0,
        0.0,
        Viewport {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        },
    );
    let view = cam.view();
    out.build(&world, &assets, &view);
    let geometry = assets.mesh(world.geometry);
    let mut checked = 0;
    for p in out.polygons.iter().filter(|p| p.kind == PolygonKind::World) {
        let PolygonSource::World { polygon, .. } = p.source else {
            unreachable!()
        };
        let source = &geometry.polygons[polygon as usize];
        if source.vertex_count != p.vertex_count {
            continue; // clipped
        }
        let Some(expected) = geometry
            .polygon_points(source)
            .map(|q| view.project(q))
            .collect::<Option<Vec<(Vec2, f32)>>>()
        else {
            continue; // partly behind the camera, so it was clipped
        };
        let got = &out.vertices[p.vertices()];
        if expected
            .iter()
            .zip(got)
            .all(|((s, w), v)| (Vec2::new(v.x, v.y) - *s).length() < 1e-3 && (v.w - w).abs() < 1e-6)
        {
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no unclipped world polygon matched View::project"
    );
}

#[test]
fn entity_poking_into_a_sector_is_seen_from_it() {
    // Doorway portals passable but not render-through (0x2): from room_a the hallway is
    // never reached, so only the crate's listing of room_a can make it visible.
    let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    let src = include_str!("../../../assets/levels/two_rooms.mmp")
        .replace("       0x3\n", "       0x2\n");
    let mut world = World::new(assets.parse_level("two_rooms.mmp", &src).unwrap(), &assets);
    let i = world
        .entities
        .iter()
        .position(|e| e.name == "crate_hall")
        .unwrap();
    world.entities[i].position = Vec3::new(0.0, 0.0, -0.3); // home: hallway, pokes 0.2 m into room_a
    world.entities[i].rotation = Quat::IDENTITY;
    world.place_entity(i, &assets);
    assert_eq!(world.entities[i].sectors, [1, 0]);

    let mut out = ViewGeometry::new();
    let cam = camera(&world, Vec3::new(0.0, 1.7, 6.0), 0.0, -0.2, 0.0, SMALL);
    let is_crate = |p: &moose_view::ViewPolygon| matches!(p.source, PolygonSource::Entity { entity, .. } if entity == i as u32);
    out.build(&world, &assets, &cam.view());
    assert_eq!(out.visits.iter().map(|v| v.sector).collect::<Vec<_>>(), [0]);
    assert!(
        out.polygons.iter().any(is_crate),
        "crate poking into room_a was culled"
    );

    // Control: listed only in its home sector, it is culled.
    world.entities[i].sectors = vec![1];
    out.build(&world, &assets, &cam.view());
    assert!(!out.polygons.iter().any(is_crate));
}

#[test]
fn entities_crossing_the_near_plane_are_clipped() {
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    // 2 cm from crate_a1's +Z face, looking at it: the face crosses the near plane at the edges.
    let cam = camera(&world, Vec3::new(-2.5, 0.5, 2.52), 0.0, 0.0, 0.0, SMALL);
    out.build(&world, &assets, &cam.view());
    assert_invariants(&out, &cam, "nose to a crate");
    assert!(out.stats.entities_drawn >= 1);
}

#[test]
fn buffers_are_reused() {
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let cam = camera(&world, Vec3::new(0.0, 1.7, 6.0), 0.0, 0.0, 0.0, SMALL);
    out.build(&world, &assets, &cam.view());
    let (v, p) = (out.vertices.capacity(), out.polygons.capacity());
    let first = out.polygons.clone();
    out.build(&world, &assets, &cam.view());
    assert_eq!(out.polygons, first, "same view, same output");
    assert_eq!((out.vertices.capacity(), out.polygons.capacity()), (v, p));
}

#[test]
fn near_clip_applies_through_a_doorway() {
    // At the door jamb, 1 mm before the portal: the hallway's +X wall starts 2 cm from the
    // eye, closer than the near plane, so the hallway must be near-clipped even though it
    // is seen through a portal.
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let cam = camera(
        &world,
        Vec3::new(0.98, 1.5, PORTAL_CLEARANCE),
        -0.3,
        0.0,
        0.0,
        SMALL,
    );
    assert_eq!(cam.sector, 0);
    out.build(&world, &assets, &cam.view());
    let hallway = out
        .visits
        .iter()
        .find(|v| v.sector == 1)
        .expect("hallway not reached");
    assert!(hallway.near_clipped);
    assert_invariants(&out, &cam, "at the door jamb");
}

/// What a pixel's center ray hits in the level, bouncing off reflective polygons up to
/// `max_bounces` times: `(sector, polygon, bounces)`, or `None` when the ray passes within a
/// fraction of a
/// pixel of an edge (checked by also tracing rays near the pixel's corners), where the
/// owner depends on rounding.
fn trace_pixel(
    world: &World,
    assets: &Assets,
    cam: &Camera,
    max_bounces: u8,
    x: i32,
    y: i32,
) -> Option<(u32, u32, u8)> {
    let view = cam.view();
    let geometry = assets.mesh(world.geometry);
    let trace = |sx: f32, sy: f32| {
        let mut dir = view.rotation
            * Vec3::new(
                (sx - view.center.x) / view.focal,
                -(sy - view.center.y) / view.focal,
                -1.0,
            );
        let (mut p, mut sector, mut bounces) = (view.position, cam.sector, 0u8);
        for _ in 0..64 {
            let s = &world.sectors[sector as usize];
            // Leaving a convex sector: through the nearest plane the ray heads out through, and
            // of the faces on that plane (a doorway wall is several), the one whose outline
            // contains the exit point.
            let outline = |face: Result<u32, u32>| -> Vec<Vec3> {
                match face {
                    Ok(i) => geometry
                        .polygon_points(&geometry.polygons[i as usize])
                        .collect(),
                    Err(i) => world.portals[i as usize]
                        .positions
                        .iter()
                        .map(|&v| geometry.positions[v as usize])
                        .collect(),
                }
            };
            let polygons = s
                .polygons
                .clone()
                .map(|i| (geometry.polygons[i as usize].plane, Ok(i)));
            let portals = s
                .portals
                .clone()
                .map(|i| (world.portals[i as usize].plane, Err(i)));
            let hits: Vec<(f32, Vec3, Result<u32, u32>)> = polygons
                .chain(portals)
                .filter(|(plane, _)| plane.normal.dot(dir) < 0.0)
                .map(|(plane, face)| {
                    (
                        plane.distance(p) / -plane.normal.dot(dir),
                        plane.normal,
                        face,
                    )
                })
                .collect();
            let t = hits.iter().map(|h| h.0).fold(f32::INFINITY, f32::min);
            p += dir * t;
            let (_, _, face) =
                *hits
                    .iter()
                    .filter(|h| h.0 <= t + 1e-5)
                    .find(|&&(_, n, face)| {
                        let pts = outline(face);
                        (0..pts.len()).all(|i| {
                            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                            (b - a).cross(p - a).dot(n) >= 0.0
                        })
                    })?;
            match face {
                Ok(i) => {
                    let polygon = &geometry.polygons[i as usize];
                    if polygon.flags.reflective() && bounces < max_bounces {
                        dir -= 2.0 * dir.dot(polygon.plane.normal) * polygon.plane.normal;
                        bounces += 1;
                    } else {
                        return Some((sector, i, bounces));
                    }
                }
                Err(i) => sector = world.portals[i as usize].target,
            }
        }
        None
    };
    let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
    let center = trace(cx, cy)?;
    let corners = [(-0.45, -0.45), (0.45, -0.45), (-0.45, 0.45), (0.45, 0.45)];
    corners
        .iter()
        .all(|&(dx, dy)| trace(cx + dx, cy + dy) == Some(center))
        .then_some(center)
}

#[test]
fn reflections_match_ray_tracing() {
    // Every pixel's opaque owner (away from edges) is what a ray through the pixel's center
    // hits after bouncing off shiny floors: the reflected room is where the mirror says it
    // is, through the right windows, including through portals.
    reflections_match_rays("shiny_rooms.mmp", 1, 24, 3);
}

#[test]
fn reflections_in_reflections_match_ray_tracing() {
    // The same between a shiny floor and ceiling, one bounce deep and three.
    reflections_match_rays("mirror_rooms.mmp", 1, 12, 5);
    reflections_match_rays("mirror_rooms.mmp", 3, 12, 5);
}

fn reflections_match_rays(level: &str, max_reflections: u8, views: usize, seed: u64) {
    let (world, assets) = load(level);
    let mut out = ViewGeometry::new();
    out.config.max_reflections = max_reflections;
    let mut rng = Lcg(seed);
    let regions = [
        (Vec3::new(-3.7, 0.3, 0.3), Vec3::new(3.7, 3.7, 7.7)),
        (Vec3::new(-0.7, 0.3, -11.7), Vec3::new(0.7, 2.7, -0.3)),
        (Vec3::new(-3.7, 0.3, -19.7), Vec3::new(3.7, 3.7, -12.3)),
    ];
    let (mut checked, mut reflected, mut deepest) = (0usize, 0usize, 0u8);
    for round in 0..views {
        let (lo, hi) = regions[round % 3];
        let position = Vec3::new(
            rng.range(lo.x, hi.x),
            rng.range(lo.y, hi.y),
            rng.range(lo.z, hi.z),
        );
        let (yaw, pitch, roll) = (
            rng.range(0.0, TAU),
            rng.range(-1.3, 1.3),
            rng.range(-0.5, 0.5),
        );
        let cam = camera(&world, position, yaw, pitch, roll, SMALL);
        out.build(&world, &assets, &cam.view());
        let what = format!(
            "{level} ({max_reflections} deep) view {round} at {position:?} ypr ({yaw:?}, {pitch:?}, {roll:?})"
        );
        let polys: Vec<(Outline, (u32, u32, u8))> = out
            .polygons
            .iter()
            .filter(|p| opaque_world(p))
            .map(|p| {
                let PolygonSource::World { sector, polygon } = p.source else {
                    unreachable!()
                };
                let points = out.vertices[p.vertices()]
                    .iter()
                    .map(|v| (v.x, v.y))
                    .collect();
                let depth = p.mirror.map_or(0, |m| out.mirrors[m as usize].depth);
                (
                    (points, out.edge_lines[p.vertices()].to_vec()),
                    (sector, polygon, depth),
                )
            })
            .collect();
        for y in (0..SMALL.height as i32).step_by(3) {
            for x in (0..SMALL.width as i32).step_by(3) {
                let Some(expected) = trace_pixel(&world, &assets, &cam, max_reflections, x, y)
                else {
                    continue;
                };
                let owner = polys.iter().find_map(|((points, lines), owner)| {
                    row_span(points, lines, y)
                        .filter(|&(lo, hi)| (lo..hi).contains(&x))
                        .map(|_| *owner)
                });
                assert_eq!(owner, Some(expected), "{what}: pixel ({x}, {y})");
                checked += 1;
                reflected += (expected.2 > 0) as usize;
                deepest = deepest.max(expected.2);
            }
        }
    }
    assert!(
        checked > views * 2000 && reflected > views * 200,
        "{level}: checked {checked}, reflected {reflected}"
    );
    assert_eq!(
        deepest, max_reflections,
        "{level}: no pixel was {max_reflections} bounces deep"
    );
}

#[test]
fn entities_are_seen_in_the_floor() {
    // From spawn, looking down at the shiny floor between two crates: the crates show up
    // reflected, and every reflected polygon belongs to the mirror of room_a's floor.
    let (world, assets) = load("shiny_rooms.mmp");
    let mut out = ViewGeometry::new();
    let cam = camera(&world, Vec3::new(0.0, 1.7, 6.0), 0.0, -0.6, 0.0, SMALL);
    out.build(&world, &assets, &cam.view());
    assert_invariants(&out, &cam, "looking at the floor");
    assert_world_tiles_screen(&out, SMALL, "looking at the floor");
    assert!(!out.mirrors.is_empty());
    assert!(
        out.mirrors
            .iter()
            .all(|m| m.plane.normal == Vec3::Y && m.plane.d == 0.0)
    );
    let reflected_entities = out
        .polygons
        .iter()
        .filter(|p| p.mirror.is_some() && matches!(p.source, PolygonSource::Entity { .. }))
        .count();
    assert!(reflected_entities > 0, "no crate seen in the floor");
    // Seen directly, the floor itself is drawn too (over its reflection).
    assert!(
        out.polygons
            .iter()
            .any(|p| p.flags.reflective() && p.mirror.is_none() && p.reflection.is_some())
    );
    // The same view of the plain level has no mirrors.
    let (plain, plain_assets) = load("two_rooms.mmp");
    out.build(&plain, &plain_assets, &cam.view());
    assert!(out.mirrors.is_empty());
    assert!(out.polygons.iter().all(|p| p.mirror.is_none()));
}

#[test]
fn picking_finds_the_nearest_polygon_under_the_cursor() {
    // In front of room_a's crate at (-2.5, 0, 2) (a 1 m box standing on the floor), looking
    // at it (yaw 0 faces -Z): the middle of the screen is the crate, not the wall behind.
    let (world, assets) = world();
    let cam = camera(&world, Vec3::new(-2.5, 0.5, 4.5), 0.0, 0.0, 0.0, SMALL);
    let mut out = ViewGeometry::new();
    out.build(&world, &assets, &cam.view());
    let crate_index = world
        .entities
        .iter()
        .position(|e| (e.position - Vec3::new(-2.5, 0.0, 2.0)).length() < 1e-3)
        .unwrap() as u32;
    let (source, w) = out.pick(160.0, 90.0).expect("something under the middle");
    assert!(
        matches!(source, PolygonSource::Entity { entity, .. } if entity == crate_index),
        "{source:?}"
    );
    // The crate's front face is 2 m away (it spans z 1.5..2.5).
    assert!((1.0 / w - 2.0).abs() < 0.01, "at {}", 1.0 / w);
    // High up in the corner, past the crate: the level.
    let (source, _) = out.pick(5.0, 5.0).expect("something in the corner");
    assert!(matches!(source, PolygonSource::World { .. }), "{source:?}");
}

#[test]
fn terrain_is_drawn_in_pieces_by_sector_and_blockers_hide_what_is_behind_them() {
    let (world, assets) = load("canyon_rooms.mmp");
    let terrain = &world.terrain[0];
    let mesh = assets.mesh(terrain.mesh);
    // Carved: every outdoor sector has pieces, the indoor ones none, and each piece lies
    // inside its sector.
    for (s, pieces) in terrain.sectors.iter().enumerate() {
        let name = &world.sectors[s].name;
        assert_eq!(pieces.is_empty(), name == "house" || name == "roof", "{name}");
        for p in pieces.clone() {
            for q in mesh.polygon_points(&mesh.polygons[p as usize]) {
                assert!(world.sector_contains(s as u32, q), "{name}: {q}");
            }
        }
    }

    // From the spawn point, the mesa's blocker hides the two crates behind it.
    let spawn = &world.spawn_points[0];
    let mut cam = Camera::at_spawn(spawn, SMALL);
    cam.position += Vec3::Y * 1.7;
    let mut out = ViewGeometry::new();
    out.build(&world, &assets, &cam.view());
    let entity = |name: &str| world.entities.iter().position(|e| e.name == name).unwrap() as u32;
    let drawn = |out: &ViewGeometry, e: u32| {
        out.polygons.iter().any(|p| matches!(p.source, PolygonSource::Entity { entity, .. } if entity == e))
    };
    let hidden = [entity("crate_hidden_1"), entity("crate_hidden_2")];
    assert_eq!(out.stats.entities_blocked, 2);
    assert!(hidden.iter().all(|&e| !drawn(&out, e)));
    assert!(drawn(&out, entity("crate_basin")));
    // Terrain pieces come from the sectors visited, sorted in like props, and hidden
    // floors are never drawn.
    let geometry = assets.mesh(world.geometry);
    assert!(out.stats.terrain_drawn > 0 && out.stats.terrain_blocked > 0);
    for p in &out.polygons {
        match p.source {
            PolygonSource::Terrain { sector, .. } => {
                assert_eq!(p.kind, PolygonKind::Prop);
                assert!(out.visits.iter().any(|v| v.sector == sector));
            }
            PolygonSource::World { polygon, .. } => {
                assert!(!geometry.polygons[polygon as usize].flags.hidden());
            }
            _ => {}
        }
    }

    // From the far side of the mesa, the crates are in plain view.
    let far = camera(&world, Vec3::new(70.0, 1.7, 84.0), PI, 0.0, 0.0, SMALL);
    out.build(&world, &assets, &far.view());
    assert_eq!(out.stats.entities_blocked, 0);
    assert!(hidden.iter().all(|&e| drawn(&out, e)));
}
