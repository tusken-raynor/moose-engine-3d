//! A script as the parser reads it, before its names and types are known.

/// One file: what it imports, then its top-level variables and functions in order.
#[derive(Debug)]
pub(crate) struct Module {
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
}

/// `import { a, b } from "./file";`
#[derive(Debug)]
pub(crate) struct Import {
    pub names: Vec<String>,
    pub from: String,
    pub line: usize,
}

#[derive(Debug)]
pub(crate) enum Item {
    Var(VarDecl),
    Func(FuncDecl),
}

/// `const`/`let NAME[: TYPE] = INIT;`, at the top level or in a function.
#[derive(Debug)]
pub(crate) struct VarDecl {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub init: Expr,
    pub is_const: bool,
    pub exported: bool,
    pub line: usize,
}

#[derive(Debug)]
pub(crate) struct FuncDecl {
    pub name: String,
    pub params: Vec<(String, TypeExpr)>,
    /// As written: an async function's is its `Promise<T>`.
    pub ret: Option<TypeExpr>,
    pub body: Vec<Stmt>,
    pub is_async: bool,
    pub exported: bool,
    pub line: usize,
}

/// A type as written.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TypeExpr {
    Name(String),
    /// `T | null`
    Nullable(Box<TypeExpr>),
    /// `Promise<T>`, `Iterable<T>`
    Generic(String, Box<TypeExpr>),
}

#[derive(Debug)]
pub(crate) struct Stmt {
    pub kind: StmtKind,
    pub line: usize,
}

#[derive(Debug)]
pub(crate) enum StmtKind {
    Var(VarDecl),
    Expr(Expr),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    For { init: Option<Box<Stmt>>, cond: Option<Expr>, step: Option<Expr>, body: Box<Stmt> },
    /// `for (const NAME of ITER) BODY`
    ForOf { name: String, iter: Expr, body: Box<Stmt> },
    Return(Option<Expr>),
    Break,
    Continue,
    Block(Vec<Stmt>),
}

#[derive(Debug)]
pub(crate) struct Expr {
    pub kind: ExprKind,
    pub line: usize,
}

#[derive(Debug)]
pub(crate) enum ExprKind {
    /// A number, and whether it was written whole.
    Num(f64, bool),
    Str(String),
    Bool(bool),
    Null,
    Ident(String),
    /// `-x`, `!x`, `+x`
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    /// `target = value`, or with an operator (`+=` has `Some("+")`).
    Assign(Option<&'static str>, Box<Expr>, Box<Expr>),
    /// `target++` / `target--` (before or after: its value is never used).
    Step(Box<Expr>, i32),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// A call; `type_arg` is `input<T>()`'s.
    Call { callee: Box<Expr>, type_arg: Option<TypeExpr>, args: Vec<Expr> },
    /// `object.name`, or `object?.name`.
    Member { object: Box<Expr>, name: String, optional: bool },
    Await(Box<Expr>),
    /// `value!`
    NonNull(Box<Expr>),
    /// `[a, b]`: only as what `Promise.all` and `Promise.race` take.
    Array(Vec<Expr>),
}
