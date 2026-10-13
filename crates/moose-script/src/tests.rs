//! Scripts compiled and run against a small world of lamps.

use std::collections::HashMap;
use std::rc::Rc;

use crate::*;

/// The test world: lamps with a name and a color, what scripts marked, and the fades they
/// started (promises the test settles).
#[derive(Default)]
struct World {
    lamps: Vec<(&'static str, [f32; 3])>,
    marks: Vec<f32>,
    fades: Vec<Value>,
}

struct Kind;

impl Host for Kind {
    type Borrowed<'a> = World;
}

macro_rules! h {
    (|$w:ident, $cx:ident, $a:ident| $body:expr) => {{
        fn f($w: &mut World, $cx: &mut Cx<'_>, $a: &[Value]) -> Value {
            let _ = (&$w, &$cx, &$a);
            $body
        }
        f
    }};
}

fn api() -> Api<Kind> {
    let mut api: Api<Kind> = Api::new();
    let lamp = api.class("Lamp", ClassKind::Handle, "A lamp.");
    let lamps = api.singleton("Lamps", "lamps", "The lamps.");
    api.self_class(lamp);
    api.prop(lamp, "name", "string", "", h!(|w, cx, a| cx.string(w.lamps[a[0].handle().unwrap()].0)), None);
    api.prop(
        lamp,
        "color",
        "Color",
        "Its light.",
        h!(|w, cx, a| Value::Vec3(w.lamps[a[0].handle().unwrap()].1)),
        Some(h!(|w, cx, a| {
            w.lamps[a[0].handle().unwrap()].1 = a[1].vec3();
            Value::Null
        })),
    );
    api.prop(lamp, "brightness", "number", "", h!(|w, cx, a| Value::Num(w.lamps[a[0].handle().unwrap()].1[0])), Some(h!(|w, cx, a| {
        w.lamps[a[0].handle().unwrap()].1[0] = a[1].num();
        Value::Null
    })));
    api.method(lamp, "fadeTo(color: Color, seconds: number): Promise<void>", "", h!(|w, cx, a| {
        w.lamps[a[0].handle().unwrap()].1 = a[1].vec3();
        let promise = cx.promise();
        w.fades.push(promise);
        promise
    }));
    api.prop(lamps, "count", "int", "", h!(|w, cx, a| Value::Int(w.lamps.len() as i32)), None);
    api.method(lamps, "find(name: string): Lamp | null", "", h!(|w, cx, a| {
        let name = cx.str(a[1]);
        w.lamps.iter().position(|l| l.0 == name).map_or(Value::Null, |i| Value::Handle(i as u32))
    }));
    api.method(lamps, "all(): Iterable<Lamp>", "", h!(|w, cx, a| cx.list((0..w.lamps.len() as u32).map(Value::Handle))));
    api.func("mark(value: number)", "", h!(|w, cx, a| {
        w.marks.push(a[0].num());
        Value::Null
    }));
    api.func("whole(value: int): int", "", h!(|w, cx, a| a[0]));
    api.event("onStart()", "The level has started.");
    api.event("onActivated(target: Lamp, by: Lamp | null)", "Something was used.");
    api.event("onTick(dt: number)", "");
    api
}

fn world() -> World {
    World { lamps: vec![("red", [255.0, 0.0, 0.0]), ("green", [0.0, 255.0, 0.0])], ..World::default() }
}

fn compile_files(runtime: &mut Runtime<Kind>, files: &[(&str, &str)]) -> Result<Program, ScriptError> {
    let files: HashMap<&str, &str> = files.iter().copied().collect();
    let desc = runtime.desc().clone();
    compile(&desc, &mut runtime.strings, "main.ts", &mut |file| {
        files.get(file).map(|s| s.to_string()).ok_or(format!("no such file"))
    })
}

/// Compiles and starts `src` with `inputs`, and fires `onStart`.
fn start_with(src: &str, owner: Value, inputs: &[Value]) -> (Runtime<Kind>, World, InstanceId) {
    let mut runtime = Runtime::new(api());
    let program = compile_files(&mut runtime, &[("main.ts", src)]).unwrap_or_else(|e| panic!("{e}"));
    let mut world = world();
    let id = runtime.spawn_instance(Rc::new(program), owner, inputs);
    runtime.start(&mut world, id);
    let on_start = runtime.desc().event_id("onStart").unwrap();
    runtime.fire(&mut world, on_start, None, &[]);
    (runtime, world, id)
}

fn start(src: &str) -> (Runtime<Kind>, World, InstanceId) {
    let (mut runtime, world, id) = start_with(src, Value::Null, &[]);
    assert_eq!(runtime.take_log(), []);
    (runtime, world, id)
}

/// What `src` marks when it starts.
fn marks(src: &str) -> Vec<f32> {
    start(src).1.marks
}

/// The error compiling `src` gives.
fn error(src: &str) -> ScriptError {
    let mut runtime = Runtime::new(api());
    compile_files(&mut runtime, &[("main.ts", src)]).expect_err(src)
}

#[test]
fn numbers_are_numbers_unless_something_says_int() {
    assert_eq!(
        marks(
            "let t = 0;\n\
             export function onStart() {\n\
               t += 0.5;            // a number, though it started as 0\n\
               mark(t);\n\
               mark(7 / 2);         // dividing gives a number, as in JavaScript\n\
               mark(7 % 4);\n\
               mark(lamps.count / 4);\n\
               const n: int = lamps.count * 3 + 1;\n\
               mark(n);\n\
               mark(whole(n) - 2 * 3);\n\
               mark(-n + 0.25);\n\
               mark(Math.floor(2.75) + Math.round(2.5) + Math.max(1, 2.5));\n\
               let i: int = 0;\n\
               i++; i += 2; --i;\n\
               mark(i);\n\
               mark(1 < 2 && 2.5 >= 2 ? 1 : 0);\n\
               mark(n == 7 ? 1.5 : 2);\n\
               mark(n != 7 ? 1 : 2.5);\n\
             }"
        ),
        [0.5, 3.5, 3.0, 0.5, 7.0, 1.0, -6.75, 7.5, 2.0, 1.0, 1.5, 2.5]
    );
    // Where an int is wanted, a number isn't one.
    assert!(error("const a = whole(1.5);").message.contains("number, where int is wanted"));
    assert!(error("let t = 0;\nconst a = whole(t);").message.contains("number, where int"));
    assert!(error("const s = \"a\" + \"b\";").message.contains("strings can't be joined"));
    assert!(error("function f() { if (1) {} }").message.contains("a condition is a boolean"));
}

#[test]
fn loops_run_break_and_continue() {
    assert_eq!(
        marks(
            "function sum(to: int): int {\n\
               let total: int = 0;\n\
               for (let i: int = 0; i < to; i++) {\n\
                 if (i % 2 == 0) continue;\n\
                 if (i > 6) break;\n\
                 total += i;\n\
               }\n\
               return total;\n\
             }\n\
             function first(): Lamp | null {\n\
               for (const lamp of lamps.all()) { return lamp; }\n\
               return null;\n\
             }\n\
             export function onStart() {\n\
               mark(sum(100));\n\
               let n = 3;\n\
               while (n > 0) { n -= 1; mark(n); }\n\
               for (const lamp of lamps.all()) {\n\
                 if (lamp.name == \"red\") continue;\n\
                 mark(lamp.color.g);\n\
                 for (const other of lamps.all()) { mark(other.color.r); break; }\n\
               }\n\
               mark(first()!.color.r);\n\
             }"
        ),
        [9.0, 2.0, 1.0, 0.0, 255.0, 255.0, 255.0]
    );
}

#[test]
fn what_may_be_null_is_tested_before_it_is_used() {
    assert_eq!(
        marks(
            "function red(lamp: Lamp | null): number {\n\
               if (lamp === null) return -1;\n\
               return lamp.color.r;              // not null past the return\n\
             }\n\
             export function onStart() {\n\
               const none = lamps.find(\"blue\");\n\
               const red_lamp = lamps.find(\"red\");\n\
               mark(red(none));\n\
               mark(red(red_lamp));\n\
               mark(none?.color.r ?? 7);\n\
               mark(red_lamp?.color.r ?? 7);\n\
               if (red_lamp !== null && red_lamp.color.r > 200) mark(1);\n\
               if (none === null || none.color.r > 200) mark(2);\n\
               if (red_lamp) { red_lamp.brightness = 9; mark(red_lamp.brightness); }\n\
               mark(none ? 1 : 0);\n\
               let found: Lamp | null = null;\n\
               for (const lamp of lamps.all()) { found = lamp; }\n\
               if (found !== null) mark(found.color.g);\n\
               none?.fadeTo(rgb(1, 2, 3), 1);\n\
               mark(none?.color.scale(2).r ?? 8);\n\
               mark(red_lamp?.color.scale(2).g ?? 8);\n\
             }"
        ),
        [-1.0, 255.0, 7.0, 255.0, 1.0, 2.0, 9.0, 0.0, 255.0, 8.0, 0.0]
    );
    for src in [
        "const l = lamps.find(\"red\");\nconst c = l.color;",
        "function f(l: Lamp | null) {\n l.brightness = 1; }",
        // A `?.` doesn't settle it for what follows.
        "function f(l: Lamp | null) {\n l?.fadeTo(rgb(1, 2, 3), 1);\n l.fadeTo(rgb(1, 2, 3), 1); }",
        // Known inside the test only.
        "function f(l: Lamp | null) {\n if (l !== null) { mark(1); }\n mark(l.brightness); }",
        // A loop may come round again after it is set to null.
        "function f() {\n let l = lamps.find(\"red\");\n if (l === null) return;\n while (true) {\n mark(l.brightness);\n l = null; } }",
    ] {
        let e = error(src);
        assert!(e.message.contains("may be null"), "{src}: {e}");
    }
    assert_eq!(error("function f(l: Lamp | null) {\n if (l !== null) { mark(1); }\n mark(l.brightness); }").line, 3);
    assert!(error("const l: Lamp = lamps.find(\"red\");").message.contains("Lamp | null, where Lamp is wanted"));
    assert!(error("const n = null;").message.contains("needs a type"));
    assert!(error("function f(): number { return 1 ?? 2; }").message.contains("never null"));
    // `!` on a null stops the task, and says where.
    let (mut runtime, world, _) = start_with("export function onStart() {\n mark(1);\n mark(lamps.find(\"blue\")!.brightness);\n mark(2); }", Value::Null, &[]);
    assert_eq!(world.marks, [1.0]);
    assert_eq!(runtime.take_log(), [Line { text: "main.ts:3: a value marked '!' was null".into(), error: true }]);
}

#[test]
fn an_await_stops_a_handler_until_its_promise_settles() {
    let (mut runtime, mut world, _) = start(
        "async function blink(lamp: Lamp, times: int) {\n\
           for (let i: int = 0; i < times; i++) {\n\
             mark(10 + i);\n\
             await sleep(1);\n\
           }\n\
         }\n\
         async function twice(x: number): Promise<number> {\n\
           await sleep(0.5);\n\
           return x * 2;\n\
         }\n\
         export async function onStart() {\n\
           mark(1);\n\
           const lamp = lamps.find(\"red\")!;\n\
           const blinking = blink(lamp, 2);   // runs up to its first await, as in JavaScript\n\
           mark(2);\n\
           mark(await twice(21));\n\
           await Promise.all([blinking, lamp.fadeTo(rgb(0, 0, 0), 5), sleep(0.1)]);\n\
           mark(3);\n\
           await Promise.race([sleep(100), sleep(1)]);\n\
           mark(4);\n\
           await Promise.all([]);\n\
           mark(time.now);\n\
         }",
    );
    assert_eq!(world.marks, [1.0, 10.0, 2.0]);
    runtime.tick(&mut world, 0.25);
    assert_eq!(world.marks.len(), 3);
    runtime.tick(&mut world, 0.25);
    assert_eq!(world.marks[3..], [42.0]);
    runtime.tick(&mut world, 0.5);
    assert_eq!(world.marks[4..], [11.0]);
    // The blinking and the sleep are over; the fade is the engine's to finish.
    runtime.tick(&mut world, 3.0);
    assert_eq!(world.marks.len(), 5);
    assert_eq!(world.lamps[0].1, [0.0; 3]);
    runtime.resolve(world.fades[0], Value::Null);
    assert_eq!(world.marks.len(), 5);
    runtime.tick(&mut world, 0.0);
    assert_eq!(world.marks[5..], [3.0]);
    runtime.tick(&mut world, 1.0);
    assert_eq!(world.marks[6..], [4.0, 5.0]);
    assert_eq!(runtime.take_log(), []);
    assert_eq!(runtime.waiting(), 0);

    assert!(error("function f() { await sleep(1); }").message.contains("async function"));
    assert!(error("async function f() { await 1; }").message.contains("await takes a promise"));
    assert!(error("async function f(): number { return 1; }").message.contains("returns a Promise"));
    assert!(error("async function f() { await Promise.all([1]); }").message.contains("takes promises"));
    assert!(error("async function f() { await Promise.any([sleep(1)]); }").message.contains("isn't supported"));
    assert!(error("const x = await sleep(1);").message.contains("async function"));
}

#[test]
fn promises_nothing_holds_are_freed() {
    let (mut runtime, mut world, _) = start(
        "export let ticks = 0;\n\
         export async function onTick(dt: number) {\n\
           await sleep(0.01);\n\
           ticks += 1;\n\
         }",
    );
    let on_tick = runtime.desc().event_id("onTick").unwrap();
    for _ in 0..2000 {
        runtime.fire(&mut world, on_tick, None, &[Value::Num(0.1)]);
        runtime.tick(&mut world, 0.1);
    }
    assert!(runtime.promises() < 200, "{} promises kept", runtime.promises());
    assert_eq!(runtime.waiting(), 0);
}

#[test]
fn events_reach_the_scripts_bound_to_what_they_are_about() {
    let src = "const lamp = input<Lamp>();\n\
               const other = input<Lamp | null>();\n\
               const gain = input(2.0);\n\
               export let uses = 0;\n\
               export function onActivated(target: Lamp) {\n\
                 uses += gain;\n\
                 mark(uses);\n\
                 target.brightness = uses;\n\
               }";
    let mut runtime = Runtime::new(api());
    let program = Rc::new(compile_files(&mut runtime, &[("main.ts", src)]).unwrap());
    let inputs: Vec<_> = program.inputs().map(|(_, g)| (g.name.clone(), g.kind.clone())).collect();
    assert_eq!(inputs[0], ("lamp".to_string(), GlobalKind::Input { fallback: None }));
    assert_eq!(inputs[2], ("gain".to_string(), GlobalKind::Input { fallback: Some(Value::Num(2.0)) }));
    let mut world = world();
    let lamp = runtime.desc().class_id("Lamp").unwrap();
    // One placed on the level, bound to the red lamp; one attached to the green lamp.
    let bound = runtime.spawn_instance(Rc::clone(&program), Value::Null, &[Value::Handle(0), Value::Null, Value::Num(2.0)]);
    let attached = runtime.spawn_instance(Rc::clone(&program), Value::Handle(1), &[Value::Handle(1), Value::Null, Value::Num(10.0)]);
    runtime.start(&mut world, bound);
    runtime.start(&mut world, attached);
    let event = runtime.desc().event_id("onActivated").unwrap();
    runtime.fire(&mut world, event, Some((lamp, Value::Handle(0))), &[Value::Handle(0), Value::Null]);
    runtime.fire(&mut world, event, Some((lamp, Value::Handle(1))), &[Value::Handle(1), Value::Null]);
    runtime.fire(&mut world, event, Some((lamp, Value::Handle(0))), &[Value::Handle(0), Value::Null]);
    assert_eq!(world.marks, [2.0, 10.0, 4.0]);
    assert_eq!((world.lamps[0].1[0], world.lamps[1].1[0]), (4.0, 10.0));
    // To all of them, with nothing to be about.
    runtime.fire(&mut world, event, None, &[Value::Handle(0), Value::Null]);
    assert_eq!(world.marks[3..], [6.0, 20.0]);

    // State goes over to the instance that replaces one whose script changed.
    let state = runtime.state(bound);
    assert_eq!(state, [("uses".to_string(), Type::Number, Value::Num(6.0))]);
    runtime.drop_instance(bound);
    let changed = src.replace("uses += gain", "uses += 100");
    let program = Rc::new(compile_files(&mut runtime, &[("main.ts", &changed)]).unwrap());
    let again = runtime.spawn_instance(program, Value::Null, &[Value::Handle(0), Value::Null, Value::Num(2.0)]);
    runtime.start(&mut world, again);
    runtime.restore(again, &state);
    runtime.fire(&mut world, event, Some((lamp, Value::Handle(0))), &[Value::Handle(0), Value::Null]);
    assert_eq!(world.marks[5..], [106.0]);
    assert_eq!(runtime.take_log(), []);

    // `self` is what a script is attached to.
    let (_, world, _) = start_with("export function onStart() { self.brightness = 5; }", Value::Handle(1), &[]);
    assert_eq!(world.lamps[1].1[0], 5.0);
    let mut runtime = Runtime::new(api());
    assert!(compile_files(&mut runtime, &[("main.ts", "export function onStart() { mark(self.brightness); }")]).unwrap().uses_self);

    for (src, says) in [
        ("export function onActivate(target: Lamp) {}", "no event is called 'onActivate' (there are: onStart, onActivated, onTick)"),
        ("export function onActivated(target: number) {}", "written onActivated(target: Lamp, by: Lamp | null)"),
        ("export function onActivated(a: Lamp, b: Lamp) {}", "written onActivated"),
        ("export function onTick(dt: number): number { return dt; }", "written onTick(dt: number)"),
        ("let a = input(1);", "an input is a const"),
        ("const a = input();", "needs a type or a fallback"),
        ("const a = input<Lamp>(3);", "the fallback is number, but the input is Lamp"),
        ("const a = input(lamps.count);", "written out"),
        ("function f() { const a = input(1); }", "top-level const"),
        ("const a = input(1);\nfunction f() { a = 2; }", "'a' is a const"),
    ] {
        assert!(error(src).message.contains(says), "{src}: {}", error(src));
    }
}

#[test]
fn utility_files_share_functions_and_constants() {
    let files = [
        (
            "main.ts",
            "import { blink, PERIOD } from \"./util/lights\";\n\
             import { half } from \"./math\";\n\
             export async function onStart() {\n\
               mark(half(PERIOD));\n\
               await blink(lamps.find(\"green\")!, 2);\n\
               mark(3);\n\
             }",
        ),
        (
            "util/lights.ts",
            "import { half } from \"../math\";\n\
             export const PERIOD = half(4) * 3;\n\
             const OFF = rgb(0, 0, 0);\n\
             export async function blink(lamp: Lamp, times: int) {\n\
               for (let i: int = 0; i < times; i++) {\n\
                 lamp.color = OFF;\n\
                 await sleep(PERIOD);\n\
                 mark(lamp.color.g);\n\
               }\n\
             }",
        ),
        ("math.ts", "export function half(x: number): number { return x / 2; }\nfunction hidden() {}"),
    ];
    let mut runtime = Runtime::new(api());
    let program = compile_files(&mut runtime, &files).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(program.files, ["main.ts", "util/lights.ts", "math.ts"]);
    let mut world = world();
    let id = runtime.spawn_instance(Rc::new(program), Value::Null, &[]);
    runtime.start(&mut world, id);
    let on_start = runtime.desc().event_id("onStart").unwrap();
    runtime.fire(&mut world, on_start, None, &[]);
    runtime.tick(&mut world, 6.0);
    runtime.tick(&mut world, 6.0);
    assert_eq!(world.marks, [3.0, 0.0, 0.0, 3.0]);
    assert_eq!(runtime.take_log(), []);

    let fails = |main: &str, util: &str| {
        let mut runtime = Runtime::new(api());
        compile_files(&mut runtime, &[("main.ts", main), ("util.ts", util)]).unwrap_err()
    };
    let e = fails("import { count } from \"./util\";", "export let count = 0;");
    assert_eq!((e.file.as_str(), e.line), ("util.ts", 1));
    assert!(e.message.contains("can't be state"), "{e}");
    assert!(fails("import { a } from \"./util\";", "export const a = input(1);").message.contains("can't be an input"));
    assert!(fails("import { hidden } from \"./util\";", "function hidden() {}").message.contains("util.ts doesn't export 'hidden'"));
    assert!(fails("import { a } from \"util\";", "").message.contains("relative"));
    assert!(fails("import { a } from \"../util\";", "").message.contains("leaves the scripts folder"));
    assert!(fails("import { a } from \"./missing\";", "").to_string().starts_with("missing.ts: no such file"));
    assert!(fails("import { f } from \"./util\";\nfunction f() {}", "export function f() {}").message.contains("declared twice"));
    // A file's own mistakes are its own.
    let e = fails("import { f } from \"./util\";", "export function f() {\n  return 1;\n}");
    assert_eq!((e.file.as_str(), e.line), ("util.ts", 2));
}

#[test]
fn mistakes_are_named_with_their_lines() {
    for (src, line, says) in [
        ("const a = 1;\nconst b = c;", 2, "'c' is not defined"),
        ("const b = a;\nconst a = 1;", 1, "used before it has its starting value"),
        ("function f() {\n  lamps.find(1);\n}", 2, "int, where string is wanted"),
        ("function f() {\n  lamps.find();\n}", 2, "find takes 1 argument(s), not 0"),
        ("function f() {\n  lamps.count = 2;\n}", 2, "'count' is readonly"),
        ("function f() {\n  lamps.find(\"red\")!.color.r = 2;\n}", 2, "'r' is readonly: a Color is a value"),
        ("function f() {\n  mark(lamps.total);\n}", 2, "Lamps has no 'total'"),
        ("function f() {\n  mark(lamps.find);\n}", 2, "'find' is a method"),
        ("function f() {\n  lamps.light();\n}", 2, "no method 'light'"),
        ("function f(): number {\n  if (lamps.count > 1) return 1;\n}", 1, "doesn't return a value on every path"),
        ("function f() {\n  return 1;\n}", 2, "returns nothing"),
        ("function f() {\n  break;\n}", 2, "belong in a loop"),
        ("function f() {\n  const a = 1;\n  const a = 2;\n}", 3, "declared twice"),
        ("function f() {\n  const a = mark(1);\n}", 2, "no value"),
        ("function f(a: Lamp, b: number) {\n  mark(a == b ? 1 : 0);\n}", 2, "never equal"),
        ("function f(a: Thing) {}", 1, "'Thing' is not a type"),
        ("function f(a: Lamps) {}", 1, "'Lamps' is not a type"),
        ("function f() {\n  for (const l of lamps) {}\n}", 2, "for...of walks"),
        ("function f() {\n  const p = Promise;\n}", 2, "Promise is only for"),
        ("function f() {\n  const x = [1, 2];\n}", 2, "only what Promise.all"),
        ("function f() {\n  mark(self.brightness == 1 ? rgb(1, 2, 3) : 4);\n}", 2, "the two results differ"),
        ("function g() {}\nfunction f() {\n  g = 1;\n}", 3, "'g' is not a variable"),
        ("function f() {\n  mark(mark);\n}", 2, "'mark' is a function"),
    ] {
        let e = error(src);
        assert!(e.line == line && e.message.contains(says), "{src}: {e}");
    }
    assert_eq!(error("function f() {\n  lamps.find(1);\n}").to_string(), "main.ts:2: this is int, where string is wanted");
}

#[test]
fn a_task_that_never_ends_is_stopped() {
    let (mut runtime, world, _) = start_with(
        "function forever(): number { return forever(); }\n\
         export function onStart() {\n\
           mark(1);\n\
           while (true) {}\n\
         }\n\
         export function onTick(dt: number) {\n\
           mark(forever());\n\
         }\n\
         export function onActivated(target: Lamp) {\n\
           mark(1 % (lamps.count - 2));\n\
         }",
        Value::Null,
        &[],
    );
    let mut world = world;
    assert_eq!(world.marks, [1.0]);
    let log = runtime.take_log();
    assert!(log[0].error && log[0].text.starts_with("main.ts:4: ran too long"), "{log:?}");
    runtime.fire(&mut world, runtime.desc().event_id("onTick").unwrap(), None, &[Value::Num(0.0)]);
    assert!(runtime.take_log()[0].text.contains("nested too deep"));
    runtime.fire(&mut world, runtime.desc().event_id("onActivated").unwrap(), None, &[Value::Handle(0)]);
    assert!(runtime.take_log()[0].text.starts_with("main.ts:10: the remainder"));
    assert_eq!(world.marks, [1.0]);
    assert_eq!(runtime.waiting(), 0);
}

#[test]
fn dropping_an_instance_stops_its_tasks() {
    let (mut runtime, mut world, id) = start(
        "export async function onStart() {\n\
           for (const lamp of lamps.all()) {\n\
             await lamp.fadeTo(rgb(1, 1, 1), 1);\n\
             mark(1);\n\
           }\n\
         }",
    );
    assert_eq!(runtime.waiting(), 1);
    runtime.drop_instance(id);
    assert_eq!(runtime.waiting(), 0);
    runtime.resolve(world.fades[0], Value::Null);
    runtime.tick(&mut world, 1.0);
    assert_eq!(world.marks, []);
    assert_eq!(runtime.take_log(), []);
}

#[test]
fn the_declarations_are_written_from_the_table() {
    let text = declarations(&api().desc);
    for line in [
        "type int = number;",
        "declare function input<T>(fallback?: T): T;",
        "declare const self: Lamp;",
        "/** A lamp. */\ninterface Lamp {",
        "  readonly name: string;",
        "  /** Its light. */\n  color: Color;",
        "  fadeTo(color: Color, seconds: number): Promise<void>;",
        "interface Lamps {\n  readonly count: int;\n  find(name: string): Lamp | null;\n  all(): Iterable<Lamp>;\n}\ndeclare const lamps: Lamps;",
        "declare function rgb(r: number, g: number, b: number): Color;",
        "declare function sleep(seconds: number): Promise<void>;",
        "  log(...values: any[]): void;",
        "declare const console: Console;",
        "//   export function onActivated(target: Lamp, by: Lamp | null)\n//     Something was used.",
        "  readonly x: number;",
    ] {
        assert!(text.contains(line), "missing: {line}\n{text}");
    }
    // TypeScript's own.
    assert!(!text.contains("interface Math"));
}
