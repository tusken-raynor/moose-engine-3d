//! The level's scripts (see the Scripting Spec): the table of what scripts can reach of the
//! world, and the scripts a level places, bound to its things and run each frame.
//!
//! The table is the one place a primitive is opened to scripts: its type, properties and
//! methods, and its manager (`entities`, `lights`, ...), each with the Rust that does it.
//! `assets/scripts/moose.d.ts`, which editors check scripts against, is written from it
//! (the `moose_d_ts` test says how).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use glam::Vec3;
use moose_assets::{Assets, Light, MetaValues, NO_SECTOR, ScriptPlacement};
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

/// What scripts' functions work on: the app's world, borrowed for a call.
pub(crate) struct Game<'a> {
    world: &'a mut World,
    assets: &'a Assets,
    /// The level's lights and ambient light as the app keeps them (`App::lights`), which
    /// it lights the world with every frame.
    lights: &'a mut Vec<Light>,
    ambient: &'a mut Vec3,
    camera: &'a Camera,
    changes: &'a mut Vec<Change>,
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
    /// Puts entity `i` at `position`, in the sector that is in.
    fn place(&mut self, i: usize, position: Vec3) {
        let sector = self.world.find_sector(position);
        let entity = &mut self.world.entities[i];
        entity.position = position;
        entity.sector = sector.unwrap_or(entity.sector);
        self.world.place_entity(i, self.assets);
    }

    /// Sets `what` on its way to `to` over `seconds`, instead of wherever it was going,
    /// and returns the promise of its getting there.
    fn change(&mut self, cx: &mut Cx<'_>, what: Changing, from: Vec3, to: Vec3, seconds: f32) -> Value {
        if let Some(at) = self.changes.iter().position(|c| c.what == what) {
            cx.resolve(self.changes.swap_remove(at).promise, Value::Null);
        }
        let promise = cx.promise();
        self.changes.push(Change { what, from, to, start: cx.now, seconds, promise });
        promise
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
        "Where it is. Setting it puts it there at once; `moveTo` takes it there over time.",
        g!(|g, cx, a| val(a[0].handle().and_then(|i| g.world.entities.get(i)).map_or(Vec3::ZERO, |e| e.position))),
        Some(g!(|g, cx, a| {
            if let Some(i) = movable(g, cx, a[0]) {
                g.changes.retain(|c| c.what != Changing::Entity(i));
                g.place(i, v3(a[1]));
            }
            Value::Null
        })),
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
        "Moves it to `position` over `seconds`, in a straight line. The promise settles when it is there (or when something else moves it).",
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

    api.event("onStart()", "The level has started: every script's variables have their starting values.");
    api.event("onTick(dt: number)", "A frame has passed: `dt` is its length in seconds.");
    api.event(
        "onActivated(target: Entity, by: Player)",
        "A player used `target` (the use key, looking at it from near). Heard by the scripts attached to it or with an input bound to it.",
    );
    api.event("onEntered(sector: Sector, by: Player)", "A player walked into `sector`. Heard by the scripts with an input bound to it.");
    api.event("onExited(sector: Sector, by: Player)", "A player walked out of `sector`.");
    api
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
            sector: None,
            watched: HashMap::new(),
        }
    }

    /// The value of input `global` from `text`, as its placement writes it (or its
    /// fallback, with none written).
    fn input(&mut self, world: &World, global: &GlobalDef, text: Option<&str>) -> Result<Value, String> {
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
    pub(crate) fn place(&mut self, world: &World, level: &str, keep: bool) -> Vec<String> {
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
                match self.input(world, global, text) {
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

    /// Runs the scripts for a frame `dt` seconds long: starts the ones just placed, moves
    /// what they set moving, tells them of the frame, of the sectors the player crossed
    /// between, and of the entity the player used (`used`), and carries on the handlers
    /// whose promises settled. Returns what they logged.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tick(
        &mut self,
        world: &mut World,
        assets: &Assets,
        lights: &mut (Vec<Light>, Vec3),
        camera: &Camera,
        dt: f32,
        used: Option<usize>,
    ) -> Vec<Line> {
        let event = |name: &str| self.desc.event_id(name).expect("an event of the table");
        let [on_start, on_tick, on_activated, on_entered, on_exited] =
            ["onStart", "onTick", "onActivated", "onEntered", "onExited"].map(event);
        let class = |name: &str| self.desc.class_id(name).expect("a class of the table");
        let (entity_class, sector_class) = (class("Entity"), class("Sector"));
        let mut game = Game { world, assets, lights: &mut lights.0, ambient: &mut lights.1, camera, changes: &mut self.changes };
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
                self.runtime.resolve(promise, Value::Null);
            } else {
                i += 1;
            }
        }
        let player = Value::Handle(0);
        if self.sector != Some(camera.sector) {
            if let Some(old) = self.sector.replace(camera.sector) {
                let old = Value::Handle(old);
                self.runtime.fire(&mut game, on_exited, Some((sector_class, old)), &[old, player]);
            }
            let new = Value::Handle(camera.sector);
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
        let problems = self.scripts.place(&self.world, &self.level, keep);
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
        let lines = self.scripts.tick(&mut self.world, &self.assets, &mut self.lights, &self.camera, dt, used);
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

    /// A world from level text, and the level's lights as the app keeps them.
    fn world_of(src: &str) -> (World, Assets, (Vec<Light>, Vec3)) {
        let mut assets = Assets::new(ASSETS);
        assets.load_materials().unwrap();
        let level = assets.parse_level("test.mmp", src).unwrap_or_else(|e| panic!("{e}"));
        let world = World::new(level, &assets);
        let lights = (world.lights().to_vec(), world.ambient);
        (world, assets, lights)
    }

    /// The scripted test level, with `more` after its last section.
    fn level(more: &str) -> String {
        format!("{}\n{more}", std::fs::read_to_string(Path::new(ASSETS).join("levels/script_rooms.mmp")).unwrap())
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
        let (mut world, assets, mut lights) = world_of(&level(""));
        let mut scripts = Scripts::new(Path::new(ASSETS).join("scripts"));
        assert_eq!(scripts.place(&world, "script_rooms.mmp", false), Vec::<String>::new());
        assert_eq!(scripts.running(), 3);
        let spawn = &world.spawn_points[0];
        let viewport = moose_scene::Viewport { x: 0, y: 0, width: 64, height: 64 };
        let mut camera = Camera::at_spawn(spawn, viewport);
        camera.move_to(&world, camera.position + Vec3::Y * 1.6);
        let lamp = world.light_names.iter().position(|n| n.as_deref() == Some("lamp")).unwrap();
        let crate_ = world.entities.iter().position(|e| e.name == "lift").unwrap();
        let (dim, low) = (lights.0[lamp].color, world.entities[crate_].position);

        // Starting: the level script counts in `$visits`, and the hall light is as it was.
        let frame = |scripts: &mut Scripts, world: &mut World, lights: &mut (Vec<Light>, Vec3), camera: &Camera, used| {
            let lines = scripts.tick(world, &assets, lights, camera, 0.1, used);
            assert!(lines.iter().all(|l| !l.error), "{lines:?}");
            lines
        };
        frame(&mut scripts, &mut world, &mut lights, &camera, None);
        let visits = |world: &World| world.meta_keys.id("visits").and_then(|k| world.meta.get(k)).map(|v| v[0]);
        assert_eq!(visits(&world), Some(0.0));
        assert_eq!(lights.0[lamp].color, dim);

        // Using the lift: it rises over two seconds while its lamp turns green, and both
        // being done, the lamp settles.
        frame(&mut scripts, &mut world, &mut lights, &camera, Some(crate_));
        for _ in 0..10 {
            frame(&mut scripts, &mut world, &mut lights, &camera, None);
        }
        let halfway = world.entities[crate_].position.y - low.y;
        assert!(halfway > 0.4 && halfway < 0.8, "{halfway}");
        assert_ne!(lights.0[lamp].color, dim);
        // (Used again on its way: it is busy, and carries on.)
        frame(&mut scripts, &mut world, &mut lights, &camera, Some(crate_));
        for _ in 0..15 {
            frame(&mut scripts, &mut world, &mut lights, &camera, None);
        }
        assert!((world.entities[crate_].position.y - low.y - 1.2).abs() < 1e-4);
        assert!((lights.0[lamp].color - Vec3::new(40.0, 255.0, 40.0) / FULL).length() < 1e-4);
        // And back down.
        frame(&mut scripts, &mut world, &mut lights, &camera, Some(crate_));
        for _ in 0..25 {
            frame(&mut scripts, &mut world, &mut lights, &camera, None);
        }
        assert!((world.entities[crate_].position - low).length() < 1e-4);

        // Walking into the hallway is counted, once each time.
        let hall = world.sectors.iter().position(|s| s.name == "hallway").unwrap() as u32;
        let (home, there) = (camera.position, world.sectors[hall as usize].center);
        (camera.position, camera.sector) = (there, hall);
        frame(&mut scripts, &mut world, &mut lights, &camera, None);
        frame(&mut scripts, &mut world, &mut lights, &camera, None);
        assert_eq!(visits(&world), Some(1.0));
        (camera.position, camera.sector) = (home, world.find_sector(home).unwrap());
        frame(&mut scripts, &mut world, &mut lights, &camera, None);
        (camera.position, camera.sector) = (there, hall);
        let lines = frame(&mut scripts, &mut world, &mut lights, &camera, None);
        assert_eq!(visits(&world), Some(2.0));
        assert_eq!(lines, [Line { text: "hallway visits: 2".into(), error: false }]);

        // A script file changing: placed again, each keeps its state.
        assert_eq!(scripts.place(&world, "script_rooms.mmp", true), Vec::<String>::new());
        frame(&mut scripts, &mut world, &mut lights, &camera, None);
        (camera.position, camera.sector) = (home, world.find_sector(home).unwrap());
        frame(&mut scripts, &mut world, &mut lights, &camera, None);
        (camera.position, camera.sector) = (there, hall);
        frame(&mut scripts, &mut world, &mut lights, &camera, None);
        assert_eq!(visits(&world), Some(3.0));
    }

    #[test]
    fn what_is_wrong_with_a_placement_is_said_and_the_rest_runs() {
        let src = level("").replace("door=hallway", "door=attic").replace("input:rise=1.2", "input:rise=high input:lamp=sun");
        let src = format!("{src}\n   2   lift.ts\n   3   hall_counter.ts door=hallway speeed=2\n   4   missing.ts\n").replace("scripts 2", "scripts 5");
        let (world, _, _) = world_of(&src);
        let mut scripts = Scripts::new(Path::new(ASSETS).join("scripts"));
        let problems = scripts.place(&world, "test.mmp", false);
        let said = |what: &str| assert!(problems.iter().any(|p| p.contains(what)), "{what}: {problems:#?}");
        said("hall_counter.ts: input 'door': no sector is named 'attic'");
        said("lift.ts: input 'rise': 'high' is not a number");
        said("lift.ts: input 'lamp': no light is named 'sun'");
        said("lift.ts uses 'self', so it belongs on an entity or a template");
        said("hall_counter.ts has no input 'speeed' (it has: door");
        said("missing.ts: cannot be read");
        assert_eq!(problems.len(), 6, "{problems:#?}");
        // The one script left alone still runs.
        assert_eq!(scripts.running(), 1);
    }
}
