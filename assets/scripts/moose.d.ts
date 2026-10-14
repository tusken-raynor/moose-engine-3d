// Written by the engine from its script API table (moose-app's `scripts` module): don't
// edit it. To write it again after the table changes:
//   MOOSE_WRITE_DTS=1 cargo test -p moose-app moose_d_ts
//
// Moose scripts are a strict subset of TypeScript. This file is what an editor checks them
// against; the engine checks them again as it loads them, and its word is the last.
//
// A script hears an event by exporting a function of its name:
//   export function onStart()
//     The level has started: every script's variables have their starting values.
//   export function onTick(dt: number)
//     A frame has passed: `dt` is its length in seconds.
//   export function onActivated(target: Entity, by: Player)
//     A player used `target` (the use key, looking at it from near). Heard by the scripts attached to it or with an input bound to it.
//   export function onEntered(sector: Sector, by: Player)
//     A player walked into `sector`. Heard by the scripts with an input bound to it.
//   export function onExited(sector: Sector, by: Player)
//     A player walked out of `sector`.
//   export function onTouched(target: Entity, other: Entity | null, speed: number)
//     `target` began to touch something, coming together at `speed` m/s: another entity, or the level itself or terrain (`other` is null). One of the two has a mass, or was moved by a script. Both are told, each as `target`.
//   export function onBumped(target: Entity, by: Player, speed: number)
//     A player ran into `target` (or landed on it), at `speed` m/s: once, however long they lean on it.

/** A whole number (an i32 in the engine). To TypeScript it is a number; the engine keeps them apart. */
type int = number;

/**
 * An input of the script, set where the script is placed (in the level, or on an entity or
 * its template): `const door = input<Entity>();`, or with a fallback, `const speed = input(2.0);`.
 * Only a top-level const can start as one.
 */
declare function input<T>(fallback?: T): T;

/** What the script is attached to. Only in a script placed on an entity or a template. */
declare const self: Entity;

/** A point or a direction, in meters. A value: its methods give new ones. */
interface Vec3 {
  readonly x: number;
  /** Up. */
  readonly y: number;
  readonly z: number;
  add(other: Vec3): Vec3;
  sub(other: Vec3): Vec3;
  scale(by: number): Vec3;
  dot(other: Vec3): number;
  length(): number;
  distance(to: Vec3): number;
  /** This direction with length 1 (or zero, if it has none). */
  normalized(): Vec3;
  /** The point a share `t` (0 to 1) of the way to `to`. */
  lerp(to: Vec3, t: number): Vec3;
}

/** A color as red, green and blue, where 255 is full (a light's can go over, to overbrighten). */
interface Color {
  readonly r: number;
  readonly g: number;
  readonly b: number;
  scale(by: number): Color;
  /** The color a share `t` (0 to 1) of the way to `to`. */
  lerp(to: Color, t: number): Color;
}

/** The scripts' clock: it stands still while they do (in the editor). */
interface Time {
  /** Seconds scripts have run for. */
  readonly now: number;
  /** The length of this tick, in seconds. */
  readonly delta: number;
}
declare const time: Time;

/** Random numbers: the same run of them every time the level starts. */
interface Random {
  /** A number from `min` up to (not including) `max`. */
  range(min: number, max: number): number;
  /** A whole number from `min` to `max`, both included. */
  int(min: int, max: int): int;
  /** True with odds `odds` (0 never, 1 always). */
  chance(odds: number): boolean;
}
declare const random: Random;

/** The engine's log. */
interface Console {
  /** Writes the values to the log. */
  log(...values: any[]): void;
  /** Writes the values to the log as a problem. */
  warn(...values: any[]): void;
}
declare const console: Console;

/** A sector of the level: one of its convex rooms. */
interface Sector {
  readonly name: string;
  /** A point inside it: the middle of its corners. */
  readonly center: Vec3;
  /** Its walls, floor and ceiling. */
  surfaces(): Iterable<Surface>;
  /** The entities in it. */
  entities(): Iterable<Entity>;
  /** The lights placed in it. */
  lights(): Iterable<Light>;
  /** Whether it has a `$key` value. */
  has(key: string): boolean;
  /** Its `$key` value (the first number of it), or `fallback` if it has none. */
  get(key: string, fallback: number): number;
  /** Sets its `$key` value, for materials and other scripts to read. */
  set(key: string, value: number): void;
  /** Its `$key` value as a color (three numbers, 255 full), or `fallback`. */
  getColor(key: string, fallback: Color): Color;
  /** Sets its `$key` value to a color. */
  setColor(key: string, value: Color): void;
}

/** A surface of the level: a wall, floor or ceiling polygon. */
interface Surface {
  /** The sector it bounds. */
  readonly sector: Sector;
  /** Whether it has a `$key` value. */
  has(key: string): boolean;
  /** Its `$key` value (the first number of it), or `fallback` if it has none. */
  get(key: string, fallback: number): number;
  /** Sets its `$key` value, for materials and other scripts to read. */
  set(key: string, value: number): void;
  /** Its `$key` value as a color (three numbers, 255 full), or `fallback`. */
  getColor(key: string, fallback: Color): Color;
  /** Sets its `$key` value to a color. */
  setColor(key: string, value: Color): void;
}

/** A thing placed in the level: a prop or an actor. */
interface Entity {
  readonly name: string;
  /** Where it is. Setting it puts it there at once (one with a mass goes on from there, at the speed it had); `moveTo` takes it there over time. */
  position: Vec3;
  /** What it weighs in kilograms, if physics moves it (the level's `mass=`): it falls, rolls and is pushed. 0 if not. */
  readonly mass: number;
  /** How fast it is moving, in meters a second. Only one with a mass can be given a velocity. */
  velocity: Vec3;
  /** The sector it is in. */
  readonly sector: Sector;
  /** Whether the level says it never moves (`static`): then it can't be moved. */
  readonly isStatic: boolean;
  /** The animation it plays, by name, if its model has one of that name. */
  animation: string | null;
  /** Pushes it through its middle. `impulse` is a mass times a change of velocity (kg m/s): `self.push(vec3(0, 3 * self.mass, 0))` sends it up at 3 m/s whatever it weighs. Only for one with a mass. */
  push(impulse: Vec3): void;
  /** Moves it to `position` over `seconds`, in a straight line, pushing what is in its way and carrying a player standing on it. One with a mass is taken out of physics' hands for the trip, and let go at rest when it arrives. The promise settles when it is there (or when something else moves it). */
  moveTo(position: Vec3, seconds: number): Promise<void>;
  /** Moves it by `offset` from where it is, over `seconds`. */
  moveBy(offset: Vec3, seconds: number): Promise<void>;
  /** Whether it has a `$key` value. */
  has(key: string): boolean;
  /** Its `$key` value (the first number of it), or `fallback` if it has none. */
  get(key: string, fallback: number): number;
  /** Sets its `$key` value, for materials and other scripts to read. */
  set(key: string, value: number): void;
  /** Its `$key` value as a color (three numbers, 255 full), or `fallback`. */
  getColor(key: string, fallback: Color): Color;
  /** Sets its `$key` value to a color. */
  setColor(key: string, value: Color): void;
}

/** One of the level's lights. */
interface Light {
  readonly name: string | null;
  /** Its color at full strength: 255 is a surface's full color, and more overbrightens. */
  color: Color;
  /** Where its light ends, in meters. */
  range: number;
  /** Where it is (the level's lights don't move, but by their own `oscillate`). */
  readonly position: Vec3;
  /** The sector it is in; null for a directional light. */
  readonly sector: Sector | null;
  /** Changes its color to `color` over `seconds`. The promise settles when it is that color. */
  fadeTo(color: Color, seconds: number): Promise<void>;
}

/** Someone playing. */
interface Player {
  /** Where their eyes are. */
  readonly position: Vec3;
  /** The sector they are in. */
  readonly sector: Sector;
  /** Where they stand (walking). */
  readonly feet: Vec3;
  /** How fast they are moving, in meters a second (walking). */
  readonly velocity: Vec3;
  /** Whether they stand on something. */
  readonly grounded: boolean;
  /** Whether they are lower than they stand. */
  readonly crouching: boolean;
  /** Puts them at once where `feet` is, standing and at rest. Nothing happens if their eyes would be outside the level. */
  teleport(feet: Vec3): void;
}

/** What a ray met (`physics.ray`). Good until 64 more rays have met something. */
interface Hit {
  /** Where the ray met it. */
  readonly point: Vec3;
  /** The way its surface faces there (length 1), back toward where the ray came from. */
  readonly normal: Vec3;
  /** How far along the ray, in meters. */
  readonly distance: number;
  /** The entity it met; null for the level itself or terrain. */
  readonly entity: Entity | null;
}

/** A sound file from `assets/sounds`, to play. An input of this type is set to a file's name. */
interface Sound {
  /** How long it is, as recorded. */
  readonly seconds: number;
  /** Plays it once, heard the same everywhere. `volume` is its loudness (1 as recorded) and `pitch` its speed (1 as recorded, 2 an octave up in half the time). */
  play(volume?: number, pitch?: number): Playing;
  /** Plays it over and over, heard the same everywhere, until it is stopped. */
  loop(volume?: number, pitch?: number): Playing;
  /** Plays it once at a place in the level: heard from there, through the level's openings, never through walls. `reach` is how far it carries, in meters, along the way sound takes through the level's openings (50 if left out). */
  playAt(position: Vec3, volume?: number, pitch?: number, reach?: number): Playing;
  /** Plays it over and over at a place, until it is stopped. */
  loopAt(position: Vec3, volume?: number, pitch?: number, reach?: number): Playing;
  /** Plays it once where an entity is, following it as it moves. */
  playOn(entity: Entity, volume?: number, pitch?: number, reach?: number): Playing;
  /** Plays it over and over on an entity, until it is stopped. */
  loopOn(entity: Entity, volume?: number, pitch?: number, reach?: number): Playing;
}

/** A sound that was played: for stopping it, changing it, and waiting for it to end. */
interface Playing {
  /** Whether it is still playing (heard or not). */
  readonly playing: boolean;
  /** Its loudness: 1 as recorded. */
  volume: number;
  /** Stops it. Nothing happens if it has ended. */
  stop(): void;
  /** Moves it to a place in the level. */
  moveTo(position: Vec3): void;
  /** Moves it onto an entity, which it follows. */
  follow(entity: Entity): void;
  /** A promise that settles when it has ended or been stopped: `await door.playOn(self).ended()`. */
  ended(): Promise<void>;
}

/** The level. Its `$KEY` values are the state every script shares. */
interface Level {
  readonly name: string;
  /** The light that reaches everything. */
  ambient: Color;
  /** Whether it has a `$key` value. */
  has(key: string): boolean;
  /** Its `$key` value (the first number of it), or `fallback` if it has none. */
  get(key: string, fallback: number): number;
  /** Sets its `$key` value, for materials and other scripts to read. */
  set(key: string, value: number): void;
  /** Its `$key` value as a color (three numbers, 255 full), or `fallback`. */
  getColor(key: string, fallback: Color): Color;
  /** Sets its `$key` value to a color. */
  setColor(key: string, value: Color): void;
}
declare const level: Level;

/** The level's sectors. */
interface Sectors {
  readonly count: int;
  /** The sector called `name`. */
  find(name: string): Sector | null;
  /** The sector `position` is in. */
  at(position: Vec3): Sector | null;
  all(): Iterable<Sector>;
}
declare const sectors: Sectors;

/** The level's entities. */
interface Entities {
  readonly count: int;
  /** The entity called `name` (the first, if several are). */
  find(name: string): Entity | null;
  all(): Iterable<Entity>;
}
declare const entities: Entities;

/** The level's lights. */
interface Lights {
  readonly count: int;
  /** The light the level names `name` (`name=`). */
  find(name: string): Light | null;
  all(): Iterable<Light>;
}
declare const lights: Lights;

/** The players. */
interface Players {
  /** The player at this screen. */
  readonly local: Player;
}
declare const players: Players;

/** The level's physics (see the Physics Spec). */
interface PhysicsWorld {
  /** How hard things fall, in m/s² downward: 9.81 unless a script changes it. */
  gravity: number;
  /** The first solid thing on the line from `from` along `direction`, within `reach` meters: the level, terrain or a prop. Null if there is none. It passes through the player. */
  ray(from: Vec3, direction: Vec3, reach: number): Hit | null;
}
declare const physics: PhysicsWorld;

/** The sound files in `assets/sounds`. */
interface Sounds {
  /** The sound in file `name` (`door.wav`), loaded if it wasn't; null, with a warning, if it can't be. */
  find(name: string): Sound | null;
}
declare const sounds: Sounds;

/** A Vec3 of these. */
declare function vec3(x: number, y: number, z: number): Vec3;
/** A color: 255 is full. */
declare function rgb(r: number, g: number, b: number): Color;
/** A promise that settles in `seconds`: await sleep(2). */
declare function sleep(seconds: number): Promise<void>;
