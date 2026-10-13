//! What scripts can reach of the engine: its classes (the world's primitives, their
//! managers, and value types), globals, functions and events, each written once in a table
//! with the Rust that does it. The compiler checks scripts against the table, the VM calls
//! into it, and `moose.d.ts` is written from it ([`crate::declarations`]), so the three
//! can't disagree.

use crate::ast::TypeExpr;
use crate::parse::{parse_signature, parse_type};
use crate::vm::{Cx, Value};

pub type ClassId = u16;

/// A script value's type.
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Void,
    /// `number`: an f32.
    Number,
    /// `int`: an i32 (to TypeScript's own checker, another name for `number`).
    Int,
    Bool,
    /// `string`: a constant text (a name or a key).
    Str,
    /// The type of `null` itself.
    Null,
    /// `any`: only what a rest parameter (`console.log`'s) takes.
    Any,
    Class(ClassId),
    /// `T | null`
    Nullable(Box<Type>),
    Promise(Box<Type>),
    /// What a `for...of` walks.
    Iterable(Box<Type>),
}

impl Type {
    pub fn nullable(self) -> Type {
        match self {
            Type::Nullable(_) | Type::Null | Type::Void | Type::Any => self,
            other => Type::Nullable(Box::new(other)),
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Type::Number | Type::Int)
    }
}

/// How a class's values are held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassKind {
    /// A reference to one of the world's things ([`Value::Handle`]).
    Handle,
    /// A value copied about, like `Vec3`.
    Value,
    /// One object, reached by its global (`level`, `lights`).
    Singleton,
}

#[derive(Clone, Debug)]
pub struct Class {
    pub name: String,
    pub doc: String,
    pub kind: ClassKind,
    /// TypeScript's own library declares it (`Math`): it is left out of `moose.d.ts`.
    pub ambient: bool,
    pub props: Vec<Prop>,
    pub methods: Vec<Method>,
}

#[derive(Clone, Debug)]
pub struct Prop {
    pub name: String,
    pub ty: Type,
    /// The host functions that read and (unless it is readonly) write it.
    pub get: u32,
    pub set: Option<u32>,
    pub doc: String,
}

/// A method, or a free function.
#[derive(Clone, Debug)]
pub struct Method {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub ret: Type,
    /// It takes any number of further values of any type.
    pub rest: bool,
    pub func: u32,
    pub doc: String,
}

/// A singleton's global name.
#[derive(Clone, Debug)]
pub struct Global {
    pub name: String,
    pub class: ClassId,
}

/// Something the engine tells scripts about: a script hears it by exporting a function of
/// its name.
#[derive(Clone, Debug)]
pub struct Event {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub doc: String,
}

/// The table's names and types (what the compiler and `moose.d.ts` need of it).
#[derive(Clone, Debug, Default)]
pub struct ApiDesc {
    pub classes: Vec<Class>,
    pub globals: Vec<Global>,
    pub funcs: Vec<Method>,
    pub events: Vec<Event>,
    /// What `self` is in a script attached to one of the world's things.
    pub self_class: Option<ClassId>,
}

impl ApiDesc {
    pub fn class_id(&self, name: &str) -> Option<ClassId> {
        self.classes.iter().position(|c| c.name == name).map(|i| i as ClassId)
    }

    pub fn event_id(&self, name: &str) -> Option<u16> {
        self.events.iter().position(|e| e.name == name).map(|i| i as u16)
    }

    pub(crate) fn resolve(&self, ty: &TypeExpr) -> Result<Type, String> {
        Ok(match ty {
            TypeExpr::Name(name) => match name.as_str() {
                "number" => Type::Number,
                "int" => Type::Int,
                "boolean" => Type::Bool,
                "string" => Type::Str,
                "void" => Type::Void,
                "any" => Type::Any,
                "null" => Type::Null,
                "undefined" => return Err("'undefined' isn't supported: use null".to_string()),
                _ => match self.class_id(name) {
                    Some(id) if self.classes[id as usize].kind != ClassKind::Singleton => Type::Class(id),
                    _ => return Err(format!("'{name}' is not a type")),
                },
            },
            TypeExpr::Nullable(inner) => self.resolve(inner)?.nullable(),
            TypeExpr::Generic(name, inner) => match name.as_str() {
                "Promise" => Type::Promise(Box::new(self.resolve(inner)?)),
                "Iterable" => Type::Iterable(Box::new(self.resolve(inner)?)),
                _ => return Err(format!("'{name}<...>' is not a type")),
            },
        })
    }

    /// A type as TypeScript writes it.
    pub fn type_name(&self, ty: &Type) -> String {
        match ty {
            Type::Void => "void".to_string(),
            Type::Number => "number".to_string(),
            Type::Int => "int".to_string(),
            Type::Bool => "boolean".to_string(),
            Type::Str => "string".to_string(),
            Type::Null => "null".to_string(),
            Type::Any => "any".to_string(),
            Type::Class(id) => self.classes[*id as usize].name.clone(),
            Type::Nullable(inner) => format!("{} | null", self.type_name(inner)),
            Type::Promise(inner) => format!("Promise<{}>", self.type_name(inner)),
            Type::Iterable(inner) => format!("Iterable<{}>", self.type_name(inner)),
        }
    }
}

/// The kind of thing an API's functions work on. It is borrowed for each call (a struct of
/// `&mut` to the world, say), which is why it is named through a trait: `Borrowed<'a>` is
/// that struct with the borrow's lifetime.
pub trait Host {
    type Borrowed<'a>;
}

/// A function of the table: `args[0]` is `this` for a method or a property (`null` for a
/// singleton's), then come its arguments (a setter's is the new value).
pub type HostFn<K> = for<'a, 'b, 'c, 'd, 'e> fn(&'b mut <K as Host>::Borrowed<'a>, &'c mut Cx<'d>, &'e [Value]) -> Value;

/// The table. [`Api::new`] starts it with what every script has (`Math`, `Vec3`, `rgb`,
/// `sleep`, `console`, ...); the engine adds its own. A mistake in it (a type nobody
/// declared) panics, where the engine starts.
pub struct Api<K: Host> {
    pub desc: ApiDesc,
    pub(crate) fns: Vec<HostFn<K>>,
}

impl<K: Host> Default for Api<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Host> Api<K> {
    pub fn new() -> Api<K> {
        let mut api = Api { desc: ApiDesc::default(), fns: Vec::new() };
        crate::prelude::install(&mut api);
        api
    }

    fn ty(&self, text: &str) -> Type {
        parse_type(text).and_then(|t| self.desc.resolve(&t)).unwrap_or_else(|e| panic!("script API: {e}"))
    }

    fn add(&mut self, f: HostFn<K>) -> u32 {
        self.fns.push(f);
        self.fns.len() as u32 - 1
    }

    fn signature(&mut self, sig: &str, doc: &str, f: HostFn<K>) -> Method {
        let (name, params, ret, rest) = parse_signature(sig).unwrap_or_else(|e| panic!("script API: {e}"));
        let resolve = |t: &TypeExpr| self.desc.resolve(t).unwrap_or_else(|e| panic!("script API: '{sig}': {e}"));
        let params = params.iter().map(|(n, t)| (n.clone(), resolve(t))).collect();
        let ret = ret.as_ref().map_or(Type::Void, resolve);
        Method { name, params, ret, rest, func: self.add(f), doc: doc.to_string() }
    }

    /// Declares a class. Declare every class before the members that name it.
    pub fn class(&mut self, name: &str, kind: ClassKind, doc: &str) -> ClassId {
        assert!(self.desc.class_id(name).is_none(), "script API: class '{name}' is declared twice");
        self.desc.classes.push(Class {
            name: name.to_string(),
            doc: doc.to_string(),
            kind,
            ambient: false,
            props: Vec::new(),
            methods: Vec::new(),
        });
        self.desc.classes.len() as ClassId - 1
    }

    /// A singleton class and the global it is reached by.
    pub fn singleton(&mut self, class: &str, global: &str, doc: &str) -> ClassId {
        let id = self.class(class, ClassKind::Singleton, doc);
        self.desc.globals.push(Global { name: global.to_string(), class: id });
        id
    }

    /// Marks `class` as one TypeScript's own library declares.
    pub fn ambient(&mut self, class: ClassId) {
        self.desc.classes[class as usize].ambient = true;
    }

    /// A property of `class`, of type `ty` (as TypeScript writes it); readonly with no `set`.
    pub fn prop(&mut self, class: ClassId, name: &str, ty: &str, doc: &str, get: HostFn<K>, set: Option<HostFn<K>>) {
        let ty = self.ty(ty);
        let (get, set) = (self.add(get), set.map(|f| self.add(f)));
        self.desc.classes[class as usize].props.push(Prop { name: name.to_string(), ty, get, set, doc: doc.to_string() });
    }

    /// A method of `class`, by its signature as TypeScript writes it:
    /// `moveTo(position: Vec3, seconds: number): Promise<void>`.
    pub fn method(&mut self, class: ClassId, sig: &str, doc: &str, f: HostFn<K>) {
        let method = self.signature(sig, doc, f);
        self.desc.classes[class as usize].methods.push(method);
    }

    /// A free function, by its signature.
    pub fn func(&mut self, sig: &str, doc: &str, f: HostFn<K>) {
        let func = self.signature(sig, doc, f);
        self.desc.funcs.push(func);
    }

    /// An event, by the signature its handlers have: `onActivated(target: Entity, by: Player)`.
    pub fn event(&mut self, sig: &str, doc: &str) {
        let (name, params, _, _) = parse_signature(sig).unwrap_or_else(|e| panic!("script API: {e}"));
        let params = params
            .iter()
            .map(|(n, t)| (n.clone(), self.desc.resolve(t).unwrap_or_else(|e| panic!("script API: '{sig}': {e}"))))
            .collect();
        self.desc.events.push(Event { name, params, doc: doc.to_string() });
    }

    /// What `self` is, in a script attached to one of the world's things.
    pub fn self_class(&mut self, class: ClassId) {
        self.desc.self_class = Some(class);
    }
}
