//! Scripts for the Moose engine (see the Scripting Spec).
//!
//! A script is a text asset written in a strict subset of TypeScript: an editor checks and
//! completes it as TypeScript (against `moose.d.ts`, see [`declarations`]), and the engine
//! [compiles](compile) it to bytecode as it loads, with its own check. A script is placed
//! on a level or on an entity; each placement is an instance of it, with the inputs it
//! declares (`const door = input<Entity>()`) bound there, its own state (its top-level
//! `let`s), and the events it handles (its exported functions named like one).
//!
//! The [`Runtime`] runs them with no garbage collector: values are numbers, vectors and
//! handles to the world's things; an `await` stops a handler until the engine settles the
//! promise (a move finishing, a timer), and other handlers run meanwhile.
//!
//! What scripts can reach is one table, an [`Api`]: each class, property, method, global
//! and event written once, with the Rust that does it. The compiler, the VM and
//! `moose.d.ts` all read that table.

mod api;
mod ast;
mod compile;
mod dts;
mod lex;
mod parse;
mod prelude;
mod vm;

pub use api::{Api, ApiDesc, Class, ClassId, ClassKind, Event, Global, Host, HostFn, Method, Prop, Type};
pub use compile::{GlobalDef, GlobalKind, Program, ScriptError, compile};
pub use dts::declarations;
pub use vm::{Cx, InstanceId, Line, Runtime, Strings, Value};

#[cfg(test)]
mod tests;
