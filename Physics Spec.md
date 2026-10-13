# Physics Spec

## Overview

Physics is the `moose-physics` crate, a wrapper over **Rapier** (`rapier3d` 0.36). Rapier runs entirely on the CPU, builds with Cargo alone, and does its math in the same glam the engine uses, so vectors pass straight through. The rest of the engine never names a Rapier type: it talks to `Physics`, so the library underneath can be changed.

**Built so far (Oct 12):** collision with the level, terrain and props; props with a mass, which fall, roll and are pushed; a walking player (gravity, jumping, crouching, stairs, slopes) who pushes them; ray casts. **Planned** sections at the end are the agenda, not gaps.

## Conventions

| Topic | Convention |
| --- | --- |
| Units and axes | Meters, +Y up, as everywhere (see the Level Format Spec) |
| Step | Fixed, 1/60 s (`STEP`), whatever the frame rate |
| Gravity | 9.81 m/s² down by default (`Physics::gravity`) |
| A character's position | Its feet: the bottom of its capsule |

**Fixed steps.** `Physics::advance(dt)` takes a frame's time and runs as many whole steps as have come due (none on a short frame, several on a long one, at most 8, so a hitch can't snowball). What is left over carries to the next frame. Positions read for drawing are blended between the last two steps by how far into the next one the frame is, so motion is smooth at any frame rate, and the simulation is the same at 30 fps as at 144.

## What collides

Built once from a `World` (`Physics::new`), and again whenever the level is rebuilt (the editor, hot reload):

| What | Collider | Notes |
| --- | --- | --- |
| The level's surfaces | One triangle mesh | Every surface of every sector, sky surfaces too (they bound the level) |
| Portals | None | Except those not passable: solid, like a surface |
| Terrain | A triangle mesh per terrain | Its carved pieces, which are already in world space |
| Props | A triangle mesh each | The model as drawn, at the entity's position, rotation and scale |
| Props with a mass | A body each (below) | A ball, a box or a convex hull |
| Characters | A capsule each | Moved to where the character is each step, so bodies meet it |
| Actors, blockers | None yet | Actors are planned (below); blockers only hide things |

Polygons are fanned into triangles from their first corner. Triangles with no area (a corner added on an edge, which the level format allows) are left out.

Every collider knows what it is (`Solid`: the level, a terrain or an entity by index, or a character), which a ray cast reports.

## Bodies

A prop with `mass=KG` in the level (see the Level Format Spec) has a **body**: physics moves it. It falls, lands, rolls, bounces off things and is pushed by characters. A prop with a body can't be `static`, so its shadows are worked out every frame as it moves.

| Option | Collides as |
| --- | --- |
| `shape=hull` (the default) | The convex hull of its model's points |
| `shape=ball` | The ball around its model's box: its center, out to its widest half |
| `shape=box` | Its model's box |

A body's shape is scaled with its entity and turns with it. Triangle meshes are for fixed things only: a body's shape is always convex.

| Constant | Value | Meaning |
| --- | --- | --- |
| `FRICTION` | 0.7 | Against what it touches (the level's and the fixed props' too) |
| `BOUNCE` | 0.3 | How much of its speed it keeps bouncing off something |
| `DRAG` | 0.3 /s | How fast its moving dies away on its own |
| `SPIN_DRAG` | 1.7 /s | How fast its turning does |

**Slowing down.** Friction alone lets a ball roll on forever: it only stops it sliding. The two drags are what slow it: a rolling ball loses `(DRAG + 0.4 SPIN_DRAG) / 1.4` of its speed each second (0.7 here), so it rolls about 1.4 m for each m/s it had, and is all but still in 6 seconds. They are the same for every body for now.

Fast bodies are swept between steps (continuous collision detection), so they don't pass through walls.

**In the world.** Each frame the game moves each body's entity to where its body is (`Physics::bodies`, blended between steps like a character; `World::move_entity`), so drawing, the sectors it is in and its shadows follow. `Physics::reset_bodies` puts them all back where the level has them, at rest.

## Characters

A character is a capsule moved by Rapier's character controller: it is swept through the world each step, slides along what it meets, and is never pushed by anything. Bodies meet it, though (a ball bounces off it): it has a capsule in the world, moved to where it is each step.

| Setting (`CharacterConfig`) | Default | Meaning |
| --- | --- | --- |
| `radius` | 0.25 m | The capsule's radius (what the camera kept clear of walls by before) |
| `height` | 1.8 m | Feet to the top of the head |
| `step_height` | 0.35 m | The tallest step it walks up without jumping |
| `max_slope` | 45° | The steepest slope it walks up; steeper ones it slides down |
| `jump_speed` | 4.5 m/s | Upward speed at takeoff (about 1 m high at default gravity) |
| `air_control` | 10 m/s² | How fast it can change its sideways speed in the air |
| `crouch_height` | 1.0 m | Its height crouched |
| `crouch_rate` | 4 m/s | How fast its height changes as it crouches and stands (0.2 s for the player) |
| `mass` | 80 kg | What it weighs when it pushes on bodies |

**Input** is three fields the game sets each frame: `wish`, the velocity it wants across the ground (m/s; its y is ignored), and `jump` and `crouch`, held or not.

**Each step:**
- Its height goes toward the one it wants (crouched or standing) at `crouch_rate`; its feet stay where they are, and its capsule is as tall as it is now. Standing up, it rises only as far as there is room: its capsule is swept upward first, and stops short of what is over it. So under something low it stays down after `crouch` is let go, and stands when it comes out.
- On the ground, its sideways velocity is `wish`, at once: no acceleration or friction yet. If `jump` is held, it takes off.
- In the air, its sideways velocity moves toward `wish` by at most `air_control`.
- Gravity is added, and the controller moves it as far as it can go.
- Landing zeroes its fall; hitting its head zeroes its rise.
- It is kept on the ground down steps and slopes (snapped down by up to 0.3 m), so it walks down stairs rather than hopping off each one.
- It pushes the bodies it ran into, as one mass meeting another without a bounce: each is brought toward the speed the character came at it with, along the way they met, by the character's share of their two masses (80 kg against a 5 kg ball: 94%). The push goes through the body's middle, so it is shoved, not spun; the ground under it sets it rolling. The character isn't slowed by it: walking into a ball, it walks on behind it.

A character that is a **ghost** (`Character::ghost`) isn't moved, and nothing meets it: the player, while flying.

How fast it moves crouched is the game's to say, through `wish` (`Character::crouching` tells it: lower than standing, for whatever reason).

`Physics::teleport` puts a character somewhere at rest and standing: for spawning, and for when something else moved the player.

## The player and sectors

Physics knows nothing of sectors or portals: the level is one triangle mesh to it. The camera still has to know its sector, since drawing starts there. So after physics moves the player, the app moves the camera's eye (1.7 m over the feet standing, and as far under the top of the head crouched: 0.9 m) with `Camera::move_to`, a trace from where it was, which follows it through portals. If that trace is stopped short (the eye cut a corner the capsule went around), the sector is found from scratch (`World::find_sector`).

## In the app

- **Walking** is how the game is played: WASD across the ground (Shift runs, at twice walking speed), Space jumps, C crouches (held; half walking speed, and no running, for as long as the player is down).
- **Flying** (the old movement, no gravity, kept clear of walls only) is what the editor uses, and a setting while playing: Controls > Fly, or `--fly`. Space and C rise and sink. While playing, bodies go on moving, and the flying player is a ghost to them.
- **In the editor** nothing is simulated, and bodies are back where the level has them: it shows the level as written.
- Whenever the camera is somewhere physics didn't put it (the level was loaded, `--at`, leaving the editor or flying), the player is teleported to it.
- Screenshots (`--screenshot`) never step physics: the camera is where it was put.

## API

```rust
pub const STEP: f32 = 1.0 / 60.0;

impl Physics {
    pub fn new(world: &World, assets: &Assets) -> Physics;
    pub gravity: f32;

    pub fn add_character(&mut self, feet: Vec3, config: CharacterConfig) -> CharacterId;
    pub fn character(&self, id: CharacterId) -> &Character;      // position, velocity, grounded, height, crouching
    pub fn character_mut(&mut self, id: CharacterId) -> &mut Character;  // wish, jump, crouch, ghost
    pub fn teleport(&mut self, id: CharacterId, feet: Vec3);
    /// A character's feet and height, blended between the last two steps, for drawing.
    pub fn feet(&self, id: CharacterId) -> Vec3;
    pub fn height(&self, id: CharacterId) -> f32;

    /// Runs the steps a frame of `dt` seconds brings due. Returns how many.
    pub fn advance(&mut self, dt: f32) -> u32;

    /// The props with a body, each as (its entity, its position, its rotation) now.
    pub fn bodies(&self) -> impl Iterator<Item = (u32, Vec3, Quat)>;
    pub fn reset_bodies(&mut self);

    /// The first thing a ray meets within `max` meters, passing through character `skip`.
    pub fn ray(&self, from: Vec3, direction: Vec3, max: f32, skip: Option<CharacterId>) -> Option<RayHit>;  // distance, point, normal, solid
}
```

## Planned

- **More for bodies:** friction, bounce and drag per entity; bodies that push the player back (standing on a rolling ball carries nobody yet); joints between bodies.
- **Level format:** per entity, bodies moved by scripts or animation (`kinematic`); per surface, an option to not collide. Editor fields for an entity's mass and shape (they are written in its options for now).
- **Actors:** a capsule each, moved like the player.
- **Seeing it:** an overlay of the colliders' wireframes, like the shadow mesh (F4).
- **Ground feel:** acceleration and friction on the ground, ladders, water.
- **Sensors:** volumes that report what enters and leaves them, for scripts.
- **Ragdolls:** the skeletal models' bones as bodies joined at their joints.
