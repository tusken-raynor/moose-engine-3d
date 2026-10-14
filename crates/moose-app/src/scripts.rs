//! The level's scripts (see the Scripting Spec): the table of what scripts can reach of the
//! world, and the scripts a level places, bound to its things and run each frame.
//!
//! The table is the one place a primitive is opened to scripts: its type, properties and
//! methods, and its manager (`entities`, `lights`, ...), each with the Rust that does it.
//! `assets/scripts/moose.d.ts`, which editors check scripts against, is written from it
//! (the `moose_d_ts` test says how).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use glam::Vec3;
use moose_assets::{Assets, Light, MetaValues, NO_SECTOR, ScriptPlacement, SoundId};
use moose_audio::{Audio, Place, Play, Playing};
use moose_physics::{CharacterId, Physics, RayHit, Solid};
use moose_scene::{Camera, World};
use moose_script::{
    Api, ApiDesc, ClassKind, Cx, GlobalDef, GlobalKind, Host, InstanceId, Line, Program, Runtime, Type, Value,
    compile,
};

use crate::App;

/// How far the player reaches to use something, in meters.
const REACH: f32 = 3.0;
/// A color's full strength in scripts (and in levels' meta values): a light's 1.
const FULL: f32 = 255.0;
/// How many rays' hits are kept for scripts to read: a `Hit` is good until this many
/// more rays have met something.
const HITS_KEPT: usize = 64;

/// The app's parts scripts work on, lent for a frame (see [`Scripts::tick`]).
pub(crate) struct Stage<'a> {
    pub world: &'a mut World,
    pub assets: &'a mut Assets,
    /// The level's lights and ambient light as the app keeps them (`App::lights`), which
    /// it lights the world with every frame.
    pub lights: &'a mut (Vec<Light>, Vec3),
    pub camera: &'a mut Camera,
    pub physics: &'a mut Physics,
    /// The player's character in `physics`.
    pub player: CharacterId,
    pub audio: &'a mut Audio,
}

/// What scripts' functions work on: the app's world, borrowed for a call.
pub(crate) struct Game<'a> {
    world: &'a mut World,
    assets: &'a mut Assets,
    lights: &'a mut Vec<Light>,
    ambient: &'a mut Vec3,
    camera: &'a mut Camera,
    physics: &'a mut Physics,
    player: CharacterId,
    audio: &'a mut Audio,
    changes: &'a mut Vec<Change>,
    kept: &'a mut Kept,
}

/// A sound a script started: the handle scripts know it by, and its loudness.
struct Voice {
    key: u32,
    playing: Playing,
    volume: f32,
}

/// What scripts hold handles to that isn't the world's: the sounds they started, and what
/// their rays met.
#[derive(Default)]
struct Kept {
    voices: Vec<Voice>,
    /// The promises of sounds ending (`playing.ended()`), by the sound's handle.
    ended: Vec<(u32, Value)>,
    hits: VecDeque<(u32, RayHit)>,
    /// The last handle given out, of either kind.
    last: u32,
}

pub(crate) struct GameKind;

impl Host for GameKind {
    type Borrowed<'a> = Game<'a>;
}

#[derive(Clone, Copy, PartialEq)]
enum Changing {
    /// An entity's position.
    Entity(usize),
    /// A light's color.
    Light(usize),
}

/// Something a script set going over time (`moveTo`, `fadeTo`): where it is going, and the
/// promise to settle when it is there.
struct Change {
    what: Changing,
    from: Vec3,
    to: Vec3,
    start: f64,
    seconds: f32,
    promise: Value,
}

impl Game<'_> {
    /// Puts entity `i` at `position`, in the sector that is in, and tells physics, whose
    /// body for it follows (see `Physics::put`).
    fn place(&mut self, i: usize, position: Vec3) {
        let sector = self.world.find_sector(position);
        let entity = &mut self.world.entities[i];
        entity.position = position;
        entity.sector = sector.unwrap_or(entity.sector);
        let rotation = entity.rotation;
        self.world.place_entity(i, self.assets);
        self.physics.put(i as u32, position, rotation);
    }

    /// Sets `what` on its way to `to` over `seconds`, instead of wherever it was going,
    /// and returns the promise of its getting there. An entity physics moves is taken out
    /// of its hands for the trip.
    fn change(&mut self, cx: &mut Cx<'_>, what: Changing, from: Vec3, to: Vec3, seconds: f32) -> Value {
        if let Some(at) = self.changes.iter().position(|c| c.what == what) {
            cx.resolve(self.changes.swap_remove(at).promise, Value::Null);
        }
        if let Changing::Entity(i) = what {
            self.physics.hold(i as u32);
        }
        let promise = cx.promise();
        self.changes.push(Change { what, from, to, start: cx.now, seconds, promise });
        promise
    }

    /// Stops `what` where it is, if it was on its way somewhere: the promise of its
    /// getting there settles, and an entity is physics' again.
    fn stop(&mut self, cx: &mut Cx<'_>, what: Changing) {
        if let Some(at) = self.changes.iter().position(|c| c.what == what) {
            cx.resolve(self.changes.swap_remove(at).promise, Value::Null);
            if let Changing::Entity(i) = what {
                self.physics.release(i as u32);
            }
        }
    }

    /// Starts sound `sound` at `place` for a script: `options` are the volume, pitch and
    /// reach it gave, or null for each it left out. Returns the script's handle to it.
    fn play(&mut self, sound: Value, place: Place, options: &[Value], looped: bool) -> Value {
        let Some(id) = sound.handle() else {
            return Value::Null;
        };
        let given = |i: usize| options.get(i).copied().filter(|v| *v != Value::Null).map(Value::num);
        let volume = given(0).unwrap_or(1.0).max(0.0);
        let mut play = Play::new(SoundId(id as u32)).volume(volume).pitch(given(1).unwrap_or(1.0).clamp(0.05, 8.0));
        (play.place, play.looped) = (place, looped);
        if let Some(reach) = given(2) {
            let reach = reach.max(0.01);
            play = play.carrying(play.near.min(reach), reach);
        }
        self.kept.last = self.kept.last.wrapping_add(1);
        self.kept.voices.push(Voice { key: self.kept.last, playing: self.audio.play(play), volume });
        Value::Handle(self.kept.last)
    }

    /// Where a sound at `position` is.
    fn at(&self, position: Vec3) -> Place {
        Place::At { sector: self.world.find_sector(position).unwrap_or(self.camera.sector), position }
    }

    /// The sound a script's `Playing` is, if it hasn't ended.
    fn voice(&mut self, playing: Value) -> Option<&mut Voice> {
        let key = playing.handle()? as u32;
        let voice = self.kept.voices.iter_mut().find(|v| v.key == key)?;
        self.audio.is_playing(voice.playing).then_some(voice)
    }

    /// What a script's `Hit` is, if it is still kept.
    fn hit(&self, hit: Value) -> Option<RayHit> {
        let key = hit.handle()? as u32;
        self.kept.hits.iter().find(|(k, _)| *k == key).map(|(_, hit)| *hit)
    }
}

fn v3(value: Value) -> Vec3 {
    Vec3::from_array(value.vec3())
}

fn val(v: Vec3) -> Value {
    Value::Vec3(v.to_array())
}

fn sector_value(sector: u32) -> Value {
    if sector == NO_SECTOR { Value::Null } else { Value::Handle(sector) }
}

/// A function of the table: `g` is the [`Game`], `cx` the script side of the call, and
/// `a` the arguments, after `this` in `a[0]`.
macro_rules! g {
    (|$g:ident, $cx:ident, $a:ident| $body:expr) => {{
        fn f($g: &mut Game<'_>, $cx: &mut Cx<'_>, $a: &[Value]) -> Value {
            let _ = (&$g, &$cx, &$a);
            $body
        }
        f
    }};
}

/// The meta values (`$KEY=VALUE`) of a class's things, which materials read: `$values` is
/// the thing's, from the game `$g` and the thing's index `$i`.
macro_rules! meta {
    ($api:ident, $class:ident, |$g:ident, $i:ident| $values:expr) => {
        $api.method($class, "has(key: string): boolean", "Whether it has a `$key` value.", g!(|$g, cx, a| {
            let key = $g.world.meta_keys.id(cx.str(a[1]));
            let $i = a[0].handle().unwrap_or(usize::MAX);
            let values: Option<&mut MetaValues> = $values;
            Value::Bool(key.is_some_and(|key| values.is_some_and(|v| v.get(key).is_some())))
        }));
        $api.method(
            $class,
            "get(key: string, fallback: number): number",
            "Its `$key` value (the first number of it), or `fallback` if it has none.",
            g!(|$g, cx, a| {
                let key = $g.world.meta_keys.id(cx.str(a[1]));
                let $i = a[0].handle().unwrap_or(usize::MAX);
                let values: Option<&mut MetaValues> = $values;
                let found = key.and_then(|key| values.and_then(|v| v.get(key).and_then(|v| v.first().copied())));
                Value::Num(found.unwrap_or(a[2].num()))
            })
        );
        $api.method($class, "set(key: string, value: number)", "Sets its `$key` value, for materials and other scripts to read.", g!(|$g, cx, a| {
            let key = $g.world.meta_keys.intern(cx.str(a[1]));
            let $i = a[0].handle().unwrap_or(usize::MAX);
            let values: Option<&mut MetaValues> = $values;
            if let Some(values) = values {
                values.set(key, vec![a[2].num()]);
            }
            Value::Null
        }));
        $api.method(
            $class,
            "getColor(key: string, fallback: Color): Color",
            "Its `$key` value as a color (three numbers, 255 full), or `fallback`.",
            g!(|$g, cx, a| {
                let key = $g.world.meta_keys.id(cx.str(a[1]));
                let $i = a[0].handle().unwrap_or(usize::MAX);
                let values: Option<&mut MetaValues> = $values;
                match key.and_then(|key| values.and_then(|v| v.get(key).map(|v| v.to_vec()))) {
                    Some(v) if v.len() == 3 => Value::Vec3([v[0], v[1], v[2]]),
                    _ => a[2],
                }
            })
        );
        $api.method($class, "setColor(key: string, value: Color)", "Sets its `$key` value to a color.", g!(|$g, cx, a| {
            let key = $g.world.meta_keys.intern(cx.str(a[1]));
            let $i = a[0].handle().unwrap_or(usize::MAX);
            let values: Option<&mut MetaValues> = $values;
            if let Some(values) = values {
                values.set(key, a[2].vec3().to_vec());
            }
            Value::Null
        }));
    };
}

/// The table: everything scripts can reach.
pub(crate) fn api() -> Api<GameKind> {
    let mut api: Api<GameKind> = Api::new();
    let sector = api.class("Sector", ClassKind::Handle, "A sector of the level: one of its convex rooms.");
    let surface = api.class("Surface", ClassKind::Handle, "A surface of the level: a wall, floor or ceiling polygon.");
    let entity = api.class("Entity", ClassKind::Handle, "A thing placed in the level: a prop or an actor.");
    let light = api.class("Light", ClassKind::Handle, "One of the level's lights.");
    let player = api.class("Player", ClassKind::Handle, "Someone playing.");
    let hit = api.class("Hit", ClassKind::Handle, "What a ray met (`physics.ray`). Good until 64 more rays have met something.");
    let sound = api.class("Sound", ClassKind::Handle, "A sound file from `assets/sounds`, to play. An input of this type is set to a file's name.");
    let playing = api.class("Playing", ClassKind::Handle, "A sound that was played: for stopping it, changing it, and waiting for it to end.");
    api.self_class(entity);

    let level = api.singleton("Level", "level", "The level. Its `$KEY` values are the state every script shares.");
    api.prop(level, "name", "string", "", g!(|g, cx, a| cx.string(&g.world.name)), None);
    api.prop(
        level,
        "ambient",
        "Color",
        "The light that reaches everything.",
        g!(|g, cx, a| val(*g.ambient * FULL)),
        Some(g!(|g, cx, a| {
            *g.ambient = (v3(a[1]) / FULL).max(Vec3::ZERO);
            Value::Null
        })),
    );
    meta!(api, level, |g, i| Some(&mut g.world.meta).filter(|_| i == usize::MAX));

    let sectors = api.singleton("Sectors", "sectors", "The level's sectors.");
    api.prop(sectors, "count", "int", "", g!(|g, cx, a| Value::Int(g.world.sectors.len() as i32)), None);
    api.method(sectors, "find(name: string): Sector | null", "The sector called `name`.", g!(|g, cx, a| {
        let name = cx.str(a[1]);
        g.world.sectors.iter().position(|s| s.name == name).map_or(Value::Null, |i| Value::Handle(i as u32))
    }));
    api.method(sectors, "at(position: Vec3): Sector | null", "The sector `position` is in.", g!(|g, cx, a| {
        g.world.find_sector(v3(a[1])).map_or(Value::Null, Value::Handle)
    }));
    api.method(sectors, "all(): Iterable<Sector>", "", g!(|g, cx, a| cx.list((0..g.world.sectors.len() as u32).map(Value::Handle))));
    api.prop(sector, "name", "string", "", g!(|g, cx, a| {
        let name = a[0].handle().and_then(|i| g.world.sectors.get(i)).map_or("", |s| s.name.as_str());
        cx.string(name)
    }), None);
    api.prop(sector, "center", "Vec3", "A point inside it: the middle of its corners.", g!(|g, cx, a| {
        val(a[0].handle().and_then(|i| g.world.sectors.get(i)).map_or(Vec3::ZERO, |s| s.center))
    }), None);
    api.method(sector, "surfaces(): Iterable<Surface>", "Its walls, floor and ceiling.", g!(|g, cx, a| {
        let range = a[0].handle().and_then(|i| g.world.sectors.get(i)).map_or(0..0, |s| s.polygons.clone());
        cx.list(range.map(Value::Handle))
    }));
    api.method(sector, "entities(): Iterable<Entity>", "The entities in it.", g!(|g, cx, a| {
        let here = a[0].handle().map(|i| i as u32);
        let entities = g.world.entities.iter().enumerate();
        cx.list(entities.filter(|(_, e)| Some(e.sector) == here).map(|(i, _)| Value::Handle(i as u32)))
    }));
    api.method(sector, "lights(): Iterable<Light>", "The lights placed in it.", g!(|g, cx, a| {
        let here = a[0].handle().map(|i| i as u32);
        let lights = g.lights.iter().enumerate();
        cx.list(lights.filter(|(_, l)| Some(l.sector) == here).map(|(i, _)| Value::Handle(i as u32)))
    }));
    meta!(api, sector, |g, i| g.world.sectors.get_mut(i).map(|s| &mut s.meta));

    api.prop(surface, "sector", "Sector", "The sector it bounds.", g!(|g, cx, a| {
        let i = a[0].handle().unwrap_or(usize::MAX) as u32;
        g.world.sectors.iter().position(|s| s.polygons.contains(&i)).map_or(Value::Null, |s| Value::Handle(s as u32))
    }), None);
    meta!(api, surface, |g, i| g.world.faces.get_mut(i));

    let entities = api.singleton("Entities", "entities", "The level's entities.");
    api.prop(entities, "count", "int", "", g!(|g, cx, a| Value::Int(g.world.entities.len() as i32)), None);
    api.method(entities, "find(name: string): Entity | null", "The entity called `name` (the first, if several are).", g!(|g, cx, a| {
        let name = cx.str(a[1]);
        g.world.entities.iter().position(|e| e.name == name).map_or(Value::Null, |i| Value::Handle(i as u32))
    }));
    api.method(entities, "all(): Iterable<Entity>", "", g!(|g, cx, a| cx.list((0..g.world.entities.len() as u32).map(Value::Handle))));
    api.prop(entity, "name", "string", "", g!(|g, cx, a| {
        let name = a[0].handle().and_then(|i| g.world.entities.get(i)).map_or("", |e| e.name.as_str());
        cx.string(name)
    }), None);
    api.prop(
        entity,
        "position",
        "Vec3",
        "Where it is. Setting it puts it there at once (one with a mass goes on from there, at the speed it had); `moveTo` takes it there over time.",
        g!(|g, cx, a| val(a[0].handle().and_then(|i| g.world.entities.get(i)).map_or(Vec3::ZERO, |e| e.position))),
        Some(g!(|g, cx, a| {
            if let Some(i) = movable(g, cx, a[0]) {
                g.stop(cx, Changing::Entity(i));
                g.place(i, v3(a[1]));
            }
            Value::Null
        })),
    );
    api.prop(entity, "mass", "number", "What it weighs in kilograms, if physics moves it (the level's `mass=`): it falls, rolls and is pushed. 0 if not.", g!(|g, cx, a| {
        Value::Num(a[0].handle().and_then(|i| g.physics.mass(i as u32)).unwrap_or(0.0))
    }), None);
    api.prop(
        entity,
        "velocity",
        "Vec3",
        "How fast it is moving, in meters a second. Only one with a mass can be given a velocity.",
        g!(|g, cx, a| val(a[0].handle().and_then(|i| g.physics.velocity(i as u32)).unwrap_or(Vec3::ZERO))),
        Some(g!(|g, cx, a| {
            if let Some(i) = a[0].handle()
                && !g.physics.set_velocity(i as u32, v3(a[1]))
            {
                not_physics(g, cx, i, "be given a velocity");
            }
            Value::Null
        })),
    );
    api.method(
        entity,
        "push(impulse: Vec3)",
        "Pushes it through its middle. `impulse` is a mass times a change of velocity (kg m/s): `self.push(vec3(0, 3 * self.mass, 0))` sends it up at 3 m/s whatever it weighs. Only for one with a mass.",
        g!(|g, cx, a| {
            if let Some(i) = a[0].handle()
                && !g.physics.push(i as u32, v3(a[1]))
            {
                not_physics(g, cx, i, "be pushed");
            }
            Value::Null
        }),
    );
    api.prop(entity, "sector", "Sector", "The sector it is in.", g!(|g, cx, a| {
        a[0].handle().and_then(|i| g.world.entities.get(i)).map_or(Value::Null, |e| sector_value(e.sector))
    }), None);
    api.prop(entity, "isStatic", "boolean", "Whether the level says it never moves (`static`): then it can't be moved.", g!(|g, cx, a| {
        Value::Bool(a[0].handle().and_then(|i| g.world.entities.get(i)).is_some_and(|e| e.is_static))
    }), None);
    api.prop(
        entity,
        "animation",
        "string | null",
        "The animation it plays, by name, if its model has one of that name.",
        g!(|g, cx, a| {
            let name = a[0].handle().and_then(|i| g.world.entities.get(i)).and_then(|e| e.animation.clone());
            name.map_or(Value::Null, |name| cx.string(&name))
        }),
        Some(g!(|g, cx, a| {
            let name = (a[1] != Value::Null).then(|| cx.str(a[1]).to_string());
            if let Some(e) = a[0].handle().and_then(|i| g.world.entities.get_mut(i)) {
                e.animation = name;
            }
            Value::Null
        })),
    );
    api.method(
        entity,
        "moveTo(position: Vec3, seconds: number): Promise<void>",
        "Moves it to `position` over `seconds`, in a straight line, pushing what is in its way and carrying a player standing on it. One with a mass is taken out of physics' hands for the trip, and let go at rest when it arrives. The promise settles when it is there (or when something else moves it).",
        g!(|g, cx, a| {
            let Some(i) = movable(g, cx, a[0]) else {
                return cx.settled(Value::Null);
            };
            let from = g.world.entities[i].position;
            g.change(cx, Changing::Entity(i), from, v3(a[1]), a[2].num())
        }),
    );
    api.method(
        entity,
        "moveBy(offset: Vec3, seconds: number): Promise<void>",
        "Moves it by `offset` from where it is, over `seconds`.",
        g!(|g, cx, a| {
            let Some(i) = movable(g, cx, a[0]) else {
                return cx.settled(Value::Null);
            };
            let from = g.world.entities[i].position;
            g.change(cx, Changing::Entity(i), from, from + v3(a[1]), a[2].num())
        }),
    );
    meta!(api, entity, |g, i| g.world.entities.get_mut(i).map(|e| &mut e.meta));

    let lights = api.singleton("Lights", "lights", "The level's lights.");
    api.prop(lights, "count", "int", "", g!(|g, cx, a| Value::Int(g.lights.len() as i32)), None);
    api.method(lights, "find(name: string): Light | null", "The light the level names `name` (`name=`).", g!(|g, cx, a| {
        let name = cx.str(a[1]);
        let found = g.world.light_names.iter().position(|n| n.as_deref() == Some(name)).filter(|i| *i < g.lights.len());
        found.map_or(Value::Null, |i| Value::Handle(i as u32))
    }));
    api.method(lights, "all(): Iterable<Light>", "", g!(|g, cx, a| cx.list((0..g.lights.len() as u32).map(Value::Handle))));
    api.prop(light, "name", "string | null", "", g!(|g, cx, a| {
        let name = a[0].handle().and_then(|i| g.world.light_names.get(i).cloned().flatten());
        name.map_or(Value::Null, |name| cx.string(&name))
    }), None);
    api.prop(
        light,
        "color",
        "Color",
        "Its color at full strength: 255 is a surface's full color, and more overbrightens.",
        g!(|g, cx, a| val(a[0].handle().and_then(|i| g.lights.get(i)).map_or(Vec3::ZERO, |l| l.color * FULL))),
        Some(g!(|g, cx, a| {
            if let Some(i) = a[0].handle().filter(|i| *i < g.lights.len()) {
                g.changes.retain(|c| c.what != Changing::Light(i));
                g.lights[i].color = (v3(a[1]) / FULL).max(Vec3::ZERO);
            }
            Value::Null
        })),
    );
    api.prop(
        light,
        "range",
        "number",
        "Where its light ends, in meters.",
        g!(|g, cx, a| Value::Num(a[0].handle().and_then(|i| g.lights.get(i)).map_or(0.0, |l| l.range))),
        Some(g!(|g, cx, a| {
            if let Some(l) = a[0].handle().and_then(|i| g.lights.get_mut(i)) {
                l.range = a[1].num().max(0.01);
            }
            Value::Null
        })),
    );
    api.prop(light, "position", "Vec3", "Where it is (the level's lights don't move, but by their own `oscillate`).", g!(|g, cx, a| {
        val(a[0].handle().and_then(|i| g.lights.get(i)).map_or(Vec3::ZERO, |l| l.position))
    }), None);
    api.prop(light, "sector", "Sector | null", "The sector it is in; null for a directional light.", g!(|g, cx, a| {
        a[0].handle().and_then(|i| g.lights.get(i)).map_or(Value::Null, |l| sector_value(l.sector))
    }), None);
    api.method(
        light,
        "fadeTo(color: Color, seconds: number): Promise<void>",
        "Changes its color to `color` over `seconds`. The promise settles when it is that color.",
        g!(|g, cx, a| {
            let Some(i) = a[0].handle().filter(|i| *i < g.lights.len()) else {
                return cx.settled(Value::Null);
            };
            let from = g.lights[i].color;
            g.change(cx, Changing::Light(i), from, (v3(a[1]) / FULL).max(Vec3::ZERO), a[2].num())
        }),
    );

    let players = api.singleton("Players", "players", "The players.");
    api.prop(players, "local", "Player", "The player at this screen.", g!(|g, cx, a| Value::Handle(0)), None);
    api.prop(player, "position", "Vec3", "Where their eyes are.", g!(|g, cx, a| val(g.camera.position)), None);
    api.prop(player, "sector", "Sector", "The sector they are in.", g!(|g, cx, a| Value::Handle(g.camera.sector)), None);
    api.prop(player, "feet", "Vec3", "Where they stand (walking).", g!(|g, cx, a| val(g.physics.feet(g.player))), None);
    api.prop(player, "velocity", "Vec3", "How fast they are moving, in meters a second (walking).", g!(|g, cx, a| {
        val(g.physics.character(g.player).velocity())
    }), None);
    api.prop(player, "grounded", "boolean", "Whether they stand on something.", g!(|g, cx, a| {
        Value::Bool(g.physics.character(g.player).grounded())
    }), None);
    api.prop(player, "crouching", "boolean", "Whether they are lower than they stand.", g!(|g, cx, a| {
        Value::Bool(g.physics.character(g.player).crouching())
    }), None);
    api.method(player, "teleport(feet: Vec3)", "Puts them at once where `feet` is, standing and at rest. Nothing happens if their eyes would be outside the level.", g!(|g, cx, a| {
        let eye = v3(a[1]) + Vec3::Y * crate::EYE_HEIGHT;
        match g.world.find_sector(eye) {
            // (The app sees the camera was put somewhere, and stands the player under it.)
            Some(sector) => (g.camera.position, g.camera.sector) = (eye, sector),
            None => cx.warn(format!("a player can't be put at {}: it is outside the level", v3(a[1]))),
        }
        Value::Null
    }));

    let physics = api.singleton("PhysicsWorld", "physics", "The level's physics (see the Physics Spec).");
    api.prop(
        physics,
        "gravity",
        "number",
        "How hard things fall, in m/s² downward: 9.81 unless a script changes it.",
        g!(|g, cx, a| Value::Num(g.physics.gravity)),
        Some(g!(|g, cx, a| {
            g.physics.gravity = a[1].num();
            Value::Null
        })),
    );
    api.method(
        physics,
        "ray(from: Vec3, direction: Vec3, reach: number): Hit | null",
        "The first solid thing on the line from `from` along `direction`, within `reach` meters: the level, terrain or a prop. Null if there is none. It passes through the player.",
        g!(|g, cx, a| {
            let Some(found) = g.physics.ray(v3(a[1]), v3(a[2]), a[3].num(), Some(g.player)) else {
                return Value::Null;
            };
            g.kept.last = g.kept.last.wrapping_add(1);
            g.kept.hits.push_back((g.kept.last, found));
            if g.kept.hits.len() > HITS_KEPT {
                g.kept.hits.pop_front();
            }
            Value::Handle(g.kept.last)
        }),
    );
    api.prop(hit, "point", "Vec3", "Where the ray met it.", g!(|g, cx, a| val(g.hit(a[0]).map_or(Vec3::ZERO, |h| h.point))), None);
    api.prop(hit, "normal", "Vec3", "The way its surface faces there (length 1), back toward where the ray came from.", g!(|g, cx, a| {
        val(g.hit(a[0]).map_or(Vec3::ZERO, |h| h.normal))
    }), None);
    api.prop(hit, "distance", "number", "How far along the ray, in meters.", g!(|g, cx, a| Value::Num(g.hit(a[0]).map_or(0.0, |h| h.distance))), None);
    api.prop(hit, "entity", "Entity | null", "The entity it met; null for the level itself or terrain.", g!(|g, cx, a| match g.hit(a[0]).map(|h| h.solid) {
        Some(Solid::Entity(i)) => Value::Handle(i),
        _ => Value::Null,
    }), None);

    let sounds = api.singleton("Sounds", "sounds", "The sound files in `assets/sounds`.");
    api.method(sounds, "find(name: string): Sound | null", "The sound in file `name` (`door.wav`), loaded if it wasn't; null, with a warning, if it can't be.", g!(|g, cx, a| {
        let name = cx.str(a[1]).to_string();
        match g.assets.load_sound(&name) {
            Ok(id) => Value::Handle(id.0),
            Err(e) => {
                cx.warn(e.to_string());
                Value::Null
            }
        }
    }));
    api.prop(sound, "seconds", "number", "How long it is, as recorded.", g!(|g, cx, a| {
        Value::Num(a[0].handle().map_or(0.0, |i| g.assets.sound(SoundId(i as u32)).seconds()))
    }), None);
    let options = "`volume` is its loudness (1 as recorded) and `pitch` its speed (1 as recorded, 2 an octave up in half the time).";
    let carrying = "`reach` is how far it carries, in meters, along the way sound takes through the level's openings (50 if left out).";
    api.method(sound, "play(volume?: number, pitch?: number): Playing", &format!("Plays it once, heard the same everywhere. {options}"), g!(|g, cx, a| {
        g.play(a[0], Place::Everywhere, &a[1..], false)
    }));
    api.method(sound, "loop(volume?: number, pitch?: number): Playing", "Plays it over and over, heard the same everywhere, until it is stopped.", g!(|g, cx, a| {
        g.play(a[0], Place::Everywhere, &a[1..], true)
    }));
    api.method(
        sound,
        "playAt(position: Vec3, volume?: number, pitch?: number, reach?: number): Playing",
        &format!("Plays it once at a place in the level: heard from there, through the level's openings, never through walls. {carrying}"),
        g!(|g, cx, a| {
            let place = g.at(v3(a[1]));
            g.play(a[0], place, &a[2..], false)
        }),
    );
    api.method(sound, "loopAt(position: Vec3, volume?: number, pitch?: number, reach?: number): Playing", "Plays it over and over at a place, until it is stopped.", g!(|g, cx, a| {
        let place = g.at(v3(a[1]));
        g.play(a[0], place, &a[2..], true)
    }));
    api.method(
        sound,
        "playOn(entity: Entity, volume?: number, pitch?: number, reach?: number): Playing",
        "Plays it once where an entity is, following it as it moves.",
        g!(|g, cx, a| g.play(a[0], Place::Entity(a[1].handle().unwrap_or(0) as u32), &a[2..], false)),
    );
    api.method(sound, "loopOn(entity: Entity, volume?: number, pitch?: number, reach?: number): Playing", "Plays it over and over on an entity, until it is stopped.", g!(|g, cx, a| {
        g.play(a[0], Place::Entity(a[1].handle().unwrap_or(0) as u32), &a[2..], true)
    }));
    api.prop(playing, "playing", "boolean", "Whether it is still playing (heard or not).", g!(|g, cx, a| Value::Bool(g.voice(a[0]).is_some())), None);
    api.prop(
        playing,
        "volume",
        "number",
        "Its loudness: 1 as recorded.",
        g!(|g, cx, a| Value::Num(g.voice(a[0]).map_or(0.0, |v| v.volume))),
        Some(g!(|g, cx, a| {
            let volume = a[1].num().max(0.0);
            if let Some(voice) = g.voice(a[0]) {
                voice.volume = volume;
                let playing = voice.playing;
                g.audio.set_sound_volume(playing, volume);
            }
            Value::Null
        })),
    );
    api.method(playing, "stop()", "Stops it. Nothing happens if it has ended.", g!(|g, cx, a| {
        if let Some(playing) = g.voice(a[0]).map(|v| v.playing) {
            g.audio.stop(playing);
        }
        Value::Null
    }));
    api.method(playing, "moveTo(position: Vec3)", "Moves it to a place in the level.", g!(|g, cx, a| {
        let place = g.at(v3(a[1]));
        if let Some(playing) = g.voice(a[0]).map(|v| v.playing) {
            g.audio.set_place(playing, place);
        }
        Value::Null
    }));
    api.method(playing, "follow(entity: Entity)", "Moves it onto an entity, which it follows.", g!(|g, cx, a| {
        if let (Some(playing), Some(entity)) = (g.voice(a[0]).map(|v| v.playing), a[1].handle()) {
            g.audio.set_place(playing, Place::Entity(entity as u32));
        }
        Value::Null
    }));
    api.method(playing, "ended(): Promise<void>", "A promise that settles when it has ended or been stopped: `await door.playOn(self).ended()`.", g!(|g, cx, a| {
        match (g.voice(a[0]).is_some(), a[0].handle()) {
            (true, Some(key)) => {
                let promise = cx.promise();
                g.kept.ended.push((key as u32, promise));
                promise
            }
            _ => cx.settled(Value::Null),
        }
    }));

    api.event("onStart()", "The level has started: every script's variables have their starting values.");
    api.event("onTick(dt: number)", "A frame has passed: `dt` is its length in seconds.");
    api.event(
        "onActivated(target: Entity, by: Player)",
        "A player used `target` (the use key, looking at it from near). Heard by the scripts attached to it or with an input bound to it.",
    );
    api.event("onEntered(sector: Sector, by: Player)", "A player walked into `sector`. Heard by the scripts with an input bound to it.");
    api.event("onExited(sector: Sector, by: Player)", "A player walked out of `sector`.");
    api.event(
        "onTouched(target: Entity, other: Entity | null, speed: number)",
        "`target` began to touch something, coming together at `speed` m/s: another entity, or the level itself or terrain (`other` is null). One of the two has a mass, or was moved by a script. Both are told, each as `target`.",
    );
    api.event(
        "onBumped(target: Entity, by: Player, speed: number)",
        "A player ran into `target` (or landed on it), at `speed` m/s: once, however long they lean on it.",
    );
    api
}

/// Warns that entity `i` isn't physics' to move, so it can't `what`.
fn not_physics(g: &Game<'_>, cx: &mut Cx<'_>, i: usize, what: &str) {
    let name = g.world.entities.get(i).map_or("?", |e| e.name.as_str());
    cx.warn(format!("'{name}' can't {what}: physics doesn't move it (it has no mass in the level, or a script is moving it)"));
}

/// The entity `this` names, if scripts can move it; says why not otherwise.
fn movable(g: &Game<'_>, cx: &mut Cx<'_>, this: Value) -> Option<usize> {
    let i = this.handle()?;
    let entity = g.world.entities.get(i)?;
    if entity.is_static || !entity.kind.is_drawn() {
        cx.warn(format!("'{}' can't be moved: the level says it never moves (static), or it is not a prop or an actor", entity.name));
        return None;
    }
    Some(i)
}

/// A placed script that is running: by a row of the level's `scripts` (`entity` is
/// `None`), or on an entity.
struct Placed {
    id: InstanceId,
    file: String,
    entity: Option<usize>,
    row: usize,
}

/// The level's scripts.
pub(crate) struct Scripts {
    runtime: Runtime<GameKind>,
    desc: ApiDesc,
    /// Where script files are: `assets/scripts`.
    dir: PathBuf,
    placed: Vec<Placed>,
    /// Instances to start at the next tick, each with the state of the one it replaces (a
    /// script that changed), if it replaces one: then it isn't told the level started.
    starting: Vec<(InstanceId, Option<Vec<(String, Type, Value)>>)>,
    changes: Vec<Change>,
    kept: Kept,
    /// The scripts were just placed, with their state kept or not: what the ones before
    /// them left going is ended at the next tick.
    placed_anew: Option<bool>,
    /// The sector the player was last in, for `onEntered` and `onExited`.
    sector: Option<u32>,
    /// When each script file last changed, as far as the app knows.
    watched: HashMap<PathBuf, SystemTime>,
}

/// The `.ts` files under `dir` (but the declarations, which aren't a script).
fn script_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for path in std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| e.ok().map(|e| e.path())) {
        if path.is_dir() {
            script_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "ts") && !path.to_string_lossy().ends_with(".d.ts") {
            out.push(path);
        }
    }
}

impl Scripts {
    pub(crate) fn new(dir: PathBuf) -> Scripts {
        let api = api();
        Scripts {
            desc: api.desc.clone(),
            runtime: Runtime::new(api),
            dir,
            placed: Vec::new(),
            starting: Vec::new(),
            changes: Vec::new(),
            kept: Kept::default(),
            placed_anew: None,
            sector: None,
            watched: HashMap::new(),
        }
    }

    /// The value of input `global` from `text`, as its placement writes it (or its
    /// fallback, with none written).
    fn input(&mut self, world: &World, assets: &mut Assets, global: &GlobalDef, text: Option<&str>) -> Result<Value, String> {
        let GlobalKind::Input { fallback } = &global.kind else { unreachable!() };
        let (ty, nullable) = match &global.ty {
            Type::Nullable(inner) => (&**inner, true),
            ty => (ty, false),
        };
        let Some(text) = text else {
            return match fallback {
                Some(fallback) => Ok(*fallback),
                None if nullable => Ok(Value::Null),
                None => Err("is not set".to_string()),
            };
        };
        if nullable && text == "none" {
            return Ok(Value::Null);
        }
        // One of the level's things, by its name: just one may have it.
        let named = |what: &str, names: &mut dyn Iterator<Item = Option<&str>>| {
            let found: Vec<usize> = names.enumerate().filter(|(_, n)| *n == Some(text)).map(|(i, _)| i).collect();
            match found.as_slice() {
                [i] => Ok(Value::Handle(*i as u32)),
                [] => Err(format!("no {what} is named '{text}'")),
                _ => Err(format!("{} {what}s are named '{text}'", found.len())),
            }
        };
        let numbers = || moose_assets::meta::parse_numbers(text);
        match ty {
            Type::Number => Ok(Value::Num(text.parse::<f32>().ok().filter(|x| x.is_finite()).ok_or(format!("'{text}' is not a number"))?)),
            Type::Int => Ok(Value::Int(text.parse().map_err(|_| format!("'{text}' is not a whole number"))?)),
            Type::Bool => match text {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(format!("'{text}' is not true or false")),
            },
            Type::Str => Ok(Value::Str(self.runtime.strings.intern(text))),
            Type::Class(class) => match self.desc.classes[*class as usize].name.as_str() {
                "Entity" => named("entity", &mut world.entities.iter().map(|e| Some(e.name.as_str()))),
                "Sector" => named("sector", &mut world.sectors.iter().map(|s| Some(s.name.as_str()))),
                "Light" => named("light", &mut world.light_names.iter().map(|n| n.as_deref())),
                "Sound" => assets.load_sound(text).map(|id| Value::Handle(id.0)).map_err(|e| e.to_string()),
                "Vec3" | "Color" => match numbers()?.as_slice() {
                    [x, y, z] => Ok(Value::Vec3([*x, *y, *z])),
                    _ => Err(format!("'{text}' is not three numbers (1,2,3)")),
                },
                name => Err(format!("a {name} can't be set from a level")),
            },
            _ => Err("this type can't be set from a level".to_string()),
        }
    }

    /// Places the world's scripts anew: compiles them and binds their inputs. With `keep`
    /// (a script file changed, the world didn't), each keeps the state it had. Returns
    /// the problems: a script with one isn't run.
    pub(crate) fn place(&mut self, world: &World, assets: &mut Assets, level: &str, keep: bool) -> Vec<String> {
        let mut kept: Vec<((String, Option<usize>, usize), Vec<(String, Type, Value)>)> = Vec::new();
        if keep {
            for p in &self.placed {
                kept.push(((p.file.clone(), p.entity, p.row), self.runtime.state(p.id)));
            }
            // What was starting still is, as it was.
            for (id, state) in self.starting.drain(..) {
                if let Some(p) = self.placed.iter().find(|p| p.id == id) {
                    let key = (p.file.clone(), p.entity, p.row);
                    match state {
                        Some(state) => kept.iter_mut().filter(|k| k.0 == key).for_each(|k| k.1 = state.clone()),
                        None => kept.retain(|k| k.0 != key),
                    }
                }
            }
        } else {
            self.sector = None;
        }
        self.runtime.clear();
        self.runtime.take_log();
        self.placed.clear();
        self.starting.clear();
        self.changes.clear();
        self.kept.ended.clear();
        // A full restart forgets a change of script that came just before it.
        self.placed_anew = Some(keep && self.placed_anew != Some(false));

        let placements: Vec<(Option<usize>, usize, &ScriptPlacement)> = world
            .scripts
            .iter()
            .enumerate()
            .map(|(row, s)| (None, row, s))
            .chain(world.entities.iter().enumerate().filter_map(|(i, e)| e.script.as_ref().map(|s| (Some(i), i, s))))
            .collect();
        let mut programs: HashMap<String, Option<Rc<Program>>> = HashMap::new();
        let mut problems = Vec::new();
        for (entity, row, placement) in placements {
            if !programs.contains_key(&placement.file) {
                let dir = self.dir.clone();
                let compiled = compile(&self.desc, &mut self.runtime.strings, &placement.file, &mut |file| {
                    std::fs::read_to_string(dir.join(file)).map_err(|e| format!("cannot be read: {e}"))
                });
                programs.insert(placement.file.clone(), match compiled {
                    Ok(program) => Some(Rc::new(program)),
                    Err(e) => {
                        problems.push(e.to_string());
                        None
                    }
                });
            }
            let Some(program) = programs[&placement.file].clone() else {
                continue;
            };
            let at = format!("{level}:{}: {}", placement.line, placement.file);
            let before = problems.len();
            if program.uses_self && entity.is_none() {
                problems.push(format!("{at} uses 'self', so it belongs on an entity or a template (script={})", placement.file));
            }
            for (name, _) in &placement.inputs {
                if !program.inputs().any(|(_, g)| g.name == *name) {
                    let has: Vec<&str> = program.inputs().map(|(_, g)| g.name.as_str()).collect();
                    problems.push(format!("{at} has no input '{name}' (it has: {})", has.join(", ")));
                }
            }
            let mut values = Vec::new();
            for (_, global) in program.inputs() {
                let text = placement.inputs.iter().find(|(name, _)| *name == global.name).map(|(_, text)| text.as_str());
                match self.input(world, assets, global, text) {
                    Ok(value) => values.push(value),
                    Err(why) => problems.push(format!("{at}: input '{}': {why}", global.name)),
                }
            }
            if problems.len() > before {
                continue;
            }
            let owner = entity.map_or(Value::Null, |i| Value::Handle(i as u32));
            let id = self.runtime.spawn_instance(program, owner, &values);
            let key = (placement.file.clone(), entity, row);
            let state = kept.iter().position(|k| k.0 == key).map(|at| kept.swap_remove(at).1);
            self.placed.push(Placed { id, file: placement.file.clone(), entity, row });
            self.starting.push((id, state));
        }
        problems
    }

    /// Whether a script file changed since this was last asked (never the first time).
    pub(crate) fn changed(&mut self) -> bool {
        let first = self.watched.is_empty();
        let mut files = Vec::new();
        script_files(&self.dir, &mut files);
        let mut changed = false;
        for path in files {
            let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
                continue;
            };
            changed |= self.watched.insert(path, modified) != Some(modified);
        }
        changed && !first
    }

    /// How many scripts are placed and running.
    pub(crate) fn running(&self) -> usize {
        self.placed.len()
    }

    /// Runs the scripts for a frame `dt` seconds long, after physics has: starts the ones
    /// just placed, moves what they set moving (and its bodies after it), tells them of
    /// the frame, of what touched what, of the sectors the player crossed between, and of
    /// the entity the player used (`used`), and carries on the handlers whose promises
    /// settled (a move done, a sound ended). Returns what they logged.
    pub(crate) fn tick(&mut self, stage: Stage<'_>, dt: f32, used: Option<usize>) -> Vec<Line> {
        let event = |name: &str| self.desc.event_id(name).expect("an event of the table");
        let [on_start, on_tick, on_activated, on_entered, on_exited, on_touched, on_bumped] =
            ["onStart", "onTick", "onActivated", "onEntered", "onExited", "onTouched", "onBumped"].map(event);
        let class = |name: &str| self.desc.class_id(name).expect("a class of the table");
        let (entity_class, sector_class) = (class("Entity"), class("Sector"));
        let mut game = Game {
            world: stage.world,
            assets: stage.assets,
            lights: &mut stage.lights.0,
            ambient: &mut stage.lights.1,
            camera: stage.camera,
            physics: stage.physics,
            player: stage.player,
            audio: stage.audio,
            changes: &mut self.changes,
            kept: &mut self.kept,
        };
        // What the scripts before these left going: bodies they were moving are physics'
        // again, and unless these carry on from them, their sounds stop.
        if let Some(carried_on) = self.placed_anew.take() {
            game.physics.release_all();
            if !carried_on {
                for voice in game.kept.voices.drain(..) {
                    game.audio.stop(voice.playing);
                }
                game.kept.hits.clear();
            }
        }
        for (id, state) in std::mem::take(&mut self.starting) {
            self.runtime.start(&mut game, id);
            match state {
                Some(state) => self.runtime.restore(id, &state),
                None => self.runtime.fire_one(&mut game, id, on_start, &[]),
            }
        }
        self.runtime.tick(&mut game, dt);
        // What is on its way somewhere, as far as it has got.
        let now = self.runtime.now();
        let mut i = 0;
        while i < game.changes.len() {
            let c = &game.changes[i];
            let t = if c.seconds > 0.0 { (((now - c.start) / c.seconds as f64) as f32).clamp(0.0, 1.0) } else { 1.0 };
            let (what, at, promise) = (c.what, c.from.lerp(c.to, t), c.promise);
            match what {
                Changing::Entity(e) if e < game.world.entities.len() => game.place(e, at),
                Changing::Light(l) if l < game.lights.len() => game.lights[l].color = at,
                _ => {}
            }
            if t >= 1.0 {
                game.changes.swap_remove(i);
                // (One with a mass is physics' again, at rest where it was taken.)
                if let Changing::Entity(e) = what {
                    game.physics.release(e as u32);
                }
                self.runtime.resolve(promise, Value::Null);
            } else {
                i += 1;
            }
        }
        // The sounds that have ended: who waited for one goes on, and it is forgotten.
        let mut i = 0;
        while i < game.kept.ended.len() {
            let (key, promise) = game.kept.ended[i];
            if game.voice(Value::Handle(key)).is_none() {
                game.kept.ended.swap_remove(i);
                self.runtime.resolve(promise, Value::Null);
            } else {
                i += 1;
            }
        }
        let audio = &*game.audio;
        game.kept.voices.retain(|v| audio.is_playing(v.playing));

        let player = Value::Handle(0);
        // What began to touch in this frame's physics.
        for touch in game.physics.take_touches() {
            let speed = Value::Num(touch.speed);
            let entity = |i: u32| Value::Handle(i);
            match (touch.a, touch.b) {
                (Solid::Entity(a), Solid::Entity(b)) => {
                    self.runtime.fire(&mut game, on_touched, Some((entity_class, entity(a))), &[entity(a), entity(b), speed]);
                    self.runtime.fire(&mut game, on_touched, Some((entity_class, entity(b))), &[entity(b), entity(a), speed]);
                }
                (Solid::Entity(e), Solid::Character(_)) | (Solid::Character(_), Solid::Entity(e)) => {
                    self.runtime.fire(&mut game, on_bumped, Some((entity_class, entity(e))), &[entity(e), player, speed]);
                }
                (Solid::Entity(e), _) | (_, Solid::Entity(e)) => {
                    self.runtime.fire(&mut game, on_touched, Some((entity_class, entity(e))), &[entity(e), Value::Null, speed]);
                }
                _ => {}
            }
        }
        let sector = game.camera.sector;
        if self.sector != Some(sector) {
            if let Some(old) = self.sector.replace(sector) {
                let old = Value::Handle(old);
                self.runtime.fire(&mut game, on_exited, Some((sector_class, old)), &[old, player]);
            }
            let new = Value::Handle(sector);
            self.runtime.fire(&mut game, on_entered, Some((sector_class, new)), &[new, player]);
        }
        if let Some(used) = used {
            let target = Value::Handle(used as u32);
            self.runtime.fire(&mut game, on_activated, Some((entity_class, target)), &[target, player]);
        }
        self.runtime.fire(&mut game, on_tick, None, &[Value::Num(dt)]);
        self.runtime.take_log()
    }
}

/// The entity the camera looks at from within reach, with nothing of the level in between.
fn looked_at(world: &World, camera: &Camera) -> Option<usize> {
    let (from, along) = (camera.position, camera.forward());
    let mut nearest: Option<(f32, usize)> = None;
    for (i, entity) in world.entities.iter().enumerate().filter(|(_, e)| e.kind.is_drawn()) {
        // Where the ray is inside its box, slab by slab.
        let (mut enter, mut leave) = (0.0f32, REACH);
        for axis in 0..3 {
            let (lo, hi) = (entity.bounds.min[axis] - from[axis], entity.bounds.max[axis] - from[axis]);
            if along[axis].abs() < 1e-6 {
                if lo > 0.0 || hi < 0.0 {
                    leave = -1.0;
                }
                continue;
            }
            let (a, b) = (lo / along[axis], hi / along[axis]);
            enter = enter.max(a.min(b));
            leave = leave.min(a.max(b));
        }
        if enter <= leave && nearest.is_none_or(|(t, _)| enter < t) {
            nearest = Some((enter, i));
        }
    }
    let (t, i) = nearest?;
    (!world.trace(camera.sector, from, from + along * t).blocked).then_some(i)
}

impl App {
    /// Places the level's scripts (see [`Scripts::place`]) and says what is wrong with them.
    /// Returns how many problems there were.
    pub(crate) fn place_scripts(&mut self, keep: bool) -> usize {
        let problems = self.scripts.place(&self.world, &mut self.assets, &self.level, keep);
        let count = problems.len();
        for problem in problems {
            self.script_says(Line { text: problem, error: true });
        }
        count
    }

    /// Reads the level's scripts again and starts them over, from their starting values
    /// (the editor's Reload the scripts): what they changed in the level stays changed.
    pub(crate) fn restart_scripts(&mut self) {
        if self.place_scripts(false) == 0 {
            self.editor.say(format!("reloaded the scripts ({} placed, starting over)", self.scripts.running()));
        }
    }

    /// Places the scripts again if one of their files changed, each keeping its state.
    pub(crate) fn reload_scripts(&mut self) {
        if self.scripts.changed() {
            self.place_scripts(true);
            self.editor.say(format!("reloaded the scripts ({} running)", self.scripts.running()));
        }
    }

    /// Runs the scripts for a frame; `using` is whether the player pressed Use (`Setting::Use`'s key).
    pub(crate) fn run_scripts(&mut self, dt: f32, using: bool) {
        let used = if using { looked_at(&self.world, &self.camera) } else { None };
        let stage = Stage {
            world: &mut self.world,
            assets: &mut self.assets,
            lights: &mut self.lights,
            camera: &mut self.camera,
            physics: &mut self.physics,
            player: self.player,
            audio: &mut self.audio,
        };
        let lines = self.scripts.tick(stage, dt, used);
        for line in lines {
            self.script_says(line);
        }
    }

    /// Shows a line of the scripts' log: on the console, and on screen for a moment.
    fn script_says(&mut self, line: Line) {
        if line.error {
            eprintln!("script: {}", line.text);
            self.editor.warn(line.text.clone());
        } else {
            println!("{}", line.text);
        }
        self.toast = Some((line.text, Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moose_audio::Listener;
    use moose_physics::CharacterConfig;
    use moose_script::declarations;

    const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

    /// `assets/scripts/moose.d.ts` is what the table says. After changing the table, write
    /// it again: `MOOSE_WRITE_DTS=1 cargo test -p moose-app moose_d_ts`.
    #[test]
    fn moose_d_ts_is_what_the_table_says() {
        let path = Path::new(ASSETS).join("scripts/moose.d.ts");
        let text = declarations(&api().desc);
        if std::env::var_os("MOOSE_WRITE_DTS").is_some() {
            std::fs::write(&path, &text).unwrap();
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(on_disk == text, "{} is out of date: MOOSE_WRITE_DTS=1 cargo test -p moose-app moose_d_ts", path.display());
    }

    /// The scripted test level, with `more` after its last section.
    fn level(more: &str) -> String {
        format!("{}\n{more}", std::fs::read_to_string(Path::new(ASSETS).join("levels/script_rooms.mmp")).unwrap())
    }

    /// A level with its physics, sound (no device) and scripts, run a frame at a time as
    /// the app does: physics, the entities after their bodies, the scripts, the sound.
    struct Rig {
        world: World,
        assets: Assets,
        lights: (Vec<Light>, Vec3),
        camera: Camera,
        physics: Physics,
        player: CharacterId,
        audio: Audio,
        scripts: Scripts,
    }

    impl Rig {
        /// The level in `src`, with the scripts in `dir`; and what is wrong with them.
        fn new(src: &str, dir: PathBuf) -> (Rig, Vec<String>) {
            let mut assets = Assets::new(ASSETS);
            assets.load_materials().unwrap();
            let level = assets.parse_level("test.mmp", src).unwrap_or_else(|e| panic!("{e}"));
            let world = World::new(level, &assets);
            let lights = (world.lights().to_vec(), world.ambient);
            let viewport = moose_scene::Viewport { x: 0, y: 0, width: 64, height: 64 };
            let spawn = &world.spawn_points[0];
            let mut camera = Camera::at_spawn(spawn, viewport);
            camera.move_to(&world, camera.position + Vec3::Y * crate::EYE_HEIGHT);
            let mut physics = Physics::new(&world, &assets);
            let player = physics.add_character(spawn.position, CharacterConfig::default());
            let mut scripts = Scripts::new(dir);
            let problems = scripts.place(&world, &mut assets, "test.mmp", false);
            (Rig { world, assets, lights, camera, physics, player, audio: Audio::silent(), scripts }, problems)
        }

        /// The scripted test level as it is.
        fn scripted() -> Rig {
            let (rig, problems) = Rig::new(&level(""), Path::new(ASSETS).join("scripts"));
            assert_eq!(problems, Vec::<String>::new());
            rig
        }

        /// A frame of `dt` seconds, the player using entity `used`. Returns what scripts logged.
        fn frame_of(&mut self, dt: f32, used: Option<usize>) -> Vec<Line> {
            self.physics.advance(dt);
            for (entity, position, rotation) in self.physics.bodies().collect::<Vec<_>>() {
                self.world.move_entity(entity as usize, position, rotation, &self.assets);
            }
            let stage = Stage {
                world: &mut self.world,
                assets: &mut self.assets,
                lights: &mut self.lights,
                camera: &mut self.camera,
                physics: &mut self.physics,
                player: self.player,
                audio: &mut self.audio,
            };
            let lines = self.scripts.tick(stage, dt, used);
            self.audio.update(&self.world, &self.assets, &Listener::of(&self.camera), dt);
            lines
        }

        /// A frame of a tenth of a second in which nothing goes wrong.
        fn frame(&mut self, used: Option<usize>) -> Vec<Line> {
            let lines = self.frame_of(0.1, used);
            assert!(lines.iter().all(|l| !l.error), "{lines:?}");
            lines
        }

        fn frames(&mut self, count: usize) {
            for _ in 0..count {
                self.frame(None);
            }
        }

        fn entity(&self, name: &str) -> usize {
            self.world.entities.iter().position(|e| e.name == name).unwrap()
        }

        /// The level's `$key` value.
        fn value(&self, key: &str) -> Option<f32> {
            self.world.meta_keys.id(key).and_then(|k| self.world.meta.get(k)).map(|v| v[0])
        }

        /// Puts the camera (which the sectors' events follow) at `position`.
        fn look_from(&mut self, position: Vec3) {
            (self.camera.position, self.camera.sector) = (position, self.world.find_sector(position).unwrap());
        }
    }

    #[test]
    fn every_script_in_the_assets_compiles() {
        let dir = Path::new(ASSETS).join("scripts");
        let (desc, mut files) = (api().desc, Vec::new());
        script_files(&dir, &mut files);
        assert!(!files.is_empty());
        for path in files {
            let file = path.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
            let mut strings = moose_script::Strings::default();
            let compiled = compile(&desc, &mut strings, &file, &mut |f| std::fs::read_to_string(dir.join(f)).map_err(|e| e.to_string()));
            // (A file of shared functions compiles as a script that handles nothing.)
            compiled.unwrap_or_else(|e| panic!("{e}"));
        }
    }

    #[test]
    fn the_scripted_level_runs_its_scripts() {
        let mut rig = Rig::scripted();
        assert_eq!(rig.scripts.running(), 4);
        let lamp = rig.world.light_names.iter().position(|n| n.as_deref() == Some("lamp")).unwrap();
        let lift = rig.entity("lift");
        let (dim, low) = (rig.lights.0[lamp].color, rig.world.entities[lift].position);

        // Starting: the level script counts in `$visits`, and the lift's lamp is as it was.
        rig.frame(None);
        assert_eq!(rig.value("visits"), Some(0.0));
        assert_eq!(rig.lights.0[lamp].color, dim);

        // Using the lift: it rises over two seconds, humming, while its lamp turns green,
        // and both being done, it is quiet.
        rig.frame(Some(lift));
        rig.frames(10);
        let halfway = rig.world.entities[lift].position.y - low.y;
        assert!(halfway > 0.4 && halfway < 0.8, "{halfway}");
        assert_ne!(rig.lights.0[lamp].color, dim);
        let humming = rig.scripts.kept.voices.len();
        // (Used again on its way: it is busy, and carries on.)
        rig.frame(Some(lift));
        rig.frames(15);
        assert!((rig.world.entities[lift].position.y - low.y - 1.2).abs() < 1e-4);
        assert!((rig.lights.0[lamp].color - Vec3::new(40.0, 255.0, 40.0) / FULL).length() < 1e-4);
        assert_eq!((humming, rig.scripts.kept.voices.len()), (1, 0));
        // And back down.
        rig.frame(Some(lift));
        rig.frames(25);
        assert!((rig.world.entities[lift].position - low).length() < 1e-4);

        // Walking into the hallway is counted, once each time.
        let hall = rig.world.sectors.iter().position(|s| s.name == "hallway").unwrap();
        let (home, there) = (rig.camera.position, rig.world.sectors[hall].center);
        rig.look_from(there);
        rig.frames(2);
        assert_eq!(rig.value("visits"), Some(1.0));
        rig.look_from(home);
        rig.frame(None);
        rig.look_from(there);
        let lines = rig.frame(None);
        assert_eq!(rig.value("visits"), Some(2.0));
        assert_eq!(lines, [Line { text: "hallway visits: 2".into(), error: false }]);

        // A script file changing: placed again, each keeps its state.
        assert_eq!(rig.scripts.place(&rig.world, &mut rig.assets, "script_rooms.mmp", true), Vec::<String>::new());
        rig.frame(None);
        rig.look_from(home);
        rig.frame(None);
        rig.look_from(there);
        rig.frame(None);
        assert_eq!(rig.value("visits"), Some(3.0));
    }

    #[test]
    fn what_scripts_move_physics_moves_and_what_physics_moves_scripts_see() {
        let mut rig = Rig::scripted();
        let (lift, ball) = (rig.entity("lift"), rig.entity("ball"));
        let low = rig.world.entities[lift].position;
        // The ball, dropped by the level from a meter up, has landed: its entity is where
        // its body is, on the floor, and its script heard it land and played its thump out.
        rig.frames(20);
        let rest = rig.world.entities[ball].position;
        let body = rig.physics.bodies().find(|b| b.0 == ball as u32).unwrap().1;
        assert!(rest.distance(body) < 1e-4 && rest.y < 0.5 && rest.y > 0.1, "{rest} and {body}");
        assert!(rig.value("thumps") >= Some(1.0), "{:?}", rig.value("thumps"));
        assert_eq!(rig.value("thumps_heard"), rig.value("thumps"));
        // Used, its script pushes it: physics sends it up, and the entity goes with it.
        rig.frame(Some(ball));
        rig.frames(2);
        let up = rig.world.entities[ball].position.y - rest.y;
        assert!(up > 0.4, "{up}");
        rig.frames(20);
        assert!(rig.world.entities[ball].position.y - rest.y < 0.05);

        // The player on the lift: used, it carries them up with it, and its top is solid
        // where it went, not where the level had it.
        rig.physics.teleport(rig.player, low + Vec3::Y * 1.02);
        rig.frames(5);
        assert!(rig.physics.character(rig.player).grounded());
        rig.frame(Some(lift));
        rig.frames(25);
        let top = rig.world.entities[lift].position.y + 1.0;
        assert!((top - low.y - 2.2).abs() < 1e-3);
        let feet = rig.physics.feet(rig.player);
        assert!((feet.y - top).abs() < 0.06 && rig.physics.character(rig.player).grounded(), "feet at {feet}, its top at {top}");
        let over = Vec3::new(low.x + 0.3, top + 0.4, low.z + 0.3);
        let hit = rig.physics.ray(over, Vec3::NEG_Y, 10.0, Some(rig.player)).unwrap();
        assert_eq!(hit.solid, Solid::Entity(lift as u32));
        assert!((hit.point.y - top).abs() < 1e-3, "{hit:?}");
        // And down again with it.
        rig.frame(Some(lift));
        rig.frames(25);
        let feet = rig.physics.feet(rig.player);
        assert!((feet.y - low.y - 1.0).abs() < 0.06, "feet at {feet}");
    }

    /// A copy of the assets' scripts with `probe.ts` added, in a folder of its own.
    fn scripts_with_probe(test: &str, probe: &str) -> PathBuf {
        let (from, to) = (Path::new(ASSETS).join("scripts"), std::env::temp_dir().join(format!("moose_scripts_{test}")));
        std::fs::create_dir_all(to.join("util")).unwrap();
        for file in ["lift.ts", "ball.ts", "hall_counter.ts", "pulse.ts", "util/lights.ts"] {
            std::fs::copy(from.join(file), to.join(file)).unwrap();
        }
        std::fs::write(to.join("probe.ts"), probe).unwrap();
        to
    }

    #[test]
    fn scripts_reach_physics_and_sound() {
        let probe = r#"
            const ball = input<Entity>();
            const lift = input<Entity>();
            const hum = input<Sound>();

            export function onStart() {
              const hit = physics.ray(lift.position.add(vec3(0.2, 3, 0.2)), vec3(0, -1, 0), 10);
              if (hit !== null) {
                level.set("hit_distance", hit.distance);
                level.set("hit_lift", hit.entity === lift ? 1 : 0);
                level.set("hit_up", hit.normal.y);
                level.set("hit_y", hit.point.y);
              }
              level.set("miss", physics.ray(vec3(0, 1, 6), vec3(0, 1, 0), 0.5) === null ? 1 : 0);
              level.set("floor", physics.ray(vec3(0, 1, 6), vec3(0, -1, 0), 5)?.entity === null ? 1 : 0);
              level.set("ball_mass", ball.mass);
              level.set("lift_mass", lift.mass);
              level.set("gravity", physics.gravity);
              level.set("hum_seconds", hum.seconds);
              lift.push(vec3(1, 0, 0));
              level.set("no_sound", sounds.find("no_such.wav") === null ? 1 : 0);
            }

            export async function onActivated(target: Entity, by: Player) {
              if (target === ball) {
                ball.position = vec3(-1, 2.5, 4.5);
                ball.velocity = vec3(1, 0, 0);
                return;
              }
              const playing = hum.loopAt(lift.position, 0.5, 1.0, 20);
              await ball.moveTo(vec3(-1, 2, 4.5), 0.5);
              level.set("carried_to", ball.position.y);
              playing.volume = 0.25;
              level.set("volume", playing.playing ? playing.volume : -1);
              playing.stop();
              await playing.ended();
              level.set("stopped", playing.playing ? 0 : 1);
              by.teleport(vec3(0, 0, 4));
            }

            export function onBumped(target: Entity, by: Player, speed: number) {
              level.set("bumped", level.get("bumped", 0) + 1);
              level.set("bump_speed", speed);
              level.set("on_ground", by.grounded ? 1 : 0);
            }
        "#;
        let src = level("   2   probe.ts ball=ball lift=lift hum=test_hum.wav\n").replace("scripts 2", "scripts 3");
        let (mut rig, problems) = Rig::new(&src, scripts_with_probe("probe", probe));
        assert_eq!(problems, Vec::<String>::new());
        let (lift, ball) = (rig.entity("lift"), rig.entity("ball"));

        // Starting: a ray meets the lift's top two meters down, and the floor is no entity.
        let lines = rig.frame_of(0.1, None);
        let warned: Vec<&str> = lines.iter().filter(|l| l.error).map(|l| l.text.as_str()).collect();
        assert_eq!(warned.len(), 2, "{lines:?}");
        assert!(warned[0].contains("'lift' can't be pushed: physics doesn't move it"), "{warned:?}");
        assert!(warned[1].contains("no_such.wav"), "{warned:?}");
        let value = |rig: &Rig, key: &str| rig.value(key).unwrap_or_else(|| panic!("${key} isn't set"));
        assert!((value(&rig, "hit_distance") - 2.0).abs() < 1e-3 && (value(&rig, "hit_y") - 1.0).abs() < 1e-3);
        assert_eq!((value(&rig, "hit_lift"), value(&rig, "hit_up"), value(&rig, "miss"), value(&rig, "floor")), (1.0, 1.0, 1.0, 1.0));
        assert_eq!((value(&rig, "ball_mass"), value(&rig, "lift_mass"), value(&rig, "no_sound")), (5.0, 0.0, 1.0));
        assert!((value(&rig, "gravity") - 9.81).abs() < 1e-4 && value(&rig, "hum_seconds") > 0.1);

        // The ball put somewhere by a script: its body is there at once, with the speed it
        // was given (and the hop its own script gives it when used), and goes on from there.
        rig.frames(15);
        rig.frame(Some(ball));
        let body = rig.physics.bodies().find(|b| b.0 == ball as u32).unwrap().1;
        assert!(body.distance(Vec3::new(-1.0, 2.5, 4.5)) < 1e-4, "{body}");
        let speed = rig.physics.velocity(ball as u32).unwrap();
        assert!((speed - Vec3::new(1.0, 4.0, 0.0)).length() < 1e-3, "{speed}");
        rig.frames(3);
        let gone = rig.world.entities[ball].position;
        assert!(gone.x > -0.8 && (gone.y - 3.25).abs() < 0.15, "{gone}");
        rig.frames(30);
        assert!(rig.world.entities[ball].position.y < 0.5);

        // Moved by a script over time, it is the script's (it doesn't fall, and physics
        // gives no place for it) and then physics' again; a sound loops meanwhile, is
        // turned down, stopped and waited for; and the player is put elsewhere. (The lift,
        // used, rises too, humming for itself.)
        rig.frame(Some(lift));
        rig.frames(3);
        assert_eq!((rig.physics.bodies().count(), rig.audio.counts().0), (0, 2));
        let on_its_way = rig.world.entities[ball].position.y;
        assert!(on_its_way > 0.7 && on_its_way < 2.0, "{on_its_way}");
        rig.frames(3);
        assert_eq!((value(&rig, "carried_to"), value(&rig, "volume")), (2.0, 0.25));
        rig.frames(2);
        assert_eq!((value(&rig, "stopped"), rig.audio.counts().0, rig.physics.bodies().count()), (1.0, 1, 1));
        assert!(rig.world.entities[ball].position.y < 2.0);
        assert!(rig.camera.position.distance(Vec3::new(0.0, crate::EYE_HEIGHT, 4.0)) < 1e-4);

        // The player walks into the lift (up now, at head height): its scripts are told, once.
        rig.frames(20);
        assert_eq!(rig.audio.counts().0, 0);
        let at = rig.world.entities[lift].position;
        rig.physics.teleport(rig.player, Vec3::new(at.x, 0.0, at.z + 1.5));
        rig.frames(3);
        assert_eq!(rig.value("bumped"), None);
        rig.physics.character_mut(rig.player).wish = Vec3::NEG_Z * 2.0;
        rig.frames(10);
        assert_eq!((value(&rig, "bumped"), value(&rig, "on_ground")), (1.0, 1.0));
        assert!((value(&rig, "bump_speed") - 2.0).abs() < 0.1);
    }

    /// The app itself on `level`, as the app's own tests make it (no window, no sound device).
    fn app_on(level: &str) -> App {
        let args = ["--level", level, "--size", "64x36"].map(String::from);
        App::new(&crate::parse_options(args).unwrap()).unwrap()
    }

    #[test]
    fn in_the_app_a_pushed_ball_is_where_physics_took_it() {
        // cheese_1's beach ball: the player walks at it, and the entity (what is drawn, what
        // scripts and sounds are told) goes where its body is pushed to.
        let mut app = app_on("cheese_1.mmp");
        let ball = app.world.entities.iter().position(|e| e.name == "ball").unwrap();
        for _ in 0..30 {
            app.walk(Vec3::ZERO, false, false, 1.0 / 60.0);
        }
        let start = app.world.entities[ball].position;
        for _ in 0..240 {
            let to_ball = (app.world.entities[ball].position - app.camera.position) * Vec3::new(1.0, 0.0, 1.0);
            app.walk(to_ball.normalize_or_zero() * 3.0, false, false, 1.0 / 60.0);
        }
        let now = app.world.entities[ball].position;
        let body = app.physics.bodies().find(|b| b.0 == ball as u32).unwrap().1;
        assert!(now.distance(start) > 1.0, "the ball is still at {now}, where it started");
        assert!(now.distance(body) < 0.05, "the entity is at {now}, its body at {body}");
        assert!(app.world.entities[ball].bounds.min.cmple(now).all() && app.world.entities[ball].bounds.max.cmpge(now).all());
        // In the editor the level is shown as written: it is back where the level has it.
        app.rest_bodies();
        assert!(app.world.entities[ball].position.distance(app.editor.doc.entities.iter().find(|e| e.name == "ball").unwrap().position) < 1e-4);
    }

    #[test]
    fn in_the_app_the_scripted_levels_scripts_meet_physics_and_sound() {
        let mut app = app_on("script_rooms.mmp");
        assert_eq!(app.scripts.running(), 4);
        let ball = app.world.entities.iter().position(|e| e.name == "ball").unwrap();
        let value = |app: &App, key: &str| app.world.meta_keys.id(key).and_then(|k| app.world.meta.get(k)).map(|v| v[0]);
        // Two seconds of standing there: the ball the level hangs in the air has fallen,
        // its script heard it land, and played its thump out.
        for _ in 0..120 {
            app.walk(Vec3::ZERO, false, false, 1.0 / 60.0);
            app.run_scripts(1.0 / 60.0, false);
            app.audio.update(&app.world, &app.assets, &moose_audio::Listener::of(&app.camera), 1.0 / 60.0);
        }
        assert!(app.world.entities[ball].position.y < 0.5, "{}", app.world.entities[ball].position);
        assert!(value(&app, "thumps") >= Some(1.0));
        assert_eq!(value(&app, "thumps_heard"), value(&app, "thumps"));
        assert_eq!(app.audio.counts().0, 0);
    }

    #[test]
    fn what_is_wrong_with_a_placement_is_said_and_the_rest_runs() {
        let src = level("").replace("door=hallway", "door=attic").replace("input:rise=1.2", "input:rise=high input:lamp=sun");
        let src = src.replace("input:thump=land.wav", "input:thump=crash.wav");
        let src = format!("{src}\n   2   lift.ts\n   3   hall_counter.ts door=hallway speeed=2\n   4   missing.ts\n").replace("scripts 2", "scripts 5");
        let (rig, problems) = Rig::new(&src, Path::new(ASSETS).join("scripts"));
        let said = |what: &str| assert!(problems.iter().any(|p| p.contains(what)), "{what}: {problems:#?}");
        said("hall_counter.ts: input 'door': no sector is named 'attic'");
        said("lift.ts: input 'rise': 'high' is not a number");
        said("lift.ts: input 'lamp': no light is named 'sun'");
        said("lift.ts uses 'self', so it belongs on an entity or a template");
        said("hall_counter.ts has no input 'speeed' (it has: door");
        said("missing.ts: cannot be read");
        said("ball.ts: input 'thump': ");
        said("crash.wav");
        assert_eq!(problems.len(), 7, "{problems:#?}");
        // The one script left alone still runs.
        assert_eq!(rig.scripts.running(), 1);
    }
}
