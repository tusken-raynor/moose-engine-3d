//! Reads a script's tokens into its [`Module`]: the subset of TypeScript that Moose scripts
//! are written in. What TypeScript has and the subset doesn't is refused by name.

use crate::ast::*;
use crate::lex::{Tok, Token, lex};

/// A problem on a line of a script.
pub(crate) type Problem = (usize, String);

/// Words of JavaScript and TypeScript the subset leaves out.
const UNSUPPORTED: [&str; 22] = [
    "class", "var", "switch", "try", "catch", "throw", "new", "typeof", "undefined", "this", "interface", "type",
    "enum", "do", "delete", "instanceof", "in", "yield", "void", "namespace", "declare", "function",
];

pub(crate) struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

pub(crate) fn parse(src: &str) -> Result<Module, Problem> {
    let mut p = Parser { tokens: lex(src)?, pos: 0 };
    let mut module = Module { imports: Vec::new(), items: Vec::new() };
    while p.peek() != &Tok::Eof {
        let line = p.line();
        if p.eat_word("import") {
            p.expect("{")?;
            let mut names = Vec::new();
            while !p.eat("}") {
                names.push(p.ident("an imported name")?);
                if !p.eat(",") {
                    p.expect("}")?;
                    break;
                }
            }
            if !p.eat_word("from") {
                return Err((line, "an import reads: import { name } from \"./file\";".to_string()));
            }
            let Tok::Str(from) = p.next() else {
                return Err((line, "an import names its file in quotes".to_string()));
            };
            p.eat(";");
            module.imports.push(Import { names, from, line });
            continue;
        }
        let exported = p.eat_word("export");
        if exported && p.is_word("default") {
            return Err((line, "'export default' isn't supported: export each handler by name".to_string()));
        }
        if p.is_word("const") || p.is_word("let") {
            module.items.push(Item::Var(p.var_decl(exported)?));
        } else if p.is_word("function") || p.is_word("async") {
            module.items.push(Item::Func(p.func_decl(exported)?));
        } else {
            return Err((
                line,
                format!("{} can't be at a script's top level (imports, const, let and functions can)", p.describe()),
            ));
        }
    }
    Ok(module)
}

/// A signature as an API table writes it: `name(a: T, b?: U): R`, or with `...rest: any[]`
/// last. Returns the name, parameters, return type, whether it takes a rest, and how many
/// of its last parameters can be left out (`b?`).
#[allow(clippy::type_complexity)]
pub(crate) fn parse_signature(text: &str) -> Result<(String, Vec<(String, TypeExpr)>, Option<TypeExpr>, bool, usize), String> {
    let fail = |(_, m): Problem| format!("'{text}': {m}");
    let mut p = Parser { tokens: lex(text).map_err(fail)?, pos: 0 };
    let name = p.ident("a name").map_err(fail)?;
    p.expect("(").map_err(fail)?;
    let (mut params, mut rest, mut optional) = (Vec::new(), false, 0);
    while !p.eat(")") {
        if p.eat("...") {
            rest = true;
            p.ident("a name").map_err(fail)?;
            for word in [":", "any", "[", "]", ")"] {
                if !(p.eat(word) || p.eat_word(word)) {
                    return Err(format!("'{text}': a rest is written '...name: any[]', last"));
                }
            }
            break;
        }
        let param = p.ident("a parameter").map_err(fail)?;
        if p.eat("?") {
            optional += 1;
        } else if optional > 0 {
            return Err(format!("'{text}': after one that can be left out, every parameter can be ({param}?)"));
        }
        p.expect(":").map_err(fail)?;
        params.push((param, p.type_expr().map_err(fail)?));
        if !p.eat(",") {
            p.expect(")").map_err(fail)?;
            break;
        }
    }
    let ret = if p.eat(":") { Some(p.type_expr().map_err(fail)?) } else { None };
    if p.peek() != &Tok::Eof {
        return Err(format!("'{text}': {} after the signature", p.describe()));
    }
    Ok((name, params, ret, rest, optional))
}

/// A type as an API table writes it.
pub(crate) fn parse_type(text: &str) -> Result<TypeExpr, String> {
    let fail = |(_, m): Problem| format!("'{text}': {m}");
    let mut p = Parser { tokens: lex(text).map_err(fail)?, pos: 0 };
    let ty = p.type_expr().map_err(fail)?;
    if p.peek() != &Tok::Eof {
        return Err(format!("'{text}': {} after the type", p.describe()));
    }
    Ok(ty)
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn peek_at(&self, ahead: usize) -> &Tok {
        &self.tokens[(self.pos + ahead).min(self.tokens.len() - 1)].tok
    }

    fn line(&self) -> usize {
        self.tokens[self.pos].line
    }

    fn next(&mut self) -> Tok {
        let tok = self.tokens[self.pos].tok.clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn is(&self, punct: &str) -> bool {
        matches!(self.peek(), Tok::Punct(p) if *p == punct)
    }

    fn is_word(&self, word: &str) -> bool {
        matches!(self.peek(), Tok::Ident(w) if w == word)
    }

    fn eat(&mut self, punct: &str) -> bool {
        let found = self.is(punct);
        if found {
            self.pos += 1;
        }
        found
    }

    fn eat_word(&mut self, word: &str) -> bool {
        let found = self.is_word(word);
        if found {
            self.pos += 1;
        }
        found
    }

    /// The next token, for an error message.
    fn describe(&self) -> String {
        match self.peek() {
            Tok::Ident(w) => format!("'{w}'"),
            Tok::Num(n, _) => format!("'{n}'"),
            Tok::Str(_) => "a string".to_string(),
            Tok::Punct(p) => format!("'{p}'"),
            Tok::Eof => "the end of the file".to_string(),
        }
    }

    fn expect(&mut self, punct: &str) -> Result<(), Problem> {
        if self.eat(punct) { Ok(()) } else { Err((self.line(), format!("expected '{punct}', found {}", self.describe()))) }
    }

    fn ident(&mut self, what: &str) -> Result<String, Problem> {
        match self.peek() {
            Tok::Ident(w) if !UNSUPPORTED.contains(&w.as_str()) => {
                let w = w.clone();
                self.pos += 1;
                Ok(w)
            }
            _ => Err((self.line(), format!("expected {what}, found {}", self.describe()))),
        }
    }

    fn type_expr(&mut self) -> Result<TypeExpr, Problem> {
        let line = self.line();
        let name = match self.next() {
            Tok::Ident(name) => name,
            _ => return Err((line, "expected a type".to_string())),
        };
        let mut ty = if self.eat("<") {
            let inner = self.type_expr()?;
            self.expect(">")?;
            TypeExpr::Generic(name, Box::new(inner))
        } else {
            TypeExpr::Name(name)
        };
        if self.is("[") {
            return Err((line, "arrays aren't supported in Moose scripts yet".to_string()));
        }
        while self.eat("|") {
            if !self.eat_word("null") {
                return Err((self.line(), "the only union is 'T | null'".to_string()));
            }
            if !matches!(ty, TypeExpr::Nullable(_)) {
                ty = TypeExpr::Nullable(Box::new(ty));
            }
        }
        Ok(ty)
    }

    fn var_decl(&mut self, exported: bool) -> Result<VarDecl, Problem> {
        let line = self.line();
        let is_const = self.eat_word("const");
        if !is_const {
            self.pos += 1;
        }
        if self.is("[") || self.is("{") {
            return Err((line, "destructuring isn't supported in Moose scripts yet".to_string()));
        }
        let name = self.ident("a name")?;
        let ty = if self.eat(":") { Some(self.type_expr()?) } else { None };
        if !self.eat("=") {
            return Err((line, format!("'{name}' needs a starting value (= ...)")));
        }
        let init = self.expr()?;
        self.eat(";");
        Ok(VarDecl { name, ty, init, is_const, exported, line })
    }

    fn func_decl(&mut self, exported: bool) -> Result<FuncDecl, Problem> {
        let line = self.line();
        let is_async = self.eat_word("async");
        if !self.eat_word("function") {
            return Err((line, format!("expected 'function' after 'async', found {}", self.describe())));
        }
        let name = self.ident("the function's name")?;
        self.expect("(")?;
        let mut params = Vec::new();
        while !self.eat(")") {
            let param = self.ident("a parameter")?;
            if !self.eat(":") {
                return Err((self.line(), format!("parameter '{param}' needs a type ({param}: Type)")));
            }
            params.push((param, self.type_expr()?));
            if self.is("=") || self.is("?") {
                return Err((self.line(), "optional parameters and defaults aren't supported".to_string()));
            }
            if !self.eat(",") {
                self.expect(")")?;
                break;
            }
        }
        let ret = if self.eat(":") { Some(self.type_expr()?) } else { None };
        let body = self.block()?;
        Ok(FuncDecl { name, params, ret, body, is_async, exported, line })
    }

    fn block(&mut self) -> Result<Vec<Stmt>, Problem> {
        self.expect("{")?;
        let mut stmts = Vec::new();
        while !self.eat("}") {
            if self.peek() == &Tok::Eof {
                return Err((self.line(), "a '{' is never closed".to_string()));
            }
            stmts.push(self.stmt()?);
        }
        Ok(stmts)
    }

    fn stmt(&mut self) -> Result<Stmt, Problem> {
        let line = self.line();
        let kind = if self.is("{") {
            StmtKind::Block(self.block()?)
        } else if self.is_word("const") || self.is_word("let") {
            StmtKind::Var(self.var_decl(false)?)
        } else if self.eat_word("if") {
            self.expect("(")?;
            let cond = self.expr()?;
            self.expect(")")?;
            let then = Box::new(self.stmt()?);
            let otherwise = if self.eat_word("else") { Some(Box::new(self.stmt()?)) } else { None };
            StmtKind::If(cond, then, otherwise)
        } else if self.eat_word("while") {
            self.expect("(")?;
            let cond = self.expr()?;
            self.expect(")")?;
            StmtKind::While(cond, Box::new(self.stmt()?))
        } else if self.eat_word("for") {
            self.expect("(")?;
            // `for (const x of things)`
            if (self.is_word("const") || self.is_word("let"))
                && matches!(self.peek_at(2), Tok::Ident(w) if w == "of")
            {
                self.pos += 1;
                let name = self.ident("a name")?;
                self.pos += 1;
                let iter = self.expr()?;
                self.expect(")")?;
                StmtKind::ForOf { name, iter, body: Box::new(self.stmt()?) }
            } else {
                let init = if self.eat(";") {
                    None
                } else if self.is_word("const") || self.is_word("let") {
                    // (It takes its own `;`.)
                    let at = self.line();
                    Some(Box::new(Stmt { kind: StmtKind::Var(self.var_decl(false)?), line: at }))
                } else {
                    let at = self.line();
                    let e = self.expr()?;
                    self.expect(";")?;
                    Some(Box::new(Stmt { kind: StmtKind::Expr(e), line: at }))
                };
                let cond = if self.is(";") { None } else { Some(self.expr()?) };
                self.expect(";")?;
                let step = if self.is(")") { None } else { Some(self.expr()?) };
                self.expect(")")?;
                StmtKind::For { init, cond, step, body: Box::new(self.stmt()?) }
            }
        } else if self.eat_word("return") {
            // A value on the same line, as JavaScript reads it.
            let value =
                if self.is(";") || self.is("}") || self.line() != line { None } else { Some(self.expr()?) };
            self.eat(";");
            StmtKind::Return(value)
        } else if self.eat_word("break") {
            self.eat(";");
            StmtKind::Break
        } else if self.eat_word("continue") {
            self.eat(";");
            StmtKind::Continue
        } else if self.is_word("function") || self.is_word("async") && matches!(self.peek_at(1), Tok::Ident(w) if w == "function") {
            return Err((line, "a function inside a function isn't supported: put it at the top level".to_string()));
        } else {
            let e = self.expr()?;
            self.eat(";");
            StmtKind::Expr(e)
        };
        Ok(Stmt { kind, line })
    }

    pub(crate) fn expr(&mut self) -> Result<Expr, Problem> {
        let line = self.line();
        let target = self.conditional()?;
        let op = match self.peek() {
            Tok::Punct("=") => None,
            Tok::Punct("+=") => Some("+"),
            Tok::Punct("-=") => Some("-"),
            Tok::Punct("*=") => Some("*"),
            Tok::Punct("/=") => Some("/"),
            Tok::Punct("%=") => Some("%"),
            Tok::Punct("=>") => return Err((line, "arrow functions aren't supported in Moose scripts".to_string())),
            _ => return Ok(target),
        };
        self.pos += 1;
        let value = self.expr()?;
        Ok(Expr { kind: ExprKind::Assign(op, Box::new(target), Box::new(value)), line })
    }

    fn conditional(&mut self) -> Result<Expr, Problem> {
        let line = self.line();
        let cond = self.binary(0)?;
        if !self.eat("?") {
            return Ok(cond);
        }
        let then = self.expr()?;
        self.expect(":")?;
        let otherwise = self.expr()?;
        Ok(Expr { kind: ExprKind::Cond(Box::new(cond), Box::new(then), Box::new(otherwise)), line })
    }

    /// Binary operators at `level` and tighter: `?? ||`, `&&`, equality, comparison, `+ -`,
    /// `* / %`.
    fn binary(&mut self, level: usize) -> Result<Expr, Problem> {
        const LEVELS: [&[&str]; 6] =
            [&["??", "||"], &["&&"], &["===", "!==", "==", "!="], &["<=", ">=", "<", ">"], &["+", "-"], &["*", "/", "%"]];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        loop {
            let line = self.line();
            let Some(op) = LEVELS[level].iter().copied().find(|op| self.is(op)) else {
                return Ok(left);
            };
            self.pos += 1;
            let right = self.binary(level + 1)?;
            left = Expr { kind: ExprKind::Binary(op, Box::new(left), Box::new(right)), line };
        }
    }

    fn unary(&mut self) -> Result<Expr, Problem> {
        let line = self.line();
        for op in ["-", "!", "+"] {
            if self.eat(op) {
                return Ok(Expr { kind: ExprKind::Unary(op, Box::new(self.unary()?)), line });
            }
        }
        for (op, by) in [("++", 1), ("--", -1)] {
            if self.eat(op) {
                return Ok(Expr { kind: ExprKind::Step(Box::new(self.unary()?), by), line });
            }
        }
        if self.eat_word("await") {
            return Ok(Expr { kind: ExprKind::Await(Box::new(self.unary()?)), line });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, Problem> {
        let mut e = self.primary()?;
        loop {
            let line = self.line();
            if self.eat(".") || self.is("?.") {
                let optional = self.eat("?.");
                let name = match self.next() {
                    Tok::Ident(name) => name,
                    _ => return Err((line, "expected a name after '.'".to_string())),
                };
                e = Expr { kind: ExprKind::Member { object: Box::new(e), name, optional }, line };
            } else if self.is("(")
                || (self.is("<") && matches!(&e.kind, ExprKind::Ident(name) if name == "input"))
            {
                // Only `input<T>()` takes a type.
                let type_arg = if self.eat("<") {
                    let ty = self.type_expr()?;
                    self.expect(">")?;
                    Some(ty)
                } else {
                    None
                };
                self.expect("(")?;
                let mut args = Vec::new();
                while !self.eat(")") {
                    args.push(self.expr()?);
                    if !self.eat(",") {
                        self.expect(")")?;
                        break;
                    }
                }
                e = Expr { kind: ExprKind::Call { callee: Box::new(e), type_arg, args }, line };
            } else if self.is("[") {
                return Err((line, "indexing ([ ]) isn't supported in Moose scripts yet".to_string()));
            } else if self.eat("!") {
                e = Expr { kind: ExprKind::NonNull(Box::new(e)), line };
            } else if self.is("++") || self.is("--") {
                let by = if self.eat("++") { 1 } else { -1 };
                if by == -1 {
                    self.pos += 1;
                }
                e = Expr { kind: ExprKind::Step(Box::new(e), by), line };
            } else if self.is_word("as") {
                return Err((line, "'as' casts aren't supported: use Math.floor or Math.round for an int".to_string()));
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, Problem> {
        let line = self.line();
        let kind = match self.next() {
            Tok::Num(n, whole) => ExprKind::Num(n, whole),
            Tok::Str(s) => ExprKind::Str(s),
            Tok::Punct("(") => {
                let e = self.expr()?;
                if self.is("=>") || self.is(",") && false {
                    return Err((line, "arrow functions aren't supported in Moose scripts".to_string()));
                }
                self.expect(")")?;
                if self.is("=>") {
                    return Err((line, "arrow functions aren't supported in Moose scripts".to_string()));
                }
                return Ok(e);
            }
            Tok::Punct("[") => {
                let mut items = Vec::new();
                while !self.eat("]") {
                    items.push(self.expr()?);
                    if !self.eat(",") {
                        self.expect("]")?;
                        break;
                    }
                }
                ExprKind::Array(items)
            }
            Tok::Punct("{") => {
                return Err((line, "object literals ({ }) aren't supported in Moose scripts".to_string()));
            }
            Tok::Ident(word) => match word.as_str() {
                "true" => ExprKind::Bool(true),
                "false" => ExprKind::Bool(false),
                "null" => ExprKind::Null,
                w if UNSUPPORTED.contains(&w) => {
                    return Err((line, format!("'{w}' isn't supported in Moose scripts")));
                }
                _ => ExprKind::Ident(word),
            },
            Tok::Punct(p) => return Err((line, format!("expected a value, found '{p}'"))),
            Tok::Eof => return Err((line, "expected a value, found the end of the file".to_string())),
        };
        Ok(Expr { kind, line })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_parses_into_imports_variables_and_functions() {
        let module = parse(
            "import { blink } from \"./util/lights\";\n\
             const door = input<Entity>();\n\
             const speed = input(2.0);\n\
             export let open = false;\n\
             export async function onActivated(target: Entity, by: Player | null): Promise<void> {\n\
               open = !open;\n\
               await Promise.all([door.moveTo(open ? 1 : 0, speed), blink(lamp, 3)]);\n\
               for (const l of lights.inSector(door.sector)) { l.color = rgb(40, 255, 40); }\n\
               for (let i = 0; i < 3; i++) { if (i == 1) continue; else break; }\n\
               return\n\
             }\n",
        )
        .unwrap();
        assert_eq!(module.imports[0].names, ["blink"]);
        assert_eq!(module.imports[0].from, "./util/lights");
        assert_eq!(module.items.len(), 4);
        let Item::Var(door) = &module.items[0] else { panic!() };
        assert!(matches!(&door.init.kind, ExprKind::Call { type_arg: Some(TypeExpr::Name(t)), .. } if t == "Entity"));
        let Item::Func(f) = &module.items[3] else { panic!() };
        assert!(f.is_async && f.exported);
        assert_eq!(f.params[1].1, TypeExpr::Nullable(Box::new(TypeExpr::Name("Player".into()))));
        assert_eq!(f.body.len(), 5);
        assert!(matches!(f.body[4].kind, StmtKind::Return(None)));
    }

    #[test]
    fn operators_bind_as_in_javascript() {
        let module = parse("const x = 1 + 2 * 3 < 8 && !a ?? b;").unwrap();
        let Item::Var(x) = &module.items[0] else { panic!() };
        let ExprKind::Binary("??", left, _) = &x.init.kind else { panic!("{:?}", x.init.kind) };
        let ExprKind::Binary("&&", less, _) = &left.kind else { panic!() };
        let ExprKind::Binary("<", sum, _) = &less.kind else { panic!() };
        assert!(matches!(&sum.kind, ExprKind::Binary("+", _, product) if matches!(product.kind, ExprKind::Binary("*", ..))));
    }

    #[test]
    fn what_the_subset_leaves_out_is_refused_by_name() {
        for (src, word) in [
            ("class A {}", "class"),
            ("const f = (a) => a;", "arrow"),
            ("const o = { a: 1 };", "object literals"),
            ("export default { };", "export default"),
            ("function f() { const [a, b] = c; }", "destructuring"),
            ("function f() { return this.x; }", "'this'"),
            ("function f(a: number[]) {}", "arrays"),
            ("function f() { function g() {} }", "inside a function"),
            ("function f(a) {}", "needs a type"),
            ("let x: number;", "starting value"),
            ("function f() { return a[0]; }", "indexing"),
        ] {
            let (_, message) = parse(src).unwrap_err();
            assert!(message.contains(word), "{src}: {message}");
        }
        assert_eq!(parse("let a = 1;\nlet b = ;").unwrap_err().0, 2);
    }

    #[test]
    fn api_signatures_read_like_typescript() {
        let (name, params, ret, rest, _) = parse_signature("moveTo(position: Vec3, seconds: number): Promise<void>").unwrap();
        assert_eq!((name.as_str(), params.len(), rest), ("moveTo", 2, false));
        assert_eq!(ret, Some(TypeExpr::Generic("Promise".into(), Box::new(TypeExpr::Name("void".into())))));
        let (_, params, ret, rest, _) = parse_signature("log(...values: any[])").unwrap();
        assert!(params.is_empty() && ret.is_none() && rest);
        assert!(parse_signature("f(a)").is_err());
        let (_, params, _, _, optional) = parse_signature("play(at: Vec3, volume?: number, pitch?: number): Playing").unwrap();
        assert_eq!((params.len(), optional), (3, 2));
        assert!(parse_signature("play(volume?: number, at: Vec3)").is_err());
        assert_eq!(parse_type("Light | null").unwrap(), TypeExpr::Nullable(Box::new(TypeExpr::Name("Light".into()))));
    }
}
