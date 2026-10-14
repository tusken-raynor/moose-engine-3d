//! Checks a script (and the files it imports) against the API and compiles it to bytecode.
//!
//! Types are settled here, so the VM never looks at one: `number` is an f32, `int` an i32,
//! and what may be `null` must be tested before it is used, as TypeScript's strict mode
//! asks. A script's top-level variables become its instance's: inputs (`const x =
//! input<T>()`), constants, and state (`let`).

use std::collections::HashMap;
use std::fmt;

use crate::api::{ApiDesc, ClassId, ClassKind, Method, Type};
use crate::ast::*;
use crate::parse::parse;
use crate::vm::{Op, Strings, Value};

/// A problem in a script, where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptError {
    pub file: String,
    /// The 1-based line; 0 when the problem is the whole file's.
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            0 => write!(f, "{}: {}", self.file, self.message),
            line => write!(f, "{}:{}: {}", self.file, line, self.message),
        }
    }
}

impl std::error::Error for ScriptError {}

type R<T> = Result<T, ScriptError>;

/// A compiled script, with the code of everything it imports.
#[derive(Debug)]
pub struct Program {
    /// The script's file, then every file it imports (directly or not), relative to the
    /// scripts folder.
    pub files: Vec<String>,
    /// Its instance's variables: inputs, constants and state.
    pub globals: Vec<GlobalDef>,
    /// Whether it names `self`: then it only makes sense attached to something.
    pub uses_self: bool,
    pub(crate) functions: Vec<Function>,
    /// The function that sets the constants and state to their starting values.
    pub(crate) init: u32,
    /// The events it handles, and with which function.
    pub(crate) handlers: Vec<(u16, u32)>,
}

impl Program {
    /// The script's inputs, in the order [`crate::Runtime::spawn_instance`] takes their values.
    pub fn inputs(&self) -> impl Iterator<Item = (usize, &GlobalDef)> {
        self.globals.iter().enumerate().filter(|(_, g)| matches!(g.kind, GlobalKind::Input { .. }))
    }

    /// Whether it handles event `event`.
    pub fn handles(&self, event: u16) -> bool {
        self.handlers.iter().any(|(e, _)| *e == event)
    }
}

#[derive(Clone, Debug)]
pub struct GlobalDef {
    pub name: String,
    pub ty: Type,
    pub kind: GlobalKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GlobalKind {
    /// `const x = input<T>()`: set where the script is placed; `fallback` is `input(2.0)`'s.
    Input { fallback: Option<Value> },
    Const,
    /// A top-level `let`: `exported` ones are for other scripts and the editor to see.
    State { exported: bool },
}

#[derive(Debug)]
pub(crate) struct Function {
    /// Which of the program's files it is from.
    pub file: u16,
    pub code: Vec<Op>,
    /// Per operation, the line it came from.
    pub lines: Vec<u32>,
    pub params: u16,
    pub locals: u16,
}

#[derive(Clone, Copy)]
enum Sym {
    Global(u16),
    Func(u32),
}

#[derive(Clone)]
struct Sig {
    params: Vec<Type>,
    ret: Type,
    is_async: bool,
}

struct GlobalInfo {
    name: String,
    /// `None` until its starting value has been compiled.
    ty: Option<Type>,
    kind: GlobalKind,
    is_const: bool,
}

struct ModuleInfo {
    file: String,
    syms: HashMap<String, Sym>,
    exported: Vec<String>,
}

struct Compiler<'a> {
    desc: &'a ApiDesc,
    strings: &'a mut Strings,
    modules: Vec<ModuleInfo>,
    globals: Vec<GlobalInfo>,
    sigs: Vec<Sig>,
    uses_self: bool,
}

/// Where an import leads: `spec` as written in file `from`, as a path in the scripts folder.
fn resolve_import(from: &str, spec: &str) -> Result<String, String> {
    if !(spec.starts_with("./") || spec.starts_with("../")) {
        return Err(format!("an import is relative to the file (\"./name\"), not \"{spec}\""));
    }
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for part in spec.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(format!("\"{spec}\" leaves the scripts folder"));
                }
            }
            part => parts.push(part),
        }
    }
    let mut path = parts.join("/");
    if !path.ends_with(".ts") {
        path.push_str(".ts");
    }
    Ok(path)
}

/// Compiles script `root` (a path in the scripts folder), reading it and what it imports
/// with `load`.
pub fn compile(
    desc: &ApiDesc,
    strings: &mut Strings,
    root: &str,
    load: &mut dyn FnMut(&str) -> Result<String, String>,
) -> R<Program> {
    let err = |file: &str, line: usize, message: String| ScriptError { file: file.to_string(), line, message };
    // The script and everything it imports, each read once.
    let (mut files, mut asts, mut deps) = (vec![root.to_string()], Vec::new(), Vec::new());
    let mut i = 0;
    while i < files.len() {
        let file = files[i].clone();
        let src = load(&file).map_err(|m| err(&file, 0, m))?;
        let ast = parse(&src).map_err(|(line, m)| err(&file, line, m))?;
        let mut imported = Vec::new();
        for import in &ast.imports {
            let path = resolve_import(&file, &import.from).map_err(|m| err(&file, import.line, m))?;
            let at = files.iter().position(|f| *f == path).unwrap_or_else(|| {
                files.push(path);
                files.len() - 1
            });
            imported.push(at);
        }
        asts.push(ast);
        deps.push(imported);
        i += 1;
    }
    // What a file imports starts before it does.
    fn visit(m: usize, deps: &[Vec<usize>], seen: &mut Vec<bool>, order: &mut Vec<usize>) {
        if std::mem::replace(&mut seen[m], true) {
            return;
        }
        for &d in &deps[m] {
            visit(d, deps, seen, order);
        }
        order.push(m);
    }
    let mut order = Vec::new();
    visit(0, &deps, &mut vec![false; files.len()], &mut order);

    let mut c = Compiler { desc, strings, modules: Vec::new(), globals: Vec::new(), sigs: Vec::new(), uses_self: false };
    // Every file's names, before any code: functions can be called before they are written.
    for (m, ast) in asts.iter().enumerate() {
        let file = &files[m];
        let mut info = ModuleInfo { file: file.clone(), syms: HashMap::new(), exported: Vec::new() };
        for item in &ast.items {
            let (name, line, exported, sym) = match item {
                Item::Var(v) => {
                    let is_input = matches!(&v.init.kind, ExprKind::Call { callee, .. } if matches!(&callee.kind, ExprKind::Ident(n) if n == "input"));
                    let shared = "it is imported by other scripts, which share only functions and constants";
                    let (kind, ty) = if is_input {
                        if m != 0 {
                            return Err(err(file, v.line, format!("'{}' can't be an input here: {shared}", v.name)));
                        }
                        if !v.is_const {
                            return Err(err(file, v.line, format!("an input is a const: const {} = input...", v.name)));
                        }
                        let (ty, fallback) = c.input(v).map_err(|m| err(file, v.line, m))?;
                        (GlobalKind::Input { fallback }, Some(ty))
                    } else if v.is_const {
                        (GlobalKind::Const, None)
                    } else if m != 0 {
                        return Err(err(file, v.line, format!("'{}' can't be state (let) here: {shared}", v.name)));
                    } else {
                        (GlobalKind::State { exported: v.exported }, None)
                    };
                    c.globals.push(GlobalInfo { name: v.name.clone(), ty, kind, is_const: v.is_const });
                    (&v.name, v.line, v.exported, Sym::Global(c.globals.len() as u16 - 1))
                }
                Item::Func(f) => {
                    let resolve = |t: &TypeExpr| desc.resolve(t).map_err(|m| err(file, f.line, m));
                    let params = f.params.iter().map(|(_, t)| resolve(t)).collect::<R<Vec<Type>>>()?;
                    let ret = match (f.ret.as_ref().map(resolve).transpose()?, f.is_async) {
                        (None, _) => Type::Void,
                        (Some(Type::Promise(inner)), true) => *inner,
                        (Some(_), true) => {
                            return Err(err(file, f.line, format!("'{}' is async: it returns a Promise<...>", f.name)));
                        }
                        (Some(ret), false) => ret,
                    };
                    c.sigs.push(Sig { params, ret, is_async: f.is_async });
                    (&f.name, f.line, f.exported, Sym::Func(c.sigs.len() as u32 - 1))
                }
            };
            if info.syms.insert(name.clone(), sym).is_some() {
                return Err(err(file, line, format!("'{name}' is declared twice")));
            }
            if exported {
                info.exported.push(name.clone());
            }
        }
        c.modules.push(info);
    }
    for (m, ast) in asts.iter().enumerate() {
        for (import, &from) in ast.imports.iter().zip(&deps[m]) {
            for name in &import.names {
                let Some(&sym) = c.modules[from].syms.get(name).filter(|_| c.modules[from].exported.contains(name)) else {
                    return Err(err(&files[m], import.line, format!("{} doesn't export '{name}'", files[from])));
                };
                if c.modules[m].syms.insert(name.clone(), sym).is_some() {
                    return Err(err(&files[m], import.line, format!("'{name}' is declared twice")));
                }
            }
        }
    }

    // The starting values, in order: they settle the variables' types.
    let mut init = Fc::new(&mut c, 0, Type::Void, false);
    init.in_init = true;
    for &m in &order {
        init.module = m;
        for item in &asts[m].items {
            if let Item::Var(v) = item {
                let Sym::Global(slot) = init.c.modules[m].syms[&v.name] else { unreachable!() };
                if matches!(init.c.globals[slot as usize].kind, GlobalKind::Input { .. }) {
                    continue;
                }
                let ty = init.var_init(v)?;
                init.c.globals[slot as usize].ty = Some(ty);
                init.emit(Op::SetGlobal(slot));
            }
        }
    }
    init.emit(Op::Push(Value::Null));
    init.emit(Op::Return);
    init.module = 0;
    let init = init.finish(0);

    let mut functions = Vec::new();
    for (m, ast) in asts.iter().enumerate() {
        for item in &ast.items {
            let Item::Func(f) = item else { continue };
            let sig = c.sigs[functions.len()].clone();
            let mut fc = Fc::new(&mut c, m, sig.ret.clone(), f.is_async);
            for ((name, _), ty) in f.params.iter().zip(&sig.params) {
                fc.declare(name, ty.clone(), false, f.line)?;
            }
            let exits = fc.block(&f.body)?;
            if !exits {
                if sig.ret != Type::Void {
                    return Err(fc.err(f.line, format!("'{}' doesn't return a value on every path", f.name)));
                }
                fc.emit(Op::Push(Value::Null));
                fc.emit(Op::Return);
            }
            functions.push(fc.finish(f.params.len() as u16));
        }
    }

    // The events the script handles: its exported functions with an event's name.
    let mut handlers = Vec::new();
    for item in &asts[0].items {
        let Item::Func(f) = item else { continue };
        if !f.exported {
            continue;
        }
        let Some(Sym::Func(index)) = c.modules[0].syms.get(&f.name).copied() else { unreachable!() };
        let Some(event) = desc.event_id(&f.name) else {
            if f.name.starts_with("on") && f.name[2..].starts_with(|ch: char| ch.is_ascii_uppercase()) {
                let known: Vec<&str> = desc.events.iter().map(|e| e.name.as_str()).collect();
                return Err(err(&files[0], f.line, format!("no event is called '{}' (there are: {})", f.name, known.join(", "))));
            }
            continue;
        };
        let (event_def, sig) = (&desc.events[event as usize], &c.sigs[index as usize]);
        let written = event_def.params.iter().map(|(n, t)| format!("{n}: {}", desc.type_name(t))).collect::<Vec<_>>().join(", ");
        let fits = sig.params.len() <= event_def.params.len()
            && sig.params.iter().zip(&event_def.params).all(|(mine, (_, theirs))| assignable(theirs, mine))
            && sig.ret == Type::Void;
        if !fits {
            return Err(err(&files[0], f.line, format!("a handler of this event is written {}({written})", f.name)));
        }
        handlers.push((event, index));
    }

    let globals = c
        .globals
        .into_iter()
        .map(|g| GlobalDef { name: g.name, ty: g.ty.unwrap_or(Type::Void), kind: g.kind })
        .collect();
    functions.push(init);
    Ok(Program { files, globals, uses_self: c.uses_self, init: functions.len() as u32 - 1, functions, handlers })
}

/// Whether a value of type `from` can go where `to` is wanted.
fn assignable(from: &Type, to: &Type) -> bool {
    match (from, to) {
        (Type::Void, _) => false,
        _ if from == to => true,
        (_, Type::Any) | (Type::Int, Type::Number) => true,
        (Type::Null, Type::Nullable(_)) => true,
        (Type::Nullable(from), Type::Nullable(to)) => assignable(from, to),
        (from, Type::Nullable(to)) => assignable(from, to),
        _ => false,
    }
}

/// Whether `e` is a number written out (`2`, `-1`, `60 * 60`): then it is a `number`, as
/// in TypeScript, unless something says `int`.
fn is_literal_number(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Num(..) => true,
        ExprKind::Unary("-" | "+", x) => is_literal_number(x),
        ExprKind::Binary("+" | "-" | "*" | "%", a, b) => is_literal_number(a) && is_literal_number(b),
        _ => false,
    }
}

/// The names assigned to anywhere in `s`.
fn assigned(s: &Stmt, out: &mut Vec<String>) {
    match &s.kind {
        StmtKind::Var(v) => assigned_in(&v.init, out),
        StmtKind::Expr(e) => assigned_in(e, out),
        StmtKind::If(c, a, b) => {
            assigned_in(c, out);
            assigned(a, out);
            if let Some(b) = b {
                assigned(b, out);
            }
        }
        StmtKind::While(c, body) => {
            assigned_in(c, out);
            assigned(body, out);
        }
        StmtKind::For { init, cond, step, body } => {
            if let Some(init) = init {
                assigned(init, out);
            }
            for e in [cond, step].into_iter().flatten() {
                assigned_in(e, out);
            }
            assigned(body, out);
        }
        StmtKind::ForOf { iter, body, .. } => {
            assigned_in(iter, out);
            assigned(body, out);
        }
        StmtKind::Return(e) => {
            if let Some(e) = e {
                assigned_in(e, out);
            }
        }
        StmtKind::Break | StmtKind::Continue => {}
        StmtKind::Block(stmts) => stmts.iter().for_each(|s| assigned(s, out)),
    }
}

fn assigned_in(e: &Expr, out: &mut Vec<String>) {
    match &e.kind {
        ExprKind::Assign(_, target, value) => {
            if let ExprKind::Ident(name) = &target.kind {
                out.push(name.clone());
            }
            assigned_in(target, out);
            assigned_in(value, out);
        }
        ExprKind::Step(target, _) => {
            if let ExprKind::Ident(name) = &target.kind {
                out.push(name.clone());
            }
        }
        ExprKind::Unary(_, x) | ExprKind::Await(x) | ExprKind::NonNull(x) => assigned_in(x, out),
        ExprKind::Binary(_, a, b) => {
            assigned_in(a, out);
            assigned_in(b, out);
        }
        ExprKind::Cond(c, a, b) => {
            assigned_in(c, out);
            assigned_in(a, out);
            assigned_in(b, out);
        }
        ExprKind::Call { callee, args, .. } => {
            assigned_in(callee, out);
            args.iter().for_each(|a| assigned_in(a, out));
        }
        ExprKind::Member { object, .. } => assigned_in(object, out),
        ExprKind::Array(items) => items.iter().for_each(|a| assigned_in(a, out)),
        ExprKind::Num(..) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Null | ExprKind::Ident(_) => {}
    }
}

impl Compiler<'_> {
    /// The type and fallback of an input, `const x = input<T>()` or `input(VALUE)`.
    fn input(&mut self, v: &VarDecl) -> Result<(Type, Option<Value>), String> {
        let ExprKind::Call { type_arg, args, .. } = &v.init.kind else { unreachable!() };
        let declared = match (type_arg, &v.ty) {
            (Some(t), _) | (None, Some(t)) => Some(self.desc.resolve(t)?),
            (None, None) => None,
        };
        let fallback = match args.as_slice() {
            [] => None,
            [value] => Some(self.constant(value).ok_or("an input's fallback is written out: a number, true or false, a string, rgb(...) or vec3(...)")?),
            _ => return Err("input() takes one fallback value, or none".to_string()),
        };
        let class = |name: &str| self.desc.class_id(name).map(Type::Class);
        let found = match (&fallback, args.first().map(|a| &a.kind)) {
            (None, _) => None,
            (Some(Value::Vec3(_)), Some(ExprKind::Call { callee, .. })) => match &callee.kind {
                ExprKind::Ident(n) if n == "rgb" => class("Color"),
                _ => class("Vec3"),
            },
            (Some(Value::Bool(_)), _) => Some(Type::Bool),
            (Some(Value::Str(_)), _) => Some(Type::Str),
            (Some(Value::Null), _) => Some(Type::Null),
            _ => Some(Type::Number),
        };
        let ty = match (declared, found) {
            (None, None) => return Err("an input needs a type or a fallback: input<Entity>() or input(2.0)".to_string()),
            (None, Some(Type::Null)) => return Err("an input that may be null needs its type: input<Light | null>()".to_string()),
            (None, Some(found)) => found,
            (Some(declared), None) => declared,
            (Some(declared), Some(found)) => {
                if !assignable(&found, &declared) {
                    return Err(format!(
                        "the fallback is {}, but the input is {}",
                        self.desc.type_name(&found),
                        self.desc.type_name(&declared)
                    ));
                }
                declared
            }
        };
        // A whole fallback for an int input stays whole.
        let fallback = fallback.map(|f| match (&ty, f) {
            (Type::Int, f) => Value::Int(f.int()),
            (_, Value::Int(i)) => Value::Num(i as f32),
            (_, f) => f,
        });
        Ok((ty, fallback))
    }

    /// The value of an expression written out with no names in it.
    fn constant(&mut self, e: &Expr) -> Option<Value> {
        Some(match &e.kind {
            ExprKind::Num(n, true) => Value::Int(*n as i32),
            ExprKind::Num(n, false) => Value::Num(*n as f32),
            ExprKind::Bool(b) => Value::Bool(*b),
            ExprKind::Str(s) => Value::Str(self.strings.intern(s)),
            ExprKind::Null => Value::Null,
            ExprKind::Unary("-", x) => match self.constant(x)? {
                Value::Int(i) => Value::Int(-i),
                Value::Num(n) => Value::Num(-n),
                _ => return None,
            },
            ExprKind::Call { callee, args, type_arg: None } => match (&callee.kind, args.as_slice()) {
                (ExprKind::Ident(n), [x, y, z]) if n == "rgb" || n == "vec3" => {
                    let mut number = |e: &Expr| match self.constant(e)? {
                        v @ (Value::Int(_) | Value::Num(_)) => Some(v.num()),
                        _ => None,
                    };
                    Value::Vec3([number(x)?, number(y)?, number(z)?])
                }
                _ => return None,
            },
            _ => return None,
        })
    }
}

/// A variable a function can name.
#[derive(Clone)]
struct Var {
    key: Key,
    ty: Type,
    is_const: bool,
}

/// A local (by its slot) or one of the instance's variables.
#[derive(Clone, Copy, PartialEq)]
enum Key {
    Local(u16),
    Global(u16),
}

struct Loop {
    breaks: Vec<usize>,
    continues: Vec<usize>,
    /// A `for...of`'s list, to free on the way out.
    list: Option<u16>,
}

/// One function being compiled.
struct Fc<'a, 'b> {
    c: &'a mut Compiler<'b>,
    module: usize,
    code: Vec<Op>,
    lines: Vec<u32>,
    line: u32,
    scopes: Vec<Vec<(String, Var)>>,
    locals: u16,
    ret: Type,
    is_async: bool,
    in_init: bool,
    loops: Vec<Loop>,
    /// The variables known not to be null here, though their types allow it.
    narrowed: Vec<Key>,
}

impl<'a, 'b> Fc<'a, 'b> {
    fn new(c: &'a mut Compiler<'b>, module: usize, ret: Type, is_async: bool) -> Self {
        Fc {
            c,
            module,
            code: Vec::new(),
            lines: Vec::new(),
            line: 0,
            scopes: vec![Vec::new()],
            locals: 0,
            ret,
            is_async,
            in_init: false,
            loops: Vec::new(),
            narrowed: Vec::new(),
        }
    }

    fn finish(self, params: u16) -> Function {
        Function { file: self.module as u16, code: self.code, lines: self.lines, params, locals: self.locals }
    }

    fn err(&self, line: usize, message: impl Into<String>) -> ScriptError {
        ScriptError { file: self.c.modules[self.module].file.clone(), line, message: message.into() }
    }

    fn name(&self, ty: &Type) -> String {
        self.c.desc.type_name(ty)
    }

    fn emit(&mut self, op: Op) -> usize {
        self.code.push(op);
        self.lines.push(self.line);
        self.code.len() - 1
    }

    fn here(&self) -> u32 {
        self.code.len() as u32
    }

    /// Points the jump at `at` to here.
    fn land(&mut self, at: usize) {
        let to = self.here();
        match &mut self.code[at] {
            Op::Jump(t) | Op::JumpIfFalse(t) | Op::JumpIfNull(t) | Op::JumpIfNotNull(t) => *t = to,
            Op::IterNext { exit, .. } => *exit = to,
            _ => unreachable!(),
        }
    }

    fn temp(&mut self) -> u16 {
        self.locals += 1;
        self.locals - 1
    }

    fn declare(&mut self, name: &str, ty: Type, is_const: bool, line: usize) -> R<u16> {
        if self.scopes.last().unwrap().iter().any(|(n, _)| n == name) {
            return Err(self.err(line, format!("'{name}' is declared twice")));
        }
        let slot = self.temp();
        self.scopes.last_mut().unwrap().push((name.to_string(), Var { key: Key::Local(slot), ty, is_const }));
        Ok(slot)
    }

    /// The variable called `name` here, if there is one.
    fn lookup(&self, name: &str, line: usize) -> R<Option<Var>> {
        if let Some((_, var)) = self.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|(n, _)| n == name) {
            return Ok(Some(var.clone()));
        }
        let Some(Sym::Global(slot)) = self.c.modules[self.module].syms.get(name).copied() else {
            return Ok(None);
        };
        let global = &self.c.globals[slot as usize];
        let Some(ty) = global.ty.clone() else {
            return Err(self.err(line, format!("'{name}' is used before it has its starting value")));
        };
        Ok(Some(Var { key: Key::Global(slot), ty, is_const: global.is_const }))
    }

    /// Whether `name` is free to mean something of the API's (no variable or function of
    /// the script's has it).
    fn is_free(&self, name: &str) -> bool {
        !self.scopes.iter().flatten().any(|(n, _)| n == name) && !self.c.modules[self.module].syms.contains_key(name)
    }

    /// Checks that a `from` can go where a `to` is wanted, converting the value on the
    /// stack if it is an int going where a number is.
    fn coerce(&mut self, from: &Type, to: &Type, line: usize) -> R<()> {
        if !assignable(from, to) {
            let (from, to) = (self.name(from), self.name(to));
            return Err(self.err(line, match from.as_str() {
                "void" => format!("this has no value (a {to} is wanted)"),
                _ => format!("this is {from}, where {to} is wanted"),
            }));
        }
        let number = |t: &Type| matches!(t, Type::Number) || matches!(t, Type::Nullable(inner) if **inner == Type::Number);
        let int = |t: &Type| matches!(t, Type::Int) || matches!(t, Type::Nullable(inner) if **inner == Type::Int);
        if int(from) && number(to) {
            self.emit(Op::IntToNum);
        }
        Ok(())
    }

    /// The variables that aren't null when `e` is `truthy`.
    fn narrows(&self, e: &Expr, truthy: bool) -> Vec<Key> {
        let nullable = |x: &Expr| match &x.kind {
            ExprKind::Ident(name) => match self.lookup(name, 0) {
                Ok(Some(Var { key, ty: Type::Nullable(_), .. })) => Some(key),
                _ => None,
            },
            _ => None,
        };
        let against_null = |a: &Expr, b: &Expr| match (&a.kind, &b.kind) {
            (_, ExprKind::Null) => nullable(a),
            (ExprKind::Null, _) => nullable(b),
            _ => None,
        };
        match &e.kind {
            ExprKind::Ident(_) if truthy => nullable(e).into_iter().collect(),
            ExprKind::Unary("!", x) => self.narrows(x, !truthy),
            ExprKind::Binary("!==" | "!=", a, b) if truthy => against_null(a, b).into_iter().collect(),
            ExprKind::Binary("===" | "==", a, b) if !truthy => against_null(a, b).into_iter().collect(),
            ExprKind::Binary(op @ ("&&" | "||"), a, b) if (*op == "&&") == truthy => {
                let mut keys = self.narrows(a, truthy);
                keys.extend(self.narrows(b, truthy));
                keys
            }
            _ => Vec::new(),
        }
    }

    fn narrow(&mut self, keys: Vec<Key>) {
        for key in keys {
            if !self.narrowed.contains(&key) {
                self.narrowed.push(key);
            }
        }
    }

    /// Forgets what is known of the variables assigned in a loop: it may run again after
    /// they change.
    fn forget_assigned(&mut self, stmts: &[&Stmt], exprs: &[&Expr]) {
        let mut names = Vec::new();
        stmts.iter().for_each(|s| assigned(s, &mut names));
        exprs.iter().for_each(|e| assigned_in(e, &mut names));
        for name in names {
            if let Ok(Some(var)) = self.lookup(&name, 0) {
                self.narrowed.retain(|k| *k != var.key);
            }
        }
    }

    // ---- Statements

    /// Compiles statements in a scope of their own. Returns whether they always leave
    /// (return, break or continue) before their end.
    fn block(&mut self, stmts: &[Stmt]) -> R<bool> {
        let entry = self.narrowed.clone();
        self.scopes.push(Vec::new());
        let mut exits = false;
        for s in stmts {
            exits |= self.stmt(s)?;
        }
        self.scopes.pop();
        self.narrowed.retain(|k| entry.contains(k));
        Ok(exits)
    }

    /// Compiles one arm of a branch, where `known` aren't null. Returns whether it always
    /// leaves, and what isn't null at its end.
    fn arm(&mut self, s: Option<&Stmt>, known: Vec<Key>) -> R<(bool, Vec<Key>)> {
        let before = self.narrowed.clone();
        self.narrow(known);
        let exits = match s {
            Some(s) => self.block(std::slice::from_ref(s))?,
            None => false,
        };
        Ok((exits, std::mem::replace(&mut self.narrowed, before)))
    }

    fn stmt(&mut self, s: &Stmt) -> R<bool> {
        self.line = s.line as u32;
        match &s.kind {
            StmtKind::Var(v) => {
                let ty = self.var_init(v)?;
                let slot = self.declare(&v.name, ty, v.is_const, v.line)?;
                self.emit(Op::SetLocal(slot));
                Ok(false)
            }
            StmtKind::Expr(e) => {
                self.expr(e)?;
                self.emit(Op::Pop);
                Ok(false)
            }
            StmtKind::If(cond, then, otherwise) => {
                self.cond(cond)?;
                let to_else = self.emit(Op::JumpIfFalse(0));
                let (then_exits, then_known) = self.arm(Some(&**then), self.narrows(cond, true))?;
                let to_end = otherwise.as_ref().map(|_| self.emit(Op::Jump(0)));
                self.land(to_else);
                let (else_exits, else_known) = self.arm(otherwise.as_deref(), self.narrows(cond, false))?;
                if let Some(to_end) = to_end {
                    self.land(to_end);
                }
                // What is known after it: what the arm that carries on knew.
                self.narrowed = match (then_exits, else_exits) {
                    (true, true) => self.narrowed.clone(),
                    (true, false) => else_known,
                    (false, true) => then_known,
                    (false, false) => then_known.into_iter().filter(|k| else_known.contains(k)).collect(),
                };
                Ok(then_exits && else_exits)
            }
            StmtKind::While(cond, body) => {
                self.forget_assigned(&[&**body], &[cond]);
                let start = self.here();
                self.cond(cond)?;
                let out = self.emit(Op::JumpIfFalse(0));
                self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(), list: None });
                self.arm(Some(&**body), self.narrows(cond, true))?;
                self.emit(Op::Jump(start));
                self.land(out);
                self.end_loop(start);
                Ok(false)
            }
            StmtKind::For { init, cond, step, body } => {
                let entry = self.narrowed.clone();
                self.scopes.push(Vec::new());
                if let Some(init) = init {
                    self.stmt(init)?;
                }
                self.forget_assigned(&[&**body], &[cond, step].into_iter().flatten().collect::<Vec<_>>());
                let start = self.here();
                let out = match cond {
                    Some(cond) => {
                        self.cond(cond)?;
                        Some(self.emit(Op::JumpIfFalse(0)))
                    }
                    None => None,
                };
                self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(), list: None });
                self.arm(Some(&**body), cond.as_ref().map_or(Vec::new(), |c| self.narrows(c, true)))?;
                let step_at = self.here();
                if let Some(step) = step {
                    self.expr(step)?;
                    self.emit(Op::Pop);
                }
                self.emit(Op::Jump(start));
                if let Some(out) = out {
                    self.land(out);
                }
                self.end_loop(step_at);
                self.scopes.pop();
                self.narrowed.retain(|k| entry.contains(k));
                Ok(false)
            }
            StmtKind::ForOf { name, iter, body } => {
                let entry = self.narrowed.clone();
                let Type::Iterable(item_ty) = self.expr(iter)? else {
                    return Err(self.err(s.line, "for...of walks what the engine lists (lights.inSector(...), entities.all(), ...)"));
                };
                self.scopes.push(Vec::new());
                let (list, index) = (self.temp(), self.temp());
                self.emit(Op::SetLocal(list));
                self.emit(Op::Push(Value::Int(0)));
                self.emit(Op::SetLocal(index));
                let item = self.declare(name, *item_ty, true, s.line)?;
                self.forget_assigned(&[&**body], &[]);
                let start = self.here();
                let next = self.emit(Op::IterNext { list, index, item, exit: 0 });
                self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(), list: Some(list) });
                self.arm(Some(&**body), Vec::new())?;
                self.emit(Op::Jump(start));
                self.land(next);
                self.end_loop(start);
                self.emit(Op::ListFree(list));
                self.scopes.pop();
                self.narrowed.retain(|k| entry.contains(k));
                Ok(false)
            }
            StmtKind::Return(value) => {
                if self.in_init {
                    return Err(self.err(s.line, "return outside a function"));
                }
                let ret = self.ret.clone();
                match value {
                    None if ret == Type::Void => {
                        self.emit(Op::Push(Value::Null));
                    }
                    None => return Err(self.err(s.line, format!("return needs a value (a {})", self.name(&ret)))),
                    Some(_) if ret == Type::Void => {
                        return Err(self.err(s.line, "this function returns nothing: give it a return type to return a value"));
                    }
                    Some(value) => {
                        let ty = self.expr(value)?;
                        self.coerce(&ty, &ret, s.line)?;
                    }
                }
                for list in self.loops.iter().rev().filter_map(|l| l.list).collect::<Vec<_>>() {
                    self.emit(Op::ListFree(list));
                }
                self.emit(Op::Return);
                Ok(true)
            }
            StmtKind::Break | StmtKind::Continue => {
                if self.loops.is_empty() {
                    return Err(self.err(s.line, "break and continue belong in a loop"));
                }
                let jump = self.emit(Op::Jump(0));
                let current = self.loops.last_mut().unwrap();
                if matches!(s.kind, StmtKind::Break) { current.breaks.push(jump) } else { current.continues.push(jump) }
                Ok(true)
            }
            StmtKind::Block(stmts) => self.block(stmts),
        }
    }

    /// Ends the innermost loop: its `break`s come here, its `continue`s go to `continue_to`.
    fn end_loop(&mut self, continue_to: u32) {
        let ended = self.loops.pop().unwrap();
        for at in ended.breaks {
            self.land(at);
        }
        for at in ended.continues {
            self.code[at] = Op::Jump(continue_to);
        }
    }

    /// Compiles a variable's starting value and returns the variable's type: the one it is
    /// given, or its value's.
    fn var_init(&mut self, v: &VarDecl) -> R<Type> {
        self.line = v.line as u32;
        if let Some(written) = &v.ty {
            let want = self.c.desc.resolve(written).map_err(|m| self.err(v.line, m))?;
            let ty = self.expr(&v.init)?;
            self.coerce(&ty, &want, v.line)?;
            return Ok(want);
        }
        match self.expr(&v.init)? {
            Type::Void => Err(self.err(v.line, format!("'{}' is given something with no value", v.name))),
            Type::Null => Err(self.err(v.line, format!("'{0}' needs a type to start as null ({0}: Entity | null = null)", v.name))),
            Type::Int if is_literal_number(&v.init) => {
                self.emit(Op::IntToNum);
                Ok(Type::Number)
            }
            ty => Ok(ty),
        }
    }

    // ---- Expressions: each leaves one value on the stack.

    /// Compiles a condition: a boolean, or something that may be null (true if it isn't).
    fn cond(&mut self, e: &Expr) -> R<()> {
        match self.expr(e)? {
            Type::Bool => {}
            Type::Nullable(inner) if *inner != Type::Bool && !inner.is_numeric() => {
                self.emit(Op::IsNull);
                self.emit(Op::Not);
            }
            ty => {
                let ty = self.name(&ty);
                return Err(self.err(e.line, format!("a condition is a boolean, not {ty} (compare it: x !== 0, x !== null)")));
            }
        }
        Ok(())
    }

    fn expr(&mut self, e: &Expr) -> R<Type> {
        self.line = e.line as u32;
        match &e.kind {
            ExprKind::Num(n, whole) => {
                if *whole && n.abs() < i32::MAX as f64 {
                    self.emit(Op::Push(Value::Int(*n as i32)));
                    Ok(Type::Int)
                } else {
                    self.emit(Op::Push(Value::Num(*n as f32)));
                    Ok(Type::Number)
                }
            }
            ExprKind::Str(s) => {
                let id = self.c.strings.intern(s);
                self.emit(Op::Push(Value::Str(id)));
                Ok(Type::Str)
            }
            ExprKind::Bool(b) => {
                self.emit(Op::Push(Value::Bool(*b)));
                Ok(Type::Bool)
            }
            ExprKind::Null => {
                self.emit(Op::Push(Value::Null));
                Ok(Type::Null)
            }
            ExprKind::Ident(name) => self.ident(name, e.line),
            ExprKind::Unary("!", x) => {
                self.cond(x)?;
                self.emit(Op::Not);
                Ok(Type::Bool)
            }
            ExprKind::Unary(op, x) => match self.expr(x)? {
                ty @ (Type::Int | Type::Number) => {
                    if *op == "-" {
                        self.emit(if ty == Type::Int { Op::NegI } else { Op::NegF });
                    }
                    Ok(ty)
                }
                ty => Err(self.err(e.line, format!("'{op}' takes a number, not {}", self.name(&ty)))),
            },
            ExprKind::Binary(op, a, b) => self.binary(op, a, b, e.line),
            ExprKind::Assign(op, target, value) => self.assign(*op, target, Some(value), 0, e.line),
            ExprKind::Step(target, by) => self.assign(Some("+"), target, None, *by, e.line),
            ExprKind::Cond(cond, then, otherwise) => {
                self.cond(cond)?;
                let to_else = self.emit(Op::JumpIfFalse(0));
                let before = self.narrowed.clone();
                self.narrow(self.narrows(cond, true));
                let then_ty = self.expr(then)?;
                // (Made a conversion below, if the other arm turns out a number.)
                let fix = self.emit(Op::Jump(self.here() + 1));
                let to_end = self.emit(Op::Jump(0));
                self.land(to_else);
                self.narrowed = before.clone();
                self.narrow(self.narrows(cond, false));
                let else_ty = self.expr(otherwise)?;
                self.narrowed = before;
                let ty = match (&then_ty, &else_ty) {
                    (a, b) if a == b => then_ty.clone(),
                    (Type::Int, Type::Number) => {
                        self.code[fix] = Op::IntToNum;
                        Type::Number
                    }
                    (Type::Number, Type::Int) => {
                        self.emit(Op::IntToNum);
                        Type::Number
                    }
                    (Type::Null, other) | (other, Type::Null) if *other != Type::Void => other.clone().nullable(),
                    (a, b) if assignable(b, a) => then_ty.clone(),
                    (a, b) if assignable(a, b) => else_ty.clone(),
                    (a, b) => {
                        let (a, b) = (self.name(a), self.name(b));
                        return Err(self.err(e.line, format!("the two results differ: {a} and {b}")));
                    }
                };
                self.land(to_end);
                Ok(ty)
            }
            ExprKind::Call { .. } | ExprKind::Member { .. } => {
                // A `?.` on something null skips the rest of its chain, which is then null.
                let (ty, skips) = self.chain(e)?;
                if skips.is_empty() {
                    return Ok(ty);
                }
                for skip in skips {
                    self.land(skip);
                }
                Ok(ty.nullable())
            }
            ExprKind::Await(x) => {
                if !self.is_async {
                    return Err(self.err(e.line, "await belongs in an async function"));
                }
                match self.expr(x)? {
                    Type::Promise(inner) => {
                        self.emit(Op::Await);
                        Ok(*inner)
                    }
                    ty => Err(self.err(e.line, format!("await takes a promise, not {}", self.name(&ty)))),
                }
            }
            ExprKind::NonNull(x) => {
                let ty = self.expr(x)?;
                self.emit(Op::NonNull);
                Ok(match ty {
                    Type::Nullable(inner) => *inner,
                    ty => ty,
                })
            }
            ExprKind::Array(_) => Err(self.err(e.line, "a list ([ ]) is only what Promise.all and Promise.race take")),
        }
    }

    fn ident(&mut self, name: &str, line: usize) -> R<Type> {
        if let Some(var) = self.lookup(name, line)? {
            self.emit(match var.key {
                Key::Local(slot) => Op::GetLocal(slot),
                Key::Global(slot) => Op::GetGlobal(slot),
            });
            return Ok(match var.ty {
                Type::Nullable(inner) if self.narrowed.contains(&var.key) => *inner,
                ty => ty,
            });
        }
        let desc = self.c.desc;
        if self.c.modules[self.module].syms.contains_key(name) || desc.funcs.iter().any(|f| f.name == name) {
            return Err(self.err(line, format!("'{name}' is a function: call it")));
        }
        if name == "self" {
            let Some(class) = desc.self_class else {
                return Err(self.err(line, "'self' is not defined"));
            };
            self.c.uses_self = true;
            self.emit(Op::SelfRef);
            return Ok(Type::Class(class));
        }
        if let Some(global) = desc.globals.iter().find(|g| g.name == name) {
            self.emit(Op::Push(Value::Null));
            return Ok(Type::Class(global.class));
        }
        Err(self.err(line, match name {
            "Promise" => "Promise is only for Promise.all([...]) and Promise.race([...])".to_string(),
            _ => format!("'{name}' is not defined"),
        }))
    }

    /// Compiles a chain of member accesses and calls (`a?.b.c()`), and returns its type
    /// when nothing in it is skipped, and the jumps its `?.`s take past the rest of it when
    /// what they are on is null.
    fn chain(&mut self, e: &Expr) -> R<(Type, Vec<usize>)> {
        match &e.kind {
            ExprKind::Call { callee, type_arg, args } => {
                if type_arg.is_some() || matches!(&callee.kind, ExprKind::Ident(n) if n == "input" && self.is_free(n)) {
                    return Err(self.err(e.line, "input() is what a top-level const starts as: const door = input<Entity>();"));
                }
                self.call(callee, args, e.line)
            }
            ExprKind::Member { object, name, optional } => {
                let (class, skips) = self.chain_object(object, *optional)?;
                let class_def = &self.c.desc.classes[class as usize];
                let Some(prop) = class_def.props.iter().find(|p| p.name == *name) else {
                    let what = match class_def.methods.iter().any(|m| m.name == *name) {
                        true => format!("'{name}' is a method: call it"),
                        false => format!("{} has no '{name}'", class_def.name),
                    };
                    return Err(self.err(e.line, what));
                };
                let (get, ty) = (prop.get, prop.ty.clone());
                self.line = e.line as u32;
                self.emit(Op::CallHost(get, 1));
                Ok((ty, skips))
            }
            _ => Ok((self.expr(e)?, Vec::new())),
        }
    }

    /// Compiles the object of a member access or method call (itself maybe a chain), and
    /// returns its class and the chain's jumps so far: with `?.` on something that may be
    /// null, one more.
    fn chain_object(&mut self, object: &Expr, optional: bool) -> R<(ClassId, Vec<usize>)> {
        let (ty, mut skips) = self.chain(object)?;
        match (ty, optional) {
            (Type::Class(class), _) => Ok((class, skips)),
            (Type::Nullable(inner), true) if matches!(*inner, Type::Class(_)) => {
                let Type::Class(class) = *inner else { unreachable!() };
                skips.push(self.emit(Op::JumpIfNull(0)));
                Ok((class, skips))
            }
            (Type::Nullable(inner), false) if matches!(*inner, Type::Class(_)) => {
                Err(self.err(object.line, "this may be null: test it first (if (x !== null) ...), or use ?."))
            }
            (ty, _) => Err(self.err(object.line, format!("{} has no members", self.name(&ty)))),
        }
    }

    /// Compiles arguments against parameters, the last `optional` of which can be left
    /// out (`null` is passed for each). Returns how many values that leaves on the stack.
    fn args(&mut self, name: &str, params: &[Type], optional: usize, rest: bool, args: &[Expr], line: usize) -> R<u8> {
        let required = params.len() - optional;
        if args.len() < required || (args.len() > params.len() && !rest) {
            let takes = match optional {
                0 => params.len().to_string(),
                _ => format!("{required} to {}", params.len()),
            };
            return Err(self.err(line, format!("{name} takes {takes} argument(s), not {}", args.len())));
        }
        if args.len() > 250 {
            return Err(self.err(line, "too many arguments"));
        }
        for (i, arg) in args.iter().enumerate() {
            let ty = self.expr(arg)?;
            self.coerce(&ty, params.get(i).unwrap_or(&Type::Any), arg.line)?;
        }
        for _ in args.len()..params.len() {
            self.emit(Op::Push(Value::Null));
        }
        Ok(args.len().max(params.len()) as u8)
    }

    fn call(&mut self, callee: &Expr, args: &[Expr], line: usize) -> R<(Type, Vec<usize>)> {
        let desc = self.c.desc;
        let types = |m: &Method| m.params.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>();
        match &callee.kind {
            ExprKind::Ident(name) => {
                if let Some(Sym::Func(index)) = self.c.modules[self.module].syms.get(name).copied()
                    && !self.scopes.iter().flatten().any(|(n, _)| n == name)
                {
                    let sig = self.c.sigs[index as usize].clone();
                    self.args(name, &sig.params, 0, false, args, line)?;
                    let ty = if sig.is_async {
                        self.emit(Op::Spawn(index, args.len() as u8));
                        Type::Promise(Box::new(sig.ret))
                    } else {
                        self.emit(Op::Call(index, args.len() as u8));
                        sig.ret
                    };
                    return Ok((ty, Vec::new()));
                }
                if self.is_free(name)
                    && let Some(func) = desc.funcs.iter().find(|f| f.name == *name)
                {
                    let count = self.args(name, &types(func), func.optional, func.rest, args, line)?;
                    self.emit(Op::CallHost(func.func, count));
                    return Ok((func.ret.clone(), Vec::new()));
                }
                Err(self.err(line, format!("'{name}' is not a function")))
            }
            ExprKind::Member { object, name, .. }
                if matches!(&object.kind, ExprKind::Ident(n) if n == "Promise" && self.is_free(n)) =>
            {
                let op = match (name.as_str(), args) {
                    ("all", [Expr { kind: ExprKind::Array(parts), .. }]) => Op::All(parts.len().min(255) as u8),
                    ("race", [Expr { kind: ExprKind::Array(parts), .. }]) => Op::Race(parts.len().min(255) as u8),
                    ("all" | "race", _) => {
                        return Err(self.err(line, format!("Promise.{name} takes one list written out: Promise.{name}([a, b])")));
                    }
                    _ => return Err(self.err(line, format!("Promise.{name} isn't supported (Promise.all and Promise.race are)"))),
                };
                let [Expr { kind: ExprKind::Array(parts), .. }] = args else { unreachable!() };
                if parts.len() > 255 {
                    return Err(self.err(line, "too many promises"));
                }
                for part in parts {
                    let ty = self.expr(part)?;
                    if !matches!(ty, Type::Promise(_)) {
                        return Err(self.err(part.line, format!("Promise.{name} takes promises, and this is {}", self.name(&ty))));
                    }
                }
                self.emit(op);
                Ok((Type::Promise(Box::new(Type::Void)), Vec::new()))
            }
            ExprKind::Member { object, name, optional } => {
                let (class, skips) = self.chain_object(object, *optional)?;
                let class_def = &desc.classes[class as usize];
                let Some(method) = class_def.methods.iter().find(|m| m.name == *name) else {
                    return Err(self.err(line, format!("{} has no method '{name}'", class_def.name)));
                };
                let count = self.args(name, &types(method), method.optional, method.rest, args, line)?;
                self.line = line as u32;
                self.emit(Op::CallHost(method.func, count + 1));
                Ok((method.ret.clone(), skips))
            }
            _ => Err(self.err(line, "this can't be called")),
        }
    }

    /// Makes the two numbers on the stack (of types `a` under `b`) the same kind, and
    /// returns whether they are ints. `float` makes them numbers whatever they are.
    fn same_kind(&mut self, a: &Type, b: &Type, float: bool) -> bool {
        if *a == Type::Int && *b == Type::Int && !float {
            return true;
        }
        if *a == Type::Int {
            self.emit(Op::IntToNumUnder);
        }
        if *b == Type::Int {
            self.emit(Op::IntToNum);
        }
        false
    }

    /// Compiles `+ - * / %` on the two values on the stack.
    fn arithmetic(&mut self, op: &str, a: &Type, b: &Type, line: usize) -> R<Type> {
        if !a.is_numeric() || !b.is_numeric() {
            let what = match (op, a, b) {
                ("+", Type::Str, _) | ("+", _, Type::Str) => "strings can't be joined in Moose scripts yet".to_string(),
                _ => format!("'{op}' takes numbers, not {} and {}", self.name(a), self.name(b)),
            };
            return Err(self.err(line, what));
        }
        // Dividing gives a number, as in JavaScript: 7 / 2 is 3.5.
        let ints = self.same_kind(a, b, op == "/");
        self.emit(match (op, ints) {
            ("+", true) => Op::AddI,
            ("-", true) => Op::SubI,
            ("*", true) => Op::MulI,
            ("%", true) => Op::RemI,
            ("+", false) => Op::AddF,
            ("-", false) => Op::SubF,
            ("*", false) => Op::MulF,
            ("/", false) => Op::DivF,
            _ => Op::RemF,
        });
        Ok(if ints { Type::Int } else { Type::Number })
    }

    fn binary(&mut self, op: &'static str, a: &Expr, b: &Expr, line: usize) -> R<Type> {
        match op {
            "&&" | "||" => {
                self.cond(a)?;
                self.emit(Op::Dup);
                if op == "||" {
                    self.emit(Op::Not);
                }
                let skip = self.emit(Op::JumpIfFalse(0));
                self.emit(Op::Pop);
                let before = self.narrowed.clone();
                self.narrow(self.narrows(a, op == "&&"));
                self.cond(b)?;
                self.narrowed = before;
                self.land(skip);
                Ok(Type::Bool)
            }
            "??" => {
                let Type::Nullable(inner) = self.expr(a)? else {
                    return Err(self.err(line, "the left of ?? is never null, so the right is never used"));
                };
                let skip = self.emit(Op::JumpIfNotNull(0));
                self.emit(Op::Pop);
                let ty = self.expr(b)?;
                self.coerce(&ty, &inner, b.line)?;
                self.land(skip);
                Ok(*inner)
            }
            "+" | "-" | "*" | "/" | "%" => {
                let ta = self.expr(a)?;
                let tb = self.expr(b)?;
                self.arithmetic(op, &ta, &tb, line)
            }
            "<" | "<=" | ">" | ">=" => {
                let ta = self.expr(a)?;
                let tb = self.expr(b)?;
                if !ta.is_numeric() || !tb.is_numeric() {
                    return Err(self.err(line, format!("'{op}' compares numbers, not {} and {}", self.name(&ta), self.name(&tb))));
                }
                let ints = self.same_kind(&ta, &tb, false);
                self.emit(match (op, ints) {
                    ("<", true) => Op::LtI,
                    ("<=", true) => Op::LeI,
                    (">", true) => Op::GtI,
                    (">=", true) => Op::GeI,
                    ("<", false) => Op::LtF,
                    ("<=", false) => Op::LeF,
                    (">", false) => Op::GtF,
                    _ => Op::GeF,
                });
                Ok(Type::Bool)
            }
            _ => {
                let ta = self.expr(a)?;
                let tb = self.expr(b)?;
                if ta.is_numeric() && tb.is_numeric() {
                    self.same_kind(&ta, &tb, false);
                } else if !(assignable(&ta, &tb) || assignable(&tb, &ta) || ta == Type::Null || tb == Type::Null) {
                    return Err(self.err(line, format!("{} and {} are never equal", self.name(&ta), self.name(&tb))));
                }
                self.emit(if op.starts_with('=') { Op::Eq } else { Op::Ne });
                Ok(Type::Bool)
            }
        }
    }

    /// Compiles `target = value`, `target op= value`, or (with no `value`) `target += by`.
    /// An assignment has no value of its own.
    fn assign(&mut self, op: Option<&'static str>, target: &Expr, value: Option<&Expr>, by: i32, line: usize) -> R<Type> {
        // The new value, from the old one on the stack if there is an operator.
        let new_value = |fc: &mut Self, old: Option<&Type>| -> R<Type> {
            let ty = match value {
                Some(value) => fc.expr(value)?,
                None => {
                    fc.emit(Op::Push(Value::Int(by)));
                    Type::Int
                }
            };
            match (op, old) {
                (Some(op), Some(old)) => fc.arithmetic(op, old, &ty, line),
                _ => Ok(ty),
            }
        };
        match &target.kind {
            ExprKind::Ident(name) => {
                let Some(var) = self.lookup(name, line)? else {
                    return Err(self.err(line, format!("'{name}' is not a variable")));
                };
                if var.is_const {
                    return Err(self.err(line, format!("'{name}' is a const: it can't be changed")));
                }
                let (get, set) = match var.key {
                    Key::Local(slot) => (Op::GetLocal(slot), Op::SetLocal(slot)),
                    Key::Global(slot) => (Op::GetGlobal(slot), Op::SetGlobal(slot)),
                };
                let ty = if op.is_some() {
                    self.emit(get);
                    let old = match &var.ty {
                        Type::Nullable(inner) if self.narrowed.contains(&var.key) => (**inner).clone(),
                        ty => ty.clone(),
                    };
                    new_value(self, Some(&old))?
                } else {
                    new_value(self, None)?
                };
                self.coerce(&ty, &var.ty, line)?;
                self.emit(set);
                // What it is known to be: not null, if what it was given can't be.
                self.narrowed.retain(|k| *k != var.key);
                if matches!(var.ty, Type::Nullable(_)) && !matches!(ty, Type::Nullable(_) | Type::Null) {
                    self.narrowed.push(var.key);
                }
            }
            ExprKind::Member { object, name, optional: false } => {
                let (class, skips) = self.chain_object(object, false)?;
                if !skips.is_empty() {
                    return Err(self.err(line, "a property can't be set through ?.: test for null first"));
                }
                let class_def = &self.c.desc.classes[class as usize];
                let Some(prop) = class_def.props.iter().find(|p| p.name == *name) else {
                    return Err(self.err(line, format!("{} has no '{name}'", class_def.name)));
                };
                let Some(set) = prop.set else {
                    let why = match class_def.kind {
                        ClassKind::Value => format!(": a {0} is a value, so make a new one", class_def.name),
                        _ => String::new(),
                    };
                    return Err(self.err(line, format!("'{name}' is readonly{why}")));
                };
                let (get, prop_ty) = (prop.get, prop.ty.clone());
                let ty = if op.is_some() {
                    self.emit(Op::Dup);
                    self.emit(Op::CallHost(get, 1));
                    new_value(self, Some(&prop_ty))?
                } else {
                    new_value(self, None)?
                };
                self.coerce(&ty, &prop_ty, line)?;
                self.emit(Op::CallHost(set, 2));
                self.emit(Op::Pop);
            }
            _ => return Err(self.err(line, "this can't be assigned to")),
        }
        self.emit(Op::Push(Value::Null));
        Ok(Type::Void)
    }
}
