//! What every script has, whatever the engine adds: `Vec3` and `Color` values, `Math`,
//! `time`, `random`, `console`, and `sleep`.

use crate::api::{Api, ClassKind, Host};
use crate::vm::{Cx, Value};

fn vec3(v: [f32; 3]) -> Value {
    Value::Vec3(v)
}

fn zip(a: Value, b: Value, f: impl Fn(f32, f32) -> f32) -> Value {
    let (a, b) = (a.vec3(), b.vec3());
    vec3([f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])])
}

fn dot(a: Value, b: Value) -> f32 {
    let (a, b) = (a.vec3(), b.vec3());
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn install<K: Host>(api: &mut Api<K>) {
    // A function of the table that doesn't touch the host.
    macro_rules! f {
        (|$cx:ident, $a:ident| $body:expr) => {{
            fn f<K: Host>(_: &mut K::Borrowed<'_>, $cx: &mut Cx<'_>, $a: &[Value]) -> Value {
                let _ = (&$cx, &$a);
                $body
            }
            f::<K>
        }};
    }

    let v = api.class("Vec3", ClassKind::Value, "A point or a direction, in meters. A value: its methods give new ones.");
    let color = api.class(
        "Color",
        ClassKind::Value,
        "A color as red, green and blue, where 255 is full (a light's can go over, to overbrighten).",
    );
    api.func("vec3(x: number, y: number, z: number): Vec3", "A Vec3 of these.", f!(|cx, a| vec3([a[0].num(), a[1].num(), a[2].num()])));
    api.func("rgb(r: number, g: number, b: number): Color", "A color: 255 is full.", f!(|cx, a| vec3([a[0].num(), a[1].num(), a[2].num()])));
    api.prop(v, "x", "number", "", f!(|cx, a| Value::Num(a[0].vec3()[0])), None);
    api.prop(v, "y", "number", "Up.", f!(|cx, a| Value::Num(a[0].vec3()[1])), None);
    api.prop(v, "z", "number", "", f!(|cx, a| Value::Num(a[0].vec3()[2])), None);
    api.method(v, "add(other: Vec3): Vec3", "", f!(|cx, a| zip(a[0], a[1], |x, y| x + y)));
    api.method(v, "sub(other: Vec3): Vec3", "", f!(|cx, a| zip(a[0], a[1], |x, y| x - y)));
    api.method(v, "scale(by: number): Vec3", "", f!(|cx, a| zip(a[0], a[0], |x, _| x * a[1].num())));
    api.method(v, "dot(other: Vec3): number", "", f!(|cx, a| Value::Num(dot(a[0], a[1]))));
    api.method(v, "length(): number", "", f!(|cx, a| Value::Num(dot(a[0], a[0]).sqrt())));
    api.method(v, "distance(to: Vec3): number", "", f!(|cx, a| {
        let d = zip(a[0], a[1], |x, y| x - y);
        Value::Num(dot(d, d).sqrt())
    }));
    api.method(v, "normalized(): Vec3", "This direction with length 1 (or zero, if it has none).", f!(|cx, a| {
        let length = dot(a[0], a[0]).sqrt();
        zip(a[0], a[0], |x, _| if length > 0.0 { x / length } else { 0.0 })
    }));
    api.method(v, "lerp(to: Vec3, t: number): Vec3", "The point a share `t` (0 to 1) of the way to `to`.", f!(|cx, a| {
        zip(a[0], a[1], |x, y| x + (y - x) * a[2].num())
    }));
    api.prop(color, "r", "number", "", f!(|cx, a| Value::Num(a[0].vec3()[0])), None);
    api.prop(color, "g", "number", "", f!(|cx, a| Value::Num(a[0].vec3()[1])), None);
    api.prop(color, "b", "number", "", f!(|cx, a| Value::Num(a[0].vec3()[2])), None);
    api.method(color, "scale(by: number): Color", "", f!(|cx, a| zip(a[0], a[0], |x, _| x * a[1].num())));
    api.method(color, "lerp(to: Color, t: number): Color", "The color a share `t` (0 to 1) of the way to `to`.", f!(|cx, a| {
        zip(a[0], a[1], |x, y| x + (y - x) * a[2].num())
    }));

    // (TypeScript's own Math, as far as it is here: `a[0]` is Math itself.)
    let math = api.singleton("Math", "Math", "");
    api.ambient(math);
    api.prop(math, "PI", "number", "", f!(|cx, a| Value::Num(std::f32::consts::PI)), None);
    api.method(math, "floor(x: number): int", "", f!(|cx, a| Value::Int(a[1].num().floor() as i32)));
    api.method(math, "ceil(x: number): int", "", f!(|cx, a| Value::Int(a[1].num().ceil() as i32)));
    api.method(math, "round(x: number): int", "", f!(|cx, a| Value::Int((a[1].num() + 0.5).floor() as i32)));
    api.method(math, "trunc(x: number): int", "", f!(|cx, a| Value::Int(a[1].num() as i32)));
    api.method(math, "abs(x: number): number", "", f!(|cx, a| Value::Num(a[1].num().abs())));
    api.method(math, "sqrt(x: number): number", "", f!(|cx, a| Value::Num(a[1].num().sqrt())));
    api.method(math, "sin(x: number): number", "", f!(|cx, a| Value::Num(a[1].num().sin())));
    api.method(math, "cos(x: number): number", "", f!(|cx, a| Value::Num(a[1].num().cos())));
    api.method(math, "atan2(y: number, x: number): number", "", f!(|cx, a| Value::Num(a[1].num().atan2(a[2].num()))));
    api.method(math, "pow(x: number, y: number): number", "", f!(|cx, a| Value::Num(a[1].num().powf(a[2].num()))));
    api.method(math, "min(a: number, b: number): number", "", f!(|cx, a| Value::Num(a[1].num().min(a[2].num()))));
    api.method(math, "max(a: number, b: number): number", "", f!(|cx, a| Value::Num(a[1].num().max(a[2].num()))));

    let time = api.singleton("Time", "time", "The scripts' clock: it stands still while they do (in the editor).");
    api.prop(time, "now", "number", "Seconds scripts have run for.", f!(|cx, a| Value::Num(cx.now as f32)), None);
    api.prop(time, "delta", "number", "The length of this tick, in seconds.", f!(|cx, a| Value::Num(cx.delta)), None);
    api.func("sleep(seconds: number): Promise<void>", "A promise that settles in `seconds`: await sleep(2).", f!(|cx, a| cx.sleep(a[0].num())));

    let random = api.singleton("Random", "random", "Random numbers: the same run of them every time the level starts.");
    api.method(random, "range(min: number, max: number): number", "A number from `min` up to (not including) `max`.", f!(|cx, a| {
        Value::Num(a[1].num() + (a[2].num() - a[1].num()) * cx.random())
    }));
    api.method(random, "int(min: int, max: int): int", "A whole number from `min` to `max`, both included.", f!(|cx, a| {
        let span = (a[2].int() - a[1].int() + 1).max(1);
        Value::Int(a[1].int() + ((cx.random() * span as f32) as i32).min(span - 1))
    }));
    api.method(random, "chance(odds: number): boolean", "True with odds `odds` (0 never, 1 always).", f!(|cx, a| Value::Bool(cx.random() < a[1].num())));

    let console = api.singleton("Console", "console", "The engine's log.");
    api.method(console, "log(...values: any[])", "Writes the values to the log.", f!(|cx, a| {
        let text = a[1..].iter().map(|v| cx.show(*v)).collect::<Vec<_>>().join(" ");
        cx.log(text);
        Value::Null
    }));
    api.method(console, "warn(...values: any[])", "Writes the values to the log as a problem.", f!(|cx, a| {
        let text = a[1..].iter().map(|v| cx.show(*v)).collect::<Vec<_>>().join(" ");
        cx.warn(text);
        Value::Null
    }));
}
