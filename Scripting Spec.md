# Scripting Spec

## Overview

Scripts are text assets (`assets/scripts/*.ts`) written in a **strict subset of TypeScript**. An editor checks and completes them as TypeScript, against `assets/scripts/moose.d.ts`. The engine compiles them itself as it loads them (the `moose-script` crate), with its own check, to bytecode that runs with no garbage collector. Nothing of Node or a JavaScript engine is involved.

The model is Jedi Knight's COG: a script declares typed **inputs**, which the level binds where it places the script; it keeps **state** per placement; and it handles **events**.

```ts
import { GO, STOP, fade } from "./util/lights";

const rise = input(1.0);
const seconds = input(2.0);
const lamp = input<Light | null>();

export let up = false;
let busy = false;

export async function onActivated(target: Entity, by: Player) {
  if (busy) return;
  busy = true;
  up = !up;
  await Promise.all([
    self.moveBy(vec3(0, up ? rise : -rise, 0), seconds),
    fade(lamp, up ? GO : STOP, seconds / 2),
  ]);
  busy = false;
}
```

**Built so far (Oct 12):** the language, compiler and VM; the API table and `moose.d.ts` written from it; level and entity/template placement; five events; hot reload that keeps state; `assets/levels/script_rooms.mmp` as a working example. **Planned** at the end is the agenda, not gaps.

## A script file

The file is the script: each placement of it is an instance with its own copy of everything at its top level.

| At the top level | Is |
| --- | --- |
| `const x = input<T>();` | An input with no fallback: the placement must set it, unless `T` is `... \| null` |
| `const x = input(2.0);` | An input with a fallback (a number, `true`/`false`, a string, `rgb(...)`, `vec3(...)`) |
| `const x = ...;` | A constant, worked out when the instance starts |
| `let x = ...;` | State, private to the script |
| `export let x = ...;` | State others may see (other scripts and the editor will read these; nothing does yet) |
| `export function onEvent(...)` | A handler: its name is the event's (see Events). It may be `async` |
| `function f(...)`, `async function f(...)` | A function of its own |
| `import { a, b } from "./file";` | Functions and constants of another file |

**Imports** are relative to the file (`./`, `../`), within `assets/scripts`, and `.ts` is added. A file that others import is a **utility file**: it may hold only functions and constants, no inputs and no `let`. What scripts share at run time lives on the level, as its `$KEY` values (`level.get`, `level.set`).

**`self`** is the entity a script is attached to. A script that names it can only be placed on an entity or a template.

## Placing scripts

A level places scripts in two ways (see the Level Format Spec for the syntax):

| Where | How | For |
| --- | --- | --- |
| On the level | A row of its `scripts` section: `0 hall_counter.ts door=hallway` | Logic about the level, naming its things through inputs |
| On an entity or a template | `script=lift.ts`, with `input:NAME=VALUE` options | Behavior of a kind of thing: every entity of the template runs its own instance |

An entity's `input:` options replace its template's one by one, so a template is bound once and a single entity can still differ. An input nobody sets takes the script's fallback.

How an input's value is written depends on its type:

| Type | Written |
| --- | --- |
| `number`, `int` | `2.5`, `3` |
| `boolean` | `true`, `false` |
| `string` | The text (quote the whole option if it has spaces) |
| `Vec3`, `Color` | `1,2,3` |
| `Entity`, `Sector`, `Light` | Its name in the level. Exactly one thing may have it |
| `T \| null` | As `T`, or `none`, or left out |

A placement with a problem (a script that doesn't compile, an input that names nothing, an input the script doesn't have, `self` in a level's script) is reported with the level's line and isn't run. The level and its other scripts still load.

## Events

A script hears an event by exporting a function with its name. It may take fewer parameters than the event has.

| Event | When | Heard by |
| --- | --- | --- |
| `onStart()` | The level has started | Every script |
| `onTick(dt: number)` | Every frame while playing | Every script |
| `onActivated(target: Entity, by: Player)` | A player used `target`: E (the Use key), looking at it within 3 m with nothing of the level between | Scripts attached to `target`, or with an input bound to it |
| `onEntered(sector: Sector, by: Player)` | A player walked into `sector` | Scripts with an input bound to it |
| `onExited(sector: Sector, by: Player)` | A player walked out of it | The same |

So a script hears about what it is attached to and what its inputs name. An entity with a template script and a level script watching it sends the event to both.

## Waiting

A call that takes time returns a promise (`entity.moveTo`, `light.fadeTo`, `sleep(seconds)`, any `async` function of the script's), and the work starts at the call. `await` stops the handler until the promise settles; other handlers, and other runs of the same one, go on meanwhile.

- `await Promise.all([a, b])` goes on when all have settled; `await Promise.race([a, b])` when the first has (the others carry on). The list is written out in place.
- An `async` function runs up to its first unsettled `await` before its caller goes on, as in JavaScript.
- A handler can be entered again while an earlier run of it waits. Guard with state where that matters (`busy` above).

## The language

What is in: `const` and `let`; `number`, `int`, `boolean`, `string`, `T | null`, `Promise<T>` and the API's types; functions with typed parameters; `if`/`else`, `while`, `for (;;)`, `for (const x of ...)`, `break`, `continue`, `return`; the arithmetic, comparison and logical operators, `?:`, `??`, `?.`, `!` after a value, `++`, `--`, `+=` and its kin; `async`/`await`; `import`/`export`.

What is out, each refused by name when the script loads: classes, objects and arrays, closures and arrow functions, nested functions, `this`, `undefined`, `switch`, `try`/`throw`, `typeof`, `as`, template strings, joining strings, destructuring, optional parameters, `export default`, and general promises (`.then`, `new Promise`).

Where the engine is stricter than TypeScript's own check (VSCode accepts these; the engine doesn't):

| Rule | Why |
| --- | --- |
| `int` and `number` are different | `number` is an f32 and `int` an i32. An `int` goes where a `number` is wanted, not the reverse: use `Math.floor`, `Math.round` or `Math.trunc` |
| `let x = 0` is a `number` | As TypeScript reads it. Write `let i: int = 0` for a whole one |
| `/` always gives a `number` | `7 / 2` is `3.5`, as in JavaScript |
| A condition is a `boolean`, or something that may be null | No truthy numbers or strings: write `x !== 0` |
| A function with no return type returns nothing | Give it one to return a value |
| An assignment has no value | `a = b = c` and `f(i++)` are refused |
| `for...of` walks only what the engine lists | `entities.all()`, `sector.lights()`, ... |
| Strings are constants | Names and keys; they are compared, not built |

**Null.** There is `null` and no `undefined`. What may be null (`T | null`) must be tested before it is used, as in strict TypeScript: `if (x !== null)`, `if (x === null) return;`, `x && ...`, `x?.name`, `x ?? fallback`. A `!` after a value claims it isn't null; if it is, the handler stops with an error.

## The API

`assets/scripts/moose.d.ts` is the reference, with a comment on each member. In short:

| Global | Is |
| --- | --- |
| `level` | The level: `name`, `ambient`, and its `$KEY` values |
| `sectors`, `entities`, `lights` | The managers: `count`, `find(name)`, `all()` (and `sectors.at(position)`) |
| `players` | `players.local`, the player at this screen |
| `time` | `now`, `delta` |
| `random` | `range`, `int`, `chance`: the same run of numbers every time the level starts |
| `console` | `log`, `warn`: to the terminal, and on screen for a moment |
| `Math` | `floor ceil round trunc abs sqrt sin cos atan2 pow min max PI` |
| `vec3(x, y, z)`, `rgb(r, g, b)`, `sleep(seconds)` | |

| Type | Has |
| --- | --- |
| `Sector` | `name`, `center`, `surfaces()`, `entities()`, `lights()`, `$KEY` values |
| `Surface` | `sector`, `$KEY` values |
| `Entity` | `name`, `position`, `sector`, `isStatic`, `animation`, `moveTo`, `moveBy`, `$KEY` values |
| `Light` | `name`, `color`, `range`, `position`, `sector`, `fadeTo` |
| `Player` | `position`, `sector` |
| `Vec3`, `Color` | Values, not references: `add sub scale dot length distance normalized lerp`; `scale lerp` |

**`$KEY` values** are the level's meta values (`$wet=200`), the ones materials read with `level:`, `sector:`, `face:` and `entity:` sources. On each thing that has them: `has(key)`, `get(key, fallback)`, `set(key, value)`, `getColor(key, fallback)`, `setColor(key, value)`. A script setting one is how it reaches a material.

**Colors** are written as the level's meta values and material inputs are: 255 is full. A light's color at 255 is the level file's 1.0, and more overbrightens.

**Readonly** properties are what the engine can't change yet (a sector's shape, a light's position). VSCode flags a write, and so does the engine.

**Static entities** (`static` in the level) can't be moved: their shadows are baked. A script that tries gets a warning and a promise that is already settled.

## The table

Everything above comes from one table, `api()` in `crates/moose-app/src/scripts.rs`: each class, property, method, global and event is written once, as its TypeScript signature next to the Rust that does it.

```rust
api.method(entity, "moveTo(position: Vec3, seconds: number): Promise<void>", "Moves it to ...", g!(|g, cx, a| { ... }));
```

The compiler checks scripts against it, the VM calls into it, and `moose.d.ts` is written from it, so the three can't disagree. A test fails when `moose.d.ts` is out of date; `MOOSE_WRITE_DTS=1 cargo test -p moose-app moose_d_ts` writes it again.

**The rule:** a primitive the world knows about isn't finished until it is in the table, with its type, its members and its manager.

## Running

- **No garbage collector.** Values are numbers, booleans, vectors, constant strings, and handles (indices) to the world's things. The only things made as scripts run are promises, kept in a table that is swept, by who still holds one, when it has grown.
- **Tasks.** Each event handled and each `async` call is a task with its own small stack. A task that runs two million operations without an `await`, or nests calls 200 deep, is stopped with an error, so a loop that never ends can't hang the engine.
- **Errors** name the file and line (`lift.ts:14: ...`), on the terminal, in the editor's message line, and on screen for a moment. A run-time error stops that task only.
- **Hot reload.** A changed `.ts` file is seen within a second. Every script is placed again; each keeps its state (every `let` that still has the same name and type) and is not told the level started again. Handlers that were waiting are dropped.
- **The editor.** Scripts run only while playing: not in the editor (Tab), nor behind the menu. Rebuilding the level (an edit, the level file changing) starts them over. So does the editor's **Reload the scripts** (F9, or the key the gear menu binds it to): it reads them again, says what is wrong with them, and they start from their starting values when play goes on.
- **The Use key** is E by default. Use is a row of the menu's Controls page, bound like any setting there: B on it, then the key (Delete unbinds it). It is kept with the other hotkeys in `~/.config/moose/keys.txt`.

## In VSCode

Open the repo, or `assets/scripts`. `tsconfig.json` there points TypeScript at `moose.d.ts` with only its plain library, so completion, hover text and red squiggles work with nothing installed. It checks more loosely than the engine in the ways listed above; the engine's check, when the script loads, is the one that decides.

## Planned

- **More of the world in the table:** portals (opening and closing), spawn points, materials, sounds (`moose-audio`), physics (rays, bodies), cameras; entity rotation and scale; spawning and destroying entities, with handles that know when their thing is gone (`alive`).
- **More events:** touch and damage, timers by name, a player joining, a script's own messages to other scripts.
- **Moved props and physics:** a prop a script moves keeps its collider where the level put it, until physics has moving bodies.
- **The editor:** placing scripts and binding inputs in the level editor, with each input shown as a field of its type; exported state shown live.
- **Results from `Promise.all`:** `const [a, b] = await Promise.all([...])`.
- **Arrays and strings as values**, when a real script needs them.
- **Saved games:** an instance's state and its waiting handlers written out and read back.
- **Speed:** meta keys looked up once when a script compiles, not per call.
