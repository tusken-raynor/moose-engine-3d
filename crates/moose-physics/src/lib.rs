//! Physics for the Moose engine (see the Physics Spec): collision with a level and what is
//! placed in it, props that fall and roll, props that go where they are put (by scripts)
//! and push and carry what they meet, characters that walk and push, what touched what,
//! and ray casts. A wrapper over Rapier, which the rest of the engine never sees.

use std::sync::Mutex;

use glam::{Quat, Vec3};
use moose_assets::{Assets, BodyShape, EntityKind, Mesh};
use moose_scene::World;
use rapier3d::control::{CharacterAutostep, CharacterCollision, CharacterLength, KinematicCharacterController};
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::prelude::{
    ActiveEvents, Capsule, Collider, ColliderBuilder, ColliderHandle, ColliderSet, CollisionEvent, ContactPair,
    EventHandler, InteractionGroups, PhysicsWorld, Pose, QueryFilter, Ray, RigidBodyBuilder, RigidBodyHandle, RigidBodySet,
    RigidBodyType, SharedShape, SoftBodySet, SoftBodyTearEvent,
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
/// How many steps a character is still taken to be touching what it last ran into, so
/// that leaning on something (which it meets on some steps and not others) is one touch.
const STILL_TOUCHING: u8 = 15;
/// How far under a character's feet what it stands on is looked for, in meters.
const UNDERFOOT: f32 = 0.08;
/// How near a character is to something to be against it (touching it), in meters: a
/// little over the gap it keeps from what it walks into.
const AGAINST: f32 = 0.08;

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

/// Marks (in a collider's data) one that only bodies meet: characters meet its entity's
/// other collider instead (see [`Body::seen`]).
const BODIES_ONLY: u128 = 1 << 127;

/// Whether characters meet a collider (and what they look for under and around them finds it).
fn for_characters(_: ColliderHandle, collider: &Collider) -> bool {
    collider.user_data & BODIES_ONLY == 0
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

/// Two things that began to touch in a step (see [`Physics::take_touches`]). One of them
/// at least is an entity with a body, or a character.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Touch {
    pub a: Solid,
    pub b: Solid,
    /// How fast they were coming together, in m/s (for a character and what it walked
    /// into or landed on, how fast the character was going).
    pub speed: f32,
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
    /// The body it stands on (index in `Physics::bodies`), if it stands on one.
    standing: Option<u32>,
    /// The entities it ran into lately, each with the steps left until it no longer counts
    /// as touching it.
    touching: Vec<(Solid, u8)>,
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
    /// The props with a body: every prop but the static ones.
    bodies: Vec<Body>,
    /// Per entity, its body (index in `bodies`), or `NO_BODY`.
    body_of: Vec<u32>,
    /// Time taken in by [`Physics::advance`] and not yet stepped, in seconds.
    pending: f32,
    /// What a character ran into in a step, to push on.
    collisions: Vec<CharacterCollision>,
    /// What began to touch since [`Physics::take_touches`] was last asked.
    touches: Vec<Touch>,
}

const NO_BODY: u32 = u32::MAX;

/// A prop's body. One with a mass is **dynamic**: physics moves it, and the game moves its
/// entity after it. One without goes only where it is put ([`Physics::put`]): the game
/// moves its entity, and its body follows, pushing what it meets. A dynamic one that is
/// [held](Physics::hold) does the same until it is let go.
struct Body {
    /// Its entity (index in `World::entities`).
    entity: u32,
    handle: RigidBodyHandle,
    dynamic: bool,
    held: bool,
    /// Where it was (position and rotation) to begin with, and a step ago.
    start: (Vec3, Quat),
    previous: (Vec3, Quat),
    /// Where it has been put and hasn't got to yet: it goes there over the steps of the
    /// next frame, so what it meets is pushed, not passed through.
    target: Option<(Vec3, Quat)>,
    /// How it was moved in the last step, if it was put somewhere: from where to where.
    moved: Option<(Pose, Pose)>,
    /// For one with no mass, the collider characters meet it by: its model again, on no
    /// body, moved with it. Its body is one that goes where it is told (kinematic), which
    /// is what makes it push bodies properly; but Rapier's character controller holds a
    /// character to anything of that kind it touches, so that it can't walk away from a
    /// crate it leaned on. Characters meet this plain collider instead, bodies never do,
    /// and [`Physics::carry`] does the carrying.
    seen: Option<ColliderHandle>,
}

impl Body {
    /// Whether it goes where it is put, rather than where physics takes it.
    fn put(&self) -> bool {
        !self.dynamic || self.held
    }
}

/// Collects what began to touch in a step: the two colliders, and how fast their bodies
/// were closing.
struct Collector(Mutex<Vec<(ColliderHandle, ColliderHandle, f32)>>);

impl EventHandler for Collector {
    fn handle_collision_event(
        &self,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        event: CollisionEvent,
        _: Option<&ContactPair>,
    ) {
        if let CollisionEvent::Started(a, b, _) = event {
            let velocity = |handle: ColliderHandle| {
                let body = colliders.get(handle).and_then(|c| c.parent()).and_then(|b| bodies.get(b));
                body.map_or(Vec3::ZERO, |b| b.linvel())
            };
            self.0.lock().unwrap().push((a, b, (velocity(a) - velocity(b)).length()));
        }
    }

    fn handle_contact_force_event(&self, _: f32, _: &RigidBodySet, _: &ColliderSet, _: &ContactPair, _: f32) {}

    fn handle_soft_body_tear_event(&self, _: &SoftBodySet, _: &SoftBodyTearEvent) {}
}

impl Physics {
    /// The colliders of a level and what is placed in it (see the Physics Spec's table):
    /// the level, terrain and static props fixed in place, and a body for every other prop.
    pub fn new(world: &World, assets: &Assets) -> Physics {
        let mut rapier = PhysicsWorld::new();
        rapier.integration_parameters.dt = STEP;
        let mut bodies = Vec::new();
        let mut body_of = vec![NO_BODY; world.entities.len()];
        let new_body = |entity: usize, handle, dynamic, start, seen| Body {
            entity: entity as u32,
            handle,
            dynamic,
            held: false,
            start,
            previous: start,
            target: None,
            moved: None,
            seen,
        };
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
                    .active_events(ActiveEvents::COLLISION_EVENTS)
                    .user_data(Solid::Entity(i as u32).to_data()),
            );
            body_of[i] = bodies.len() as u32;
            bodies.push(new_body(i, handle, true, start, None));
        }
        // A prop with no mass that isn't static can be put somewhere else (a script moves
        // it): its model as it is drawn, on a body that goes where it is told.
        for (i, entity) in world.entities.iter().enumerate() {
            if entity.kind != EntityKind::Prop || entity.body.is_some() || entity.is_static {
                continue;
            }
            let (vertices, triangles) = mesh_triangles(assets.mesh(entity.mesh), |p| p * entity.scale);
            let model = || (!triangles.is_empty()).then(|| ColliderBuilder::trimesh(vertices.clone(), triangles.clone()).ok()).flatten();
            let (Some(collider), Some(seen)) = (model(), model()) else {
                continue;
            };
            let start = (entity.position, entity.rotation);
            let solid = Solid::Entity(i as u32).to_data();
            let (handle, _) = rapier.insert(
                RigidBodyBuilder::kinematic_position_based().pose(Pose::from_parts(start.0, start.1)),
                collider
                    .friction(FRICTION)
                    .active_events(ActiveEvents::COLLISION_EVENTS)
                    .user_data(solid | BODIES_ONLY),
            );
            let seen = rapier.insert_collider(
                seen.position(Pose::from_parts(start.0, start.1)).collision_groups(InteractionGroups::none()).user_data(solid),
                None,
            );
            body_of[i] = bodies.len() as u32;
            bodies.push(new_body(i, handle, false, start, Some(seen)));
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
            if entity.kind == EntityKind::Prop && body_of[i] == NO_BODY {
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
            bodies,
            body_of,
            pending: 0.0,
            collisions: Vec::new(),
            touches: Vec::new(),
        }
    }

    fn body(&self, entity: u32) -> Option<&Body> {
        self.body_of.get(entity as usize).and_then(|&b| self.bodies.get(b as usize))
    }

    fn body_mut(&mut self, entity: u32) -> Option<&mut Body> {
        self.body_of.get(entity as usize).and_then(|&b| self.bodies.get_mut(b as usize))
    }

    /// Tells physics an entity was put at `position`, turned `rotation`, by something
    /// other than physics (a script). Returns whether physics has a body for it.
    ///
    /// A body that goes where it is put (one with no mass, or [held](Self::hold)) travels
    /// there over the steps of the next frame: what is in its way is pushed, and a
    /// character on it is carried. A dynamic body is there at once, keeping its speed.
    pub fn put(&mut self, entity: u32, position: Vec3, rotation: Quat) -> bool {
        let Some(body) = self.body_mut(entity) else {
            return false;
        };
        if body.put() {
            body.target = Some((position, rotation));
        } else {
            let handle = body.handle;
            body.previous = (position, rotation);
            self.world.bodies[handle].set_position(Pose::from_parts(position, rotation), true);
        }
        true
    }

    /// Takes a dynamic body out of physics' hands: it stops, and goes only where it is
    /// [put](Self::put) (pushing what it meets, pushed by nothing) until it is
    /// [released](Self::release).
    pub fn hold(&mut self, entity: u32) {
        if let Some(body) = self.body_mut(entity).filter(|b| b.dynamic && !b.held) {
            body.held = true;
            let handle = body.handle;
            self.world.bodies[handle].set_body_type(RigidBodyType::KinematicPositionBased, true);
        }
    }

    /// Gives a [held](Self::hold) body back to physics, at rest where it is.
    pub fn release(&mut self, entity: u32) {
        if let Some(body) = self.body_mut(entity).filter(|b| b.held) {
            (body.held, body.target) = (false, None);
            let handle = body.handle;
            let rigid = &mut self.world.bodies[handle];
            rigid.set_body_type(RigidBodyType::Dynamic, true);
            rigid.set_linvel(Vec3::ZERO, true);
            rigid.set_angvel(Vec3::ZERO, true);
        }
    }

    /// Releases every held body.
    pub fn release_all(&mut self) {
        for entity in self.bodies.iter().filter(|b| b.held).map(|b| b.entity).collect::<Vec<_>>() {
            self.release(entity);
        }
    }

    /// An entity's mass in kilograms, if physics moves it (it is dynamic).
    pub fn mass(&self, entity: u32) -> Option<f32> {
        self.body(entity).filter(|b| b.dynamic).map(|b| self.world.bodies[b.handle].mass())
    }

    /// How fast an entity's body is moving, in m/s; `None` if it has none.
    pub fn velocity(&self, entity: u32) -> Option<Vec3> {
        self.body(entity).map(|b| self.world.bodies[b.handle].linvel())
    }

    /// Sets a dynamic body's velocity. Returns whether it is one physics moves just now.
    pub fn set_velocity(&mut self, entity: u32, velocity: Vec3) -> bool {
        let Some(handle) = self.body(entity).filter(|b| !b.put()).map(|b| b.handle) else {
            return false;
        };
        self.world.bodies[handle].set_linvel(velocity, true);
        true
    }

    /// Pushes a dynamic body through its middle: `impulse` is a mass times a change of
    /// velocity (kg m/s), so the same push moves a heavy thing less. Returns whether it is
    /// one physics moves just now.
    pub fn push(&mut self, entity: u32, impulse: Vec3) -> bool {
        let Some(handle) = self.body(entity).filter(|b| !b.put()).map(|b| b.handle) else {
            return false;
        };
        self.world.bodies[handle].apply_impulse(impulse, true);
        true
    }

    /// What began to touch since this was last asked: each pair once.
    pub fn take_touches(&mut self) -> Vec<Touch> {
        let mut touches: Vec<Touch> = Vec::new();
        for touch in self.touches.drain(..) {
            if !touches.iter().any(|t| (t.a, t.b) == (touch.a, touch.b) || (t.a, t.b) == (touch.b, touch.a)) {
                touches.push(touch);
            }
        }
        touches
    }

    /// The props physics moves (the dynamic ones nothing holds), each as (its entity's index
    /// in `World::entities`, its position, its rotation) now: blended between the last two steps like a character's
    /// [`feet`](Self::feet). For `World::move_entity`, each frame.
    pub fn bodies(&self) -> impl Iterator<Item = (u32, Vec3, Quat)> + '_ {
        let blend = self.blend();
        self.bodies.iter().filter(|b| !b.put()).map(move |m| {
            let now = self.world.bodies[m.handle].position();
            let (was, turned) = m.previous;
            (m.entity, was.lerp(now.translation, blend), turned.slerp(now.rotation, blend))
        })
    }

    /// Puts every dynamic body back where the level has it, at rest (and lets go of the
    /// held ones). The others stay where they were put: their entities are the game's.
    pub fn reset_bodies(&mut self) {
        self.release_all();
        for m in self.bodies.iter_mut().filter(|b| b.dynamic) {
            let body = &mut self.world.bodies[m.handle];
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
            standing: None,
            touching: Vec::new(),
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
        let due = ((self.pending / STEP) as u32).min(MAX_STEPS);
        let mut steps = 0;
        while self.pending >= STEP {
            self.pending -= STEP;
            if steps < MAX_STEPS {
                self.step(due - steps);
                steps += 1;
            }
        }
        steps
    }

    /// One step, of the `left` this frame still has to run: the characters, then the
    /// world, then the characters again where something that was put elsewhere took them.
    fn step(&mut self, left: u32) {
        let gravity = self.gravity;
        self.world.gravity = Vec3::new(0.0, -gravity, 0.0);
        for m in &mut self.bodies {
            let pose = *self.world.bodies[m.handle].position();
            m.previous = (pose.translation, pose.rotation);
            // One that was put somewhere goes an even share of the way there each step.
            m.moved = None;
            if let Some((to, turned)) = m.target.filter(|_| m.put()) {
                let share = 1.0 / left.max(1) as f32;
                let next = Pose::from_parts(pose.translation.lerp(to, share), pose.rotation.slerp(turned, share));
                self.world.bodies[m.handle].set_next_kinematic_position(next);
                m.moved = Some((pose, next));
                if left <= 1 {
                    m.target = None;
                }
            }
        }
        for (id, c) in self.characters.iter_mut().enumerate() {
            let config = c.config;
            self.world.colliders[c.collider].set_enabled(!c.ghost);
            if c.ghost {
                continue;
            }
            // What it meets: everything but itself (and what only bodies meet).
            let filter = QueryFilter::default().exclude_rigid_body(c.body).predicate(&for_characters);
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
            // The body it stands on, if it does: where that goes, it is carried (below).
            c.standing = None;
            if c.grounded {
                let ray = Ray::new(c.position + Vec3::Y * UNDERFOOT, Vec3::NEG_Y);
                let under = queries.cast_ray(&ray, UNDERFOOT * 2.0 + SKIN, true);
                c.standing = match under.map(|(handle, _)| Solid::from_data(self.world.colliders[handle].user_data)) {
                    Some(Solid::Entity(e)) => self.body_of.get(e as usize).copied().filter(|b| *b != NO_BODY),
                    _ => None,
                };
            }
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
            // The entities it is against now (within `AGAINST` of it) are touched: once
            // each, for as long as it stays. (Not from what stopped it above: stopped
            // short of something by the gap it keeps, it isn't always told what by.)
            c.touching.retain_mut(|(_, steps)| {
                *steps -= 1;
                *steps > 0
            });
            let wider = Capsule::new_y(shape.half_height(), config.radius + AGAINST);
            let at = Pose::from_translation(c.position + center);
            let own = QueryFilter::default().exclude_rigid_body(c.body).predicate(&for_characters);
            for (_, collider) in self.world.intersect_shape(at, &wider, own) {
                let solid = Solid::from_data(collider.user_data);
                if !matches!(solid, Solid::Entity(_)) {
                    continue;
                }
                match c.touching.iter_mut().find(|(s, _)| *s == solid) {
                    Some((_, steps)) => *steps = STILL_TOUCHING,
                    None => {
                        c.touching.push((solid, STILL_TOUCHING));
                        let touch = Touch { a: Solid::Character(CharacterId(id as u32)), b: solid, speed: coming.length() };
                        self.touches.push(touch);
                    }
                }
            }
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
        // What characters meet a put prop by goes where the prop does.
        for body in &self.bodies {
            if let (Some((_, to)), Some(seen)) = (body.moved, body.seen) {
                self.world.colliders[seen].set_position(to);
            }
        }
        let collector = Collector(Mutex::new(Vec::new()));
        self.world.step_with_events(&(), &collector);
        for (a, b, speed) in collector.0.into_inner().unwrap() {
            let solid = |handle| self.world.colliders.get(handle).map(|c| Solid::from_data(c.user_data));
            if let (Some(a), Some(b)) = (solid(a), solid(b)) {
                // (A character and what it walks into are told of above.)
                let known = |c: Solid, e: Solid| match c {
                    Solid::Character(id) => self.characters[id.0 as usize].touching.iter().any(|(s, _)| *s == e),
                    _ => false,
                };
                if !known(a, b) && !known(b, a) {
                    self.touches.push(Touch { a, b, speed });
                }
            }
        }
        // (Nobody asking after them, the oldest are dropped.)
        if self.touches.len() > 256 {
            self.touches.drain(..128);
        }
        self.carry();
    }

    /// Moves the characters that something put elsewhere took with it in the step just
    /// run: one standing on it goes where the spot under its feet went (a lift, a cart),
    /// and one it moved into is moved out of its way. Either stops at what else is in the way.
    fn carry(&mut self) {
        for i in 0..self.characters.len() {
            let c = &self.characters[i];
            if c.ghost {
                continue;
            }
            let (shape, center) = Self::capsule(&c.config, c.height);
            let here = Pose::from_translation(c.position + center);
            let own = QueryFilter::default().exclude_rigid_body(c.body);
            let taken = self.bodies.iter().enumerate().find_map(|(b, body)| {
                let (from, to) = body.moved?;
                let it = Solid::Entity(body.entity);
                let on = c.standing == Some(b as u32);
                let met = || {
                    let seen = self.world.intersect_shape(here, &shape, own.predicate(&for_characters));
                    seen.into_iter().any(|(_, collider)| Solid::from_data(collider.user_data) == it)
                };
                let by = to.transform_point(from.inverse_transform_point(c.position)) - c.position;
                match (on, body.seen) {
                    (true, Some(_)) => Some((it, by)),
                    // (On a held body, which is of the kind Rapier's character controller
                    // carries along the ground itself: up and down only.)
                    (true, None) => Some((it, Vec3::Y * by.y)),
                    (false, _) => met().then_some((it, by)),
                }
            });
            let Some((carrier, by)) = taken else {
                continue;
            };
            // (Not stopped by what carries it: that is where it was, or in it.)
            let others = |handle: ColliderHandle, collider: &Collider| {
                for_characters(handle, collider) && Solid::from_data(collider.user_data) != carrier
            };
            let queries = self.world.query_pipeline_with_filter(own.predicate(&others));
            let controller = KinematicCharacterController {
                offset: CharacterLength::Absolute(SKIN),
                autostep: None,
                snap_to_ground: None,
                ..Default::default()
            };
            let moved = controller.move_shape(STEP, &queries, &shape, &here, by, |_| {});
            self.characters[i].position += moved.translation;
        }
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

    /// The entity called `name`.
    fn named(world: &World, name: &str) -> u32 {
        world.entities.iter().position(|e| e.name == name).unwrap() as u32
    }

    /// two_rooms.mmp with its stacked crate (on `crate_a1`) given a mass.
    fn crates() -> (World, Assets) {
        let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/levels/two_rooms.mmp")).unwrap();
        let src = src.replace("crate_a2_stacked material=crate", "crate_a2_stacked material=crate mass=5 shape=box");
        assert!(src.contains("mass=5"));
        let level = assets.parse_level("two_rooms.mmp", &src).unwrap();
        (World::new(level, &assets), assets)
    }

    /// How high the top of whatever is under `at` is.
    fn top_under(physics: &Physics, at: Vec3) -> (f32, Solid) {
        let hit = physics.ray(Vec3::new(at.x, 3.5, at.z), Vec3::NEG_Y, 10.0, None).unwrap();
        (hit.point.y, hit.solid)
    }

    #[test]
    fn a_prop_put_somewhere_takes_its_collider_with_it() {
        let (world, assets) = level("two_rooms.mmp");
        let mut physics = Physics::new(&world, &assets);
        let e = named(&world, "crate_a3");
        let (at, turned) = (world.entities[e as usize].position, world.entities[e as usize].rotation);
        let (top, solid) = top_under(&physics, at);
        assert_eq!(solid, Solid::Entity(e));
        assert!((top - 1.0).abs() < 1e-3, "{top}");
        // Up: its top is where it went, a step later. Aside: the floor is under where it was.
        assert!(physics.put(e, at + Vec3::Y * 0.75, turned));
        physics.advance(STEP);
        assert!((top_under(&physics, at).0 - 1.75).abs() < 1e-3);
        physics.put(e, at + Vec3::new(-1.5, 0.0, 0.0), turned);
        physics.advance(STEP * 4.0);
        assert_eq!(top_under(&physics, at), (0.0, Solid::Level));
        assert_eq!(top_under(&physics, at + Vec3::new(-1.5, 0.0, 0.0)).1, Solid::Entity(e));
        // A static prop, and what isn't a prop, have no body to put anywhere.
        let (shiny, assets) = level("shiny_rooms.mmp");
        let mut physics = Physics::new(&shiny, &assets);
        let e = named(&shiny, "crate_a3");
        assert!(shiny.entities[e as usize].is_static && !physics.put(e, Vec3::ZERO, Quat::IDENTITY));
        assert!(!physics.put(99, Vec3::ZERO, Quat::IDENTITY));
        assert_eq!(top_under(&physics, shiny.entities[e as usize].position).1, Solid::Entity(e));
    }

    #[test]
    fn a_character_on_a_prop_rides_it_up_down_and_along() {
        for frame in [STEP, STEP * 2.0, STEP * 0.5] {
            let (world, assets) = level("two_rooms.mmp");
            let mut physics = Physics::new(&world, &assets);
            let e = named(&world, "crate_a3");
            let (start, turned) = (world.entities[e as usize].position, world.entities[e as usize].rotation);
            let id = physics.add_character(start + Vec3::Y * 1.05, CharacterConfig::default());
            run(&mut physics, 30);
            assert!(physics.character(id).grounded());
            let mut at = start;
            // Up 0.8 m, along 1 m, and back down, at 1 m/s: its feet stay on the prop's top.
            for (way, seconds) in [(Vec3::Y, 0.8), (Vec3::X, 1.0), (Vec3::NEG_Y, 0.8)] {
                let frames = (seconds / frame as f64).round() as u32;
                for k in 0..frames {
                    at += way * frame;
                    physics.put(e, at, turned);
                    physics.advance(frame);
                    let c = physics.character(id);
                    let (top, under) = top_under(&physics, c.position());
                    assert_eq!(under, Solid::Character(id), "off it going {way} at frame {k} of {frame}");
                    let _ = top;
                    let lift = physics.velocity(e).unwrap();
                    assert!(lift.length() < 1.5, "{lift}");
                }
                run(&mut physics, 10);
                let c = physics.character(id);
                // (Over the prop's top by the gap a character keeps from what it touches.)
                let over = c.position() - (at + Vec3::Y);
                assert!(c.grounded() && (0.0..0.06).contains(&over.y), "after going {way} by frames of {frame}: {over} over it");
                assert!(over.x.abs() < 0.08 && over.z.abs() < 0.08, "after going {way} by frames of {frame}: {over} from its middle");
            }
        }
    }

    #[test]
    fn a_prop_put_into_a_character_moves_it_out_of_the_way() {
        let (world, assets) = level("two_rooms.mmp");
        let mut physics = Physics::new(&world, &assets);
        let e = named(&world, "crate_a1");
        let (start, turned) = (world.entities[e as usize].position, world.entities[e as usize].rotation);
        let id = physics.add_character(start + Vec3::new(0.9, 0.0, 0.0), CharacterConfig::default());
        run(&mut physics, 30);
        let before = physics.character(id).position();
        for k in 1..=60 {
            physics.put(e, start + Vec3::X * (k as f32 / 60.0), turned);
            physics.advance(STEP);
        }
        run(&mut physics, 10);
        // The crate's side came 1 m: the character is past it still, on the floor.
        let c = physics.character(id);
        assert!(c.position().x > start.x + 1.5 + 0.25, "{} then {}", before, c.position());
        assert!(c.grounded() && c.position().y.abs() < 0.05);
    }

    #[test]
    fn a_body_is_carried_put_held_pushed_and_let_go() {
        let (world, assets) = crates();
        let mut physics = Physics::new(&world, &assets);
        let (b, u) = (named(&world, "crate_a2_stacked"), named(&world, "crate_a1"));
        let b_id = b;
        let place = |physics: &Physics| physics.bodies().find(|b| b.0 == b_id).unwrap().1;
        assert_eq!(physics.mass(b), Some(5.0));
        assert_eq!((physics.mass(u), physics.mass(99)), (None, None));
        assert_eq!(physics.bodies().count(), 1);
        run(&mut physics, 30);
        assert!((place(&physics).y - 1.0).abs() < 0.03);
        // The crate under it is put higher: it is lifted on it.
        let (under, turned) = (world.entities[u as usize].position, world.entities[u as usize].rotation);
        for k in 1..=60 {
            physics.put(u, under + Vec3::Y * (0.5 * k as f32 / 60.0), turned);
            physics.advance(STEP);
        }
        run(&mut physics, 30);
        assert!((place(&physics).y - 1.5).abs() < 0.03, "{}", place(&physics));
        // Put: it is there at once, and goes on from there (it falls).
        physics.put(b, Vec3::new(0.0, 2.0, 4.0), Quat::IDENTITY);
        assert_eq!(place(&physics), Vec3::new(0.0, 2.0, 4.0));
        physics.take_touches();
        run(&mut physics, 90);
        assert!(place(&physics).y.abs() < 0.03, "{}", place(&physics));
        // It landed on the level, once, falling at about the 6 m/s a 2 m drop gives.
        let touches = physics.take_touches();
        let landing: Vec<&Touch> = touches.iter().filter(|t| [t.a, t.b].contains(&Solid::Level)).collect();
        assert_eq!(landing.len(), 1, "{touches:?}");
        assert!([landing[0].a, landing[0].b].contains(&Solid::Entity(b)) && (5.0..7.0).contains(&landing[0].speed), "{touches:?}");
        // Pushed: 10 kg m/s on 5 kg is 2 m/s.
        assert!(physics.push(b, Vec3::new(10.0, 0.0, 0.0)));
        assert!((physics.velocity(b).unwrap().x - 2.0).abs() < 1e-3);
        assert!(physics.set_velocity(b, Vec3::ZERO));
        assert!(!physics.push(u, Vec3::X) && !physics.set_velocity(u, Vec3::X));
        // Held: it goes where it is put and stays there, and isn't in `bodies` meanwhile.
        physics.hold(b);
        assert_eq!(physics.bodies().count(), 0);
        assert!(!physics.push(b, Vec3::X));
        physics.put(b, Vec3::new(0.0, 1.5, 4.0), Quat::IDENTITY);
        run(&mut physics, 60);
        assert!(!physics.push(b, Vec3::X));
        // Let go: it is physics' again, and falls from where it was held.
        physics.release(b);
        assert!((place(&physics) - Vec3::new(0.0, 1.5, 4.0)).length() < 1e-3, "{}", place(&physics));
        run(&mut physics, 90);
        assert!(place(&physics).y.abs() < 0.03);
        // Reset: back where the level has it; the crate that was put higher stays there.
        physics.reset_bodies();
        assert_eq!(place(&physics), world.entities[b as usize].position);
        assert!((top_under(&physics, under + Vec3::new(0.4, 0.0, 0.4)).0 - 1.5).abs() < 0.02);
    }

    #[test]
    fn a_character_walking_into_a_prop_touches_it_once() {
        let (world, assets) = level("two_rooms.mmp");
        let mut physics = Physics::new(&world, &assets);
        let e = named(&world, "crate_b2");
        let at = world.entities[e as usize].position;
        let id = physics.add_character(at - Vec3::new(1.5, 0.0, 0.0), CharacterConfig::default());
        run(&mut physics, 30);
        assert_eq!(physics.take_touches(), []);
        physics.character_mut(id).wish = Vec3::X * 2.0;
        run(&mut physics, 60);
        let touches = physics.take_touches();
        assert_eq!(touches.len(), 1, "{touches:?}");
        assert_eq!((touches[0].a, touches[0].b), (Solid::Character(id), Solid::Entity(e)));
        // (The speed it was walking at.)
        assert!((touches[0].speed - 2.0).abs() < 0.05, "{touches:?}");
        // Leaning on it still: nothing new. Away and back: again.
        run(&mut physics, 120);
        assert_eq!(physics.take_touches(), []);
        physics.character_mut(id).wish = Vec3::NEG_X * 2.0;
        run(&mut physics, 40);
        physics.character_mut(id).wish = Vec3::X * 2.0;
        run(&mut physics, 60);
        assert_eq!(physics.take_touches().len(), 1);
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
