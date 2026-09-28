use std::f32::consts::FRAC_PI_2;

use glam::{Vec2, Vec3};
use moose_assets::{Assets, EntityKind};
use moose_scene::{Camera, Viewport, World};

const EYE: f32 = 1.7;
const FULL: Viewport = Viewport {
    x: 0,
    y: 0,
    width: 1280,
    height: 720,
};

fn world() -> (World, Assets) {
    let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    let level = assets.load_level("two_rooms.mmp").unwrap();
    (World::new(level, &assets), assets)
}

fn close(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-4
}

#[test]
fn world_from_level() {
    let (world, assets) = world();
    assert_eq!(world.name, "Two Rooms");
    assert_eq!(world.sectors.len(), 3);
    assert_eq!(world.spawn_points.len(), 1);
    assert_eq!(world.spawn_points[0].name, "player_start");
    assert_eq!(world.entities.len(), 6);
    assert!(world.entities.iter().all(|e| e.kind == EntityKind::Prop));
    let stacked = world
        .entities
        .iter()
        .find(|e| e.name == "crate_a2_stacked")
        .unwrap();
    // The crate's top-front-right corner, turned 15 degrees and lifted onto the crate below.
    let corner = stacked
        .transform()
        .transform_point3(Vec3::new(0.5, 1.0, 0.5));
    let turned = glam::Quat::from_rotation_y(15f32.to_radians()) * Vec3::new(0.5, 1.0, 0.5);
    assert!(close(corner, Vec3::new(-2.5, 1.0, 2.0) + turned));
    assert_eq!(assets.mesh(stacked.mesh).name, "crate.obj");
}

#[test]
fn sector_lookup() {
    let (world, _) = world();
    assert_eq!(world.find_sector(Vec3::new(0.0, EYE, 6.0)), Some(0));
    assert_eq!(world.find_sector(Vec3::new(0.0, EYE, -6.0)), Some(1));
    assert_eq!(world.find_sector(Vec3::new(3.0, EYE, -16.0)), Some(2));
    assert_eq!(world.find_sector(Vec3::new(3.0, EYE, -6.0)), None); // beside the hallway, in the rock
    assert_eq!(world.find_sector(Vec3::new(0.0, 5.0, 6.0)), None); // above room_a's ceiling
}

#[test]
fn walk_from_room_a_to_room_b() {
    let (world, _) = world();
    let mut camera = Camera::at_spawn(&world.spawn_points[0], FULL);
    camera.position.y = EYE;
    assert!(
        close(camera.forward(), Vec3::NEG_Z),
        "spawn faces the hallway"
    );

    // Walk forward in 0.5 m steps and record every sector change.
    let mut visited = vec![camera.sector];
    for _ in 0..44 {
        assert!(
            !camera.move_local(&world, Vec3::new(0.0, 0.0, -0.5)),
            "blocked at {}",
            camera.position
        );
        if *visited.last().unwrap() != camera.sector {
            visited.push(camera.sector);
        }
        assert_eq!(
            world.find_sector(camera.position),
            Some(camera.sector),
            "at {}",
            camera.position
        );
    }
    assert_eq!(visited, [0, 1, 2]);
    // Steps landing exactly on a portal plane are held 1 mm back, so allow a few mm.
    assert!(
        (camera.position - Vec3::new(0.0, EYE, -16.0)).length()
            < 3.0 * moose_scene::PORTAL_CLEARANCE
    );

    // One long move crosses both portals at once.
    let trace = world.trace(0, Vec3::new(0.0, EYE, 6.0), Vec3::new(0.5, EYE, -19.0));
    assert_eq!((trace.sector, trace.blocked), (2, false));
}

#[test]
fn solid_surfaces_block() {
    let (world, _) = world();
    // Into room_a's back wall.
    let t = world.trace(0, Vec3::new(0.0, EYE, 6.0), Vec3::new(0.0, EYE, 12.0));
    assert!(t.blocked && t.sector == 0 && t.position.z < 8.0 && t.position.z > 7.99);
    // Into the doorway wall's pillar, beside the portal: the plane is shared, the polygon isn't.
    let t = world.trace(0, Vec3::new(-3.0, EYE, 6.0), Vec3::new(-3.0, EYE, -6.0));
    assert!(t.blocked && t.sector == 0 && t.position.z > 0.0 && t.position.z < 0.01);
    // Into the lintel above the door (the portal is 3 m tall, the room 4 m).
    let t = world.trace(0, Vec3::new(0.0, 3.5, 6.0), Vec3::new(0.0, 3.5, -6.0));
    assert!(t.blocked && t.sector == 0);
    // Diagonally into the hallway's side wall.
    let t = world.trace(1, Vec3::new(0.0, EYE, -2.0), Vec3::new(4.0, EYE, -6.0));
    assert!(t.blocked && t.sector == 1 && t.position.x < 1.0 && t.position.x > 0.99);
    assert_eq!(world.find_sector(t.position), Some(1));
}

#[test]
fn orientation_conventions() {
    let (world, _) = world();
    let mut c = Camera::at_spawn(&world.spawn_points[0], FULL);
    c.yaw = FRAC_PI_2;
    assert!(close(c.forward(), Vec3::NEG_X), "positive yaw turns left");
    c.yaw = 0.0;
    c.pitch = 0.3;
    assert!(c.forward().y > 0.0, "positive pitch looks up");
    c.pitch = 0.0;
    c.roll = 0.3;
    assert!(
        c.right().y > 0.0 && close(c.forward(), Vec3::NEG_Z),
        "positive roll lifts the right side"
    );
    let (f, r, u) = (c.forward(), c.right(), c.up());
    assert!(close(r.cross(u), -f), "right-handed basis");
}

#[test]
fn projection() {
    let (world, _) = world();
    let mut c = Camera::at_spawn(&world.spawn_points[0], FULL);
    c.position = Vec3::new(0.0, EYE, 6.0);
    let view = c.view();

    // Straight ahead lands on the viewport center, with w = 1 / distance.
    let (p, w) = view.project(Vec3::new(0.0, EYE, 4.0)).unwrap();
    assert!((p - Vec2::new(640.0, 360.0)).length() < 1e-3 && (w - 0.5).abs() < 1e-6);
    // Up and to the right lands up and to the right (screen y grows downward).
    let (p, _) = view.project(Vec3::new(1.0, EYE + 1.0, 4.0)).unwrap();
    assert!(p.x > 640.0 && p.y < 360.0);
    // Half the vertical field of view above the axis lands on the top edge.
    let (p, _) = view.project(Vec3::new(0.0, EYE + 2.0, 4.0)).unwrap(); // 45 degrees up
    assert!(p.y.abs() < 1e-3);
    // Behind the camera, or closer than the near plane: not projectable.
    assert!(view.project(Vec3::new(0.0, EYE, 8.0)).is_none());
    assert!(
        view.project(Vec3::new(0.0, EYE, 6.0 - c.near / 2.0))
            .is_none()
    );

    // The camera sits at the view-space origin.
    assert!(close(
        view.world_to_view.transform_point3(c.position),
        Vec3::ZERO
    ));
}

#[test]
fn splitscreen_views_are_independent() {
    let (world, _) = world();
    let top = Viewport {
        x: 0,
        y: 0,
        width: 1280,
        height: 360,
    };
    let bottom = Viewport {
        x: 0,
        y: 360,
        width: 1280,
        height: 360,
    };
    let mut one = Camera::at_spawn(&world.spawn_points[0], top);
    let mut two = Camera::at_spawn(&world.spawn_points[0], bottom);
    one.position.y = EYE;
    two.position.y = EYE;
    two.move_local(&world, Vec3::new(0.0, 0.0, -10.0));

    let (a, b) = (one.view(), two.view());
    assert_eq!((a.sector, b.sector), (0, 1));
    assert_eq!(
        (a.center, b.center),
        (Vec2::new(640.0, 180.0), Vec2::new(640.0, 540.0))
    );
    // Straight ahead lands on each viewport's own center.
    let ahead = |c: &Camera| c.position + c.forward() * 5.0;
    assert!((a.project(ahead(&one)).unwrap().0 - a.center).length() < 1e-3);
    assert!((b.project(ahead(&two)).unwrap().0 - b.center).length() < 1e-3);
    // Fixed vertical field of view: 45 degrees up reaches each viewport's top edge,
    // and because the views are wide, 45 degrees right is still well inside.
    let up45 = one.position + Vec3::new(0.0, 5.0, -5.0);
    assert!((a.project(up45).unwrap().0.y - 0.0).abs() < 1e-3);
    let right45 = one.position + Vec3::new(5.0, 0.0, -5.0);
    let x = a.project(right45).unwrap().0.x;
    assert!((x - (640.0 + 180.0)).abs() < 1e-3 && x < 1280.0);
}

#[test]
fn impassable_portal_blocks_movement() {
    // Same level with every adjoin render-through only (0x1), not passable.
    let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    let src = include_str!("../../../assets/levels/two_rooms.mmp");
    let src = src.replace("       0x3\n", "       0x1\n");
    assert_eq!(src.matches("       0x1\n").count(), 4);
    let world = World::new(assets.parse_level("two_rooms.mmp", &src).unwrap(), &assets);
    let t = world.trace(0, Vec3::new(0.0, EYE, 6.0), Vec3::new(0.0, EYE, -6.0));
    assert!(t.blocked && t.sector == 0 && t.position.z > 0.0, "{t:?}");
}

#[test]
fn moves_keep_clear_of_portal_planes() {
    let (world, _) = world();
    // Heading into room_a's doorway portal (z = 0) and stopping on either side of it within
    // 1 mm: carried through to 1 mm inside the hallway.
    for end in [0.0002, 0.0, -0.0002] {
        let t = world.trace(0, Vec3::new(0.0, EYE, 6.0), Vec3::new(0.0, EYE, end));
        assert_eq!(t.sector, 1, "{t:?}");
        assert!(
            (t.position.z + moose_scene::PORTAL_CLEARANCE).abs() < 1e-6,
            "{t:?}"
        );
    }
    // Backing away from the portal and stopping within 1 mm of it: held back in the hallway.
    let t = world.trace(
        1,
        Vec3::new(0.0, EYE, -0.0005),
        Vec3::new(0.0, EYE, -0.0007),
    );
    assert_eq!(t.sector, 1);
    assert!(
        (t.position.z + moose_scene::PORTAL_CLEARANCE).abs() < 1e-6,
        "{t:?}"
    );
    // Creeping toward the door in steps smaller than the clearance still gets through.
    let mut cam = Camera::at_spawn(&world.spawn_points[0], FULL);
    cam.position = Vec3::new(0.0, EYE, 0.01);
    for _ in 0..40 {
        cam.move_to(&world, cam.position + Vec3::new(0.0, 0.0, -0.0004));
    }
    assert_eq!(cam.sector, 1, "stuck at {}", cam.position);
    // Beside the door, the same plane is a solid wall: no push.
    let t = world.trace(0, Vec3::new(-3.0, EYE, 6.0), Vec3::new(-3.0, EYE, 0.0005));
    assert!((t.position.z - 0.0005).abs() < 1e-6, "{t:?}");
}

#[test]
fn entities_list_every_sector_they_touch() {
    let (mut world, assets) = world();
    let i = world
        .entities
        .iter()
        .position(|e| e.name == "crate_hall")
        .unwrap();
    assert_eq!(world.entities[i].sectors, [1]);
    let b = world.entities[i].bounds;
    assert!(
        b.min.y == 0.0 && b.max.y == 1.0 && b.max.x - b.min.x > 1.0,
        "rotated box is wider: {b:?}"
    );
    // Push it into the doorway so it pokes 0.2 m into room_a.
    world.entities[i].position = Vec3::new(0.0, 0.0, -0.3);
    world.entities[i].rotation = glam::Quat::IDENTITY;
    world.place_entity(i, &assets);
    assert_eq!(world.entities[i].sectors, [1, 0]);
}

#[test]
fn keep_clear_pushes_off_walls_but_not_through_doorways() {
    let (world, _) = world();
    // 5 cm from room_a's back wall (z = 8): pushed back to 0.25 m.
    let p = world.keep_clear(0, Vec3::new(0.0, EYE, 7.95), 0.25);
    assert!((p.z - 7.75).abs() < 1e-4, "{p}");
    // In the middle of the doorway (z = 0, x = 0): the opening is a portal, the frame is
    // 1 m away, so nothing moves.
    let p = world.keep_clear(0, Vec3::new(0.0, EYE, 0.05), 0.25);
    assert!((p - Vec3::new(0.0, EYE, 0.05)).length() < 1e-6, "{p}");
    // Brushing the door jamb (x = 1): pushed back toward the middle of the opening.
    let p = world.keep_clear(0, Vec3::new(0.9, EYE, 0.05), 0.25);
    let jamb = Vec3::new(1.0, EYE, 0.0);
    assert!(
        ((p - jamb).length() - 0.25).abs() < 1e-4 && p.x < 0.9,
        "{p}"
    );
}

#[test]
fn lights_reach_sectors_through_openings_in_range() {
    use moose_assets::Light;
    let (mut world, _) = world();
    let light = |sector: u32, position: Vec3, range: f32| {
        Light::point(sector, position, Vec3::ONE, range)
    };
    world.set_lights(
        vec![
            // In room_a, 2 m from the hallway's doorway (z = 0): it reaches the hallway,
            // but not room_b beyond it (whose doorway is 14 m away).
            light(0, Vec3::new(0.0, 1.5, 2.0), 5.0),
            // The same, too short to reach the doorway.
            light(0, Vec3::new(0.0, 1.5, 2.0), 1.5),
            // Beside the doorway but behind the wall (x = 3, z = 0.5): the opening, 2 m to
            // the side, is still in range, so it passes through.
            light(0, Vec3::new(3.0, 1.5, 0.5), 2.5),
            // Long range from the hallway's middle: both rooms.
            light(1, Vec3::new(0.0, 1.5, -6.0), 7.0),
            // A spot light by the doorway, in range of it, but aimed away from it (at the far
            // wall, +Z): its cone never reaches the opening.
            Light::spot(
                0,
                Vec3::new(0.0, 1.5, 2.0),
                Vec3::ONE,
                10.0,
                Vec3::Z,
                10.0,
                20.0,
            ),
            // The same spot light aimed at the doorway: it reaches the hallway.
            Light::spot(
                0,
                Vec3::new(0.0, 1.5, 2.0),
                Vec3::ONE,
                10.0,
                Vec3::NEG_Z,
                10.0,
                20.0,
            ),
        ],
        Vec3::splat(0.1),
    );
    assert_eq!(world.ambient, Vec3::splat(0.1));
    assert_eq!(world.lights().len(), 6);
    assert_eq!(world.sector_lights(0), [0, 1, 2, 3, 4, 5]);
    assert_eq!(world.sector_lights(1), [0, 2, 3, 5]);
    assert_eq!(world.sector_lights(2), [3]); // room_b's doorway is 14 m from the spot
}
