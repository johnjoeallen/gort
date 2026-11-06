use crate::ast::*;
use crate::diag::{Diag, R};
use crate::lexer::{lex, Tok, Token};

pub fn parse_file(name: &str, src: &str) -> R<File> {
    let toks = lex(name, src)?;
    let mut p = Parser { file: name.to_string(), toks, pos: 0, in_type_args: 0 };
    p.file()
}

struct Parser {
    file: String,
    toks: Vec<Token>,
    pos: usize,
    in_type_args: u32,
}

impl Parser {
    fn err<T>(&self, t: &Token, msg: impl Into<String>) -> R<T> {
        Err(Diag { file: self.file.clone(), line: t.line, col: t.col, msg: msg.into() })
    }
    fn cur(&self) -> &Token {
        &self.toks[self.pos]
    }
    fn peek_at(&self, n: usize) -> &Token {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)]
    }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn is_sym(&self, s: &str) -> bool {
        matches!(&self.cur().tok, Tok::Sym(x) if *x == s)
    }
    fn is_kw(&self, s: &str) -> bool {
        matches!(&self.cur().tok, Tok::Ident(x) if x == s)
    }
    fn eat_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn eat_kw(&mut self, s: &str) -> bool {
        if self.is_kw(s) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn describe(t: &Token) -> String {
        match &t.tok {
            Tok::Ident(s) => format!("'{}'", s),
            Tok::Sym(s) => format!("'{}'", s),
            Tok::Int(_) | Tok::Float(_) => "a number".into(),
            Tok::Str(_) => "a string".into(),
            Tok::Char(_) => "a character".into(),
            Tok::Eof => "end of file".into(),
        }
    }
    fn expect_sym(&mut self, s: &str) -> R<Token> {
        if self.is_sym(s) {
            Ok(self.bump())
        } else {
            let t = self.cur().clone();
            self.err(&t, format!("expected '{}', found {}", s, Self::describe(&t)))
        }
    }
    fn expect_kw(&mut self, s: &str) -> R<Token> {
        if self.is_kw(s) {
            Ok(self.bump())
        } else {
            let t = self.cur().clone();
            self.err(&t, format!("expected '{}', found {}", s, Self::describe(&t)))
        }
    }
    fn ident(&mut self) -> R<(String, Token)> {
        let t = self.cur().clone();
        match &t.tok {
            Tok::Ident(s) if !is_reserved(s) => {
                self.bump();
                Ok((s.clone(), t))
            }
            _ => self.err(&t, format!("expected a name, found {}", Self::describe(&t))),
        }
    }
    fn skip_semis(&mut self) {
        while self.eat_sym(";") {}
    }

    // ---------------- items ----------------

    fn file(&mut self) -> R<File> {
        let mut package = None;
        let mut items = Vec::new();
        self.skip_semis();
        while self.cur().tok != Tok::Eof {
            if self.eat_kw("package") {
                let (n, _) = self.ident()?;
                let mut name = n;
                // package[yield] name: the attribute is parsed and ignored in slice 0
                while self.eat_sym(".") {
                    let (n, _) = self.ident()?;
                    name = format!("{}.{}", name, n);
                }
                package = Some(name);
            } else if self.eat_kw("import") {
                let (n, _) = self.ident()?;
                let mut name = n;
                while self.eat_sym(".") {
                    let (n, _) = self.ident()?;
                    name = format!("{}.{}", name, n);
                }
                items.push(Item::Import(name));
            } else if self.is_kw("extern") {
                self.bump();
                if self.is_kw("let") {
                    let kw = self.bump();
                    let (name, _) = self.ident()?;
                    self.expect_sym(":")?;
                    let ty = self.type_expr()?;
                    let value = Expr { kind: ExprKind::Int(0), line: kw.line, col: kw.col };
                    items.push(Item::ExternLet(ConstDecl { name, ty, value, line: kw.line, col: kw.col }));
                } else if self.is_kw("impl") {
                    self.bump();
                    let ty = self.type_expr()?;
                    self.expect_sym("{")?;
                    let mut ms = Vec::new();
                    loop {
                        self.skip_semis();
                        if self.eat_sym("}") {
                            break;
                        }
                        ms.push(self.method(true)?);
                    }
                    items.push(Item::Impl(ty, ms));
                } else if self.is_kw("struct") {
                    let mut sd = self.struct_decl(true)?;
                    sd.is_extern = true;
                    items.push(Item::Struct(sd));
                } else {
                    let mut f = self.method(true)?;
                    f.is_extern = true;
                    items.push(Item::Fn(f));
                }
            } else if self.is_kw("let") {
                let kw = self.bump();
                let (name, _) = self.ident()?;
                let ty = if self.eat_sym(":") { Some(self.type_expr()?) } else { None };
                self.expect_sym("=")?;
                let init = self.expr()?;
                items.push(Item::Global { name, ty, init, line: kw.line, col: kw.col });
            } else if self.is_kw("const") {
                items.push(Item::Const(self.const_decl()?));
            } else if self.is_kw("struct") || self.is_kw("enum") || self.is_kw("exception") {
                items.push(Item::Struct(self.struct_decl(false)?));
            } else if self.is_kw("fn") {
                items.push(Item::Fn(self.fn_decl(false)?));
            } else {
                let t = self.cur().clone();
                return self.err(&t, format!("expected a declaration, found {}", Self::describe(&t)));
            }
            self.skip_semis();
        }
        Ok(File { name: self.file.clone(), package, items })
    }

    fn const_decl(&mut self) -> R<ConstDecl> {
        let kw = self.expect_kw("const")?;
        let (name, _) = self.ident()?;
        self.expect_sym(":")?;
        let ty = self.type_expr()?;
        self.expect_sym("=")?;
        let value = self.expr()?;
        Ok(ConstDecl { name, ty, value, line: kw.line, col: kw.col })
    }

    /// `[static] [parks] fn ...`, a method or an extern function
    fn method(&mut self, in_struct: bool) -> R<FnDecl> {
        let is_static = self.eat_kw("static");
        let parks = self.eat_kw("parks");
        let mut f = self.fn_decl(in_struct)?;
        f.is_static = is_static;
        f.parks = parks;
        Ok(f)
    }

    fn struct_decl(&mut self, is_extern: bool) -> R<StructDecl> {
        let is_enum = self.is_kw("enum");
        let is_exception = self.is_kw("exception");
        let kw = self.bump();
        let (name, _) = self.ident()?;
        if is_exception && !self.is_sym("{") {
            return Ok(StructDecl { name, fields: Vec::new(), methods: Vec::new(), variants: Vec::new(), is_enum, is_exception, is_extern, type_params: Vec::new(), line: kw.line, col: kw.col });
        }
        let mut type_params = Vec::new();
        if self.eat_sym("<") {
            loop {
                type_params.push(self.ident()?.0);
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym(">")?;
        }
        let t = self.cur().clone();
        if self.is_sym(":") || self.is_kw("with") {
            return self.err(&t, "slice 1: interfaces and embedding are not supported yet");
        }
        self.expect_sym("{")?;
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        let mut variants = Vec::new();
        loop {
            self.skip_semis();
            if self.eat_sym(",") {
                continue;
            }
            if self.eat_sym("}") {
                break;
            }
            if self.is_kw("fn") || self.is_kw("static") || self.is_kw("parks") {
                let mut f = self.method(true)?;
                f.is_extern = is_extern;
                methods.push(f);
            } else if is_enum {
                let (vname, vt) = self.ident()?;
                let mut vf = Vec::new();
                if self.eat_sym("(") {
                    while !self.is_sym(")") {
                        // `name: Type` or just `Type`
                        let named = matches!(self.peek_at(1).tok, Tok::Sym(":"));
                        let n = if named {
                            let n = self.ident()?.0;
                            self.expect_sym(":")?;
                            n
                        } else {
                            format!("{}", vf.len())
                        };
                        vf.push((n, self.type_expr()?));
                        if !self.eat_sym(",") {
                            break;
                        }
                    }
                    self.expect_sym(")")?;
                }
                variants.push(VariantDecl { name: vname, fields: vf, line: vt.line, col: vt.col });
            } else {
                let (fname, ft) = self.ident()?;
                self.expect_sym(":")?;
                let ty = self.type_expr()?;
                let default = if self.eat_sym("=") { Some(self.expr()?) } else { None };
                fields.push(FieldDecl { name: fname, ty, default, line: ft.line, col: ft.col });
            }
        }
        Ok(StructDecl { name, fields, methods, variants, is_enum, is_exception, is_extern, type_params, line: kw.line, col: kw.col })
    }

    fn fn_decl(&mut self, in_struct: bool) -> R<FnDecl> {
        let kw = self.expect_kw("fn")?;
        // `fn[yield]`, `fn[nopark]`: an effect attribute (`nopark` is checked against the inferred effect)
        let mut attr = None;
        if self.eat_sym("[") {
            attr = Some(self.ident()?.0);
            self.expect_sym("]")?;
        }
        let (mut name, _) = self.ident()?;
        let mut owner = None;
        if self.eat_sym("::") {
            owner = Some(name);
            name = self.ident()?.0;
        }
        self.expect_sym("(")?;
        let mut params = Vec::new();
        while !self.is_sym(")") {
            let (pn, _) = self.ident()?;
            self.expect_sym(":")?;
            let ty = self.type_expr()?;
            let variadic = self.eat_sym("...");
            params.push(Param { name: pn, ty, variadic });
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(")")?;
        let ret = if self.eat_sym(":") { Some(self.type_expr()?) } else { None };
        let mut throws = Vec::new();
        if self.eat_kw("throws") {
            loop {
                throws.push(self.type_expr()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        let body = if self.is_sym("{") {
            Some(self.block()?)
        } else if in_struct {
            None
        } else {
            let t = self.cur().clone();
            return self.err(&t, format!("expected '{{', found {}", Self::describe(&t)));
        };
        Ok(FnDecl { name, owner, is_static: false, params, ret, throws, body, is_extern: false, parks: false, attr, line: kw.line, col: kw.col })
    }

    // ---------------- types ----------------

    fn type_expr(&mut self) -> R<TypeExpr> {
        if self.is_kw("fn") {
            // `fn[nopark](A, B): R throws X`: name "fn" (or "fn!" when it throws); args are the parameters, then the result
            let t = self.bump();
            if self.eat_sym("[") {
                self.ident()?;
                self.expect_sym("]")?;
            }
            self.expect_sym("(")?;
            let mut args = Vec::new();
            self.in_type_args += 1;
            while !self.is_sym(")") {
                match self.type_expr() {
                    Ok(a) => args.push(a),
                    Err(e) => {
                        self.in_type_args -= 1;
                        return Err(e);
                    }
                }
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.in_type_args -= 1;
            self.expect_sym(")")?;
            let ret = if self.eat_sym(":") { self.type_expr()? } else { TypeExpr { name: "()".into(), args: vec![], optional: false, fixed: None, line: t.line, col: t.col } };
            args.push(ret);
            let mut throws = false;
            if self.is_kw("throws") {
                self.bump();
                throws = true;
                self.ident()?; // a function type names one exception (usually `Exception`): a comma would be the next parameter
            }
            return Ok(TypeExpr { name: if throws { "fn!".into() } else { "fn".into() }, args, optional: false, fixed: None, line: t.line, col: t.col });
        }
        let (name, t) = self.ident()?;
        let mut args = Vec::new();
        if self.eat_sym("<") {
            self.in_type_args += 1;
            loop {
                match self.type_expr() {
                    Ok(a) => args.push(a),
                    Err(e) => {
                        self.in_type_args -= 1;
                        return Err(e);
                    }
                }
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.in_type_args -= 1;
            self.expect_sym(">")?;
        }
        let mut fixed = None;
        if self.in_type_args > 0 && self.is_sym("[") && matches!(self.peek_at(1).tok, Tok::Int(_)) && matches!(self.peek_at(2).tok, Tok::Sym("]")) {
            self.bump();
            if let Tok::Int(n) = self.bump().tok {
                fixed = Some(n);
            }
            self.bump();
        } else if self.is_sym("[") && matches!(self.peek_at(1).tok, Tok::Ident(_)) && matches!(self.peek_at(2).tok, Tok::Sym("]")) && !matches!(self.peek_at(3).tok, Tok::Sym("(")) && self.in_type_args > 0 {
            // `u8[OUTSZ]` inside type arguments: a named constant size
            self.bump();
            self.bump();
            self.bump();
            fixed = Some(0);
        }
        let optional = self.eat_sym("?");
        // `T?&`: a borrowed view. Slice 1 treats it as the plain type (a counted copy)
        self.eat_sym("&");
        Ok(TypeExpr { name, args, optional, fixed, line: t.line, col: t.col })
    }

    // ---------------- statements ----------------

    fn block(&mut self) -> R<Block> {
        let open = self.expect_sym("{")?;
        let mut stmts = Vec::new();
        loop {
            self.skip_semis();
            if self.eat_sym("}") {
                break;
            }
            if self.cur().tok == Tok::Eof {
                let t = self.cur().clone();
                return self.err(&t, "unexpected end of file; missing '}'");
            }
            stmts.push(self.stmt()?);
        }
        Ok(Block { stmts, line: open.line })
    }

    /// Looks for `Type name =` (the C-style declaration of the design notes) without consuming.
    fn try_c_decl(&mut self) -> Option<(TypeExpr, String, Token)> {
        let save = self.pos;
        if let Tok::Ident(s) = &self.cur().tok {
            if is_reserved(s) {
                return None;
            }
        } else {
            return None;
        }
        if let Ok(ty) = self.type_expr() {
            if let Tok::Ident(n) = self.cur().tok.clone() {
                if !is_reserved(&n) && matches!(self.peek_at(1).tok, Tok::Sym("=")) {
                    let t = self.bump();
                    return Some((ty, n, t));
                }
            }
        }
        self.pos = save;
        None
    }

    fn stmt(&mut self) -> R<Stmt> {
        let t = self.cur().clone();
        let (line, col) = (t.line, t.col);
        let mk = |kind| Stmt { kind, line, col };
        if self.is_sym("{") {
            return Ok(mk(StmtKind::Block(self.block()?)));
        }
        if self.is_kw("let") || self.is_kw("var") {
            let mutable = self.is_kw("var");
            self.bump();
            let (name, _) = self.ident()?;
            let ty = if self.eat_sym(":") { Some(self.type_expr()?) } else { None };
            // `var x: T?` with no value starts as null
            if let Some(t) = &ty {
                if mutable && t.optional && !self.is_sym("=") {
                    let init = Expr { kind: ExprKind::Ident("null".into()), line: t.line, col: t.col };
                    self.end_stmt()?;
                    return Ok(mk(StmtKind::Let { name, mutable, ty, init }));
                }
            }
            self.expect_sym("=")?;
            let init = self.expr()?;
            self.end_stmt()?;
            return Ok(mk(StmtKind::Let { name, mutable, ty, init }));
        }
        if self.is_kw("if") {
            return self.if_stmt();
        }
        if self.is_kw("while") {
            self.bump();
            let cond = self.cond_expr()?;
            let body = self.body_block()?;
            return Ok(mk(StmtKind::While { cond, body }));
        }
        if self.is_kw("loop") {
            self.bump();
            return Ok(mk(StmtKind::Loop(self.block()?)));
        }
        if let Tok::Ident(l) = &t.tok {
            if !is_reserved(l) && matches!(self.peek_at(1).tok, Tok::Sym(":")) && (self.peek_at(2).tok == Tok::Ident("while".into()) || self.peek_at(2).tok == Tok::Ident("loop".into()) || self.peek_at(2).tok == Tok::Ident("for".into())) {
                let l = l.clone();
                self.bump();
                self.bump();
                let inner = self.stmt()?;
                return Ok(mk(StmtKind::Labeled(l, Box::new(inner))));
            }
        }
        for (kw, cont) in [("break", false), ("continue", true)] {
            if self.is_kw(kw) {
                self.bump();
                let label = match &self.cur().tok {
                    Tok::Ident(l) if !self.cur().nl && !is_reserved(l) => Some(l.clone()),
                    _ => None,
                };
                if label.is_some() {
                    self.bump();
                }
                self.end_stmt()?;
                return Ok(mk(match (label, cont) {
                    (None, false) => StmtKind::Break,
                    (None, true) => StmtKind::Continue,
                    (Some(l), false) => StmtKind::BreakTo(l),
                    (Some(l), true) => StmtKind::ContinueTo(l),
                }));
            }
        }
        if self.is_kw("return") {
            self.bump();
            let v = if self.is_sym(";") || self.is_sym("}") || self.cur().nl { None } else { Some(self.expr()?) };
            self.end_stmt()?;
            return Ok(mk(StmtKind::Return(v)));
        }
        if self.is_kw("switch") {
            return self.switch_stmt();
        }
        if self.is_kw("throw") {
            self.bump();
            let e = self.expr()?;
            self.end_stmt()?;
            return Ok(mk(StmtKind::Throw(e)));
        }
        if self.is_kw("try") {
            self.bump();
            let body = self.block()?;
            let mut catches = Vec::new();
            while self.is_kw("catch") {
                let ct = self.bump();
                self.expect_sym("(")?;
                let (name, _) = self.ident()?;
                self.expect_sym(":")?;
                let ty = self.type_expr()?;
                self.expect_sym(")")?;
                let cb = self.block()?;
                catches.push(Catch { name, ty, body: cb, line: ct.line, col: ct.col });
            }
            if catches.is_empty() {
                return self.err(&t, "`try` needs at least one `catch`");
            }
            return Ok(mk(StmtKind::Try { body, catches }));
        }
        if self.is_kw("for") {
            self.bump();
            self.expect_sym("(")?;
            let (var, _) = self.ident()?;
            if !self.is_kw("in") {
                let t = self.cur().clone();
                return self.err(&t, "expected `in`");
            }
            self.bump();
            let iter = self.no_struct_off(|p| p.expr())?;
            self.expect_sym(")")?;
            let body = self.body_block()?;
            return Ok(mk(StmtKind::For { var, iter, body }));
        }
        // `scope { }` is still a stub (runs in place); `mutate(x) { }` and `sync(x) { }` take the object's lock
        if self.is_kw("scope") {
            self.bump();
            return Ok(mk(StmtKind::Block(self.block()?)));
        }
        if self.is_kw("mutate") || self.is_kw("sync") {
            let mutate = self.is_kw("mutate");
            self.bump();
            self.expect_sym("(")?;
            let mut targets = vec![self.no_struct_off(|p| p.expr())?];
            while self.eat_sym(",") {
                targets.push(self.no_struct_off(|p| p.expr())?);
            }
            self.expect_sym(")")?;
            let body = self.block()?;
            return Ok(mk(StmtKind::Sync { mutate, targets, body }));
        }
        for kw in ["defer"] {
            if self.is_kw(kw) {
                return self.err(&t, format!("slice 0: '{}' is not supported yet", kw));
            }
        }
        if let Some((ty, name, _)) = self.try_c_decl() {
            self.expect_sym("=")?;
            let init = self.expr()?;
            self.end_stmt()?;
            return Ok(mk(StmtKind::Let { name, mutable: true, ty: Some(ty), init }));
        }
        let e = self.expr()?;
        let kind = if self.eat_sym("=") {
            let v = self.expr()?;
            StmtKind::Assign { target: e, op: None, value: v }
        } else if let Some(op) = self.compound_op() {
            let v = self.expr()?;
            StmtKind::Assign { target: e, op: Some(op), value: v }
        } else if self.is_sym("++") || self.is_sym("--") {
            let op = if self.is_sym("++") { BinOp::Add } else { BinOp::Sub };
            let tk = self.bump();
            let one = Expr { kind: ExprKind::Int(1), line: tk.line, col: tk.col };
            StmtKind::Assign { target: e, op: Some(op), value: one }
        } else {
            StmtKind::Expr(e)
        };
        self.end_stmt()?;
        Ok(mk(kind))
    }

    fn switch_stmt(&mut self) -> R<Stmt> {
        let t = self.expect_kw("switch")?;
        let subject = self.cond_expr()?;
        self.expect_sym("{")?;
        let mut arms = Vec::new();
        loop {
            self.skip_semis();
            if self.eat_sym("}") {
                break;
            }
            let mut pats = Vec::new();
            loop {
                pats.push(self.pattern()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym("->")?;
            let body = self.block()?;
            arms.push(Arm { pats, body });
        }
        Ok(Stmt { kind: StmtKind::Switch { subject, arms }, line: t.line, col: t.col })
    }

    fn pattern(&mut self) -> R<Pat> {
        let t = self.cur().clone();
        let mk = |kind| Pat { kind, line: t.line, col: t.col };
        if self.eat_kw("default") {
            return Ok(mk(PatKind::Default));
        }
        if matches!(&t.tok, Tok::Ident(s) if !is_reserved(s) && s != "true" && s != "false") {
            let (mut name, _) = self.ident()?;
            let mut qual = None;
            if self.eat_sym(".") {
                qual = Some(name);
                name = self.ident()?.0;
            }
            let mut binds = None;
            if self.eat_sym("(") {
                let mut b = Vec::new();
                while !self.is_sym(")") {
                    b.push(self.ident()?.0);
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                self.expect_sym(")")?;
                binds = Some(b);
            }
            return Ok(mk(PatKind::Variant { qual, name, binds }));
        }
        let e = self.unary()?;
        Ok(mk(PatKind::Lit(e)))
    }

    fn compound_op(&mut self) -> Option<BinOp> {
        let op = match &self.cur().tok {
            Tok::Sym("+=") => BinOp::Add,
            Tok::Sym("-=") => BinOp::Sub,
            Tok::Sym("*=") => BinOp::Mul,
            Tok::Sym("/=") => BinOp::Div,
            Tok::Sym("%=") => BinOp::Rem,
            _ => return None,
        };
        self.bump();
        Some(op)
    }

    /// A statement ends at ';', before '}', or at a line break.
    fn end_stmt(&mut self) -> R<()> {
        if self.eat_sym(";") || self.is_sym("}") || self.is_kw("else") || self.cur().nl || self.cur().tok == Tok::Eof {
            Ok(())
        } else {
            let t = self.cur().clone();
            self.err(&t, format!("expected end of statement, found {}", Self::describe(&t)))
        }
    }

    fn if_stmt(&mut self) -> R<Stmt> {
        let t = self.expect_kw("if")?;
        let cond = self.cond_expr()?;
        let then = self.body_block()?;
        let els = if self.eat_kw("else") {
            if self.is_kw("if") {
                let inner = self.if_stmt()?;
                Some(Block { line: inner.line, stmts: vec![inner] })
            } else {
                Some(self.body_block()?)
            }
        } else {
            None
        };
        Ok(Stmt { kind: StmtKind::If { cond, then, els }, line: t.line, col: t.col })
    }

    /// A `{ block }`, or a single statement standing for one.
    fn body_block(&mut self) -> R<Block> {
        if self.is_sym("{") {
            return self.block();
        }
        let line = self.cur().line;
        let st = self.stmt()?;
        Ok(Block { stmts: vec![st], line })
    }

    /// A condition: struct literals are not allowed unparenthesised, so `{` starts the body.
    fn cond_expr(&mut self) -> R<Expr> {
        self.no_struct(|p| p.expr())
    }

    fn no_struct<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        let old = NO_STRUCT.with(|c| c.replace(true));
        let r = f(self);
        NO_STRUCT.with(|c| c.set(old));
        r
    }

    // ---------------- expressions ----------------

    pub fn expr(&mut self) -> R<Expr> {
        let c = self.binary(0)?;
        // `a..b` is a Range value (inside `x[..]` the `..` belongs to the slice)
        if self.is_sym("..") && !self.cur().nl && !NO_RANGE.with(|c| c.get()) {
            let d = self.bump();
            let hi = self.binary(0)?;
            return Ok(range_lit(c, hi, d.line, d.col));
        }
        if self.is_sym("?") && !self.cur().nl {
            let q = self.bump();
            let a = self.no_struct_off(|p| p.expr())?;
            self.expect_sym(":")?;
            let b = self.expr()?;
            return Ok(Expr { kind: ExprKind::Cond(Box::new(c), Box::new(a), Box::new(b)), line: q.line, col: q.col });
        }
        Ok(c)
    }

    fn bin_info(&self) -> Option<(BinOp, u8)> {
                let t = self.cur();
        let s = match &t.tok {
            Tok::Sym(s) => *s,
            _ => return None,
        };
        let (op, prec) = match s {
            "||" => (BinOp::Or, 1),
            "&&" => (BinOp::And, 2),
            "==" => (BinOp::Eq, 3),
            "!=" => (BinOp::Ne, 3),
            "<" => (BinOp::Lt, 4),
            "<=" => (BinOp::Le, 4),
            ">" => (BinOp::Gt, 4),
            ">=" => (BinOp::Ge, 4),
            "+" => (BinOp::Add, 5),
            "-" => (BinOp::Sub, 5),
            "*" => (BinOp::Mul, 6),
            "/" => (BinOp::Div, 6),
            "%" => (BinOp::Rem, 6),
            _ => return None,
        };
        // a line that starts with `-` is a new statement (a negation); any other operator continues the line before
        if t.nl && op == BinOp::Sub {
            return None;
        }
        Some((op, prec))
    }

    fn binary(&mut self, min: u8) -> R<Expr> {
        let mut lhs = self.unary()?;
        while let Some((op, prec)) = self.bin_info() {
            if prec < min {
                break;
            }
            let t = self.bump();
            let rhs = self.binary(prec + 1)?;
            lhs = Expr { kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), line: t.line, col: t.col };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> R<Expr> {
        let t = self.cur().clone();
        // `spawn f(x)` is `spawnTask(fn() { f(x) })`: the runtime starts the call on its own thread
        if self.is_kw("spawn") {
            let start = self.pos;
            self.bump();
            let call = self.unary()?;
            let mut idents: Vec<String> = Vec::new();
            for tk in &self.toks[start..self.pos] {
                if let Tok::Ident(n) = &tk.tok {
                    if !idents.contains(n) {
                        idents.push(n.clone());
                    }
                }
            }
            let st = Stmt { kind: StmtKind::Expr(call), line: t.line, col: t.col };
            let body = Block { stmts: vec![st], line: t.line };
            let lam = Expr { kind: ExprKind::Lambda(Box::new(LambdaDecl { params: vec![], ret: None, body, idents })), line: t.line, col: t.col };
            let f = Expr { kind: ExprKind::Ident("spawnTask".into()), line: t.line, col: t.col };
            return Ok(Expr { kind: ExprKind::Call(Box::new(f), vec![lam]), line: t.line, col: t.col });
        }
        if self.eat_sym("-") {
            let e = self.unary()?;
            return Ok(Expr { kind: ExprKind::Unary(UnOp::Neg, Box::new(e)), line: t.line, col: t.col });
        }
        if self.eat_sym("!") {
            let e = self.unary()?;
            return Ok(Expr { kind: ExprKind::Unary(UnOp::Not, Box::new(e)), line: t.line, col: t.col });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        loop {
            let t = self.cur().clone();
            if self.is_sym(".") {
                self.bump();
                let (name, _) = self.ident()?;
                e = Expr { kind: ExprKind::Field(Box::new(e), name), line: t.line, col: t.col };
                if self.is_sym("<") && !self.cur().nl && self.type_args_call_ahead() {
                    self.bump();
                    self.in_type_args += 1;
                    let mut targs = Vec::new();
                    loop {
                        match self.type_expr() {
                            Ok(a) => targs.push(a),
                            Err(er) => {
                                self.in_type_args -= 1;
                                return Err(er);
                            }
                        }
                        if !self.eat_sym(",") {
                            break;
                        }
                    }
                    self.in_type_args -= 1;
                    self.expect_sym(">")?;
                    e = Expr { kind: ExprKind::TypeArgs(Box::new(e), targs), line: t.line, col: t.col };
                }
            } else if self.is_sym("(") && !t.nl {
                self.bump();
                let old_r = NO_RANGE.with(|c| c.replace(false));
                let args = self.args(")");
                NO_RANGE.with(|c| c.set(old_r));
                let mut args = args?;
                if matches!(&e.kind, ExprKind::Ident(n) if n == "Range") && args.len() == 2 {
                    let hi = args.pop().unwrap();
                    let lo = args.pop().unwrap();
                    e = range_lit(lo, hi, t.line, t.col);
                    continue;
                }
                e = Expr { kind: ExprKind::Call(Box::new(e), args), line: t.line, col: t.col };
            } else if self.is_sym("[") && !t.nl {
                self.bump();
                let old_r = NO_RANGE.with(|c| c.replace(true));
                let sl = self.no_struct_off(|p| {
                    let lo = if p.is_sym("..") { None } else { Some(Box::new(p.expr()?)) };
                    if p.eat_sym("..") {
                        let hi = if p.is_sym("]") { None } else { Some(Box::new(p.expr()?)) };
                        Ok((lo, true, hi))
                    } else {
                        Ok((lo, false, None))
                    }
                });
                NO_RANGE.with(|c| c.set(old_r));
                let (lo, is_slice, hi) = sl?;
                self.expect_sym("]")?;
                e = if is_slice {
                    Expr { kind: ExprKind::Slice(Box::new(e), lo, hi), line: t.line, col: t.col }
                } else {
                    Expr { kind: ExprKind::Index(Box::new(e), lo.unwrap()), line: t.line, col: t.col }
                };
            } else if self.is_kw("as") && !t.nl {
                self.bump();
                let ty = self.type_expr()?;
                e = Expr { kind: ExprKind::Cast(Box::new(e), ty), line: t.line, col: t.col };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn no_struct_off<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        let old = NO_STRUCT.with(|c| c.replace(false));
        let r = f(self);
        NO_STRUCT.with(|c| c.set(old));
        r
    }

    fn args(&mut self, close: &str) -> R<Vec<Expr>> {
        let mut v = Vec::new();
        self.no_struct_off(|p| {
            while !p.is_sym(close) {
                v.push(p.expr()?);
                if !p.eat_sym(",") {
                    break;
                }
            }
            p.expect_sym(close)?;
            Ok(())
        })?;
        Ok(v)
    }

    fn field_inits(&mut self) -> R<Vec<(String, Expr)>> {
        self.expect_sym("{")?;
        let mut v = Vec::new();
        self.no_struct_off(|p| {
            loop {
                p.skip_semis();
                if p.eat_sym("}") {
                    break;
                }
                let (n, _) = p.ident()?;
                p.expect_sym(":")?;
                let e = p.expr()?;
                v.push((n, e));
                if !p.eat_sym(",") {
                    p.skip_semis();
                    if !p.is_sym("}") && !matches!(p.cur().tok, Tok::Ident(_)) {
                        let t = p.cur().clone();
                        return p.err(&t, format!("expected ',' or '}}', found {}", Self::describe(&t)));
                    }
                }
            }
            Ok(())
        })?;
        Ok(v)
    }

    fn looks_like_struct_lit(&self) -> bool {
        if NO_STRUCT.with(|c| c.get()) || !self.is_sym("{") {
            return false;
        }
        match (&self.peek_at(1).tok, &self.peek_at(2).tok) {
            (Tok::Sym("}"), _) => true,
            (Tok::Ident(_), Tok::Sym(":")) => true,
            _ => false,
        }
    }

    /// At `T?[](`, `Ref<T>?[](`: a type, then `[](`, which makes an array of n zeroed elements
    fn array_ctor_ahead(&mut self) -> bool {
        let save = self.pos;
        let ok = self.type_expr().is_ok()
            && matches!(self.cur().tok, Tok::Sym("["))
            && matches!(self.peek_at(1).tok, Tok::Sym("]"))
            && matches!(self.peek_at(2).tok, Tok::Sym("("));
        self.pos = save;
        ok
    }

    /// At `<` after `a.name`: `<T, ..>(` means explicit type arguments to a call
    fn type_args_call_ahead(&mut self) -> bool {
        let (save, depth) = (self.pos, self.in_type_args);
        self.bump();
        self.in_type_args += 1;
        let mut ok = true;
        loop {
            if self.type_expr().is_err() {
                ok = false;
                break;
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        ok = ok && self.is_sym(">") && matches!(self.peek_at(1).tok, Tok::Sym("("));
        self.pos = save;
        self.in_type_args = depth;
        ok
    }

    fn growable_ahead(&mut self) -> bool {
        let save = self.pos;
        let ok = self.type_expr().is_ok() && matches!(self.cur().tok, Tok::Sym("(")) && matches!(self.peek_at(1).tok, Tok::Sym(")"));
        self.pos = save;
        ok
    }

    /// At `Name<..` : is this a type with arguments followed by `.` (a static call), not a comparison?
    fn generic_qualifier(&mut self) -> bool {
        let save = self.pos;
        let ok = self.type_expr().is_ok() && (self.is_sym(".") || self.looks_like_struct_lit());
        self.pos = save;
        ok
    }

    fn primary(&mut self) -> R<Expr> {
        let t = self.cur().clone();
        let mk = |kind| Expr { kind, line: t.line, col: t.col };
        match &t.tok {
            Tok::Int(v) => {
                self.bump();
                // `50us`: a unit written right after the number stands for `50.us`
                if let Tok::Ident(u) = &self.cur().tok {
                    if !self.cur().nl && matches!(u.as_str(), "kb" | "mb" | "gb" | "s" | "ms" | "us" | "ns") {
                        let u = u.clone();
                        self.bump();
                        return Ok(mk(ExprKind::Field(Box::new(mk(ExprKind::Int(*v))), u)));
                    }
                }
                Ok(mk(ExprKind::Int(*v)))
            }
            Tok::Float(v) => {
                self.bump();
                Ok(mk(ExprKind::Float(*v)))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(mk(ExprKind::Str(s.clone())))
            }
            Tok::Char(c) => {
                self.bump();
                Ok(mk(ExprKind::Char(*c)))
            }
            Tok::Sym("(") => {
                self.bump();
                let e = self.no_struct_off(|p| p.expr())?;
                self.expect_sym(")")?;
                Ok(e)
            }
            Tok::Ident(s) if s == "true" || s == "false" => {
                self.bump();
                Ok(mk(ExprKind::Bool(s == "true")))
            }
            Tok::Ident(s) if s == "new" => {
                self.bump();
                let ty = self.type_expr()?;
                if self.is_sym("[") {
                    self.bump();
                    let len = self.no_struct_off(|p| p.expr())?;
                    self.expect_sym("]")?;
                    return Ok(mk(ExprKind::NewArray(ty, Box::new(len))));
                }
                if self.is_sym("(") {
                    self.bump();
                    if !self.is_sym(")") {
                        let c = self.cur().clone();
                        return self.err(&c, "slice 0: constructor arguments are not supported; write `new T { field: value }`");
                    }
                    self.bump();
                    return Ok(mk(ExprKind::New(ty, Vec::new())));
                }
                if self.is_sym("{") {
                    let f = self.field_inits()?;
                    return Ok(mk(ExprKind::New(ty, f)));
                }
                let c = self.cur().clone();
                self.err(&c, format!("expected '{{' or '(' after 'new {}'", ty.name))
            }
            Tok::Ident(s) if !is_reserved(s) && matches!(self.peek_at(1).tok, Tok::Sym("?") | Tok::Sym("<")) && self.array_ctor_ahead() =>
            {
                // `T?[](n)`: an array of n null slots
                let ty = self.type_expr()?;
                self.expect_sym("[")?;
                self.expect_sym("]")?;
                self.expect_sym("(")?;
                let len = self.no_struct_off(|p| p.expr())?;
                self.expect_sym(")")?;
                Ok(mk(ExprKind::NewArray(ty, Box::new(len))))
            }
            Tok::Ident(s) if s == "fn" && (matches!(self.peek_at(1).tok, Tok::Sym("(")) || (matches!(self.peek_at(1).tok, Tok::Sym("[")) && matches!(self.peek_at(3).tok, Tok::Sym("]")) && matches!(self.peek_at(4).tok, Tok::Sym("(")))) => {
                let start = self.pos;
                self.bump();
                if self.eat_sym("[") {
                    self.bump(); // the effect attribute: `nopark`, `yield`
                    self.expect_sym("]")?;
                }
                self.expect_sym("(")?;
                let mut params = Vec::new();
                while !self.is_sym(")") {
                    let (pn, _) = self.ident()?;
                    self.expect_sym(":")?;
                    let ty = self.type_expr()?;
                    params.push(Param { name: pn, ty, variadic: false });
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                self.expect_sym(")")?;
                let ret = if self.eat_sym(":") { Some(self.type_expr()?) } else { None };
                let body = self.no_struct_off(|p| p.block())?;
                let mut idents: Vec<String> = Vec::new();
                for tk in &self.toks[start..self.pos] {
                    if let Tok::Ident(n) = &tk.tok {
                        if !idents.contains(n) {
                            idents.push(n.clone());
                        }
                    }
                }
                Ok(mk(ExprKind::Lambda(Box::new(LambdaDecl { params, ret, body, idents }))))
            }
            Tok::Sym("[") => {
                self.bump();
                let items = self.args("]")?;
                Ok(mk(ExprKind::ArrayLit(items)))
            }
            Tok::Ident(s) if s == "Array" && matches!(self.peek_at(1).tok, Tok::Sym("<")) && self.growable_ahead() => {
                // `Array<T>()`: a growable array in the smallest size class
                let ty = self.type_expr()?;
                self.expect_sym("(")?;
                self.expect_sym(")")?;
                let el = ty.args.into_iter().next().unwrap();
                Ok(mk(ExprKind::NewArray(el, Box::new(mk(ExprKind::Int(0))))))
            }
            Tok::Ident(s) if !is_reserved(s) && s.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false) && matches!(self.peek_at(1).tok, Tok::Sym("<")) && self.generic_qualifier() => {
                let ty = self.type_expr()?;
                if self.looks_like_struct_lit() {
                    let f = self.field_inits()?;
                    return Ok(mk(ExprKind::StructLit(ty, f)));
                }
                Ok(mk(ExprKind::TypeName(ty)))
            }
            Tok::Ident(s) if !is_reserved(s) => {
                self.bump();
                let starts_upper = s.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false);
                if starts_upper && self.looks_like_struct_lit() {
                    let f = self.field_inits()?;
                    return Ok(mk(ExprKind::StructLit(TypeExpr { name: s.clone(), args: Vec::new(), optional: false, fixed: None, line: t.line, col: t.col }, f)));
                }
                Ok(mk(ExprKind::Ident(s.clone())))
            }
            _ => self.err(&t, format!("expected an expression, found {}", Self::describe(&t))),
        }
    }
}

thread_local! {
    static NO_STRUCT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static NO_RANGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn is_reserved(s: &str) -> bool {
    matches!(
        s,
        "fn" | "struct" | "let" | "var" | "if" | "else" | "while" | "loop" | "break" | "continue" | "return" | "new" | "true" | "false" | "const"
            | "package" | "import" | "as" | "for" | "in" | "switch" | "enum" | "static" | "exception" | "throw" | "try" | "catch" | "throws" | "extern" | "parks" | "impl"
    )
}

/// `Range(lo, hi)` and `lo..hi`: the prelude's `Range { lo, hi }`.
fn range_lit(lo: Expr, hi: Expr, line: u32, col: u32) -> Expr {
    let te = TypeExpr { name: "Range".into(), args: vec![], optional: false, fixed: None, line, col };
    Expr { kind: ExprKind::StructLit(te, vec![("lo".into(), lo), ("hi".into(), hi)]), line, col }
}
