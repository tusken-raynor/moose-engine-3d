//! Physics for the Moose engine (see the Physics Spec): collision with a level and what is
//! placed in it, props that fall and roll, characters that walk and push them, and ray
//! casts. A wrapper over Rapier, which the rest of the engine never sees.

use glam::{Quat, Vec3};
use moose_assets::{Assets, BodyShape, EntityKind, Mesh};
use moose_scene::World;
use rapier3d::control::{CharacterAutostep, CharacterCollision, CharacterLength, KinematicCharacterController};
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::prelude::{
    Capsule, ColliderBuilder, ColliderHandle, PhysicsWorld, Pose, QueryFilter, Ray, RigidBodyBuilder,
    RigidBodyHandle, SharedShape,
};

/// The length of a physics step, in seconds: fixed, whatever the frame rate.
pub const STEP: f32 = 1.0 / 60.0;
/// The most steps one frame runs: a long hitch is dropped rather than caught up on.
const MAX_STEPS: u32 = 8;
/// The gap the character controller keeps between a character and what it touches, in
/// meters (so its sweeps never start in contact).
const SKIN: f32 = 0.02;
/// How far down, in meters, a character on the ground is brought to stay on it: down
/// steps and slopes.
const SNAP: f32 = 0.3;
/// The narrowest ledge a character steps up onto, in meters.
const STEP_WIDTH: f32 = 0.1;
/// Triangles under this area (twice it, squared, in m^4) are left out of colliders.
const NO_AREA: f32 = 1e-12;
/// Bodies' friction against what they touch, and how much of their speed they keep when
/// they bounce off it.
const FRICTION: f32 = 0.7;
const BOUNCE: f32 = 0.3;
/// How fast bodies' moving and turning die away on their own, per second: what slows a
/// rolling ball, which friction alone would let roll on. (A rolling ball loses
/// `(DRAG + 0.4 SPIN_DRAG) / 1.4` of its speed a second: 0.7 here, so it rolls 1.4 m for
/// each m/s it had.)
const DRAG: f32 = 0.3;
const SPIN_DRAG: f32 = 1.7;

/// What a collider is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Solid {
    /// The level's surfaces (and portals that aren't passable).
    Level,
    /// A terrain (index in `World::terrain`).
    Terrain(u32),
    /// A prop (index in `World::entities`).
    Entity(u32),
    /// A character.
    Character(CharacterId),
}

impl Solid {
    fn to_data(self) -> u128 {
        match self {
            Solid::Level => 0,
            Solid::Terrain(i) => 1 | (i as u128) << 8,
            Solid::Entity(i) => 2 | (i as u128) << 8,
            Solid::Character(id) => 3 | (id.0 as u128) << 8,
        }
    }

    fn from_data(data: u128) -> Solid {
        let i = (data >> 8) as u32;
        match data & 0xFF {
            1 => Solid::Terrain(i),
            2 => Solid::Entity(i),
            3 => Solid::Character(CharacterId(i)),
            _ => Solid::Level,
        }
    }
}

/// What a ray met (see [`Physics::ray`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    /// How far along the ray, in meters, and where.
    pub distance: f32,
    pub point: Vec3,
    /// The surface's normal there, facing back along the ray.
    pub normal: Vec3,
    pub solid: Solid,
}

/// A character's size and how it moves (see the Physics Spec).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterConfig {
    /// Its capsule's radius, and its height from feet to the top of the head, in meters.
    pub radius: f32,
    pub height: f32,
    /// The tallest step it walks up without jumping, in meters.
    pub step_height: f32,
    /// The steepest slope it walks up, in radians; steeper ones it slides down.
    pub max_slope: f32,
    /// Its upward speed when it jumps, in m/s.
    pub jump_speed: f32,
    /// How fast it can change its sideways velocity in the air, in m/s².
    pub air_control: f32,
    /// Its height crouched, in meters (its feet stay where they are), and how fast its
    /// height changes as it crouches and stands, in m/s.
    pub crouch_height: f32,
    pub crouch_rate: f32,
    /// What it weighs, in kilograms, when it pushes on bodies. Nothing pushes it.
    pub mass: f32,
}

impl Default for CharacterConfig {
    fn default() -> Self {
        Self {
            radius: 0.25,
            height: 1.8,
            step_height: 0.35,
            max_slope: 45f32.to_radians(),
            jump_speed: 4.5,
            air_control: 10.0,
            crouch_height: 1.0,
            crouch_rate: 4.0,
            mass: 80.0,
        }
    }
}

/// A character's handle (see [`Physics::add_character`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharacterId(u32);

/// A capsule that walks: moved each step by what it wants ([`wish`](Self::wish),
/// [`jump`](Self::jump)), gravity and what is in its way.
#[derive(Clone, Debug)]
pub struct Character {
    pub config: CharacterConfig,
    /// The velocity it wants across the ground, in m/s (its y is ignored).
    pub wish: Vec3,
    /// Whether it wants to jump: it does when it is on the ground, for as long as this
    /// is held.
    pub jump: bool,
    /// Whether it wants to crouch. It stands again when this is let go and it has the room.
    pub crouch: bool,
    /// Not there: it isn't moved, and nothing meets it (the player, while flying).
    pub ghost: bool,
    /// What bodies meet it as: a body moved to where it is each step, with its capsule.
    body: RigidBodyHandle,
    collider: ColliderHandle,
    /// Its feet as of the last step, and the step before (see [`Physics::feet`]); and its
    /// height, between its crouched and standing ones (see [`Physics::height`]).
    position: Vec3,
    previous: Vec3,
    height: f32,
    previous_height: f32,
    velocity: Vec3,
    grounded: bool,
}

impl Character {
    /// Its feet as of the last step.
    pub fn position(&self) -> Vec3 {
        self.position
    }

    pub fn velocity(&self) -> Vec3 {
        self.velocity
    }

    /// Whether it is standing on something.
    pub fn grounded(&self) -> bool {
        self.grounded
    }

    /// Its height as of the last step, in meters.
    pub fn height(&self) -> f32 {
        self.height
    }

    /// Whether it is lower than it stands: crouching, on its way down or up, or kept down
    /// by something over it.
    pub fn crouching(&self) -> bool {
        self.height < self.config.height
    }
}

/// The physics of one loaded level.
pub struct Physics {
    /// Gravity, in m/s² downward.
    pub gravity: f32,
    world: PhysicsWorld,
    characters: Vec<Character>,
    /// The props physics moves (those with a body).
    moving: Vec<Moving>,
    /// Time taken in by [`Physics::advance`] and not yet stepped, in seconds.
    pending: f32,
    /// What a character ran into in a step, to push on.
    collisions: Vec<CharacterCollision>,
}

/// A prop physics moves: its entity (index in `World::entities`), its body, and where it
/// was (position and rotation) to begin with and a step ago.
struct Moving {
    entity: u32,
    body: RigidBodyHandle,
    start: (Vec3, Quat),
    previous: (Vec3, Quat),
}

impl Physics {
    /// The colliders of a level and what is placed in it (see the Physics Spec's table):
    /// fixed in place, but for props with a body.
    pub fn new(world: &World, assets: &Assets) -> Physics {
        let mut rapier = PhysicsWorld::new();
        rapier.integration_parameters.dt = STEP;
        let mut moving = Vec::new();
        for (i, entity) in world.entities.iter().enumerate() {
            let Some(body) = entity.body.filter(|_| entity.kind == EntityKind::Prop) else {
                continue;
            };
            // Its shape, around its model's box (in the body's own space: scaled, not turned).
            let mesh = assets.mesh(entity.mesh);
            let (center, half) = (
                (mesh.bounds.min + mesh.bounds.max) * 0.5 * entity.scale,
                (mesh.bounds.max - mesh.bounds.min) * 0.5 * entity.scale,
            );
            let boxed = || ColliderBuilder::cuboid(half.x, half.y, half.z).translation(center);
            let collider = match body.shape {
                BodyShape::Ball => ColliderBuilder::ball(half.max_element()).translation(center),
                BodyShape::Box => boxed(),
                // (A model with no volume has no hull: its box, then.)
                BodyShape::Hull => {
                    let points: Vec<Vec3> = mesh.positions.iter().map(|&p| p * entity.scale).collect();
                    ColliderBuilder::convex_hull(&points).unwrap_or_else(boxed)
                }
            };
            let start = (entity.position, entity.rotation);
            let (handle, _) = rapier.insert(
                RigidBodyBuilder::dynamic()
                    .pose(Pose::from_parts(start.0, start.1))
                    .linear_damping(DRAG)
                    .angular_damping(SPIN_DRAG)
                    // Swept, so a fast one doesn't pass through a wall between steps.
                    .ccd_enabled(true),
                collider
                    .mass(body.mass)
                    .friction(FRICTION)
                    .restitution(BOUNCE)
                    .user_data(Solid::Entity(i as u32).to_data()),
            );
            moving.push(Moving { entity: i as u32, body: handle, start, previous: start });
        }
        let mut add = |vertices: Vec<Vec3>, triangles: Vec<[u32; 3]>, solid: Solid| {
            if triangles.is_empty() {
                return;
            }
            if let Ok(collider) = ColliderBuilder::trimesh(vertices, triangles) {
                rapier.insert_collider(collider.friction(FRICTION).user_data(solid.to_data()), None);
            }
        };

        // The level: every sector's surfaces, and its portals that can't be passed.
        let geometry = assets.mesh(world.geometry);
        let mut triangles = Vec::new();
        for sector in &world.sectors {
            for polygon in &geometry.polygons[sector.polygons.start as usize..sector.polygons.end as usize] {
                fan(&geometry.positions, &geometry.vertex_positions[polygon.vertices()], &mut triangles);
            }
            for portal in &world.portals[sector.portals.start as usize..sector.portals.end as usize] {
                if !portal.flags.passable() {
                    fan(&geometry.positions, &portal.positions, &mut triangles);
                }
            }
        }
        add(geometry.positions.clone(), triangles, Solid::Level);

        // Terrains' pieces are in world space already.
        for (i, terrain) in world.terrain.iter().enumerate() {
            let (vertices, triangles) = mesh_triangles(assets.mesh(terrain.mesh), |p| p);
            add(vertices, triangles, Solid::Terrain(i as u32));
        }
        for (i, entity) in world.entities.iter().enumerate() {
            if entity.kind == EntityKind::Prop && entity.body.is_none() {
                let model = entity.transform();
                let (vertices, triangles) =
                    mesh_triangles(assets.mesh(entity.mesh), |p| model.transform_point3(p));
                add(vertices, triangles, Solid::Entity(i as u32));
            }
        }

        // A step takes the colliders in, so rays and characters find them from the start.
        rapier.step();
        Physics {
            gravity: 9.81,
            world: rapier,
            characters: Vec::new(),
            moving,
            pending: 0.0,
            collisions: Vec::new(),
        }
    }

    /// The props physics moves, each as (its entity's index in `World::entities`, its
    /// position, its rotation) now: blended between the last two steps like a character's
    /// [`feet`](Self::feet). For `World::move_entity`, each frame.
    pub fn bodies(&self) -> impl Iterator<Item = (u32, Vec3, Quat)> + '_ {
        let blend = self.blend();
        self.moving.iter().map(move |m| {
            let now = self.world.bodies[m.body].position();
            let (was, turned) = m.previous;
            (m.entity, was.lerp(now.translation, blend), turned.slerp(now.rotation, blend))
        })
    }

    /// Puts every body back where the level has it, at rest.
    pub fn reset_bodies(&mut self) {
        for m in &mut self.moving {
            let body = &mut self.world.bodies[m.body];
            body.set_position(Pose::from_parts(m.start.0, m.start.1), true);
            body.set_linvel(Vec3::ZERO, true);
            body.set_angvel(Vec3::ZERO, true);
            m.previous = m.start;
        }
    }

    /// A character's capsule at `height`, and its middle over its feet.
    fn capsule(config: &CharacterConfig, height: f32) -> (Capsule, Vec3) {
        let half = (height * 0.5 - config.radius).max(0.0);
        (Capsule::new_y(half, config.radius), Vec3::Y * (half + config.radius))
    }

    /// Adds a character standing (or falling, if nothing is under it) at `feet`.
    pub fn add_character(&mut self, feet: Vec3, config: CharacterConfig) -> CharacterId {
        let id = CharacterId(self.characters.len() as u32);
        let (shape, center) = Self::capsule(&config, config.height);
        let (body, collider) = self.world.insert(
            RigidBodyBuilder::kinematic_position_based().translation(feet + center),
            ColliderBuilder::new(SharedShape::new(shape))
                .friction(FRICTION)
                .user_data(Solid::Character(id).to_data()),
        );
        self.characters.push(Character {
            config,
            wish: Vec3::ZERO,
            jump: false,
            crouch: false,
            ghost: false,
            body,
            collider,
            position: feet,
            previous: feet,
            height: config.height,
            previous_height: config.height,
            velocity: Vec3::ZERO,
            grounded: false,
        });
        id
    }

    pub fn character(&self, id: CharacterId) -> &Character {
        &self.characters[id.0 as usize]
    }

    pub fn character_mut(&mut self, id: CharacterId) -> &mut Character {
        &mut self.characters[id.0 as usize]
    }

    /// Puts a character at `feet`, at rest and standing.
    pub fn teleport(&mut self, id: CharacterId, feet: Vec3) {
        let c = &mut self.characters[id.0 as usize];
        (c.position, c.previous) = (feet, feet);
        (c.height, c.previous_height) = (c.config.height, c.config.height);
        let (shape, center) = Self::capsule(&c.config, c.height);
        self.world.bodies[c.body].set_position(Pose::from_translation(feet + center), true);
        self.world.colliders[c.collider].set_shape(SharedShape::new(shape));
        c.velocity = Vec3::ZERO;
        c.grounded = false;
    }

    /// A character's feet now: blended between the last two steps by how far into the next
    /// the time taken in is, so it moves smoothly at any frame rate.
    pub fn feet(&self, id: CharacterId) -> Vec3 {
        let c = &self.characters[id.0 as usize];
        c.previous.lerp(c.position, self.blend())
    }

    /// A character's height now, blended like its [`feet`](Self::feet).
    pub fn height(&self, id: CharacterId) -> f32 {
        let c = &self.characters[id.0 as usize];
        c.previous_height + (c.height - c.previous_height) * self.blend()
    }

    /// How far into the next step the time taken in is (0 to 1).
    fn blend(&self) -> f32 {
        (self.pending / STEP).clamp(0.0, 1.0)
    }

    /// Takes in a frame of `dt` seconds and runs the steps it brings due (at most
    /// `MAX_STEPS`; the rest of a longer frame is dropped). Returns how many.
    pub fn advance(&mut self, dt: f32) -> u32 {
        self.pending += dt.max(0.0);
        let mut steps = 0;
        while self.pending >= STEP {
            self.pending -= STEP;
            if steps < MAX_STEPS {
                self.step();
                steps += 1;
            }
        }
        steps
    }

    /// One step: the characters, then the world.
    fn step(&mut self) {
        let gravity = self.gravity;
        self.world.gravity = Vec3::new(0.0, -gravity, 0.0);
        for m in &mut self.moving {
            let pose = self.world.bodies[m.body].position();
            m.previous = (pose.translation, pose.rotation);
        }
        for c in &mut self.characters {
            let config = c.config;
            self.world.colliders[c.collider].set_enabled(!c.ghost);
            if c.ghost {
                continue;
            }
            // What it meets: everything but itself.
            let filter = QueryFilter::default().exclude_rigid_body(c.body);
            let queries = self.world.query_pipeline_with_filter(filter);
            // Crouching and standing: its height goes toward the one it wants, its feet
            // staying put. Standing, only as far as there is room over it (its capsule is
            // swept up to where it would reach).
            c.previous_height = c.height;
            let wanted = if c.crouch { config.crouch_height.min(config.height) } else { config.height };
            let change = config.crouch_rate * STEP;
            if c.height > wanted {
                c.height = (c.height - change).max(wanted);
            } else if c.height < wanted {
                let grow = change.min(wanted - c.height);
                let (shape, center) = Self::capsule(&config, c.height);
                let room = queries.cast_shape(
                    &Pose::from_translation(c.position + center),
                    Vec3::Y,
                    &shape,
                    ShapeCastOptions {
                        max_time_of_impact: grow,
                        target_distance: SKIN,
                        stop_at_penetration: false,
                        compute_impact_geometry_on_penetration: false,
                    },
                );
                c.height += room.map_or(grow, |(_, hit)| hit.time_of_impact.clamp(0.0, grow));
            }

            let wish = Vec3::new(c.wish.x, 0.0, c.wish.z);
            let mut velocity = c.velocity;
            if c.grounded {
                // On the ground it moves as it wishes, at once, and may take off.
                velocity = Vec3::new(wish.x, if c.jump { config.jump_speed } else { 0.0 }, wish.z);
            } else {
                let sideways = Vec3::new(velocity.x, 0.0, velocity.z);
                let change = (wish - sideways).clamp_length_max(config.air_control * STEP);
                velocity += change;
            }
            velocity.y -= gravity * STEP;

            let (shape, center) = Self::capsule(&config, c.height);
            let jumping = velocity.y > 0.0;
            let controller = KinematicCharacterController {
                offset: CharacterLength::Absolute(SKIN),
                autostep: Some(CharacterAutostep {
                    max_height: CharacterLength::Absolute(config.step_height),
                    min_width: CharacterLength::Absolute(STEP_WIDTH),
                    include_dynamic_bodies: false,
                }),
                max_slope_climb_angle: config.max_slope,
                min_slope_slide_angle: config.max_slope,
                // Not while it rises: a jump would be pulled straight back down.
                snap_to_ground: (!jumping).then_some(CharacterLength::Absolute(SNAP)),
                ..Default::default()
            };
            let wanted = velocity * STEP;
            self.collisions.clear();
            let moved = controller.move_shape(
                STEP,
                &queries,
                &shape,
                &Pose::from_translation(c.position + center),
                wanted,
                |collision| self.collisions.push(collision),
            );
            c.previous = c.position;
            c.position += moved.translation;
            c.grounded = moved.grounded && !jumping;
            // Landing ends its fall; hitting its head ends its rise.
            if c.grounded || (jumping && moved.translation.y < wanted.y * 0.5) {
                velocity.y = 0.0;
            }
            c.velocity = velocity;

            // It pushes on the bodies it ran into, as one mass meeting another without a
            // bounce: each is brought toward the speed it came at them with (along the way
            // it met them), by its share of their two masses. Through their middles, so
            // they are shoved, not spun: the ground under them sets them rolling.
            let coming = wanted / STEP;
            for collision in &self.collisions {
                let Some(body) = self.world.colliders[collision.handle].parent() else { continue };
                let body = &mut self.world.bodies[body];
                if !body.is_dynamic() {
                    continue;
                }
                // The way it met them: the hit's normal, turned to the way it was going.
                let going = collision.translation_applied + collision.translation_remaining;
                let normal = collision.hit.normal1;
                let toward = if normal.dot(going) < 0.0 { -normal } else { normal };
                let closing = (coming - body.linvel()).dot(toward);
                if closing > 0.0 {
                    let share = config.mass / (config.mass + body.mass());
                    body.apply_impulse(toward * (closing * share * body.mass()), true);
                }
            }
            // And its own body goes where it went, for bodies to meet.
            self.world.bodies[c.body].set_next_kinematic_translation(c.position + center);
            if c.height != c.previous_height {
                self.world.colliders[c.collider].set_shape(SharedShape::new(shape));
            }
        }
        self.world.step();
    }

    /// The first thing a ray from `from` along `direction` meets within `max` meters,
    /// passing through character `skip` (whose ray it is), if any.
    pub fn ray(&self, from: Vec3, direction: Vec3, max: f32, skip: Option<CharacterId>) -> Option<RayHit> {
        let direction = direction.try_normalize()?;
        let ray = Ray::new(from, direction);
        let filter = match skip {
            Some(id) => QueryFilter::default().exclude_rigid_body(self.characters[id.0 as usize].body),
            None => QueryFilter::default(),
        };
        let (handle, hit) = self.world.cast_ray_and_get_normal(&ray, max, true, filter)?;
        Some(RayHit {
            distance: hit.time_of_impact,
            point: from + direction * hit.time_of_impact,
            normal: hit.normal,
            solid: Solid::from_data(self.world.colliders.get(handle)?.user_data),
        })
    }
}

/// Fans a polygon (its corners, as indices into `positions`) into triangles from its first
/// corner, leaving out those with no area (a corner on an edge).
fn fan(positions: &[Vec3], corners: &[u32], triangles: &mut Vec<[u32; 3]>) {
    for k in 1..corners.len().saturating_sub(1) {
        let t = [corners[0], corners[k], corners[k + 1]];
        let [a, b, c] = t.map(|i| positions[i as usize]);
        if (b - a).cross(c - a).length_squared() > NO_AREA {
            triangles.push(t);
        }
    }
}

/// A mesh's positions, each through `place`, and its polygons as triangles.
fn mesh_triangles(mesh: &Mesh, place: impl Fn(Vec3) -> Vec3) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let vertices: Vec<Vec3> = mesh.positions.iter().map(|&p| place(p)).collect();
    let mut triangles = Vec::new();
    for polygon in &mesh.polygons {
        fan(&vertices, &mesh.vertex_positions[polygon.vertices()], &mut triangles);
    }
    (vertices, triangles)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `steps` steps.
    fn run(physics: &mut Physics, steps: u32) {
        for _ in 0..steps {
            physics.advance(STEP);
        }
    }

    fn level(name: &str) -> (World, Assets) {
        let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let level = assets.load_level(name).unwrap();
        (World::new(level, &assets), assets)
    }

    #[test]
    fn a_ray_down_from_a_spawn_point_meets_the_floor_under_it() {
        let (world, assets) = level("two_rooms.mmp");
        let physics = Physics::new(&world, &assets);
        let spawn = world.spawn_points[0].position;
        let hit = physics.ray(spawn + Vec3::Y, Vec3::NEG_Y, 10.0, None).unwrap();
        assert!((hit.distance - 1.0).abs() < 1e-3, "{hit:?}");
        assert!(hit.normal.distance(Vec3::Y) < 1e-3, "{hit:?}");
        assert_eq!(hit.solid, Solid::Level);
    }

    #[test]
    fn a_character_dropped_over_the_floor_lands_and_stands_on_it() {
        let (world, assets) = level("two_rooms.mmp");
        let mut physics = Physics::new(&world, &assets);
        let spawn = world.spawn_points[0].position;
        let id = physics.add_character(spawn + Vec3::Y, CharacterConfig::default());
        for _ in 0..12 {
            physics.advance(STEP);
        }
        assert!(!physics.character(id).grounded(), "still falling after 0.2 s");
        for _ in 0..108 {
            physics.advance(STEP);
        }
        let c = physics.character(id);
        assert!(c.grounded());
        assert!((c.position().y - spawn.y).abs() < 0.05, "{} over {}", c.position().y, spawn.y);
        assert_eq!(c.velocity(), Vec3::ZERO);
    }

    #[test]
    fn a_character_stands_on_terrain_and_walks_up_and_down_it() {
        let (world, assets) = level("canyon_rooms.mmp");
        let mut physics = Physics::new(&world, &assets);
        let spawn = world.spawn_points[0].position;
        let ground = |physics: &Physics, at: Vec3| {
            let hit = physics.ray(at + Vec3::Y * 2.0, Vec3::NEG_Y, 50.0, Some(CharacterId(0))).unwrap();
            assert!(matches!(hit.solid, Solid::Terrain(_)), "{hit:?}");
            hit.point.y
        };
        let id = physics.add_character(spawn + Vec3::Y, CharacterConfig::default());
        for k in 0..600 {
            // Standing for a second, then walking across it, turning as it goes.
            let angle = k as f32 * 0.01;
            physics.character_mut(id).wish =
                if k < 60 { Vec3::ZERO } else { Vec3::new(angle.cos(), 0.0, angle.sin()) * 3.0 };
            physics.advance(STEP);
            if k >= 59 {
                let c = physics.character(id);
                let under = ground(&physics, c.position());
                assert!(c.grounded(), "off the ground at step {k}: {}", c.position());
                // (Its round foot touches a slope, or a ridge it is going over, to one side,
                // so its lowest point is over the ground under its middle: by less than its
                // radius. Never under it.)
                let over = c.position().y - under;
                assert!((-0.03..0.28).contains(&over), "step {k}: feet at {} over ground at {under}", c.position().y);
            }
        }
        assert!(physics.character(id).position().distance(spawn) > 3.0);
    }

    #[test]
    fn a_character_walking_at_a_wall_stops_at_it_inside_the_level() {
        let (world, assets) = level("two_rooms.mmp");
        let mut physics = Physics::new(&world, &assets);
        let spawn = world.spawn_points[0].position;
        for wish in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
            let id = physics.add_character(spawn, CharacterConfig::default());
            physics.character_mut(id).wish = wish * 5.0;
            for _ in 0..600 {
                physics.advance(STEP);
            }
            let c = physics.character(id);
            let chest = c.position() + Vec3::Y;
            assert!(world.find_sector(chest).is_some(), "walked out of the level toward {wish}: {chest}");
            assert!(c.grounded());
            physics.character_mut(id).wish = Vec3::ZERO;
        }
    }

    #[test]
    fn a_jump_leaves_the_ground_and_comes_back_to_it() {
        let (world, assets) = level("two_rooms.mmp");
        let mut physics = Physics::new(&world, &assets);
        let spawn = world.spawn_points[0].position;
        let id = physics.add_character(spawn, CharacterConfig::default());
        for _ in 0..30 {
            physics.advance(STEP);
        }
        assert!(physics.character(id).grounded());
        physics.character_mut(id).jump = true;
        physics.advance(STEP * 1.5);
        physics.character_mut(id).jump = false;
        let mut top = 0.0f32;
        for _ in 0..120 {
            physics.advance(STEP);
            top = top.max(physics.character(id).position().y - spawn.y);
        }
        // About jump_speed² / 2g = 1.03 m.
        assert!((0.8..1.2).contains(&top), "jumped {top} m");
        assert!(physics.character(id).grounded());
    }

    #[test]
    fn a_character_crouches_to_its_crouched_height_and_stands_again() {
        let (world, assets) = level("two_rooms.mmp");
        let mut physics = Physics::new(&world, &assets);
        let spawn = world.spawn_points[0].position;
        let config = CharacterConfig::default();
        let id = physics.add_character(spawn, config);
        run(&mut physics, 30);
        assert!(!physics.character(id).crouching());
        physics.character_mut(id).crouch = true;
        run(&mut physics, 3);
        // On its way down: 4 m/s, a step at a time; its feet stay on the floor.
        let c = physics.character(id);
        assert!(c.crouching() && (c.height() - (config.height - 3.0 * 4.0 * STEP)).abs() < 1e-4, "{}", c.height());
        run(&mut physics, 30);
        let c = physics.character(id);
        assert_eq!(c.height(), config.crouch_height);
        assert!(c.grounded() && (c.position().y - spawn.y).abs() < 0.05);
        physics.character_mut(id).crouch = false;
        run(&mut physics, 30);
        let c = physics.character(id);
        assert_eq!(c.height(), config.height);
        assert!(!c.crouching() && c.grounded() && (c.position().y - spawn.y).abs() < 0.05);
    }

    #[test]
    fn a_character_stands_only_as_far_as_there_is_room_over_it() {
        // A giant, crouched from the start, in a room it is too tall for: let go, it
        // stands up to the ceiling and no further.
        let (world, assets) = level("two_rooms.mmp");
        let mut physics = Physics::new(&world, &assets);
        let spawn = world.spawn_points[0].position;
        let ceiling = physics.ray(spawn + Vec3::Y, Vec3::Y, 100.0, None).unwrap().point.y - spawn.y;
        let config = CharacterConfig { height: ceiling + 5.0, ..Default::default() };
        let id = physics.add_character(spawn, config);
        physics.character_mut(id).crouch = true;
        run(&mut physics, 600);
        assert_eq!(physics.character(id).height(), config.crouch_height);
        physics.character_mut(id).crouch = false;
        run(&mut physics, 600);
        let c = physics.character(id);
        let top = c.position().y + c.height() - spawn.y;
        assert!(c.crouching(), "it stood up through the ceiling: {} tall under {ceiling}", c.height());
        assert!(top < ceiling && top > ceiling - 0.1, "its head is at {top} under a ceiling at {ceiling}");
    }

    #[test]
    fn frames_of_any_length_run_the_same_steps() {
        let (world, assets) = level("two_rooms.mmp");
        let run = |frame: f32| {
            let mut physics = Physics::new(&world, &assets);
            let id = physics.add_character(world.spawn_points[0].position + Vec3::Y, CharacterConfig::default());
            physics.character_mut(id).wish = Vec3::new(1.0, 0.0, 0.5);
            let mut steps = 0;
            while steps < 60 {
                steps += physics.advance(frame);
            }
            assert_eq!(steps, 60);
            physics.character(id).position()
        };
        let at_60 = run(STEP);
        assert_eq!(run(STEP / 3.0 * 1.0001), at_60);
        assert_eq!(run(STEP * 2.0), at_60);
    }
}
