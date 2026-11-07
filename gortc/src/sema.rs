//! Name resolution, type checking and lowering to HIR.

use crate::ast::{self, BinOp, Expr, ExprKind, Item, StmtKind, TypeExpr, UnOp};
use crate::diag::Diag;
use crate::hir::*;
use std::collections::HashMap;

#[derive(Clone, Debug)]
enum ConstVal {
    Int(i128),
    Float(f64),
    Bool(bool),
}

struct FnCtx {
    vars: Vec<VarInfo>,
    scopes: Vec<HashMap<String, VarId>>,
    ret: Ty,
    self_sid: Option<usize>,
    loops: u32,
    loop_labels: Vec<Option<String>>,
    /// what the function declares in `throws`
    declared: Vec<usize>,
    /// the catch lists of the enclosing `try` blocks, innermost last
    catching: Vec<Vec<usize>>,
}

impl FnCtx {
    fn new(ret: Ty, self_sid: Option<usize>) -> FnCtx {
        FnCtx { vars: Vec::new(), scopes: vec![HashMap::new()], ret, self_sid, loops: 0, loop_labels: Vec::new(), declared: Vec::new(), catching: Vec::new() }
    }
    fn lookup(&self, name: &str) -> Option<VarId> {
        for s in self.scopes.iter().rev() {
            if let Some(v) = s.get(name) {
                return Some(*v);
            }
        }
        None
    }
}

pub struct Sema<'a> {
    files: &'a [ast::File],
    pub diags: Vec<Diag>,
    prog: Program,
    cur_file: String,
    struct_ids: HashMap<String, usize>,
    fn_ids: HashMap<String, FnId>,
    consts: HashMap<String, (Ty, ConstVal)>,
    globals: HashMap<String, usize>,
    /// bodies to lower: (fn id, file name, declaration)
    bodies: Vec<(FnId, String, ast::FnDecl, HashMap<String, Ty>)>,
    /// generic struct templates by name, with the file that declares them
    generics: HashMap<String, (ast::StructDecl, String)>,
    /// instances of the templates by `Name<args>`
    instances: HashMap<String, usize>,
    /// the type parameters in scope while an instance is being declared or lowered
    tmap: HashMap<String, Ty>,
    /// for each instance: its template and arguments, to unify against
    inst_args: HashMap<usize, (String, Vec<Ty>)>,
    /// the type the expression about to be lowered is expected to have (set by its context)
    expected: Option<Ty>,
    for_count: usize,
    /// (callee, parameter, line, col): a value lent to a `Ref` parameter, checked once every body is known
    borrow_sites: Vec<(FnId, usize, u32, u32)>,
    lambda_count: usize,
    /// functions given where a `fn[nopark]` is wanted: (function, line, col); checked once the effects are known
    nopark_uses: Vec<(usize, u32, u32)>,
    pending_label: Option<String>,
    /// the expected type of the expression being lowered right now
    exp_here: Option<Ty>,
    string_ids: HashMap<Vec<u8>, usize>,
}

fn lit(kind: EK, ty: Ty, line: u32) -> TExpr {
    TExpr { kind, ty, line, lit: true }
}
fn typed(kind: EK, ty: Ty, line: u32) -> TExpr {
    TExpr { kind, ty, line, lit: false }
}
fn error_expr(line: u32) -> TExpr {
    TExpr { kind: EK::Int(0), ty: Ty::Error, line, lit: false }
}

pub fn analyze(files: &[ast::File]) -> Result<Program, Vec<Diag>> {
    let mut s = Sema {
        files,
        diags: Vec::new(),
        prog: Program::default(),
        cur_file: String::new(),
        struct_ids: HashMap::new(),
        fn_ids: HashMap::new(),
        consts: HashMap::new(),
        globals: HashMap::new(),
        bodies: Vec::new(),
        generics: HashMap::new(),
        instances: HashMap::new(),
        tmap: HashMap::new(),
        inst_args: HashMap::new(),
        expected: None,
        for_count: 0,
        borrow_sites: Vec::new(),
        lambda_count: 0,
        nopark_uses: Vec::new(),
        pending_label: None,
        exp_here: None,
        string_ids: HashMap::new(),
    };
    s.run();
    if s.diags.is_empty() {
        Ok(s.prog)
    } else {
        Err(s.diags)
    }
}

impl<'a> Sema<'a> {
    fn error(&mut self, line: u32, col: u32, msg: impl Into<String>) {
        self.diags.push(Diag { file: self.cur_file.clone(), line, col, msg: msg.into() });
    }

    fn run(&mut self) {
        let files = self.files;
        // every file names its package; an import must name a package among the files given. Slice 1 keeps one
        // namespace for all packages, so a name declared in two of them is an error.
        let mut pkgs: Vec<&String> = Vec::new();
        for f in files {
            match &f.package {
                Some(p) => {
                    if !pkgs.contains(&p) {
                        pkgs.push(p);
                    }
                }
                None => {
                    self.cur_file = f.name.clone();
                    self.error(1, 1, "missing `package` line");
                }
            }
        }
        for f in files {
            self.cur_file = f.name.clone();
            for it in &f.items {
                if let Item::Import(n) = it {
                    if !pkgs.contains(&n) {
                        self.error(1, 1, format!("import `{}`: no file of that package was given", n));
                    }
                }
            }
        }
        // the built-in NullValue exception
        self.struct_ids.insert("NullValue".into(), 0);
        self.prog.null_exc = Some(0);
        self.prog.structs.push(StructInfo {
            name: "NullValue".into(),
            file: String::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            variants: Vec::new(),
            is_exception: true,
            is_extern: false,
            cleanup: None,
            needs_drop: false,
            no_copy: false,
            line: 0,
        });
        // 1. struct names
        for f in files {
            self.cur_file = f.name.clone();
            for it in &f.items {
                if let Item::Struct(sd) = it {
                    if !sd.type_params.is_empty() {
                        if self.struct_ids.contains_key(&sd.name) || self.generics.contains_key(&sd.name) {
                            self.error(sd.line, sd.col, format!("struct `{}` is already declared", sd.name));
                        } else {
                            self.generics.insert(sd.name.clone(), (sd.clone(), f.name.clone()));
                        }
                        continue;
                    }
                    if self.struct_ids.contains_key(&sd.name) {
                        self.error(sd.line, sd.col, format!("struct `{}` is already declared", sd.name));
                        continue;
                    }
                    let id = self.prog.structs.len();
                    self.struct_ids.insert(sd.name.clone(), id);
                    self.prog.structs.push(StructInfo {
                        name: sd.name.clone(),
                        file: f.name.clone(),
                        fields: Vec::new(),
                        methods: Vec::new(),
                        variants: Vec::new(),
                        is_exception: sd.is_exception,
                        is_extern: sd.is_extern,
                        cleanup: None,
                        needs_drop: false,
                        no_copy: false,
                        line: sd.line,
                    });
                }
            }
        }
        // 2. field types (enums last, so a struct payload already has its fields)
        for enums in [false, true] {
        for f in files {
            self.cur_file = f.name.clone();
            for it in &f.items {
                if let Item::Struct(sd) = it {
                    if sd.is_enum != enums || !sd.type_params.is_empty() {
                        continue;
                    }
                    let Some(&sid) = self.struct_ids.get(&sd.name) else { continue };
                    if !self.prog.structs[sid].fields.is_empty() {
                        continue;
                    }
                    if sd.is_enum {
                        self.declare_enum_fields(sid, sd);
                        continue;
                    }
                    let mut seen: Vec<String> = Vec::new();
                    for fd in &sd.fields {
                        if seen.contains(&fd.name) {
                            self.error(fd.line, fd.col, format!("field `{}` is declared twice", fd.name));
                            continue;
                        }
                        seen.push(fd.name.clone());
                        let ty = self.resolve_ty(&fd.ty);
                        self.prog.structs[sid].fields.push(FieldInfo { name: fd.name.clone(), ty, default: fd.default.clone() });
                    }
                }
            }
        }
        }
        // 3. function and method signatures
        for f in files {
            self.cur_file = f.name.clone();
            for it in &f.items {
                match it {
                    Item::Fn(fd) if fd.owner.is_none() => self.declare_fn(f, fd, None),
                    Item::Struct(sd) if sd.type_params.is_empty() => {
                        let Some(&sid) = self.struct_ids.get(&sd.name) else { continue };
                        if self.prog.structs[sid].file != f.name {
                            continue;
                        }
                        for m in &sd.methods {
                            self.declare_fn(f, m, Some(sid));
                        }
                    }
                    Item::Impl(te, ms) => {
                        match self.struct_ids.get(&te.name).copied().filter(|_| te.args.is_empty()) {
                            Some(sid) => {
                                for m in ms {
                                    let mut m = m.clone();
                                    m.is_extern = true;
                                    self.declare_fn(f, &m, Some(sid));
                                }
                            }
                            None => self.error(te.line, te.col, format!("slice 1: `extern impl {}` is not supported yet", te.name)),
                        }
                    }
                    _ => {}
                }
            }
        }
        // out-of-line implementations: fn T::name
        for f in files {
            self.cur_file = f.name.clone();
            for it in &f.items {
                if let Item::Fn(fd) = it {
                    if let Some(owner) = &fd.owner {
                        self.attach_impl(f, fd, owner);
                    }
                }
            }
        }
        // declared but never implemented
        for fi in 0..self.prog.fns.len() {
            let has_body = self.bodies.iter().any(|b| b.0 == fi);
            if !has_body {
                let (n, line, o) = (self.prog.fns[fi].name.clone(), self.prog.fns[fi].line, self.prog.fns[fi].owner);
                if self.prog.fns[fi].is_extern {
                    continue;
                }
                if let Some(o) = o {
                    self.cur_file = self.prog.structs[o].file.clone();
                    let sn = self.prog.structs[o].name.clone();
                    self.error(line, 1, format!("`{}::{}` is declared but has no implementation", sn, n));
                }
            }
        }
        // an array handed to or returned by C holds plain data: scalars, or structs made only of them
        fn plain(prog: &Program, t: &Ty, depth: usize) -> bool {
            match t {
                Ty::Bool | Ty::Int(_) | Ty::F64 | Ty::Dur | Ty::Inst => true,
                Ty::Struct(s) => {
                    let si = &prog.structs[*s];
                    depth < 16 && !si.is_extern && si.variants.is_empty() && !si.methods.iter().any(|m| m.0 == "cleanup") && si.fields.iter().all(|f| plain(prog, &f.ty, depth + 1))
                }
                _ => false,
            }
        }
        for fi in 0..self.prog.fns.len() {
            if !self.prog.fns[fi].is_extern {
                continue;
            }
            let tys: Vec<Ty> = self.prog.fns[fi].param_tys.iter().cloned().chain(std::iter::once(self.prog.fns[fi].ret.clone())).collect();
            for t in tys {
                if let Ty::Arr(el) = &t {
                    if !plain(&self.prog, el, 0) {
                        let (line, name, file) = (self.prog.fns[fi].line, self.prog.fns[fi].name.clone(), self.prog.fns[fi].file.clone());
                        self.cur_file = file;
                        let en = self.prog.type_name(el);
                        self.error(line, 1, format!("`extern fn {}` takes or returns Array<{}>: an array that crosses into C holds only numbers, bool, or structs of them", name, en));
                        break;
                    }
                }
            }
        }
        // cleanup() methods
        for sid in 0..self.prog.structs.len() {
            let c = self.prog.structs[sid].methods.iter().find(|m| m.0 == "cleanup").map(|m| m.1);
            if let Some(fid) = c {
                let ok = !self.prog.fns[fid].is_static && self.prog.fns[fid].param_tys.is_empty() && self.prog.fns[fid].ret == Ty::Unit;
                if ok {
                    self.prog.structs[sid].cleanup = Some(fid);
                } else {
                    self.cur_file = self.prog.structs[sid].file.clone();
                    let line = self.prog.fns[fid].line;
                    self.error(line, 1, "cleanup() must be a plain function with no parameters and no result");
                }
            }
        }
        self.struct_flags(true);
        // 4. constants
        for f in files {
            self.cur_file = f.name.clone();
            for it in &f.items {
                if let Item::Const(cd) = it {
                    self.declare_const(cd);
                }
                if let Item::Global { name, ty, init, line, col } = it {
                    self.declare_global(name, ty.as_ref(), init, (*line, *col));
                }
                if let Item::ExternLet(cd) = it {
                    let ty = self.resolve_ty(&cd.ty);
                    if self.globals.contains_key(&cd.name) {
                        self.error(cd.line, cd.col, format!("`{}` is already declared", cd.name));
                    } else {
                        self.globals.insert(cd.name.clone(), self.prog.globals.len());
                        self.prog.globals.push(GlobalInfo { name: cd.name.clone(), ty, is_extern: true });
                    }
                }
            }
        }
        // 5. bodies
        // (lowering a body can instantiate a generic struct, which queues more bodies)
        let mut done = 0;
        while done < self.bodies.len() {
            let (fid, file, decl, tmap) = self.bodies[done].clone();
            done += 1;
            self.cur_file = file;
            self.tmap = tmap;
            self.lower_fn(fid, &decl);
            self.tmap = HashMap::new();
        }
        self.compute_borrows();
        self.compute_effects();
        for fi in crate::share::check(&self.prog) {
            let d = Diag { file: fi.file, line: fi.line, col: 1, msg: fi.msg };
            if fi.warn {
                self.prog.warnings.push(d);
            } else {
                self.diags.push(d);
            }
        }
        // main: no parameters, or one config struct whose fields (numbers, bool, String) are set from `--name=value`
        if let Some(&m) = self.fn_ids.get("main") {
            let (line, ret, ptys) = {
                let f = &self.prog.fns[m];
                (f.line, f.ret.clone(), f.param_tys.clone())
            };
            let mut bad = ret != Ty::Unit;
            match ptys.as_slice() {
                [] => {}
                [Ty::Struct(sid)] => {
                    let si = &self.prog.structs[*sid];
                    let flat = si.variants.is_empty() && !si.is_extern && si.fields.iter().all(|f| matches!(f.ty, Ty::Int(_) | Ty::Bool | Ty::F64 | Ty::Str));
                    match (flat, self.fn_ids.get("__main_cfg")) {
                        (true, Some(&c)) => self.prog.main_cfg = Some((*sid, c)),
                        _ => {
                            self.error(line, 1, "the config struct of `main` may only have number, bool and String fields");
                            bad = false;
                        }
                    }
                }
                _ => bad = true,
            }
            if bad {
                self.error(line, 1, "`main` takes no parameters (or one config struct) and returns nothing");
            }
            self.prog.main = Some(m);
        }
    }

    /// An enum becomes a struct: field 0 is the tag (`i32`), then every variant's payload fields.
    /// The struct for `Name<args>`: declared on first use, with its methods queued for lowering.
    /// The struct a type expression names, instantiating a generic one.
    fn struct_of(&mut self, te: &TypeExpr) -> Option<usize> {
        if te.args.is_empty() && !self.generics.contains_key(&te.name) {
            return self.struct_ids.get(&te.name).copied();
        }
        match self.resolve_ty(te) {
            Ty::Struct(s) => Some(s),
            _ => None,
        }
    }

    /// Binds the template's type parameters by matching a declared type against an actual one.
    fn unify(&self, te: &TypeExpr, ty: &Ty, params: &[String], b: &mut HashMap<String, Ty>) {
        if te.args.is_empty() && params.contains(&te.name) {
            let t = match (te.optional, ty) {
                (true, Ty::Opt(i)) => (**i).clone(),
                _ => ty.clone(),
            };
            if t != Ty::Null && t != Ty::Error {
                b.entry(te.name.clone()).or_insert(t);
            }
            return;
        }
        let ty = match (te.optional, ty) {
            (true, Ty::Opt(i)) => &**i,
            _ => ty,
        };
        match (te.name.as_str(), ty) {
            ("Ref", Ty::Ref(s)) if te.args.len() == 1 => self.unify(&te.args[0], &Ty::Struct(*s), params, b),
            ("Array", Ty::Arr(el)) if te.args.len() == 1 => self.unify(&te.args[0], el, params, b),
            (n, Ty::Struct(s)) => {
                if let Some((tn, targs)) = self.inst_args.get(s) {
                    if tn == n && targs.len() == te.args.len() {
                        for (a, t) in te.args.iter().zip(targs.iter()) {
                            self.unify(a, t, params, b);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The instance of `tn` when every type parameter is bound.
    fn instance_from(&mut self, tn: &str, b: &HashMap<String, Ty>, at: (u32, u32)) -> Option<usize> {
        let params = self.generics.get(tn)?.0.type_params.clone();
        let mut args = Vec::new();
        for p in &params {
            match b.get(p) {
                Some(t) => args.push(t.clone()),
                None => {
                    self.error(at.0, at.1, format!("cannot infer the type argument `{}` of `{}`; write `{}<..>`", p, tn, tn));
                    return None;
                }
            }
        }
        self.instantiate(tn, args, at)
    }

    /// Lowers `e` only to learn its type: nothing it reports or allocates is kept.
    fn probe(&mut self, cx: &mut FnCtx, e: &Expr) -> Ty {
        let nd = self.diags.len();
        let saved = self.expected.take();
        let t = self.lower_expr(cx, e).ty;
        self.expected = saved;
        self.diags.truncate(nd);
        t
    }

    /// `Name.fn(args)` for a generic `Name` written without arguments: they come from the arguments, or from the expected type.
    fn generic_static_call(&mut self, cx: &mut FnCtx, e: &Expr, callee: &Expr, tn: &str, name: &str, args: &[Expr]) -> TExpr {
        let line = e.line;
        let (sd, _) = self.generics.get(tn).cloned().unwrap();
        let Some(md) = sd.methods.iter().find(|m| m.name == name && m.is_static) else {
            self.error(callee.line, callee.col, format!("`{}` has no static function `{}`", tn, name));
            return error_expr(line);
        };
        let mut b: HashMap<String, Ty> = HashMap::new();
        if md.params.len() == args.len() {
            for (p, a) in md.params.iter().zip(args.iter()) {
                let t = self.probe(cx, a);
                self.unify(&p.ty, &t, &sd.type_params, &mut b);
            }
        }
        if let (Some(ret), Some(exp)) = (&md.ret, self.exp_here.clone()) {
            self.unify(ret, &exp, &sd.type_params, &mut b);
        }
        let Some(sid) = self.instance_from(tn, &b, (e.line, e.col)) else { return error_expr(line) };
        let Some(&(_, fid)) = self.prog.structs[sid].methods.iter().find(|m| m.0 == name) else { return error_expr(line) };
        let Some(a) = self.lower_args(cx, fid, args, e) else { return error_expr(line) };
        let ret = self.prog.fns[fid].ret.clone();
        typed(EK::Call(fid, a), ret, line)
    }

    /// The struct a literal or `new` names; a generic one without arguments is inferred from the expected type or the fields.
    fn struct_for(&mut self, cx: &mut FnCtx, te: &TypeExpr, fields: &[(String, Expr)]) -> Option<usize> {
        if !te.args.is_empty() || !self.generics.contains_key(&te.name) {
            return self.struct_of(te);
        }
        if let Some(Ty::Struct(s) | Ty::Ref(s)) = self.exp_here.clone() {
            if self.inst_args.get(&s).map(|x| x.0 == te.name).unwrap_or(false) {
                return Some(s);
            }
        }
        let (sd, _) = self.generics.get(&te.name).cloned().unwrap();
        let mut b: HashMap<String, Ty> = HashMap::new();
        for (fname, fe) in fields {
            if let Some(fd) = sd.fields.iter().find(|f| f.name == *fname) {
                let t = self.probe(cx, fe);
                self.unify(&fd.ty, &t, &sd.type_params, &mut b);
            }
        }
        self.instance_from(&te.name, &b, (te.line, te.col))
    }

    fn instantiate(&mut self, name: &str, args: Vec<Ty>, at: (u32, u32)) -> Option<usize> {
        let shown: Vec<String> = args.iter().map(|a| self.prog.type_name(a)).collect();
        let key = format!("{}<{}>", name, shown.join(", "));
        if let Some(&s) = self.instances.get(&key) {
            return Some(s);
        }
        let (sd, file) = self.generics.get(name).cloned()?;
        if sd.type_params.len() != args.len() {
            self.error(at.0, at.1, format!("`{}` takes {} type argument(s), found {}", name, sd.type_params.len(), args.len()));
            return None;
        }
        let cname: String = key.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let sid = self.prog.structs.len();
        self.instances.insert(key.clone(), sid);
        self.inst_args.insert(sid, (name.to_string(), args.clone()));
        self.prog.instances.push(Instance { template: name.to_string(), shown: key.clone(), sid, is_extern: sd.is_extern });
        self.prog.structs.push(StructInfo {
            name: cname,
            file: file.clone(),
            fields: Vec::new(),
            methods: Vec::new(),
            variants: Vec::new(),
            is_exception: sd.is_exception,
            is_extern: sd.is_extern,
            cleanup: None,
            needs_drop: false,
            no_copy: false,
            line: sd.line,
        });
        let saved_tmap = std::mem::replace(&mut self.tmap, sd.type_params.iter().cloned().zip(args).collect());
        let saved_file = std::mem::replace(&mut self.cur_file, file.clone());
        if sd.is_enum {
            self.declare_enum_fields(sid, &sd);
        } else {
            let mut seen: Vec<String> = Vec::new();
            for fd in &sd.fields {
                if seen.contains(&fd.name) {
                    self.error(fd.line, fd.col, format!("field `{}` is declared twice", fd.name));
                    continue;
                }
                seen.push(fd.name.clone());
                let ty = self.resolve_ty(&fd.ty);
                self.prog.structs[sid].fields.push(FieldInfo { name: fd.name.clone(), ty, default: fd.default.clone() });
            }
        }
        let fake = ast::File { name: file.clone(), package: None, items: Vec::new() };
        for m in &sd.methods {
            self.declare_fn(&fake, m, Some(sid));
        }
        if let Some(&(_, fid)) = self.prog.structs[sid].methods.iter().find(|m| m.0 == "cleanup") {
            self.prog.structs[sid].cleanup = Some(fid);
        }
        self.tmap = saved_tmap;
        self.cur_file = saved_file;
        self.struct_flags(false);
        Some(sid)
    }

    fn declare_enum_fields(&mut self, sid: usize, sd: &ast::StructDecl) {
        if sd.variants.is_empty() {
            self.error(sd.line, sd.col, format!("enum `{}` has no variants", sd.name));
        }
        self.prog.structs[sid].fields.push(FieldInfo { name: "_tag".into(), ty: Ty::Int(IntTy::I32), default: None });
        let mut seen: Vec<String> = Vec::new();
        for v in &sd.variants {
            if seen.contains(&v.name) {
                self.error(v.line, v.col, format!("variant `{}` is declared twice", v.name));
                continue;
            }
            seen.push(v.name.clone());
            let mut idxs = Vec::new();
            for (fname, fty) in &v.fields {
                let ty = self.resolve_ty(fty);
                let bad = match &ty {
                    Ty::Struct(t) => self.prog.structs[*t].no_copy,
                    Ty::Unit | Ty::Null | Ty::Obj(_) | Ty::Exc => true,
                    _ => false,
                };
                if bad {
                    let n = self.prog.type_name(&ty);
                    self.error(fty.line, fty.col, format!("an enum payload cannot be {}", n));
                }
                idxs.push(self.prog.structs[sid].fields.len());
                self.prog.structs[sid].fields.push(FieldInfo { name: format!("{}_{}", v.name, fname), ty, default: None });
            }
            self.prog.structs[sid].variants.push(VariantInfo { name: v.name.clone(), fields: idxs });
        }
    }

    /// A payload type that can be zero-filled in the variants that do not hold it: scalars and structs of them.
    fn zeroable(&self, t: &Ty) -> bool {
        match t {
            Ty::Int(_) | Ty::Bool | Ty::F64 | Ty::Dur | Ty::Inst | Ty::Fn(..) | Ty::Error => true,
            Ty::Struct(s) => {
                let si = &self.prog.structs[*s];
                si.variants.is_empty() && !si.methods.iter().any(|m| m.0 == "cleanup") && si.fields.iter().all(|f| self.zeroable(&f.ty))
            }
            _ => false,
        }
    }

    /// An array is created zero-filled, so its element must have a valid zero: a number, an optional (null), or a plain value.
    fn elem_ok(&self, t: &Ty) -> bool {
        matches!(t, Ty::Opt(_)) || self.zeroable(t) || matches!(t, Ty::Struct(_))
    }

    /// `new T[n]` fills the run with zeros, so T must have a valid zero.
    fn zero_fillable(&self, t: &Ty) -> bool {
        matches!(t, Ty::Opt(_)) || self.zeroable(t)
    }

    fn zero(&self, t: &Ty, line: u32) -> TExpr {
        match t {
            Ty::Bool => typed(EK::Bool(false), Ty::Bool, line),
            Ty::F64 => typed(EK::Float(0.0), Ty::F64, line),
            Ty::Int(_) | Ty::Dur | Ty::Inst | Ty::Fn(..) | Ty::Error => typed(EK::Int(0), t.clone(), line),
            _ => typed(EK::Zero, t.clone(), line),
        }
    }

    fn resolve_throws(&mut self, list: &[TypeExpr]) -> Vec<usize> {
        let mut out = Vec::new();
        for t in list {
            if t.name == "Exception" {
                out.push(ANY_EXC);
            } else if let Some(&sid) = self.struct_ids.get(&t.name).filter(|s| self.prog.structs[**s].is_exception) {
                out.push(sid);
            } else {
                self.error(t.line, t.col, format!("`{}` is not an exception", t.name));
            }
        }
        out
    }

    /// Is a throw of `exc` allowed here: caught by an enclosing `try`, or declared by the function?
    fn check_throw(&mut self, cx: &FnCtx, exc: usize, at: (u32, u32)) {
        let ok_in = |l: &Vec<usize>| l.contains(&ANY_EXC) || (exc != ANY_EXC && l.contains(&exc));
        if cx.catching.iter().any(ok_in) || ok_in(&cx.declared) {
            return;
        }
        let n = if exc == ANY_EXC { "Exception".to_string() } else { self.prog.structs[exc].name.clone() };
        self.error(at.0, at.1, format!("`{}` may be thrown here; catch it or add it to this function's `throws`", n));
    }

    /// A top-level `let`: its initialiser becomes a function that runs before `main`, in declaration order.
    /// Which `Ref` parameters only borrow. A parameter borrows when its function never lets the handle out: it is only
    /// read through (`p.x`, `p.f()`), compared, or passed on to another borrowing parameter. Storing it, returning it,
    /// throwing it or capturing it would let it outlive the call, and a value lent to such a parameter is an error.
    /// A function used as a value is called by someone who does not know its parameters, so it borrows nothing.
    /// Which functions may park (wait): an `extern` declared `parks`, a call through a function value (its target is unknown), or a call to
    /// a function that may. A fixpoint over the call graph. Then `fn[nopark]` and `cleanup()` are checked against it.
    fn compute_effects(&mut self) {
        let n = self.prog.fns.len();
        // why[i]: the call that makes fn i park, for the message
        let mut why: Vec<Option<String>> = vec![None; n];
        let mut may: Vec<bool> = self.prog.fns.iter().map(|f| f.is_extern && f.parks).collect();
        for i in 0..n {
            if may[i] {
                why[i] = Some("it is declared `parks`".into());
            }
        }
        loop {
            let mut changed = false;
            for i in 0..n {
                if may[i] {
                    continue;
                }
                let Some(body) = &self.prog.fns[i].body else { continue };
                let mut found: Option<String> = None;
                visit_block(body, &mut |e| {
                    visit_expr(e, &mut |x| {
                        if found.is_some() {
                            return;
                        }
                        match &x.kind {
                            EK::Call(f, _) | EK::Method(f, _, _) if may[*f] => found = Some(format!("it calls `{}`", self.prog.fns[*f].name)),
                            EK::CallInd(fx, _) if !matches!(fx.ty, Ty::Fn(.., true)) => found = Some("it calls a function value, whose target is unknown".into()),
                            _ => {}
                        }
                    })
                });
                if let Some(w) = found {
                    may[i] = true;
                    why[i] = Some(w);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // A function that enters a `sync` block (or calls one that does) can wait for the lock, and the wait parks the vthread when it holds no
        // other lock. That is not a park for the purpose of a `sync` body (a nested block, or a call that takes a lock the body may already hold,
        // never parks: it blocks the thread instead), so it is tracked apart from `may`.
        let mut locks: Vec<bool> = vec![false; n];
        let mut lwhy: Vec<Option<String>> = vec![None; n];
        for i in 0..n {
            if let Some(b) = &self.prog.fns[i].body {
                let mut has = false;
                walk_stmts(b, &mut |s| {
                    if matches!(s.kind, TS::Sync(..)) {
                        has = true;
                    }
                });
                if has {
                    locks[i] = true;
                    lwhy[i] = Some("it enters a sync block, which waits by parking".into());
                }
            }
        }
        loop {
            let mut changed = false;
            for i in 0..n {
                if locks[i] {
                    continue;
                }
                let Some(body) = &self.prog.fns[i].body else { continue };
                let mut found: Option<String> = None;
                visit_block(body, &mut |e| {
                    visit_expr(e, &mut |x| {
                        if let EK::Call(f, _) | EK::Method(f, _, _) = &x.kind {
                            if locks[*f] && found.is_none() {
                                found = Some(format!("it calls `{}`, which enters a sync block", self.prog.fns[*f].name));
                            }
                        }
                    })
                });
                if let Some(w) = found {
                    locks[i] = true;
                    lwhy[i] = Some(w);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for i in 0..n {
            self.prog.fns[i].may_park = may[i] || locks[i];
        }
        let reason = |i: usize| -> String { why[i].clone().or_else(|| lwhy[i].clone()).unwrap_or_default() };
        for i in 0..n {
            let f = &self.prog.fns[i];
            if f.nopark && (may[i] || locks[i]) && !f.is_extern {
                let msg = format!("`{}` is marked nopark but may park: {}", f.name, reason(i));
                self.error(f.line, 1, msg);
            }
        }
        let uses = std::mem::take(&mut self.nopark_uses);
        for (fid, line, col) in uses {
            let f = &self.prog.fns[fid];
            if (may[fid] || locks[fid]) && !f.is_extern && !f.nopark {
                let msg = format!("`{}` may park, so it cannot be used as a `fn[nopark]`: {}", f.name, reason(fid));
                self.error(line, col, msg);
            }
        }
        self.check_sync(&may);
        for si in 0..self.prog.structs.len() {
            if let Some(c) = self.prog.structs[si].cleanup {
                if (may[c] || locks[c]) && !self.prog.fns[c].is_extern {
                    let f = &self.prog.fns[c];
                    let msg = format!("cleanup() of `{}` may park: {}", self.prog.structs[si].name, reason(c));
                    self.error(f.line, 1, msg);
                }
            }
        }
    }

    /// The rules for `sync` / `mutate` blocks: the body may not park; a `sync` body may not write through its object; a block may not lock
    /// the object it is already inside.
    fn check_sync(&mut self, may: &[bool]) {
        let writes = param_writes(&self.prog);
        let mut errs: Vec<(u32, String)> = Vec::new();
        for f in &self.prog.fns {
            let Some(body) = &f.body else { continue };
            let mut open: Vec<&TExpr> = Vec::new();
            sync_walk(body, &mut open, &mut |targets: &[(bool, TExpr)], b, line| {
                // 1. no parking
                let mut found: Option<String> = None;
                visit_block(b, &mut |e| {
                    visit_expr(e, &mut |x| {
                        if found.is_some() {
                            return;
                        }
                        match &x.kind {
                            EK::Call(c, _) | EK::Method(c, _, _) if may[*c] => found = Some(format!("`{}` may park", self.prog.fns[*c].name)),
                            EK::CallInd(fx, _) if !matches!(fx.ty, Ty::Fn(.., true)) => found = Some("a call through a function value may park".into()),
                            _ => {}
                        }
                    })
                });
                if let Some(w) = found {
                    errs.push((line, format!("a sync block must not park: {}", w)));
                }
                // 2. an object listed under reads may not be written through
                for (write, target) in targets {
                    if !*write && block_writes(b, target, &writes) {
                        errs.push((line, format!("this block writes through `{}`, which is listed under reads: it needs writes", place_name(&self.prog, f, target))));
                    }
                }
            });
        }
        for (line, msg) in errs {
            self.error(line, 1, msg);
        }
    }

    fn compute_borrows(&mut self) {
        let n = self.prog.fns.len();
        let mut taken = vec![false; n];
        for f in &self.prog.fns {
            if let Some(b) = &f.body {
                visit_block(b, &mut |e| {
                    visit_expr(e, &mut |x| {
                        if let EK::FnRef(id, _) | EK::Lambda(id, _) = &x.kind {
                            taken[*id] = true;
                        }
                    })
                });
            }
        }
        let mut borrowed: Vec<Vec<bool>> = (0..n)
            .map(|i| {
                let f = &self.prog.fns[i];
                f.param_tys
                    .iter()
                    .map(|t| match t {
                        Ty::Ref(s) => f.body.is_some() && !f.is_extern && !taken[i] && !self.prog.structs[*s].is_extern,
                        _ => false,
                    })
                    .collect()
            })
            .collect();
        loop {
            let mut changed = false;
            for i in 0..n {
                let Some(body) = &self.prog.fns[i].body else { continue };
                for k in 0..borrowed[i].len() {
                    if !borrowed[i][k] {
                        continue;
                    }
                    let pid = self.prog.fns[i].params[k];
                    let mut esc = false;
                    visit_block(body, &mut |e| {
                        if escapes(e, pid, false, &borrowed) {
                            esc = true;
                        }
                    });
                    if esc {
                        borrowed[i][k] = false;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for (fid, idx, line, col) in std::mem::take(&mut self.borrow_sites) {
            if !borrowed[fid].get(idx).copied().unwrap_or(false) {
                let f = &self.prog.fns[fid];
                let why = if taken[fid] { "it is used as a function value" } else { "it stores, returns or throws the handle, so it could outlive the call" };
                self.error(line, col, format!("cannot lend a value to parameter {} of `{}`: {}; pass a `Ref` made with `new`", idx + 1, f.name, why));
            }
        }
        for (f, b) in self.prog.fns.iter_mut().zip(borrowed) {
            f.borrowed = b;
        }
    }

    fn declare_global(&mut self, name: &str, ty: Option<&TypeExpr>, init: &Expr, at: (u32, u32)) {
        if self.globals.contains_key(name) {
            self.error(at.0, at.1, format!("`{}` is already declared", name));
            return;
        }
        let mut cx = FnCtx::new(Ty::Unit, None);
        let v = self.lower_expr(&mut cx, init);
        let v = self.decay(&mut cx, v, (init.line, init.col));
        let (v, t) = match ty {
            Some(te) => {
                let t = self.resolve_ty(te);
                (self.coerce(&mut cx, v, &t, (init.line, init.col)), t)
            }
            None => {
                if v.ty == Ty::Null || v.ty == Ty::Unit {
                    self.error(init.line, init.col, "this top-level `let` needs a type");
                    return;
                }
                let v = if v.lit { self.finish_lit(v) } else { v };
                let t = v.ty.clone();
                (v, t)
            }
        };
        if t == Ty::Error {
            return;
        }
        let g = self.prog.globals.len();
        self.globals.insert(name.to_string(), g);
        self.prog.globals.push(GlobalInfo { name: name.to_string(), ty: t, is_extern: false });
        let fid = self.prog.fns.len();
        self.prog.fns.push(FnInfo {
            name: format!("init {}", name),
            cname: format!("gort_init_{}", name),
            owner: None,
            is_static: true,
            params: Vec::new(),
            param_tys: Vec::new(),
            ret: Ty::Unit,
            throws: Vec::new(),
            is_extern: false,
            parks: false,
            nopark: false,
            may_park: false,
            variadic: false,
            borrowed: Vec::new(),
            vars: cx.vars,
            body: Some(TBlock { stmts: vec![TStmt { kind: TS::SetGlobal(g, v), line: at.0 }] }),
            line: at.0,
            file: self.cur_file.clone(),
        });
        self.prog.global_inits.push(fid);
    }

    fn declare_fn(&mut self, f: &ast::File, fd: &ast::FnDecl, owner: Option<usize>) {
        self.cur_file = f.name.clone();
        let mut param_tys = Vec::new();
        for p in &fd.params {
            let t = self.resolve_ty(&p.ty);
            param_tys.push(if p.variadic && t != Ty::Error { Ty::Arr(Box::new(t)) } else { t });
        }
        let ret = match &fd.ret {
            Some(t) => self.resolve_ty(t),
            None => Ty::Unit,
        };
        let throws = self.resolve_throws(&fd.throws);
        let id = self.prog.fns.len();
        let cname = match (owner, fd.is_extern) {
            (Some(o), true) => format!("gort_x_{}_{}", self.prog.structs[o].name, fd.name),
            (None, true) => format!("gort_x_{}", fd.name),
            (Some(o), false) => format!("G_{}_{}", self.prog.structs[o].name, fd.name),
            (None, false) => format!("g_{}", fd.name),
        };
        let dup = match owner {
            Some(o) => self.prog.structs[o].methods.iter().any(|m| m.0 == fd.name),
            None => self.fn_ids.contains_key(&fd.name),
        };
        if dup {
            self.error(fd.line, fd.col, format!("`{}` is already declared", fd.name));
            return;
        }
        self.prog.fns.push(FnInfo {
            name: fd.name.clone(),
            cname,
            owner,
            is_static: owner.is_none() || fd.is_static,
            params: Vec::new(),
            param_tys,
            ret,
            throws,
            is_extern: fd.is_extern,
            parks: fd.parks,
            nopark: fd.attr.as_deref() == Some("nopark"),
            may_park: fd.parks,
            variadic: fd.params.last().map(|p| p.variadic).unwrap_or(false),
            borrowed: Vec::new(),
            vars: Vec::new(),
            body: None,
            line: fd.line,
            file: self.cur_file.clone(),
        });
        match owner {
            Some(o) => self.prog.structs[o].methods.push((fd.name.clone(), id)),
            None => {
                self.fn_ids.insert(fd.name.clone(), id);
            }
        }
        if fd.body.is_some() {
            self.bodies.push((id, f.name.clone(), fd.clone(), self.tmap.clone()));
        }
    }

    fn attach_impl(&mut self, f: &ast::File, fd: &ast::FnDecl, owner: &str) {
        let Some(&sid) = self.struct_ids.get(owner) else {
            self.error(fd.line, fd.col, format!("unknown struct `{}`", owner));
            return;
        };
        let Some(&(_, fid)) = self.prog.structs[sid].methods.iter().find(|m| m.0 == fd.name) else {
            self.error(fd.line, fd.col, format!("struct `{}` does not declare a function `{}`", owner, fd.name));
            return;
        };
        if self.bodies.iter().any(|b| b.0 == fid) {
            self.error(fd.line, fd.col, format!("`{}::{}` already has an implementation", owner, fd.name));
            return;
        }
        let mut ptys = Vec::new();
        for p in &fd.params {
            ptys.push(self.resolve_ty(&p.ty));
        }
        let ret = match &fd.ret {
            Some(t) => self.resolve_ty(t),
            None => Ty::Unit,
        };
        if ptys != self.prog.fns[fid].param_tys || ret != self.prog.fns[fid].ret {
            self.error(fd.line, fd.col, format!("the signature of `{}::{}` does not match its declaration in the struct", owner, fd.name));
            return;
        }
        self.bodies.push((fid, f.name.clone(), fd.clone(), self.tmap.clone()));
    }

    fn struct_flags(&mut self, report: bool) {
        // inline-by-value cycles
        let n = self.prog.structs.len();
        for s in 0..n {
            let mut stack = vec![s];
            let mut seen = vec![false; n];
            let mut cyc = false;
            while let Some(t) = stack.pop() {
                for f in &self.prog.structs[t].fields {
                    if let Some(u) = f.ty.inline_struct() {
                        if u == s {
                            cyc = true;
                        } else if !seen[u] {
                            seen[u] = true;
                            stack.push(u);
                        }
                    }
                }
            }
            if cyc && !report {
                continue;
            }
            if cyc {
                self.cur_file = self.prog.structs[s].file.clone();
                let (name, line) = (self.prog.structs[s].name.clone(), self.prog.structs[s].line);
                self.error(line, 1, format!("struct `{}` contains itself by value; use Ref<{}> for the link", name, name));
                return;
            }
        }
        loop {
            let mut changed = false;
            for s in 0..n {
                let mut nd = self.prog.structs[s].cleanup.is_some();
                let mut nc = nd;
                for f in &self.prog.structs[s].fields {
                    nd |= self.prog.needs_drop(&f.ty);
                    if let Some(u) = f.ty.inline_struct() {
                        nc |= self.prog.structs[u].no_copy;
                    }
                }
                if nd != self.prog.structs[s].needs_drop || nc != self.prog.structs[s].no_copy {
                    self.prog.structs[s].needs_drop = nd;
                    self.prog.structs[s].no_copy = nc;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn declare_const(&mut self, cd: &ast::ConstDecl) {
        let ty = self.resolve_ty(&cd.ty);
        if ty == Ty::Error {
            return;
        }
        if self.consts.contains_key(&cd.name) {
            self.error(cd.line, cd.col, format!("constant `{}` is already declared", cd.name));
            return;
        }
        let v = match self.const_eval(&cd.value) {
            Some(v) => v,
            None => {
                self.error(cd.value.line, cd.value.col, "slice 0: a constant must be a literal or arithmetic on constants");
                return;
            }
        };
        let v = match (&ty, v) {
            (Ty::Int(it), ConstVal::Int(x)) => {
                let (lo, hi) = it.range();
                if x < lo || x > hi {
                    self.error(cd.value.line, cd.value.col, format!("value {} is out of range for {}", x, it.name()));
                    return;
                }
                ConstVal::Int(x)
            }
            (Ty::F64, ConstVal::Float(x)) => ConstVal::Float(x),
            (Ty::F64, ConstVal::Int(x)) => ConstVal::Float(x as f64),
            (Ty::Bool, ConstVal::Bool(x)) => ConstVal::Bool(x),
            _ => {
                self.error(cd.value.line, cd.value.col, format!("constant `{}` has type {}, which does not match its value", cd.name, self.prog.type_name(&ty)));
                return;
            }
        };
        self.consts.insert(cd.name.clone(), (ty, v));
    }

    fn const_eval(&self, e: &Expr) -> Option<ConstVal> {
        Some(match &e.kind {
            ExprKind::Int(v) => ConstVal::Int(*v as i128),
            ExprKind::Char(c) => ConstVal::Int(*c as i128),
            ExprKind::Float(v) => ConstVal::Float(*v),
            ExprKind::Bool(b) => ConstVal::Bool(*b),
            ExprKind::Ident(n) => self.consts.get(n)?.1.clone(),
            ExprKind::Unary(UnOp::Neg, x) => match self.const_eval(x)? {
                ConstVal::Int(v) => ConstVal::Int(-v),
                ConstVal::Float(v) => ConstVal::Float(-v),
                _ => return None,
            },
            ExprKind::Unary(UnOp::Not, x) => match self.const_eval(x)? {
                ConstVal::Bool(v) => ConstVal::Bool(!v),
                _ => return None,
            },
            ExprKind::Binary(op, a, b) => {
                let (a, b) = (self.const_eval(a)?, self.const_eval(b)?);
                match (a, b) {
                    (ConstVal::Int(x), ConstVal::Int(y)) => match op {
                        BinOp::Add => ConstVal::Int(x + y),
                        BinOp::Sub => ConstVal::Int(x - y),
                        BinOp::Mul => ConstVal::Int(x * y),
                        BinOp::Div if y != 0 => ConstVal::Int(x / y),
                        BinOp::Rem if y != 0 => ConstVal::Int(x % y),
                        _ => return None,
                    },
                    (ConstVal::Float(x), ConstVal::Float(y)) => match op {
                        BinOp::Add => ConstVal::Float(x + y),
                        BinOp::Sub => ConstVal::Float(x - y),
                        BinOp::Mul => ConstVal::Float(x * y),
                        BinOp::Div => ConstVal::Float(x / y),
                        _ => return None,
                    },
                    _ => return None,
                }
            }
            _ => return None,
        })
    }

    fn resolve_ty(&mut self, t: &TypeExpr) -> Ty {
        let base = self.resolve_ty_base(t);
        if t.optional && base != Ty::Error {
            return match base {
                Ty::Int(_) | Ty::Bool | Ty::F64 | Ty::Ref(_) | Ty::Str | Ty::Arr(_) | Ty::Struct(_) => Ty::Opt(Box::new(base)),
                other => {
                    let n = self.prog.type_name(&other);
                    self.error(t.line, t.col, format!("{} cannot be optional", n));
                    Ty::Error
                }
            };
        }
        base
    }

    fn resolve_ty_base(&mut self, t: &TypeExpr) -> Ty {
        let bad = |s: &mut Self, msg: String| {
            s.error(t.line, t.col, msg);
            Ty::Error
        };
        if t.args.is_empty() {
            if let Some(ty) = self.tmap.get(&t.name) {
                return ty.clone();
            }
        }
        if self.generics.contains_key(&t.name) {
            let args: Vec<Ty> = t.args.iter().map(|a| self.resolve_ty(a)).collect();
            if args.iter().any(|a| *a == Ty::Error) {
                return Ty::Error;
            }
            return match self.instantiate(&t.name, args, (t.line, t.col)) {
                Some(s) => self.struct_ty(s),
                None => Ty::Error,
            };
        }
        if matches!(t.name.as_str(), "fn" | "fn!" | "fn.np" | "fn!.np") {
            let mut tys: Vec<Ty> = t.args.iter().map(|a| if a.name == "()" { Ty::Unit } else { self.resolve_ty(a) }).collect();
            let ret = tys.pop().unwrap_or(Ty::Unit);
            if tys.iter().any(|a| *a == Ty::Error) || ret == Ty::Error {
                return Ty::Error;
            }
            return Ty::Fn(tys, Box::new(ret), t.name.starts_with("fn!"), t.name.ends_with(".np"));
        }
        if t.args.is_empty() {
            if let Some(i) = IntTy::from_name(&t.name) {
                return Ty::Int(i);
            }
            match t.name.as_str() {
                "bool" => return Ty::Bool,
                "f64" => return Ty::F64,
                "String" => return Ty::Str,
                "Duration" => return Ty::Dur,
                "Instant" => return Ty::Inst,
                "Ref" | "Array" => return bad(self, format!("`{}` needs a type argument", t.name)),
                _ => {}
            }
            return match self.struct_ids.get(&t.name) {
                Some(&s) => self.struct_ty(s),
                None => bad(self, format!("unknown type `{}`", t.name)),
            };
        }
        if t.args.len() != 1 {
            return bad(self, format!("`{}` takes one type argument", t.name));
        }
        let inner = self.resolve_ty(&t.args[0]);
        if inner == Ty::Error {
            return Ty::Error;
        }
        match t.name.as_str() {
            "Ref" => match inner {
                Ty::Struct(s) => Ty::Ref(s),
                // a reference to a reference is the reference (`Ref<T>` where T is itself a `Ref`, also an extern struct)
                Ty::Ref(s) => Ty::Ref(s),
                // inside a generic, `Ref<T>` with a scalar or handle T is a borrow of the value: the value
                other if !self.tmap.is_empty() && !matches!(other, Ty::Error) => other,
                other => bad(self, format!("slice 0: Ref<T> needs a struct type, found {}", self.prog.type_name(&other))),
            },
            "Array" => {
                if self.elem_ok(&inner) {
                    Ty::Arr(Box::new(inner))
                } else {
                    let n = self.prog.type_name(&inner);
                    bad(self, format!("slice 1: an array element must be a scalar, an optional or a plain value, not {}", n))
                }
            }
            _ => bad(self, format!("type `{}` takes no type arguments", t.name)),
        }
    }

    /// An `extern` struct is an object of the runtime and always lives behind a counted handle.
    fn struct_ty(&self, s: usize) -> Ty {
        if self.prog.structs[s].is_extern {
            Ty::Ref(s)
        } else {
            Ty::Struct(s)
        }
    }

    fn intern(&mut self, s: &[u8]) -> usize {
        if let Some(&i) = self.string_ids.get(s) {
            return i;
        }
        let i = self.prog.strings.len();
        self.prog.strings.push(s.to_vec());
        self.string_ids.insert(s.to_vec(), i);
        i
    }

    // ---------------- functions ----------------

    fn lower_fn(&mut self, fid: FnId, decl: &ast::FnDecl) {
        let (ret, owner, is_static, ptys) = {
            let f = &self.prog.fns[fid];
            (f.ret.clone(), f.owner, f.is_static, f.param_tys.clone())
        };
        let self_sid = if is_static { None } else { owner };
        let mut cx = FnCtx::new(ret.clone(), self_sid);
        cx.declared = self.prog.fns[fid].throws.clone();
        let mut params = Vec::new();
        for (p, ty) in decl.params.iter().zip(ptys.iter()) {
            if cx.scopes[0].contains_key(&p.name) {
                self.error(p.ty.line, p.ty.col, format!("parameter `{}` is declared twice", p.name));
                continue;
            }
            let id = cx.vars.len();
            cx.vars.push(VarInfo {
                name: p.name.clone(),
                ty: ty.clone(),
                mutable: false,
                converted: false,
                converted_line: 0,
                is_obj: false,
                line: decl.line,
                alias: None,
            });
            cx.scopes[0].insert(p.name.clone(), id);
            params.push(id);
        }
        let body = decl.body.as_ref().unwrap();
        let tb = self.lower_block_in_current_scope(&mut cx, body);
        if ret != Ty::Unit && !block_returns(&tb) {
            self.error(decl.line, decl.col, format!("function `{}` may end without returning a value", decl.name));
        }
        // notes on counted / scope-owned objects
        for v in &cx.vars {
            if v.is_obj {
                if v.converted {
                    self.prog.notes.push(format!(
                        "{}:{}: note: `{}` is counted because of line {}",
                        self.cur_file, v.line, v.name, v.converted_line
                    ));
                } else {
                    self.prog.notes.push(format!("{}:{}: note: `{}` is scope-owned (never converted to a Ref)", self.cur_file, v.line, v.name));
                }
            }
        }
        let f = &mut self.prog.fns[fid];
        f.params = params;
        f.vars = cx.vars;
        f.body = Some(tb);
    }

    fn lower_block(&mut self, cx: &mut FnCtx, b: &ast::Block) -> TBlock {
        cx.scopes.push(HashMap::new());
        let r = self.lower_block_in_current_scope(cx, b);
        cx.scopes.pop();
        r
    }

    fn lower_block_in_current_scope(&mut self, cx: &mut FnCtx, b: &ast::Block) -> TBlock {
        let mut stmts = Vec::new();
        for (i, s) in b.stmts.iter().enumerate() {
            // `var i = 0`: the integer type comes from what the variable is later combined with
            let hinted;
            let s = match &s.kind {
                StmtKind::Let { name, mutable, ty: None, init } if matches!(init.kind, ExprKind::Int(_)) => match self.int_hint(cx, name, &b.stmts[i + 1..]) {
                    Some(t) => {
                        let te = ast::TypeExpr { name: t.name().to_string(), args: vec![], optional: false, fixed: None, line: s.line, col: s.col };
                        hinted = ast::Stmt { kind: StmtKind::Let { name: name.clone(), mutable: *mutable, ty: Some(te), init: init.clone() }, line: s.line, col: s.col };
                        &hinted
                    }
                    None => s,
                },
                _ => s,
            };
            if let Some(t) = self.lower_stmt(cx, s) {
                stmts.push(t);
            }
        }
        TBlock { stmts }
    }

    /// The first integer type that `name` is compared, combined or assigned with in `rest`, when the other side can be typed here.
    fn int_hint(&mut self, cx: &mut FnCtx, name: &str, rest: &[ast::Stmt]) -> Option<IntTy> {
        let mut cands: Vec<Option<&Expr>> = Vec::new();
        for s in rest {
            hint_stmt(s, name, &mut cands);
        }
        for e in cands {
            let Some(e) = e else {
                // `return x`: the function's return type
                if let Ty::Int(it) = cx.ret {
                    return Some(it);
                }
                continue;
            };
            let n = self.diags.len();
            let t = self.lower_expr(cx, e).ty;
            self.diags.truncate(n);
            if let Ty::Int(it) = t {
                return Some(it);
            }
        }
        None
    }

    fn new_var(&mut self, cx: &mut FnCtx, name: &str, ty: Ty, mutable: bool, is_obj: bool, at: (u32, u32)) -> Option<VarId> {
        if cx.scopes.last().unwrap().contains_key(name) {
            self.error(at.0, at.1, format!("`{}` is already declared in this scope", name));
            return None;
        }
        let id = cx.vars.len();
        cx.vars.push(VarInfo { name: name.to_string(), ty, mutable, converted: false, converted_line: 0, is_obj, line: at.0, alias: None });
        cx.scopes.last_mut().unwrap().insert(name.to_string(), id);
        Some(id)
    }

    fn lower_stmt(&mut self, cx: &mut FnCtx, s: &ast::Stmt) -> Option<TStmt> {
        let line = s.line;
        let mk = |kind| Some(TStmt { kind, line });
        match &s.kind {
            StmtKind::Let { name, mutable, ty, init } => {
                match ty {
                    Some(te) => {
                        let t = self.resolve_ty(te);
                        let v = self.lower_expecting(cx, init, &t);
                        let v = self.coerce(cx, v, &t, (init.line, init.col));
                        let id = self.new_var(cx, name, t, *mutable, false, (s.line, s.col))?;
                        mk(TS::Let(id, v))
                    }
                    None => {
                        // `let a = new T {..}`: counted or scope-owned, decided from the uses of `a`
                        if let ExprKind::New(tn, fields) = &init.kind {
                            if let Some(sid) = self.struct_for(cx, tn, fields) {
                                let fs = self.lower_fields(cx, sid, fields, init.line, init.col);
                                let id = self.new_var(cx, name, Ty::Obj(sid), *mutable, true, (s.line, s.col))?;
                                return mk(TS::LetObj(id, sid, fs));
                            }
                        }
                        let v = self.lower_expr(cx, init);
                        let v = self.decay(cx, v, (init.line, init.col));
                        if v.ty == Ty::Null {
                            self.error(init.line, init.col, "`null` needs a type: write `let x: T? = null`");
                            return None;
                        }
                        let t = match &v.ty {
                            Ty::Int(_) => v.ty.clone(),
                            Ty::Unit => {
                                self.error(init.line, init.col, "this expression has no value");
                                Ty::Error
                            }
                            t => t.clone(),
                        };
                        let v = if v.lit { self.finish_lit(v) } else { v };
                        let t = if v.lit { v.ty.clone() } else { t };
                        self.check_copyable(&v, (init.line, init.col));
                        let id = self.new_var(cx, name, t, *mutable, false, (s.line, s.col))?;
                        mk(TS::Let(id, v))
                    }
                }
            }
            StmtKind::Assign { target, op, value } => {
                let t = self.lower_expr(cx, target);
                if t.ty == Ty::Error {
                    self.lower_expr(cx, value);
                    return None;
                }
                if matches!(t.ty, Ty::Obj(_)) {
                    self.error(target.line, target.col, "slice 0: a variable bound to `new` cannot be reassigned; declare it with an explicit Ref<T>");
                    return None;
                }
                if let Err(m) = self.assignable(cx, &t) {
                    self.error(target.line, target.col, m);
                    return None;
                }
                let v = self.lower_expr(cx, value);
                let v = self.coerce(cx, v, &t.ty, (value.line, value.col));
                match op {
                    None => mk(TS::Assign(t, v)),
                    Some(op) => {
                        if !matches!(t.ty, Ty::Int(_) | Ty::F64) {
                            self.error(target.line, target.col, format!("`{}` cannot be used with {}", op_name(*op), self.prog.type_name(&t.ty)));
                            return None;
                        }
                        mk(TS::AssignOp(*op, t, v))
                    }
                }
            }
            StmtKind::Expr(e) => {
                let v = self.lower_expr(cx, e);
                let v = if v.lit { self.finish_lit(v) } else { v };
                mk(TS::Expr(v))
            }
            StmtKind::If { cond, then, els } => {
                let c = self.lower_expr(cx, cond);
                let c = self.coerce(cx, c, &Ty::Bool, (cond.line, cond.col));
                let t = self.lower_block(cx, then);
                let e = els.as_ref().map(|b| self.lower_block(cx, b));
                mk(TS::If(c, t, e))
            }
            StmtKind::While { cond, body } => {
                let c = self.lower_expr(cx, cond);
                let c = self.coerce(cx, c, &Ty::Bool, (cond.line, cond.col));
                cx.loops += 1;
                cx.loop_labels.push(self.pending_label.take());
                let b = self.lower_block(cx, body);
                cx.loop_labels.pop();
                cx.loops -= 1;
                mk(TS::While(c, b))
            }
            StmtKind::For { var, iter, body } => {
                let line = s.line;
                let col = s.col;
                let ex = |kind: ExprKind| Expr { kind, line, col };
                let st = |kind: StmtKind| ast::Stmt { kind, line, col };
                let id = |n: &str| ex(ExprKind::Ident(n.to_string()));
                let field = |n: &str, f: &str| ex(ExprKind::Field(Box::new(id(n)), f.to_string()));
                self.for_count += 1;
                let (r, i) = (format!("_for{}", self.for_count), format!("_for{}i", self.for_count));
                let ity = self.probe(cx, iter);
                let is_range = matches!(&ity, Ty::Struct(sid) if self.prog.structs[*sid].name == "Range");
                let is_arr = matches!(ity, Ty::Arr(_));
                if !is_range && !is_arr {
                    if ity != Ty::Error {
                        let n = self.prog.type_name(&ity);
                        self.error(iter.line, iter.col, format!("cannot loop over {}; use a Range or an Array", n));
                    }
                    return None;
                }
                let usize_ty = TypeExpr { name: "usize".into(), args: vec![], optional: false, fixed: None, line, col };
                let (start, limit, elem) = if is_range {
                    (field(&r, "lo"), field(&r, "hi"), id(&i))
                } else {
                    (
                        ex(ExprKind::Cast(Box::new(ex(ExprKind::Int(0))), usize_ty)),
                        ex(ExprKind::Call(Box::new(field(&r, "length")), vec![])),
                        ex(ExprKind::Index(Box::new(id(&r)), Box::new(id(&i)))),
                    )
                };
                // the element is read and the counter moves on before the body runs, so `continue` is safe
                let inner = vec![
                    st(StmtKind::Let { name: var.clone(), mutable: false, ty: None, init: elem }),
                    st(StmtKind::Assign { target: id(&i), op: Some(BinOp::Add), value: ex(ExprKind::Int(1)) }),
                ];
                let mut stmts = inner;
                stmts.extend(body.stmts.iter().cloned());
                let w = st(StmtKind::While {
                    cond: ex(ExprKind::Binary(BinOp::Lt, Box::new(id(&i)), Box::new(limit))),
                    body: ast::Block { stmts, line },
                });
                let blk = ast::Block {
                    stmts: vec![
                        st(StmtKind::Let { name: r.clone(), mutable: false, ty: None, init: iter.clone() }),
                        st(StmtKind::Let { name: i.clone(), mutable: true, ty: None, init: start }),
                        w,
                    ],
                    line,
                };
                return self.lower_stmt(cx, &st(StmtKind::Block(blk)));
            }
            StmtKind::Loop(body) => {
                cx.loops += 1;
                cx.loop_labels.push(self.pending_label.take());
                let b = self.lower_block(cx, body);
                cx.loop_labels.pop();
                cx.loops -= 1;
                mk(TS::Loop(b))
            }
            StmtKind::Labeled(l, inner) => {
                self.pending_label = Some(l.clone());
                let r = self.lower_stmt(cx, inner);
                self.pending_label = None;
                let r = r?;
                mk(TS::Labeled(Box::new(r)))
            }
            StmtKind::BreakTo(l) | StmtKind::ContinueTo(l) => {
                match cx.loop_labels.iter().rposition(|x| x.as_deref() == Some(l.as_str())) {
                    Some(level) => mk(TS::BreakL(level, matches!(s.kind, StmtKind::ContinueTo(_)))),
                    None => {
                        self.error(s.line, s.col, format!("no enclosing loop is labeled `{}`", l));
                        None
                    }
                }
            }
            StmtKind::Break | StmtKind::Continue => {
                if cx.loops == 0 {
                    self.error(s.line, s.col, "`break` and `continue` are only allowed inside a loop");
                    return None;
                }
                if matches!(s.kind, StmtKind::Break) {
                    mk(TS::Break)
                } else {
                    mk(TS::Continue)
                }
            }
            StmtKind::Return(v) => match v {
                None => {
                    if cx.ret != Ty::Unit {
                        self.error(s.line, s.col, format!("this function returns {}; `return` needs a value", self.prog.type_name(&cx.ret)));
                        return None;
                    }
                    mk(TS::Return(None))
                }
                Some(e) => {
                    if cx.ret == Ty::Unit {
                        self.error(e.line, e.col, "this function returns nothing, so `return` takes no value");
                        return None;
                    }
                    let ret = cx.ret.clone();
                    let v = self.lower_expecting(cx, e, &ret);
                    let v = self.coerce(cx, v, &ret, (e.line, e.col));
                    mk(TS::Return(Some(v)))
                }
            },
            StmtKind::Block(b) => {
                let b = self.lower_block(cx, b);
                mk(TS::Block(b))
            }
            StmtKind::Sync { targets, body } => {
                let mut vs: Vec<(bool, TExpr)> = Vec::new();
                let mut ok = true;
                for (write, target) in targets {
                    let kw = if *write { "writes" } else { "reads" };
                    let v = self.lower_expr(cx, target);
                    let v = self.decay(cx, v, (target.line, target.col));
                    match &v.ty {
                        Ty::Error => ok = false,
                        Ty::Ref(sid) if self.prog.structs[*sid].is_extern => {
                            let n = self.prog.structs[*sid].name.clone();
                            self.error(target.line, target.col, format!("`{}` is an extern type: it synchronises itself and needs no sync block", n));
                            ok = false;
                        }
                        Ty::Ref(_) | Ty::Arr(_) => {
                            // the same object under both reads and writes is a mistake, not a request for the stronger lock
                            if let Some((w0, _)) = vs.iter().find(|(w0, t0)| *w0 != *write && same_place(t0, &v)) {
                                let (a, b) = if *w0 { ("writes", "reads") } else { ("reads", "writes") };
                                self.error(target.line, target.col, format!("this object is listed under both {} and {}", a, b));
                                ok = false;
                            }
                            vs.push((*write, v));
                        }
                        other => {
                            let n = self.prog.type_name(other);
                            self.error(target.line, target.col, format!("{} needs a Ref<T> or an Array<T> to lock, found {}", kw, n));
                            ok = false;
                        }
                    }
                }
                let b = self.lower_block(cx, body);
                if ok { mk(TS::Sync(vs, b)) } else { mk(TS::Block(b)) }
            }
            StmtKind::Switch { subject, arms } => self.lower_switch(cx, s, subject, arms),
            StmtKind::Throw(e) => {
                let ExprKind::Call(callee, args) = &e.kind else {
                    self.error(e.line, e.col, "`throw` needs a new exception: `throw Name(args)`");
                    return None;
                };
                let ExprKind::Ident(name) = &callee.kind else {
                    self.error(e.line, e.col, "`throw` needs a new exception: `throw Name(args)`");
                    return None;
                };
                let Some(&sid) = self.struct_ids.get(name).filter(|s| self.prog.structs[**s].is_exception) else {
                    self.error(callee.line, callee.col, format!("`{}` is not an exception", name));
                    return None;
                };
                let nf = self.prog.structs[sid].fields.len();
                if args.len() != nf {
                    self.error(e.line, e.col, format!("exception `{}` takes {} value(s), found {}", name, nf, args.len()));
                    return None;
                }
                let mut fs = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let fty = self.prog.structs[sid].fields[i].ty.clone();
                    let v = self.lower_expr(cx, a);
                    let v = self.coerce(cx, v, &fty, (a.line, a.col));
                    fs.push((i, v));
                }
                self.check_throw(cx, sid, (s.line, s.col));
                mk(TS::Throw(sid, fs))
            }
            StmtKind::Try { body, catches } => {
                let mut kinds = Vec::new();
                for c in catches {
                    if c.ty.name == "Exception" {
                        kinds.push(ANY_EXC);
                    } else if let Some(&sid) = self.struct_ids.get(&c.ty.name).filter(|s| self.prog.structs[**s].is_exception) {
                        kinds.push(sid);
                    } else {
                        self.error(c.ty.line, c.ty.col, format!("`{}` is not an exception", c.ty.name));
                        kinds.push(ANY_EXC);
                    }
                }
                cx.catching.push(kinds.clone());
                let tb = self.lower_block(cx, body);
                cx.catching.pop();
                let mut tcs = Vec::new();
                for (c, k) in catches.iter().zip(kinds.iter()) {
                    cx.scopes.push(HashMap::new());
                    let vty = if *k == ANY_EXC { Ty::Exc } else { Ty::Ref(*k) };
                    let var = self.new_var(cx, &c.name, vty, false, false, (c.line, c.col));
                    let cb = self.lower_block_in_current_scope(cx, &c.body);
                    cx.scopes.pop();
                    if let Some(var) = var {
                        tcs.push(TCatch { exc: *k, var, body: cb });
                    }
                }
                mk(TS::Try(tb, tcs))
            }
        }
    }

    /// `switch` becomes a block: a copy of the subject, then an if-chain on its tag (enum) or value (integer).
    fn lower_switch(&mut self, cx: &mut FnCtx, s: &ast::Stmt, subject: &Expr, arms: &[ast::Arm]) -> Option<TStmt> {
        let line = s.line;
        let subj = self.lower_expr(cx, subject);
        let subj = self.decay(cx, subj, (subject.line, subject.col));
        if subj.ty == Ty::Error {
            return None;
        }
        let subj = if subj.lit { self.finish_lit(subj) } else { subj };
        // a `Ref` to an enum is switched on through the handle
        let subj = match &subj.ty {
            Ty::Ref(sid) if self.prog.is_enum(&Ty::Struct(*sid)).is_some() => {
                let l = subj.line;
                let sid = *sid;
                typed(EK::Deref(Box::new(subj)), Ty::Struct(sid), l)
            }
            _ => subj,
        };
        let sty = subj.ty.clone();
        let enum_sid = self.prog.is_enum(&sty);
        if enum_sid.is_none() && !matches!(sty, Ty::Int(_)) {
            let n = self.prog.type_name(&sty);
            self.error(subject.line, subject.col, format!("slice 1: `switch` works on an enum or an integer, not {}", n));
            return None;
        }
        self.check_copyable(&subj, (subject.line, subject.col));
        cx.scopes.push(HashMap::new());
        let tmp = self.new_var(cx, "$switch", sty.clone(), false, false, (s.line, s.col));
        let Some(tmp) = tmp else {
            cx.scopes.pop();
            return None;
        };
        let nvariants = enum_sid.map(|e| self.prog.structs[e].variants.len()).unwrap_or(0);
        let mut covered = vec![false; nvariants];
        let mut default_body: Option<TBlock> = None;
        let mut guarded: Vec<(TExpr, TBlock)> = Vec::new();
        // a subject that is a plain place (a variable, `self`, a field, a handle's target) is switched on in place:
        // no copy, and a binding that the arm assigns to or calls a method on names the payload itself
        let place = if enum_sid.is_some() && pure_place(&subj) { Some(subj.clone()) } else { None };
        let var = |ty: &Ty| match &place {
            Some(p) => p.clone(),
            None => typed(EK::Var(tmp), ty.clone(), line),
        };
        for arm in arms {
            cx.scopes.push(HashMap::new());
            let mut conds: Vec<TExpr> = Vec::new();
            let mut binds: Vec<(String, usize, Ty, (u32, u32))> = Vec::new();
            let mut is_default = false;
            let multi = arm.pats.len() > 1;
            for p in &arm.pats {
                match &p.kind {
                    ast::PatKind::Default => is_default = true,
                    ast::PatKind::Variant { qual, name, binds: bs } => {
                        let Some(esid) = enum_sid else {
                            self.error(p.line, p.col, format!("`{}` is not a value of an integer; use a literal", name));
                            continue;
                        };
                        if let Some(q) = qual {
                            if *q != self.prog.structs[esid].name {
                                let en = self.prog.structs[esid].name.clone();
                                self.error(p.line, p.col, format!("`{}.{}` is not a variant of `{}`", q, name, en));
                                continue;
                            }
                        }
                        let Some(vi) = self.prog.structs[esid].variants.iter().position(|v| v.name == *name) else {
                            let en = self.prog.structs[esid].name.clone();
                            self.error(p.line, p.col, format!("enum `{}` has no variant `{}`", en, name));
                            continue;
                        };
                        if covered[vi] {
                            self.error(p.line, p.col, format!("variant `{}` is matched twice", name));
                        }
                        covered[vi] = true;
                        let fields = self.prog.structs[esid].variants[vi].fields.clone();
                        if let Some(bs) = bs {
                            if bs.len() != fields.len() {
                                self.error(p.line, p.col, format!("variant `{}` has {} field(s), the pattern names {}", name, fields.len(), bs.len()));
                                continue;
                            }
                            if multi && bs.iter().any(|b| b != "_") {
                                self.error(p.line, p.col, "a pattern list cannot bind names; give each variant its own arm");
                                continue;
                            }
                            for (b, f) in bs.iter().zip(fields.iter()) {
                                if b != "_" {
                                    let fty = self.prog.structs[esid].fields[*f].ty.clone();
                                    binds.push((b.clone(), *f, fty, (p.line, p.col)));
                                }
                            }
                        }
                        let tag = typed(EK::Field(Box::new(var(&sty)), 0), Ty::Int(IntTy::I32), line);
                        let k = typed(EK::Int(vi as i128), Ty::Int(IntTy::I32), line);
                        conds.push(typed(EK::Binary(BinOp::Eq, Box::new(tag), Box::new(k)), Ty::Bool, line));
                    }
                    ast::PatKind::Lit(e) => {
                        if enum_sid.is_some() {
                            self.error(p.line, p.col, "an enum is matched by variant names");
                            continue;
                        }
                        let v = self.lower_expr(cx, e);
                        let v = self.coerce(cx, v, &sty, (p.line, p.col));
                        conds.push(typed(EK::Binary(BinOp::Eq, Box::new(var(&sty)), Box::new(v)), Ty::Bool, line));
                    }
                }
            }
            let mut pre: Vec<TStmt> = Vec::new();
            for (name, f, fty, at) in binds {
                let sid = enum_sid.unwrap();
                let in_place = place.is_some() && block_mutates(&arm.body, &name);
                if let Some(id) = self.new_var(cx, &name, fty.clone(), false, false, at) {
                    let fe = typed(EK::Field(Box::new(var(&sty)), f), fty, line);
                    let _ = sid;
                    if in_place {
                        cx.vars[id].alias = Some(fe);
                    } else {
                        pre.push(TStmt { kind: TS::Let(id, fe), line });
                    }
                }
            }
            let mut body = self.lower_block_in_current_scope(cx, &arm.body);
            pre.append(&mut body.stmts);
            let body = TBlock { stmts: pre };
            cx.scopes.pop();
            if is_default {
                if default_body.is_some() {
                    self.error(arm.pats[0].line, arm.pats[0].col, "`default` appears twice");
                }
                default_body = Some(body);
                continue;
            }
            if let Some(first) = conds.into_iter().reduce(|a, b| typed(EK::Binary(BinOp::Or, Box::new(a), Box::new(b)), Ty::Bool, line)) {
                guarded.push((first, body));
            }
        }
        cx.scopes.pop();
        let exhaustive = enum_sid.is_some() && covered.iter().all(|c| *c);
        if default_body.is_none() && !exhaustive {
            let msg = match enum_sid {
                Some(esid) => {
                    let missing: Vec<String> =
                        (0..nvariants).filter(|i| !covered[*i]).map(|i| self.prog.structs[esid].variants[i].name.clone()).collect();
                    format!("`switch` does not cover {}; add the variant(s) or a `default` arm", missing.join(", "))
                }
                None => "`switch` on an integer needs a `default` arm".to_string(),
            };
            self.error(s.line, s.col, msg);
            return None;
        }
        // the chain, built from the last arm backwards; with no default the last arm needs no test
        let mut acc: Option<TBlock> = default_body;
        for (cond, body) in guarded.into_iter().rev() {
            acc = Some(match acc {
                None => body,
                Some(e) => TBlock { stmts: vec![TStmt { kind: TS::If(cond, body, Some(e)), line }] },
            });
        }
        let mut stmts = if place.is_some() { vec![] } else { vec![TStmt { kind: TS::Let(tmp, subj), line }] };
        if let Some(chain) = acc {
            stmts.push(TStmt { kind: TS::Block(chain), line });
        }
        Some(TStmt { kind: TS::Block(TBlock { stmts }), line })
    }

    fn assignable(&self, cx: &FnCtx, te: &TExpr) -> Result<(), String> {
        match &te.kind {
            EK::Var(id) => {
                if cx.vars[*id].mutable {
                    Ok(())
                } else {
                    Err(format!("`{}` is not mutable; declare it with `var`", cx.vars[*id].name))
                }
            }
            EK::Field(b, _) => self.assignable(cx, b),
            EK::Global(g) => Err(format!("`{}` is a top-level `let`; it cannot be assigned", self.prog.globals[*g].name)),
            EK::Deref(_) | EK::ObjPlace(_) | EK::SelfPlace | EK::Index(..) => Ok(()),
            _ => Err("this expression cannot be assigned to".into()),
        }
    }

    // ---------------- conversions ----------------

    fn is_place(te: &TExpr) -> bool {
        matches!(te.kind, EK::Var(_) | EK::Global(_) | EK::SelfPlace | EK::Field(..) | EK::Deref(_) | EK::ObjPlace(_) | EK::Index(..))
    }

    fn check_copyable(&mut self, te: &TExpr, at: (u32, u32)) {
        if let Ty::Struct(s) = te.ty {
            if self.prog.structs[s].no_copy && Self::is_place(te) {
                let n = self.prog.structs[s].name.clone();
                self.error(at.0, at.1, format!("cannot copy a value of type {}: it has a cleanup() that would run twice; use Ref<{}>", n, n));
            }
        }
    }

    fn mark_converted(&mut self, cx: &mut FnCtx, id: VarId, line: u32) {
        if !cx.vars[id].converted {
            cx.vars[id].converted = true;
            cx.vars[id].converted_line = line;
        }
    }

    /// Obj variables used with no type to guide them become `Ref<T>` (a conversion).
    fn decay(&mut self, cx: &mut FnCtx, te: TExpr, at: (u32, u32)) -> TExpr {
        if let (Ty::Obj(s), EK::Var(id)) = (&te.ty, &te.kind) {
            let (s, id) = (*s, *id);
            self.mark_converted(cx, id, at.0);
            return typed(EK::ObjRef(id), Ty::Ref(s), te.line);
        }
        te
    }

    fn finish_lit(&mut self, te: TExpr) -> TExpr {
        // an untyped literal that never met a typed operand: i64 or f64, already its type
        TExpr { lit: false, ..te }
    }

    fn retype_lit(&mut self, te: TExpr, ty: &Ty, at: (u32, u32)) -> TExpr {
        let line = te.line;
        match (te.kind, ty) {
            (EK::Int(v), Ty::Int(it)) => {
                let (lo, hi) = it.range();
                if v < lo || v > hi {
                    self.error(at.0, at.1, format!("value {} is out of range for {}", v, it.name()));
                }
                TExpr { kind: EK::Int(v), ty: ty.clone(), line, lit: false }
            }
            (EK::Int(v), Ty::F64) => TExpr { kind: EK::Float(v as f64), ty: Ty::F64, line, lit: false },
            (EK::Float(v), Ty::F64) => TExpr { kind: EK::Float(v), ty: Ty::F64, line, lit: false },
            (EK::Binary(op, a, b), _) => {
                let a = self.retype_lit(*a, ty, at);
                let b = self.retype_lit(*b, ty, at);
                TExpr { kind: EK::Binary(op, Box::new(a), Box::new(b)), ty: ty.clone(), line, lit: false }
            }
            (EK::Unary(op, a), _) => {
                let a = self.retype_lit(*a, ty, at);
                TExpr { kind: EK::Unary(op, Box::new(a)), ty: ty.clone(), line, lit: false }
            }
            (k, _) => TExpr { kind: k, ty: ty.clone(), line, lit: false },
        }
    }

    fn mismatch(&mut self, got: &Ty, want: &Ty, at: (u32, u32), extra: &str) {
        let (g, w) = (self.prog.type_name(got), self.prog.type_name(want));
        self.error(at.0, at.1, format!("expected {}, found {}{}", w, g, extra));
    }

    fn coerce(&mut self, cx: &mut FnCtx, te: TExpr, target: &Ty, at: (u32, u32)) -> TExpr {
        if te.ty == Ty::Error || *target == Ty::Error {
            return te;
        }
        // the effect in a function type: a `fn[nopark]` value goes where a plain function is wanted; a plain one only if it is a function (or a
        // lambda) that is checked not to park, once the effects are known
        if let (Ty::Fn(p1, r1, t1, n1), Ty::Fn(p2, r2, _, n2)) = (&te.ty, target) {
            if p1 == p2 && r1 == r2 && n1 != n2 {
                let line = te.line;
                let retag = Ty::Fn(p1.clone(), r1.clone(), *t1, *n2);
                if *n2 {
                    match &te.kind {
                        EK::FnRef(fid, _) | EK::Lambda(fid, _) => self.nopark_uses.push((*fid, at.0, at.1)),
                        _ => {
                            self.error(at.0, at.1, format!("a function value that may park cannot be given where {} is wanted", self.prog.type_name(target)));
                            return te;
                        }
                    }
                }
                let mut te = te;
                te.ty = retag;
                let _ = line;
                return self.coerce(cx, te, target, at);
            }
        }
        // a function that does not throw, given where a throwing function is wanted: it is called through a thunk
        if let (EK::FnRef(fid, false), Ty::Fn(p1, r1, false, n1), Ty::Fn(p2, r2, true, n2)) = (&te.kind, &te.ty, target) {
            if p1 == p2 && r1 == r2 && n1 == n2 {
                let fid = *fid;
                return typed(EK::FnRef(fid, true), target.clone(), te.line);
            }
        }
        if te.ty == *target && !te.lit {
            self.check_copyable(&te, at);
            return te;
        }
        // a string literal given where bytes are wanted: its bytes, as a fresh array
        if let (EK::StrLit(_), Ty::Arr(el)) = (&te.kind, target) {
            if **el == Ty::Int(IntTy::U8) {
                let line = te.line;
                return typed(EK::RtCall("toBytes", vec![te]), target.clone(), line);
            }
        }
        // optionals: null, a present value, a checked unwrap
        if te.ty == Ty::Null {
            if let Ty::Opt(_) = target {
                return typed(EK::Null, target.clone(), te.line);
            }
        }
        if let Ty::Opt(inner) = target {
            if te.ty != Ty::Null && !matches!(te.ty, Ty::Opt(_)) {
                let line = te.line;
                let v = self.coerce(cx, te, inner, at);
                return typed(EK::Some(Box::new(v)), target.clone(), line);
            }
        }
        if let Ty::Opt(_) = te.ty {
            if !matches!(target, Ty::Opt(_)) {
                let u = self.unwrap_opt(te);
                return self.coerce(cx, u, target, at);
            }
        }
        if te.lit {
            let ok = match (&te.ty, target) {
                (Ty::Int(_), Ty::Int(_)) | (Ty::Int(_), Ty::F64) | (Ty::F64, Ty::F64) => true,
                _ => false,
            };
            if ok {
                return self.retype_lit(te, target, at);
            }
        }
        if let (Ty::Int(f), Ty::Int(t)) = (&te.ty, target) {
            if Self::widens(*f, *t) {
                let line = te.line;
                return typed(EK::Cast(Box::new(te)), target.clone(), line);
            }
        }
        match (&te.ty, target, &te.kind) {
            (Ty::Obj(s), Ty::Ref(t), EK::Var(id)) if s == t => {
                let id = *id;
                self.mark_converted(cx, id, at.0);
                typed(EK::ObjRef(id), Ty::Ref(*s), te.line)
            }
            (Ty::Obj(s), Ty::Struct(t), EK::Var(id)) if s == t => {
                let r = typed(EK::ObjPlace(*id), Ty::Struct(*s), te.line);
                self.check_copyable(&r, at);
                r
            }
            (Ty::Ref(s), Ty::Struct(t), _) if s == t => {
                let r = typed(EK::Deref(Box::new(te.clone())), Ty::Struct(*s), te.line);
                self.check_copyable(&r, at);
                r
            }
            (Ty::Struct(s), Ty::Ref(t), _) if s == t => {
                self.mismatch(&te.ty, target, at, "; write `new` to allocate a counted object");
                error_expr(te.line)
            }
            _ => {
                let (g, t2) = (te.ty.clone(), target.clone());
                self.mismatch(&g, &t2, at, "");
                error_expr(te.line)
            }
        }
    }

    // ---------------- expressions ----------------

    fn lower_expr(&mut self, cx: &mut FnCtx, e: &Expr) -> TExpr {
        let exp = self.expected.take();
        let saved = std::mem::replace(&mut self.exp_here, exp);
        let r = self.lower_expr_inner(cx, e);
        self.exp_here = saved;
        r
    }

    /// Lowers `e` knowing the type its context wants (used to infer generic arguments).
    fn lower_expecting(&mut self, cx: &mut FnCtx, e: &Expr, want: &Ty) -> TExpr {
        self.expected = Some(want.clone());
        let r = self.lower_expr(cx, e);
        self.expected = None;
        r
    }

    fn lower_expr_inner(&mut self, cx: &mut FnCtx, e: &Expr) -> TExpr {
        let line = e.line;
        match &e.kind {
            ExprKind::Int(v) => lit(EK::Int(*v as i128), Ty::Int(IntTy::I64), line),
            ExprKind::Float(v) => lit(EK::Float(*v), Ty::F64, line),
            ExprKind::Str(s) => {
                let i = self.intern(s);
                typed(EK::StrLit(i), Ty::Str, line)
            }
            ExprKind::Char(c) => typed(EK::Int(*c as i128), Ty::Int(IntTy::U8), line),
            ExprKind::Bool(b) => typed(EK::Bool(*b), Ty::Bool, line),
            ExprKind::Ident(name) => self.lower_ident(cx, e, name),
            ExprKind::Field(base, name) => self.lower_field(cx, e, base, name),
            ExprKind::Call(callee, args) => {
                let r = self.lower_call(cx, e, callee, args);
                if let EK::Call(fid, _) | EK::Method(fid, _, _) = &r.kind {
                    for x in self.prog.fns[*fid].throws.clone() {
                        self.check_throw(cx, x, (e.line, e.col));
                    }
                }
                r
            }
            ExprKind::TypeName(te) => {
                self.error(te.line, te.col, "a type is not a value; call one of its functions, as in `Type<A>.make(..)`");
                error_expr(line)
            }
            ExprKind::Cond(c, a, b) => {
                let cv = self.lower_expr(cx, c);
                let cv = self.coerce(cx, cv, &Ty::Bool, (c.line, c.col));
                let av = self.lower_expr(cx, a);
                let bv = self.lower_expr(cx, b);
                let av = self.decay(cx, av, (a.line, a.col));
                let bv = self.decay(cx, bv, (b.line, b.col));
                if av.ty == Ty::Error || bv.ty == Ty::Error {
                    return error_expr(line);
                }
                let ty = match (&av.ty, &bv.ty) {
                    (Ty::Null, Ty::Null) => {
                        self.error(e.line, e.col, "both branches are `null`; give one of them a type");
                        return error_expr(line);
                    }
                    (Ty::Null, Ty::Opt(_)) => bv.ty.clone(),
                    (Ty::Null, t) => Ty::Opt(Box::new(t.clone())),
                    (Ty::Opt(_), Ty::Null) => av.ty.clone(),
                    (t, Ty::Null) => Ty::Opt(Box::new(t.clone())),
                    (Ty::Opt(_), _) => av.ty.clone(),
                    (_, Ty::Opt(_)) => bv.ty.clone(),
                    _ if av.lit && !bv.lit => bv.ty.clone(),
                    _ => av.ty.clone(),
                };
                if ty == Ty::Unit {
                    self.error(e.line, e.col, "the branches of `?:` need a value");
                    return error_expr(line);
                }
                let av = self.coerce(cx, av, &ty, (a.line, a.col));
                let bv = self.coerce(cx, bv, &ty, (b.line, b.col));
                typed(EK::Cond(Box::new(cv), Box::new(av), Box::new(bv)), ty, line)
            }
            ExprKind::Slice(b, lo, hi) => {
                let base = self.lower_expr(cx, b);
                let base = self.unwrap_opt(base);
                if base.ty == Ty::Error {
                    return base;
                }
                if !matches!(base.ty, Ty::Arr(_)) {
                    let n = self.prog.type_name(&base.ty);
                    self.error(e.line, e.col, format!("only an array can be sliced, not {}", n));
                    return error_expr(line);
                }
                let bound = |s: &mut Self, cx: &mut FnCtx, x: &Option<Box<Expr>>| -> Option<Box<TExpr>> {
                    let x = x.as_ref()?;
                    let v = s.lower_expr(cx, x);
                    let v = s.unwrap_opt(v);
                    Some(Box::new(s.coerce(cx, v, &Ty::Int(IntTy::Usize), (x.line, x.col))))
                };
                let l = bound(self, cx, lo);
                let h = bound(self, cx, hi);
                let ty = base.ty.clone();
                typed(EK::Slice(Box::new(base), l, h), ty, line)
            }
            ExprKind::Index(b, i) => {
                let base = self.lower_expr(cx, b);
                let base = self.unwrap_opt(base);
                let idx = self.lower_expr(cx, i);
                let idx = self.unwrap_opt(idx);
                let idx = self.decay(cx, idx, (i.line, i.col));
                if base.ty == Ty::Error || idx.ty == Ty::Error {
                    return error_expr(line);
                }
                let idx = match &idx.ty {
                    Ty::Int(_) => {
                        if idx.lit {
                            self.retype_lit(idx, &Ty::Int(IntTy::Usize), (i.line, i.col))
                        } else {
                            idx
                        }
                    }
                    t => {
                        let t = t.clone();
                        self.mismatch(&t, &Ty::Int(IntTy::Usize), (i.line, i.col), "");
                        return error_expr(line);
                    }
                };
                match &base.ty {
                    Ty::Arr(el) => {
                        let el = (**el).clone();
                        typed(EK::Index(Box::new(base), Box::new(idx)), el, line)
                    }
                    Ty::Str => typed(EK::Index(Box::new(base), Box::new(idx)), Ty::Int(IntTy::U8), line),
                    t => {
                        let n = self.prog.type_name(t);
                        self.error(e.line, e.col, format!("cannot index a value of type {}", n));
                        error_expr(line)
                    }
                }
            }
            ExprKind::Unary(op, x) => {
                let v = self.lower_expr(cx, x);
                let v = self.decay(cx, v, (x.line, x.col));
                if v.ty == Ty::Error {
                    return v;
                }
                match op {
                    UnOp::Not => {
                        let v = self.coerce(cx, v, &Ty::Bool, (x.line, x.col));
                        typed(EK::Unary(UnOp::Not, Box::new(v)), Ty::Bool, line)
                    }
                    UnOp::Neg => match v.ty {
                        Ty::Int(it) => {
                            if v.lit {
                                if let EK::Int(n) = v.kind {
                                    return lit(EK::Int(-n), v.ty, line);
                                }
                            }
                            if matches!(it, IntTy::U8 | IntTy::U16 | IntTy::U32 | IntTy::U64 | IntTy::Usize) {
                                self.error(e.line, e.col, "cannot negate an unsigned integer");
                                return error_expr(line);
                            }
                            let l = v.lit;
                            TExpr { kind: EK::Unary(UnOp::Neg, Box::new(v)), ty: Ty::Int(it), line, lit: l }
                        }
                        Ty::F64 => {
                            let l = v.lit;
                            TExpr { kind: EK::Unary(UnOp::Neg, Box::new(v)), ty: Ty::F64, line, lit: l }
                        }
                        ref t => {
                            let n = self.prog.type_name(t);
                            self.error(e.line, e.col, format!("cannot negate a value of type {}", n));
                            error_expr(line)
                        }
                    },
                }
            }
            ExprKind::Binary(op, a, b) => self.lower_binary(cx, e, *op, a, b),
            ExprKind::StructLit(name, fields) => {
                let Some(sid) = self.struct_for(cx, name, fields) else {
                    self.error(e.line, e.col, format!("unknown struct `{}`", name.name));
                    return error_expr(line);
                };
                let fs = self.lower_fields(cx, sid, fields, e.line, e.col);
                typed(EK::StructLit(sid, fs), Ty::Struct(sid), line)
            }
            ExprKind::New(name, fields) => {
                let Some(sid) = self.struct_for(cx, name, fields) else {
                    self.error(e.line, e.col, format!("unknown struct `{}`", name.name));
                    return error_expr(line);
                };
                let fs = self.lower_fields(cx, sid, fields, e.line, e.col);
                typed(EK::NewObj(sid, fs), Ty::Ref(sid), line)
            }
            ExprKind::NewArray(ty, len) => {
                let el = self.resolve_ty(ty);
                let l = self.lower_expr(cx, len);
                let l = self.decay(cx, l, (len.line, len.col));
                if el == Ty::Error || l.ty == Ty::Error {
                    return error_expr(line);
                }
                if !self.elem_ok(&el) || !self.zero_fillable(&el) {
                    self.error(ty.line, ty.col, format!("slice 1: an array element must be a scalar, an optional or a plain value, not {}", self.prog.type_name(&el)));
                    return error_expr(line);
                }
                let l = self.coerce(cx, l, &Ty::Int(IntTy::Usize), (len.line, len.col));
                typed(EK::NewArr(el.clone(), Box::new(l)), Ty::Arr(Box::new(el)), line)
            }
            ExprKind::Lambda(l) => match self.exp_here.clone() {
                Some(t @ Ty::Fn(..)) => self.lower_lambda(cx, e, l, &t, false),
                _ => {
                    self.error(e.line, e.col, "an anonymous function needs a function type to match; pass it where one is expected");
                    error_expr(line)
                }
            },
            ExprKind::TypeArgs(..) => {
                self.error(e.line, e.col, "explicit type arguments must be followed by a call");
                error_expr(line)
            }
            ExprKind::ArrayLit(items) => {
                let want = match self.exp_here.clone() {
                    Some(Ty::Arr(el)) => Some(*el),
                    _ => None,
                };
                let el = match want {
                    Some(t) => t,
                    None => match items.first() {
                        Some(f) => {
                            let t = self.probe(cx, f);
                            let t = if t == Ty::Null { Ty::Error } else { t };
                            t
                        }
                        None => {
                            self.error(e.line, e.col, "an empty array literal needs a known element type");
                            return error_expr(line);
                        }
                    },
                };
                if el == Ty::Error {
                    return error_expr(line);
                }
                if !self.elem_ok(&el) {
                    self.error(e.line, e.col, format!("an array element must be a scalar, an optional or a value, not {}", self.prog.type_name(&el)));
                    return error_expr(line);
                }
                let mut out = Vec::new();
                for it in items {
                    let v = self.lower_expecting(cx, it, &el);
                    out.push(self.coerce(cx, v, &el, (it.line, it.col)));
                }
                typed(EK::ArrLit(el.clone(), out), Ty::Arr(Box::new(el)), line)
            }
            ExprKind::Cast(x, ty) => {
                let t = self.resolve_ty(ty);
                let v = self.lower_expr(cx, x);
                let v = self.unwrap_opt(v);
                if t == Ty::Error || v.ty == Ty::Error {
                    return error_expr(line);
                }
                let numeric = |t: &Ty| matches!(t, Ty::Int(_) | Ty::F64 | Ty::Bool);
                if !numeric(&v.ty) || !matches!(t, Ty::Int(_) | Ty::F64) {
                    let (a, b) = (self.prog.type_name(&v.ty), self.prog.type_name(&t));
                    self.error(e.line, e.col, format!("cannot cast {} to {}", a, b));
                    return error_expr(line);
                }
                let v = if v.lit { self.finish_lit(v) } else { v };
                typed(EK::Cast(Box::new(v)), t, line)
            }
        }
    }

    fn lower_fields(&mut self, cx: &mut FnCtx, sid: usize, inits: &[(String, Expr)], line: u32, col: u32) -> Vec<(usize, TExpr)> {
        let mut out: Vec<(usize, TExpr)> = Vec::new();
        for (n, e) in inits {
            let Some(idx) = self.prog.structs[sid].fields.iter().position(|f| f.name == *n) else {
                let sn = self.prog.structs[sid].name.clone();
                self.error(e.line, e.col, format!("struct `{}` has no field `{}`", sn, n));
                self.lower_expr(cx, e);
                continue;
            };
            if out.iter().any(|(i, _)| *i == idx) {
                self.error(e.line, e.col, format!("field `{}` is initialised twice", n));
                continue;
            }
            let fty = self.prog.structs[sid].fields[idx].ty.clone();
            let v = self.lower_expecting(cx, e, &fty);
            let v = self.coerce(cx, v, &fty, (e.line, e.col));
            out.push((idx, v));
        }
        for idx in 0..self.prog.structs[sid].fields.len() {
            if out.iter().any(|(i, _)| *i == idx) {
                continue;
            }
            let (fname, fty, def) = {
                let f = &self.prog.structs[sid].fields[idx];
                (f.name.clone(), f.ty.clone(), f.default.clone())
            };
            match def {
                Some(d) => {
                    let mut dcx = FnCtx::new(Ty::Unit, None);
                    // a default written in a generic struct is read with that instance's type arguments
                    let inst_map: Option<HashMap<String, Ty>> = self.inst_args.get(&sid).and_then(|(n, a)| {
                        self.generics.get(n).map(|(sd, _)| sd.type_params.iter().cloned().zip(a.iter().cloned()).collect())
                    });
                    let saved = inst_map.map(|m| std::mem::replace(&mut self.tmap, m));
                    let v = self.lower_expr(&mut dcx, &d);
                    let v = self.coerce(&mut dcx, v, &fty, (d.line, d.col));
                    if let Some(sv) = saved {
                        self.tmap = sv;
                    }
                    out.push((idx, v));
                }
                None => {
                    // a `Ref<Map<K, V>>` field left out starts as an empty map
                    if let Ty::Ref(m) = &fty {
                        if self.inst_args.get(m).map(|(n, _)| n == "Map").unwrap_or(false) {
                            let m = *m;
                            let fs = self.lower_fields(cx, m, &[], line, col);
                            out.push((idx, typed(EK::NewObj(m, fs), fty.clone(), line)));
                            continue;
                        }
                    }
                    // anything else left out starts empty: zero, false, null, "", an empty array, a plain value of defaults
                    let v = match &fty {
                        Ty::Bool | Ty::F64 | Ty::Int(_) | Ty::Dur | Ty::Inst | Ty::Fn(..) | Ty::Opt(_) => Some(self.zero(&fty, line)),
                        Ty::Str => {
                            let i = self.intern(b"");
                            Some(typed(EK::StrLit(i), Ty::Str, line))
                        }
                        Ty::Arr(el) => {
                            let n = typed(EK::Int(0), Ty::Int(IntTy::Usize), line);
                            Some(typed(EK::NewArr((**el).clone(), Box::new(n)), fty.clone(), line))
                        }
                        Ty::Struct(t) if self.prog.structs[*t].variants.is_empty() && !self.prog.structs[*t].is_extern => {
                            let t = *t;
                            let fs = self.lower_fields(cx, t, &[], line, col);
                            Some(typed(EK::StructLit(t, fs), fty.clone(), line))
                        }
                        Ty::Struct(_) => Some(self.zero(&fty, line)),
                        _ => None,
                    };
                    match v {
                        Some(v) => out.push((idx, v)),
                        None => {
                            let sn = self.prog.structs[sid].name.clone();
                            self.error(line, col, format!("missing field `{}` in `{}`", fname, sn));
                        }
                    }
                }
            }
        }
        out
    }

    fn lower_ident(&mut self, cx: &mut FnCtx, e: &Expr, name: &str) -> TExpr {
        let line = e.line;
        if let Some(id) = cx.lookup(name) {
            if let Some(a) = &cx.vars[id].alias {
                return TExpr { line, ..a.clone() };
            }
            return typed(EK::Var(id), cx.vars[id].ty.clone(), line);
        }
        if name == "self" {
            return match cx.self_sid {
                Some(s) => typed(EK::SelfPlace, Ty::Struct(s), line),
                None => {
                    self.error(e.line, e.col, "`self` is only available inside a method");
                    error_expr(line)
                }
            };
        }
        if let Some(sid) = cx.self_sid {
            if let Some(idx) = self.prog.structs[sid].fields.iter().position(|f| f.name == name) {
                let fty = self.prog.structs[sid].fields[idx].ty.clone();
                let base = typed(EK::SelfPlace, Ty::Struct(sid), line);
                return typed(EK::Field(Box::new(base), idx), fty, line);
            }
        }
        if let Some((ty, v)) = self.consts.get(name).cloned() {
            return match v {
                ConstVal::Int(x) => typed(EK::Int(x), ty, line),
                ConstVal::Float(x) => typed(EK::Float(x), ty, line),
                ConstVal::Bool(x) => typed(EK::Bool(x), ty, line),
            };
        }
        if let Some(&g) = self.globals.get(name) {
            return typed(EK::Global(g), self.prog.globals[g].ty.clone(), line);
        }
        if name == "null" {
            return typed(EK::Null, Ty::Null, line);
        }
        if let Some(&fid) = self.fn_ids.get(name) {
            let f = &self.prog.fns[fid];
            if f.owner.is_none() || f.is_static {
                let ty = Ty::Fn(f.param_tys.clone(), Box::new(f.ret.clone()), !f.throws.is_empty(), f.nopark);
                return typed(EK::FnRef(fid, !f.throws.is_empty()), ty, line);
            }
        }
        self.error(e.line, e.col, format!("unknown name `{}`", name));
        error_expr(line)
    }

    /// An optional used where its value is needed: a checked unwrap.
    /// Integer types convert implicitly to a wider type of the same signedness (and between `usize` and `u64`,
    /// both 64 bits). Mixing signed and unsigned stays an error.
    fn widens(from: IntTy, to: IntTy) -> bool {
        let (fb, fs) = Self::int_shape(from);
        let (tb, ts) = Self::int_shape(to);
        from != to && fs == ts && fb <= tb
    }

    fn int_shape(t: IntTy) -> (u32, bool) {
        match t {
            IntTy::I8 => (8, true),
            IntTy::I16 => (16, true),
            IntTy::I32 => (32, true),
            IntTy::I64 | IntTy::Isize => (64, true),
            IntTy::U8 => (8, false),
            IntTy::U16 => (16, false),
            IntTy::U32 => (32, false),
            IntTy::U64 | IntTy::Usize => (64, false),
        }
    }

    /// Structs and enums compare field by field (an enum: the tag, then the live variant's payload).
    fn eq_ok(&self, t: &Ty, depth: usize) -> bool {
        match t {
            Ty::Int(_) | Ty::F64 | Ty::Bool | Ty::Str | Ty::Ref(_) | Ty::Dur | Ty::Inst | Ty::Fn(..) => true,
            Ty::Opt(i) => self.eq_ok(i, depth),
            Ty::Struct(s) => depth < 16 && self.prog.structs[*s].fields.iter().all(|f| self.eq_ok(&f.ty, depth + 1)),
            _ => false,
        }
    }

    fn unwrap_opt(&mut self, te: TExpr) -> TExpr {
        match te.ty.clone() {
            Ty::Opt(inner) => {
                let line = te.line;
                typed(EK::Unwrap(Box::new(te)), *inner, line)
            }
            _ => te,
        }
    }

    /// The base of `.field` or `.method()`: yields a place of struct type.
    fn place_base(&mut self, cx: &mut FnCtx, b: TExpr) -> TExpr {
        let line = b.line;
        match (&b.ty, &b.kind) {
            (Ty::Obj(s), EK::Var(id)) => typed(EK::ObjPlace(*id), Ty::Struct(*s), line),
            (Ty::Ref(s), _) => {
                let s = *s;
                typed(EK::Deref(Box::new(b)), Ty::Struct(s), line)
            }
            _ => {
                let _ = cx;
                b
            }
        }
    }

    /// `Enum.Variant` or `Enum.Variant(args)`: a struct value with the tag set and the other payloads zero.
    fn lower_variant(&mut self, cx: &mut FnCtx, e: &Expr, sid: usize, vname: &str, args: &[Expr]) -> TExpr {
        let line = e.line;
        let Some(vi) = self.prog.structs[sid].variants.iter().position(|v| v.name == vname) else {
            let en = self.prog.structs[sid].name.clone();
            self.error(e.line, e.col, format!("enum `{}` has no variant `{}`", en, vname));
            for a in args {
                self.lower_expr(cx, a);
            }
            return error_expr(line);
        };
        let fields = self.prog.structs[sid].variants[vi].fields.clone();
        if fields.len() != args.len() {
            self.error(e.line, e.col, format!("variant `{}` takes {} value(s), found {}", vname, fields.len(), args.len()));
            for a in args {
                self.lower_expr(cx, a);
            }
            return error_expr(line);
        }
        let mut out: Vec<(usize, TExpr)> = vec![(0, typed(EK::Int(vi as i128), Ty::Int(IntTy::I32), line))];
        let nf = self.prog.structs[sid].fields.len();
        for f in 1..nf {
            if let Some(pos) = fields.iter().position(|x| *x == f) {
                let fty = self.prog.structs[sid].fields[f].ty.clone();
                let a = &args[pos];
                let v = self.lower_expr(cx, a);
                let v = self.coerce(cx, v, &fty, (a.line, a.col));
                out.push((f, v));
            } else {
                let fty = self.prog.structs[sid].fields[f].ty.clone();
                out.push((f, self.zero(&fty, line)));
            }
        }
        typed(EK::StructLit(sid, out), Ty::Struct(sid), line)
    }

    /// `16.kb`, `2.s`, `50.us`: a size (an untyped integer) or a Duration
    fn unit_literal(&mut self, v: u64, unit: &str, line: u32) -> Option<TExpr> {
        let v = v as i128;
        let size = |k: i128| Some(lit(EK::Int(v * k), Ty::Int(IntTy::I64), line));
        let time = |k: i128| Some(typed(EK::Int(v * k), Ty::Dur, line));
        match unit {
            "kb" => size(1 << 10),
            "mb" => size(1 << 20),
            "gb" => size(1 << 30),
            "s" => time(1_000_000_000),
            "ms" => time(1_000_000),
            "us" => time(1_000),
            "ns" => time(1),
            _ => None,
        }
    }

    fn lower_field(&mut self, cx: &mut FnCtx, e: &Expr, base: &Expr, name: &str) -> TExpr {
        let line = e.line;
        if let ExprKind::Int(v) = &base.kind {
            if let Some(t) = self.unit_literal(*v, name, line) {
                return t;
            }
        }
        if let ExprKind::Ident(tn) = &base.kind {
            if cx.lookup(tn).is_none() {
                if let Some(&sid) = self.struct_ids.get(tn) {
                    if !self.prog.structs[sid].variants.is_empty() {
                        return self.lower_variant(cx, e, sid, name, &[]);
                    }
                }
            }
        }
        let b = self.lower_expr(cx, base);
        let b = self.unwrap_opt(b);
        if b.ty == Ty::Error {
            return b;
        }
        if matches!(b.ty, Ty::Arr(_) | Ty::Str) {
            if name == "length" {
                return typed(EK::Len(Box::new(b)), Ty::Int(IntTy::Usize), line);
            }
            let n = self.prog.type_name(&b.ty);
            self.error(e.line, e.col, format!("{} has no field `{}`", n, name));
            return error_expr(line);
        }
        let b = self.place_base(cx, b);
        match b.ty {
            Ty::Struct(sid) => match self.prog.structs[sid].fields.iter().position(|f| f.name == name) {
                Some(idx) => {
                    let fty = self.prog.structs[sid].fields[idx].ty.clone();
                    typed(EK::Field(Box::new(b), idx), fty, line)
                }
                None => {
                    let sn = self.prog.structs[sid].name.clone();
                    self.error(e.line, e.col, format!("struct `{}` has no field `{}`", sn, name));
                    error_expr(line)
                }
            },
            ref t => {
                let n = self.prog.type_name(t);
                self.error(e.line, e.col, format!("a value of type {} has no fields", n));
                error_expr(line)
            }
        }
    }

    fn lower_args(&mut self, cx: &mut FnCtx, fid: FnId, args: &[Expr], e: &Expr) -> Option<Vec<TExpr>> {
        let ptys = self.prog.fns[fid].param_tys.clone();
        let variadic = self.prog.fns[fid].variadic;
        let fixed = if variadic { ptys.len() - 1 } else { ptys.len() };
        if (!variadic && ptys.len() != args.len()) || args.len() < fixed {
            let n = self.prog.fns[fid].name.clone();
            let more = if variadic { "at least " } else { "" };
            self.error(e.line, e.col, format!("`{}` takes {}{} argument(s), found {}", n, more, fixed, args.len()));
            for a in args {
                self.lower_expr(cx, a);
            }
            return None;
        }
        let mut out = Vec::new();
        let is_extern = self.prog.fns[fid].is_extern;
        for (a, t) in args.iter().zip(ptys.iter()).take(fixed) {
            let v = if let ExprKind::Lambda(l) = &a.kind {
                self.lower_lambda(cx, a, l, t, is_extern)
            } else {
                self.lower_expecting(cx, a, t)
            };
            // a plain value given to a `Ref<T>` parameter is lent, if the callee only borrows (checked later)
            let v = match (&v.ty, t) {
                (Ty::Opt(i), Ty::Ref(y)) if matches!(&**i, Ty::Struct(x) if x == y) => self.unwrap_opt(v),
                _ => v,
            };
            let v = match (&v.ty, t) {
                (Ty::Struct(x), Ty::Ref(y)) if x == y && !is_extern => {
                    let l = v.line;
                    self.borrow_sites.push((fid, out.len(), a.line, a.col));
                    typed(EK::Borrow(Box::new(v)), t.clone(), l)
                }
                _ => v,
            };
            let v = self.coerce(cx, v, t, (a.line, a.col));
            if is_extern && matches!(t, Ty::Fn(..)) && v.ty != Ty::Error {
                if !matches!(v.kind, EK::FnRef(..) | EK::Lambda(..)) {
                    self.error(a.line, a.col, "a runtime function takes a named function or an anonymous `fn(..) { }`, not a function-typed variable");
                    out.push(error_expr(a.line));
                    continue;
                }
                out.push(typed(EK::ExternFn(Box::new(v)), t.clone(), a.line));
            } else {
                out.push(v);
            }
        }
        if variadic {
            let Ty::Arr(el) = ptys[fixed].clone() else { return None };
            let mut items = Vec::new();
            for a in &args[fixed..] {
                let v = self.lower_expecting(cx, a, &el);
                items.push(self.coerce(cx, v, &el, (a.line, a.col)));
            }
            out.push(typed(EK::ArrLit((*el).clone(), items), ptys[fixed].clone(), e.line));
        }
        Some(out)
    }

    /// An anonymous function: lifted to an ordinary function whose leading parameters are the captured values.
    /// Captures are copies; a function that captures anything can only be given to a runtime (`extern`) function,
    /// which owns the environment (the new task's frame).
    fn lower_lambda(&mut self, cx: &mut FnCtx, e: &Expr, l: &ast::LambdaDecl, want: &Ty, extern_ctx: bool) -> TExpr {
        let line = e.line;
        let Ty::Fn(ps, ret, th, np) = want.clone() else {
            self.error(e.line, e.col, "an anonymous function needs a function type to match");
            return error_expr(line);
        };
        if ps.len() != l.params.len() {
            self.error(e.line, e.col, format!("this function takes {} parameter(s), the function type wants {}", l.params.len(), ps.len()));
            return error_expr(line);
        }
        let mut names: Vec<String> = Vec::new();
        for n in &l.idents {
            if l.params.iter().any(|p| p.name == *n) || names.contains(n) {
                continue;
            }
            if cx.lookup(n).is_some() {
                names.push(n.clone());
            }
        }
        let mut caps = Vec::new();
        let mut tmap = self.tmap.clone();
        let mut params = Vec::new();
        for (k, n) in names.iter().enumerate() {
            let id = Expr { kind: ExprKind::Ident(n.clone()), line: e.line, col: e.col };
            let v = self.lower_expr(cx, &id);
            let v = self.decay(cx, v, (e.line, e.col));
            if v.ty == Ty::Error {
                return error_expr(line);
            }
            let tn = format!("__cap{}", k);
            tmap.insert(tn.clone(), v.ty.clone());
            params.push(ast::Param { name: n.clone(), ty: TypeExpr { name: tn, args: vec![], optional: false, fixed: None, line: e.line, col: e.col }, variadic: false });
            caps.push(v);
        }
        if !caps.is_empty() && !extern_ctx {
            self.error(e.line, e.col, "an anonymous function that captures variables can only be given to a runtime function (spawnWorker and the like)");
            return error_expr(line);
        }
        params.extend(l.params.iter().cloned());
        self.lambda_count += 1;
        let name = format!("_lambda{}", self.lambda_count);
        let fd = ast::FnDecl {
            name: name.clone(),
            owner: None,
            is_static: true,
            params,
            ret: l.ret.clone(),
            throws: if th { vec![TypeExpr { name: "Exception".into(), args: vec![], optional: false, fixed: None, line: e.line, col: e.col }] } else { vec![] },
            body: Some(l.body.clone()),
            is_extern: false,
            parks: false,
            attr: if np { Some("nopark".into()) } else { None },   // the function type promises it: checked like `fn[nopark]`
            line: e.line,
            col: e.col,
        };
        let saved = std::mem::replace(&mut self.tmap, tmap);
        let file = ast::File { name: self.cur_file.clone(), package: None, items: vec![] };
        self.declare_fn(&file, &fd, None);
        self.tmap = saved;
        let Some(&fid) = self.fn_ids.get(&name) else { return error_expr(line) };
        // the written signature must be the one the type asks for
        let f = &self.prog.fns[fid];
        let got: Vec<Ty> = f.param_tys[caps.len()..].to_vec();
        if got != ps || f.ret != *ret {
            self.error(e.line, e.col, format!("this function does not have the type {}", self.prog.type_name(want)));
            return error_expr(line);
        }
        if caps.is_empty() {
            typed(EK::FnRef(fid, th), want.clone(), line)
        } else {
            typed(EK::Lambda(fid, caps), want.clone(), line)
        }
    }

    /// `f(args)` where `f` is a value of function type.
    fn call_value(&mut self, cx: &mut FnCtx, e: &Expr, f: TExpr, args: &[Expr]) -> TExpr {
        let line = e.line;
        let Ty::Fn(ps, ret, throws, _) = f.ty.clone() else { return error_expr(line) };
        if ps.len() != args.len() {
            self.error(e.line, e.col, format!("expected {} argument(s), found {}", ps.len(), args.len()));
            return error_expr(line);
        }
        let mut a = Vec::new();
        for (x, p) in args.iter().zip(ps.iter()) {
            let v = self.lower_expecting(cx, x, p);
            let v = match (&v.ty, p) {
                (Ty::Struct(s1), Ty::Ref(s2)) if s1 == s2 => {
                    let l = v.line;
                    typed(EK::BoxVal(Box::new(v)), p.clone(), l)
                }
                _ => v,
            };
            a.push(self.coerce(cx, v, p, (x.line, x.col)));
        }
        if throws {
            self.check_throw(cx, ANY_EXC, (e.line, e.col));
        }
        typed(EK::CallInd(Box::new(f), a), *ret, line)
    }

    /// Can a JSON object be decoded into this field type?
    fn json_ok(&self, t: &Ty, depth: usize) -> Result<(), String> {
        match t {
            Ty::Int(_) | Ty::Bool | Ty::F64 | Ty::Str => Ok(()),
            Ty::Opt(i) => self.json_ok(i, depth),
            Ty::Struct(s) => self.json_struct_ok(*s, depth + 1),
            other => Err(format!("a field of type {}", self.prog.type_name(other))),
        }
    }

    fn json_struct_ok(&self, sid: usize, depth: usize) -> Result<(), String> {
        let si = &self.prog.structs[sid];
        if !si.variants.is_empty() || si.cleanup.is_some() {
            return Err(format!("{} is an enum or has a cleanup()", si.name));
        }
        if si.fields.len() > 64 {
            return Err(format!("{} has more than 64 fields", si.name));
        }
        if depth > 16 {
            return Err("structs nested too deeply".into());
        }
        for f in &si.fields {
            self.json_ok(&f.ty, depth).map_err(|m| format!("{}.{}: {}", si.name, f.name, m))?;
        }
        Ok(())
    }

    /// `json.parse<T>(bytes[, limits])`, built in: the compiler writes the decoder for T.
    fn lower_json(&mut self, cx: &mut FnCtx, e: &Expr, what: &str, targs: &[TypeExpr], args: &[Expr]) -> TExpr {
        let line = e.line;
        for a in args {
            let _ = a;
        }
        if what != "parse" {
            self.error(e.line, e.col, format!("json.{} is not supported yet; only json.parse<T>", what));
            return error_expr(line);
        }
        if targs.len() != 1 || args.is_empty() || args.len() > 2 {
            self.error(e.line, e.col, "json.parse<T>(bytes[, limits]) takes one type and one or two arguments");
            return error_expr(line);
        }
        let ty = self.resolve_ty(&targs[0]);
        let Ty::Struct(sid) = ty else {
            if ty != Ty::Error {
                self.error(targs[0].line, targs[0].col, "json.parse<T> needs a struct type");
            }
            return error_expr(line);
        };
        if let Err(m) = self.json_struct_ok(sid, 0) {
            self.error(targs[0].line, targs[0].col, format!("json cannot decode this type: {}", m));
            return error_expr(line);
        }
        let bytes_ty = Ty::Arr(Box::new(Ty::Int(IntTy::U8)));
        let b = self.lower_expecting(cx, &args[0], &bytes_ty);
        let b = self.coerce(cx, b, &bytes_ty, (args[0].line, args[0].col));
        let lim = if args.len() == 2 {
            let ls = self.struct_ids.get("JsonLimits").copied().unwrap();
            let l = self.lower_expecting(cx, &args[1], &Ty::Struct(ls));
            Some(Box::new(self.coerce(cx, l, &Ty::Struct(ls), (args[1].line, args[1].col))))
        } else {
            None
        };
        let je = self.struct_ids.get("JsonError").copied().unwrap();
        self.check_throw(cx, je, (e.line, e.col));
        typed(EK::Json(sid, Box::new(b), lim), Ty::Struct(sid), line)
    }

    fn lower_call(&mut self, cx: &mut FnCtx, e: &Expr, callee: &Expr, args: &[Expr]) -> TExpr {
        let line = e.line;
        match &callee.kind {
            ExprKind::TypeArgs(inner, targs) => {
                if let ExprKind::Field(b, what) = &inner.kind {
                    if matches!(&b.kind, ExprKind::Ident(n) if n == "json") {
                        return self.lower_json(cx, e, what, targs, args);
                    }
                }
                self.error(callee.line, callee.col, "explicit type arguments are only supported on json.parse<T>");
                error_expr(line)
            }
            ExprKind::Ident(name) => {
                if let Some(id) = cx.lookup(name) {
                    if let Ty::Fn(..) = cx.vars[id].ty {
                        let f = typed(EK::Var(id), cx.vars[id].ty.clone(), line);
                        return self.call_value(cx, e, f, args);
                    }
                }
                // a method of the enclosing struct, called with the implicit self
                if cx.lookup(name).is_none() {
                    if let Some(sid) = cx.self_sid {
                        if let Some(&(_, fid)) = self.prog.structs[sid].methods.iter().find(|m| m.0 == *name) {
                            let Some(a) = self.lower_args(cx, fid, args, e) else { return error_expr(line) };
                            let ret = self.prog.fns[fid].ret.clone();
                            if self.prog.fns[fid].is_static {
                                return typed(EK::Call(fid, a), ret, line);
                            }
                            let me = typed(EK::SelfPlace, Ty::Struct(sid), line);
                            return typed(EK::Method(fid, Box::new(me), a), ret, line);
                        }
                    }
                    if let Some(&fid) = self.fn_ids.get(name) {
                        let Some(a) = self.lower_args(cx, fid, args, e) else { return error_expr(line) };
                        let ret = self.prog.fns[fid].ret.clone();
                        return typed(EK::Call(fid, a), ret, line);
                    }
                    if (name == "min" || name == "max") && args.len() == 2 {
                        let x = self.lower_expr(cx, &args[0]);
                        let y = self.lower_expr(cx, &args[1]);
                        let (x, y) = (self.decay(cx, x, (args[0].line, args[0].col)), self.decay(cx, y, (args[1].line, args[1].col)));
                        if x.ty == Ty::Error || y.ty == Ty::Error {
                            return error_expr(line);
                        }
                        // an untyped literal takes the other side's type; a narrower integer widens
                        let (x, y) = if x.lit && !y.lit { let t = y.ty.clone(); (self.coerce(cx, x, &t, (args[0].line, args[0].col)), y) }
                            else if y.lit && !x.lit { let t = x.ty.clone(); let y2 = self.coerce(cx, y, &t, (args[1].line, args[1].col)); (x, y2) }
                            else { (x, y) };
                        let (x, y) = match (&x.ty, &y.ty) {
                            (Ty::Int(a), Ty::Int(b)) if Self::widens(*a, *b) => { let t = y.ty.clone(); let l = x.line; (typed(EK::Cast(Box::new(x)), t, l), y) }
                            (Ty::Int(a), Ty::Int(b)) if Self::widens(*b, *a) => { let t = x.ty.clone(); let l = y.line; (x, typed(EK::Cast(Box::new(y)), t, l)) }
                            _ => (x, y),
                        };
                        if x.ty != y.ty || !matches!(x.ty, Ty::Int(_) | Ty::F64) {
                            self.error(e.line, e.col, format!("`{}` needs two numbers of the same type", name));
                            return error_expr(line);
                        }
                        let t = x.ty.clone();
                        let rn = if name == "min" { "min" } else { "max" };
                        return typed(EK::RtCall(rn, vec![x, y]), t, line);
                    }
                    if let Some(b) = self.builtin(name) {
                        return self.lower_builtin(cx, e, b, args);
                    }
                }
                self.error(callee.line, callee.col, format!("unknown function `{}`", name));
                for a in args {
                    self.lower_expr(cx, a);
                }
                error_expr(line)
            }
            ExprKind::Field(base, name) if matches!(base.kind, ExprKind::TypeName(_)) => {
                let ExprKind::TypeName(te) = &base.kind else { unreachable!() };
                match self.resolve_ty(te) {
                    Ty::Struct(sid) | Ty::Ref(sid) => {
                        if self.prog.structs[sid].variants.iter().any(|v| v.name == *name) {
                            return self.lower_variant(cx, e, sid, name, args);
                        }
                        let found = self.prog.structs[sid].methods.iter().find(|m| m.0 == *name).map(|m| m.1);
                        match found {
                            Some(fid) if self.prog.fns[fid].is_static => {
                                let Some(a) = self.lower_args(cx, fid, args, e) else { return error_expr(line) };
                                let ret = self.prog.fns[fid].ret.clone();
                                typed(EK::Call(fid, a), ret, line)
                            }
                            _ => {
                                let sn = self.prog.structs[sid].name.clone();
                                self.error(callee.line, callee.col, format!("`{}` has no static function `{}`", sn, name));
                                error_expr(line)
                            }
                        }
                    }
                    Ty::Error => error_expr(line),
                    _ => {
                        self.error(callee.line, callee.col, "only a struct or enum has functions");
                        error_expr(line)
                    }
                }
            }
            ExprKind::Field(base, name) => {
                // Type.staticFn(...)
                if let ExprKind::Ident(tn) = &base.kind {
                    if cx.lookup(tn).is_none() && self.generics.contains_key(tn) {
                        return self.generic_static_call(cx, e, callee, tn, name, args);
                    }
                    if cx.lookup(tn).is_none() && (tn == "Duration" || tn == "Instant") {
                        if tn == "Instant" && name == "now" && args.is_empty() {
                            return typed(EK::Builtin(Builtin::Now, Vec::new()), Ty::Inst, line);
                        }
                        let k: Option<i128> = match (tn.as_str(), name.as_str()) {
                            ("Duration", "seconds") => Some(1_000_000_000),
                            ("Duration", "millis") => Some(1_000_000),
                            ("Duration", "micros") => Some(1_000),
                            ("Duration", "nanos") => Some(1),
                            _ => None,
                        };
                        if let (Some(k), 1) = (k, args.len()) {
                            let n = self.lower_expr(cx, &args[0]);
                            let n = self.coerce(cx, n, &Ty::Int(IntTy::I64), (args[0].line, args[0].col));
                            let kk = typed(EK::Int(k), Ty::Int(IntTy::I64), line);
                            return typed(EK::Binary(BinOp::Mul, Box::new(n), Box::new(kk)), Ty::Dur, line);
                        }
                        self.error(callee.line, callee.col, format!("`{}.{}` is not a function of {}", tn, name, tn));
                        return error_expr(line);
                    }
                    if cx.lookup(tn).is_none() {
                        if let Some(&sid) = self.struct_ids.get(tn) {
                            if self.prog.structs[sid].variants.iter().any(|v| v.name == *name) {
                                return self.lower_variant(cx, e, sid, name, args);
                            }
                            let found = self.prog.structs[sid].methods.iter().find(|m| m.0 == *name).map(|m| m.1);
                            return match found {
                                Some(fid) if self.prog.fns[fid].is_static => {
                                    let Some(a) = self.lower_args(cx, fid, args, e) else { return error_expr(line) };
                                    let ret = self.prog.fns[fid].ret.clone();
                                    typed(EK::Call(fid, a), ret, line)
                                }
                                Some(_) => {
                                    self.error(callee.line, callee.col, format!("`{}.{}` needs an instance; it is not static", tn, name));
                                    error_expr(line)
                                }
                                None => {
                                    self.error(callee.line, callee.col, format!("struct `{}` has no function `{}`", tn, name));
                                    error_expr(line)
                                }
                            };
                        }
                    }
                }
                let b = self.lower_expr(cx, base);
                let b = self.unwrap_opt(b);
                if b.ty == Ty::Error {
                    for a in args {
                        self.lower_expr(cx, a);
                    }
                    return b;
                }
                if matches!(b.ty, Ty::Str | Ty::Arr(_)) {
                    // `length` is how many elements are in use; `capacity()` how many the run holds (indexing and slicing go up to it)
                    if name == "length" && args.is_empty() {
                        return typed(EK::Len(Box::new(b)), Ty::Int(IntTy::Usize), line);
                    }
                    if name == "capacity" && matches!(b.ty, Ty::Arr(_)) && args.is_empty() {
                        return typed(EK::RtCall("capacity", vec![b]), Ty::Int(IntTy::Usize), line);
                    }
                    if let (Ty::Arr(el), "append", 1) = (&b.ty, name.as_str(), args.len()) {
                        let el = (**el).clone();
                        if !matches!(el, Ty::Int(_) | Ty::Bool | Ty::F64) {
                            self.error(callee.line, callee.col, "append works on arrays of numbers and booleans");
                            return error_expr(line);
                        }
                        let a0 = self.lower_expecting(cx, &args[0], &b.ty);
                        if a0.ty == b.ty {
                            return typed(EK::RtCall("appendArr", vec![b, a0]), Ty::Unit, line);
                        }
                        let a0 = self.coerce(cx, a0, &el, (args[0].line, args[0].col));
                        return typed(EK::RtCall("appendOne", vec![b, a0]), Ty::Unit, line);
                    }
                    // searching: String or Array<u8> receivers, a String or Array<u8> needle
                    let bytes_ty = Ty::Arr(Box::new(Ty::Int(IntTy::U8)));
                    let byteish = matches!(b.ty, Ty::Str) || b.ty == bytes_ty;
                    if byteish && matches!(name.as_str(), "find" | "contains" | "startsWith" | "copyFrom") && args.len() == 1 {
                        let n = self.lower_expr(cx, &args[0]);
                        let n = self.decay(cx, n, (args[0].line, args[0].col));
                        if n.ty != Ty::Str && n.ty != bytes_ty {
                            if n.ty != Ty::Error {
                                self.error(args[0].line, args[0].col, "expected a String or an Array<u8>");
                            }
                            return error_expr(line);
                        }
                        if name == "copyFrom" && b.ty != bytes_ty {
                            self.error(callee.line, callee.col, "only an Array<u8> can be copied into");
                            return error_expr(line);
                        }
                        let (rn, rt) = match name.as_str() {
                            "find" => ("find", Ty::Opt(Box::new(Ty::Int(IntTy::Usize)))),
                            "contains" => ("contains", Ty::Bool),
                            "startsWith" => ("startsWith", Ty::Bool),
                            _ => ("copyFrom", Ty::Unit),
                        };
                        return typed(EK::RtCall(rn, vec![b, n]), rt, line);
                    }
                    if b.ty == bytes_ty && name == "copy" && args.is_empty() {
                        return typed(EK::RtCall("toBytes", vec![b]), bytes_ty, line);
                    }
                    if matches!(b.ty, Ty::Str) && name == "toBytes" && args.is_empty() {
                        return typed(EK::RtCall("toBytes", vec![b]), Ty::Arr(Box::new(Ty::Int(IntTy::U8))), line);
                    }
                    if matches!(b.ty, Ty::Arr(_)) && name == "grow" && args.is_empty() {
                        return typed(EK::RtCall("grow", vec![b]), Ty::Unit, line);
                    }
                    if matches!(b.ty, Ty::Arr(_)) && name == "moveWithin" && args.len() == 2 {
                        let rs = self.struct_ids.get("Range").copied();
                        let r = match rs {
                            Some(sid) => {
                                let v = self.lower_expecting(cx, &args[0], &Ty::Struct(sid));
                                self.coerce(cx, v, &Ty::Struct(sid), (args[0].line, args[0].col))
                            }
                            None => return error_expr(line),
                        };
                        let to = self.lower_expr(cx, &args[1]);
                        let to = self.coerce(cx, to, &Ty::Int(IntTy::Usize), (args[1].line, args[1].col));
                        return typed(EK::RtCall("moveWithin", vec![b, r, to]), Ty::Unit, line);
                    }
                    let n = self.prog.type_name(&b.ty);
                    self.error(callee.line, callee.col, format!("{} has no function `{}`", n, name));
                    return error_expr(line);
                }
                let b = self.place_base(cx, b);
                match b.ty {
                    Ty::Struct(sid) => {
                        let found = self.prog.structs[sid].methods.iter().find(|m| m.0 == *name).map(|m| m.1);
                        match found {
                            Some(fid) if !self.prog.fns[fid].is_static => {
                                let Some(a) = self.lower_args(cx, fid, args, e) else { return error_expr(line) };
                                let ret = self.prog.fns[fid].ret.clone();
                                typed(EK::Method(fid, Box::new(b), a), ret, line)
                            }
                            Some(_) => {
                                let sn = self.prog.structs[sid].name.clone();
                                self.error(callee.line, callee.col, format!("`{}` is static; call it as `{}.{}(..)`", name, sn, name));
                                error_expr(line)
                            }
                            None => {
                                let sn = self.prog.structs[sid].name.clone();
                                self.error(callee.line, callee.col, format!("struct `{}` has no function `{}`", sn, name));
                                error_expr(line)
                            }
                        }
                    }
                    ref t => {
                        let n = self.prog.type_name(t);
                        self.error(callee.line, callee.col, format!("a value of type {} has no functions", n));
                        error_expr(line)
                    }
                }
            }
            _ => {
                self.error(callee.line, callee.col, "this expression cannot be called");
                error_expr(line)
            }
        }
    }

    fn builtin(&self, name: &str) -> Option<Builtin> {
        Some(match name {
            "print" => Builtin::Print,
            "println" => Builtin::Println,
            "assert" => Builtin::Assert,
            "str" => Builtin::Str,
            "hash" => Builtin::Hash,
            _ => return None,
        })
    }

    fn lower_builtin(&mut self, cx: &mut FnCtx, e: &Expr, b: Builtin, args: &[Expr]) -> TExpr {
        let line = e.line;
        if args.len() != 1 {
            self.error(e.line, e.col, "this builtin takes exactly one argument");
            return error_expr(line);
        }
        let a = self.lower_expr(cx, &args[0]);
        let a = self.decay(cx, a, (args[0].line, args[0].col));
        let a = self.unwrap_opt(a);
        if a.ty == Ty::Error {
            return a;
        }
        let a = if a.lit { self.finish_lit(a) } else { a };
        match b {
            Builtin::Now => error_expr(line),
            Builtin::Assert => {
                let a = self.coerce(cx, a, &Ty::Bool, (args[0].line, args[0].col));
                typed(EK::Builtin(b, vec![a]), Ty::Unit, line)
            }
            Builtin::Hash => {
                if !matches!(a.ty, Ty::Int(_) | Ty::Bool | Ty::Str) {
                    let n = self.prog.type_name(&a.ty);
                    self.error(args[0].line, args[0].col, format!("cannot hash a value of type {}", n));
                    return error_expr(line);
                }
                typed(EK::Builtin(b, vec![a]), Ty::Int(IntTy::U64), line)
            }
            Builtin::Print | Builtin::Println | Builtin::Str => {
                if !matches!(a.ty, Ty::Int(_) | Ty::F64 | Ty::Bool | Ty::Str) {
                    let n = self.prog.type_name(&a.ty);
                    self.error(args[0].line, args[0].col, format!("cannot print or convert a value of type {}", n));
                    return error_expr(line);
                }
                if b == Builtin::Str && a.ty == Ty::Str {
                    return a;
                }
                let ty = if b == Builtin::Str { Ty::Str } else { Ty::Unit };
                typed(EK::Builtin(b, vec![a]), ty, line)
            }
        }
    }

    fn lower_binary(&mut self, cx: &mut FnCtx, e: &Expr, op: BinOp, l: &Expr, r: &Expr) -> TExpr {
        let line = e.line;
        let a = self.lower_expr(cx, l);
        let b = self.lower_expr(cx, r);
        let a = self.decay(cx, a, (l.line, l.col));
        let b = self.decay(cx, b, (r.line, r.col));
        if a.ty == Ty::Error || b.ty == Ty::Error {
            return error_expr(line);
        }
        if matches!(op, BinOp::Eq | BinOp::Ne) && (a.ty == Ty::Null || b.ty == Ty::Null) {
            let other = if a.ty == Ty::Null { b } else { a };
            if !matches!(other.ty, Ty::Opt(_) | Ty::Ref(_)) {
                let n = self.prog.type_name(&other.ty);
                self.error(e.line, e.col, format!("{} is never null; only an optional or a `Ref` can be compared with `null`", n));
                return error_expr(line);
            }
            let t = typed(EK::IsNull(Box::new(other)), Ty::Bool, line);
            return if op == BinOp::Eq { t } else { typed(EK::Unary(UnOp::Not, Box::new(t)), Ty::Bool, line) };
        }
        let a = self.unwrap_opt(a);
        let b = self.unwrap_opt(b);
        if matches!(op, BinOp::And | BinOp::Or) {
            let a = self.coerce(cx, a, &Ty::Bool, (l.line, l.col));
            let b = self.coerce(cx, b, &Ty::Bool, (r.line, r.col));
            return typed(EK::Binary(op, Box::new(a), Box::new(b)), Ty::Bool, line);
        }
        // time: Instant - Instant is a Duration; Instant and Duration add and subtract; a Duration scales by an integer
        let timey = |t: &Ty| matches!(t, Ty::Dur | Ty::Inst);
        if timey(&a.ty) || timey(&b.ty) {
            let int = |t: &Ty| matches!(t, Ty::Int(_));
            let res = match (&a.ty, &b.ty, op) {
                (Ty::Dur, Ty::Dur, BinOp::Add | BinOp::Sub) => Some(Ty::Dur),
                (Ty::Inst, Ty::Inst, BinOp::Sub) => Some(Ty::Dur),
                (Ty::Inst, Ty::Dur, BinOp::Add | BinOp::Sub) | (Ty::Dur, Ty::Inst, BinOp::Add) => Some(Ty::Inst),
                (Ty::Dur, t, BinOp::Mul | BinOp::Div) if int(t) => Some(Ty::Dur),
                (t, Ty::Dur, BinOp::Mul) if int(t) => Some(Ty::Dur),
                (x, y, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) if x == y => Some(Ty::Bool),
                _ => None,
            };
            let Some(res) = res else {
                let (ta, tb) = (self.prog.type_name(&a.ty), self.prog.type_name(&b.ty));
                self.error(e.line, e.col, format!("`{}` cannot combine {} and {}", op_name(op), ta, tb));
                return error_expr(line);
            };
            let i64t = Ty::Int(IntTy::I64);
            let a = if a.lit { self.coerce(cx, a, &i64t, (l.line, l.col)) } else { a };
            let b = if b.lit { self.coerce(cx, b, &i64t, (r.line, r.col)) } else { b };
            return typed(EK::Binary(op, Box::new(a), Box::new(b)), res, line);
        }
        // unify the operand types; an untyped literal takes the other side's type
        let (a, b) = match (a.lit, b.lit) {
            (true, false) if matches!(b.ty, Ty::Int(_) | Ty::F64) => {
                let t = b.ty.clone();
                (self.coerce(cx, a, &t, (l.line, l.col)), b)
            }
            (false, true) if matches!(a.ty, Ty::Int(_) | Ty::F64) => {
                let t = a.ty.clone();
                let b2 = self.coerce(cx, b, &t, (r.line, r.col));
                (a, b2)
            }
            (true, true) if a.ty != b.ty => {
                // int literal with float literal: both float
                let a2 = self.retype_lit(a, &Ty::F64, (l.line, l.col));
                let b2 = self.retype_lit(b, &Ty::F64, (r.line, r.col));
                (TExpr { lit: true, ..a2 }, TExpr { lit: true, ..b2 })
            }
            _ => (a, b),
        };
        // String + number/bool: the other side is written out
        let (a, b) = match (&a.ty, &b.ty, op) {
            (Ty::Str, Ty::Int(_) | Ty::Bool | Ty::F64, BinOp::Add) if !b.lit || true => {
                let b = if b.lit { self.finish_lit(b) } else { b };
                let line = b.line;
                (a, typed(EK::Builtin(Builtin::Str, vec![b]), Ty::Str, line))
            }
            (Ty::Int(_) | Ty::Bool | Ty::F64, Ty::Str, BinOp::Add) => {
                let a = if a.lit { self.finish_lit(a) } else { a };
                let line = a.line;
                (typed(EK::Builtin(Builtin::Str, vec![a]), Ty::Str, line), b)
            }
            _ => (a, b),
        };
        // the narrower integer widens to the wider
        let (a, b) = match (&a.ty, &b.ty) {
            (Ty::Int(x), Ty::Int(y)) if Self::widens(*x, *y) => {
                let t = b.ty.clone();
                let l = a.line;
                (typed(EK::Cast(Box::new(a)), t, l), b)
            }
            (Ty::Int(x), Ty::Int(y)) if Self::widens(*y, *x) => {
                let t = a.ty.clone();
                let l = b.line;
                (a, typed(EK::Cast(Box::new(b)), t, l))
            }
            _ => (a, b),
        };
        if a.ty != b.ty {
            let (ta, tb) = (self.prog.type_name(&a.ty), self.prog.type_name(&b.ty));
            self.error(e.line, e.col, format!("`{}` needs operands of the same type, found {} and {}", op_name(op), ta, tb));
            return error_expr(line);
        }
        let both_lit = a.lit && b.lit;
        let t = a.ty.clone();
        let arith = matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem);
        let ok = if arith {
            match &t {
                Ty::Int(_) => true,
                Ty::F64 => op != BinOp::Rem,
                Ty::Str => op == BinOp::Add,
                _ => false,
            }
        } else if matches!(op, BinOp::Eq | BinOp::Ne) {
            matches!(t, Ty::Int(_) | Ty::F64 | Ty::Bool | Ty::Str | Ty::Ref(_)) || self.eq_ok(&t, 0)
        } else {
            matches!(t, Ty::Int(_) | Ty::F64)
        };
        if !ok {
            let n = self.prog.type_name(&t);
            self.error(e.line, e.col, format!("`{}` cannot be used with {}", op_name(op), n));
            return error_expr(line);
        }
        let rty = if arith { t } else { Ty::Bool };
        TExpr { kind: EK::Binary(op, Box::new(a), Box::new(b)), ty: rty.clone(), line, lit: both_lit && arith }
    }
}

fn op_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn block_returns(b: &TBlock) -> bool {
    match b.stmts.last() {
        None => false,
        Some(s) => stmt_returns(s),
    }
}

fn stmt_returns(s: &TStmt) -> bool {
    match &s.kind {
        TS::Return(_) => true,
        TS::If(_, t, Some(e)) => block_returns(t) && block_returns(e),
        TS::Block(b) | TS::Sync(_, b) => block_returns(b),
        TS::Throw(..) => true,
        TS::Try(b, cs) => block_returns(b) && cs.iter().all(|c| block_returns(&c.body)),
        TS::Loop(b) => !block_breaks(b),
        TS::While(c, b) => matches!(c.kind, EK::Bool(true)) && !block_breaks(b),
        _ => false,
    }
}

fn block_breaks(b: &TBlock) -> bool {
    b.stmts.iter().any(|s| match &s.kind {
        TS::Break | TS::BreakL(..) => true,
        TS::Labeled(i) => block_breaks(&TBlock { stmts: vec![(**i).clone()], ..Default::default() }),
        TS::If(_, t, e) => block_breaks(t) || e.as_ref().map(block_breaks).unwrap_or(false),
        TS::Block(b) | TS::Sync(_, b) => block_breaks(b),
        TS::Try(b, cs) => block_breaks(b) || cs.iter().any(|c| block_breaks(&c.body)),
        _ => false, // a break inside a nested loop belongs to that loop
    })
}

/// Writes of parameters: `writes[f][k]` is true if function f assigns through, grows, or passes on for writing its parameter k.
pub(crate) fn param_writes(prog: &Program) -> Vec<Vec<bool>> {
    let n = prog.fns.len();
    // index 0 is the receiver (`self`), index k + 1 is parameter k
    let mut w: Vec<Vec<bool>> = prog.fns.iter().map(|f| vec![false; f.params.len() + 1]).collect();
    loop {
        let mut changed = false;
        for i in 0..n {
            let f = &prog.fns[i];
            let Some(body) = &f.body else { continue };
            for k in 0..=f.params.len() {
                if w[i][k] {
                    continue;
                }
                let kind = if k == 0 { EK::SelfPlace } else { EK::Var(f.params[k - 1]) };
                let pv = TExpr { kind, ty: Ty::Unit, line: 0, lit: false };
                if block_writes(body, &pv, &w) {
                    w[i][k] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    w
}


/// How a place is written in the source, for messages.
pub(crate) fn place_name(prog: &Program, f: &FnInfo, e: &TExpr) -> String {
    match &e.kind {
        EK::Var(v) | EK::ObjRef(v) | EK::ObjPlace(v) => f.vars[*v].name.clone(),
        EK::Field(b, i) => {
            let bn = place_name(prog, f, b);
            match &b.ty {
                Ty::Struct(s) | Ty::Ref(s) | Ty::Obj(s) => format!("{}.{}", bn, prog.structs[*s].fields[*i].name),
                _ => bn,
            }
        }
        EK::Deref(b) | EK::Unwrap(b) | EK::Index(b, _) | EK::Borrow(b) | EK::BoxVal(b) => place_name(prog, f, b),
        EK::Global(g) => prog.globals[*g].name.clone(),
        EK::SelfPlace => "self".into(),
        _ => "the object".into(),
    }
}

/// Are `a` and `b` the same place (the same variable, or the same field of the same place)?
pub(crate) fn same_place(a: &TExpr, b: &TExpr) -> bool {
    match (&a.kind, &b.kind) {
        (EK::Var(x), EK::Var(y)) => x == y,
        (EK::Global(x), EK::Global(y)) => x == y,
        (EK::SelfPlace, EK::SelfPlace) => true,
        (EK::ObjRef(x), EK::ObjRef(y)) | (EK::ObjPlace(x), EK::ObjPlace(y)) | (EK::ObjRef(x), EK::ObjPlace(y)) | (EK::ObjPlace(x), EK::ObjRef(y)) => x == y,
        (EK::Field(x, f), EK::Field(y, g)) => f == g && same_place(x, y),
        (EK::Deref(x), EK::Deref(y)) => same_place(x, y),
        _ => false,
    }
}

/// Does the place expression `w` reach the object `t` points to? That is: `t` itself (a callee may write through it), or a field,
/// element or target below it.
pub(crate) fn reaches(w: &TExpr, t: &TExpr, below_only: bool) -> bool {
    if !below_only && same_place(w, t) {
        return true;
    }
    match &w.kind {
        EK::Field(a, _) | EK::Index(a, _) | EK::Deref(a) | EK::Len(a) => same_place(a, t) || reaches(a, t, true),
        _ => false,
    }
}

/// Does the block write through the object that `t` (a `Ref` place) points to? `writes[f][k]` says which parameters a function writes.
pub(crate) fn block_writes(b: &TBlock, t: &TExpr, writes: &[Vec<bool>]) -> bool {
    let mut hit = false;
    walk_stmts(b, &mut |s| match &s.kind {
        TS::Assign(w, _) | TS::AssignOp(_, w, _) => {
            if reaches(w, t, true) {
                hit = true;
            }
        }
        _ => {}
    });
    visit_block(b, &mut |e| {
        visit_expr(e, &mut |x| match &x.kind {
            EK::RtCall(n, args) if matches!(*n, "appendArr" | "appendOne" | "grow" | "moveWithin" | "copyFrom") => {
                if args.first().map_or(false, |a| reaches(a, t, true)) {
                    hit = true;
                }
            }
            EK::Call(f, args) => {
                if args.iter().enumerate().any(|(k, a)| writes[*f].get(k + 1).copied().unwrap_or(false) && reaches(a, t, false)) {
                    hit = true;
                }
            }
            EK::Method(f, r, args) => {
                let all = std::iter::once(&**r).chain(args.iter());
                if all.enumerate().any(|(k, a)| writes[*f].get(k).copied().unwrap_or(false) && reaches(a, t, false)) {
                    hit = true;
                }
            }
            _ => {}
        })
    });
    hit
}

/// Every statement of the block, nested ones included, outermost first.
pub(crate) fn walk_stmts(b: &TBlock, f: &mut dyn FnMut(&TStmt)) {
    for s in &b.stmts {
        walk_stmt(s, f);
    }
}

pub(crate) fn walk_stmt(s: &TStmt, f: &mut dyn FnMut(&TStmt)) {
    f(s);
    match &s.kind {
        TS::If(_, t, e) => {
            walk_stmts(t, f);
            if let Some(e) = e {
                walk_stmts(e, f);
            }
        }
        TS::While(_, b) | TS::Loop(b) | TS::Block(b) | TS::Sync(_, b) => walk_stmts(b, f),
        TS::Labeled(i) => walk_stmt(i, f),
        TS::Try(b, cs) => {
            walk_stmts(b, f);
            for c in cs {
                walk_stmts(&c.body, f);
            }
        }
        _ => {}
    }
}

/// Calls `f(targets, body, line)` for every sync block, outer ones first.
fn sync_walk<'a>(b: &'a TBlock, open: &mut Vec<&'a TExpr>, f: &mut dyn FnMut(&[(bool, TExpr)], &TBlock, u32)) {
    for s in &b.stmts {
        sync_walk_stmt(s, open, f);
    }
}

fn sync_walk_stmt<'a>(s: &'a TStmt, open: &mut Vec<&'a TExpr>, f: &mut dyn FnMut(&[(bool, TExpr)], &TBlock, u32)) {
    match &s.kind {
        TS::Sync(ts, b) => {
            f(ts, b, s.line);
            sync_walk(b, open, f);
        }
        TS::If(_, t, e) => {
            sync_walk(t, open, f);
            if let Some(e) = e {
                sync_walk(e, open, f);
            }
        }
        TS::While(_, b) | TS::Loop(b) | TS::Block(b) => sync_walk(b, open, f),
        TS::Labeled(i) => sync_walk_stmt(i, open, f),
        TS::Try(b, cs) => {
            sync_walk(b, open, f);
            for c in cs {
                sync_walk(&c.body, open, f);
            }
        }
        _ => {}
    }
}

/// Calls `f` on every root expression of every statement in the block (nested blocks included).
pub(crate) fn visit_block(b: &TBlock, f: &mut dyn FnMut(&TExpr)) {
    for s in &b.stmts {
        visit_stmt(s, f);
    }
}

pub(crate) fn visit_stmt(s: &TStmt, f: &mut dyn FnMut(&TExpr)) {
    match &s.kind {
        TS::Let(_, e) | TS::Expr(e) | TS::SetGlobal(_, e) => f(e),
        TS::LetObj(_, _, fs) | TS::Throw(_, fs) => {
            for (_, e) in fs {
                f(e);
            }
        }
        TS::Assign(a, b) | TS::AssignOp(_, a, b) => {
            f(a);
            f(b);
        }
        TS::If(c, t, e) => {
            f(c);
            visit_block(t, f);
            if let Some(e) = e {
                visit_block(e, f);
            }
        }
        TS::While(c, b) => {
            f(c);
            visit_block(b, f);
        }
        TS::Loop(b) | TS::Block(b) => visit_block(b, f),
        TS::Sync(ts, b) => {
            // naming an object to lock it only reads through it; it does not let the handle escape
            for (_, t) in ts {
                f(&TExpr { kind: EK::Deref(Box::new(t.clone())), ty: Ty::Unit, line: t.line, lit: false });
            }
            visit_block(b, f);
        }
        TS::Labeled(i) => visit_stmt(i, f),
        TS::Return(Some(e)) => f(e),
        TS::Try(b, cs) => {
            visit_block(b, f);
            for c in cs {
                visit_block(&c.body, f);
            }
        }
        TS::Return(None) | TS::Break | TS::Continue | TS::BreakL(..) => {}
    }
}

pub(crate) fn children(e: &TExpr) -> Vec<&TExpr> {
    match &e.kind {
        EK::Field(a, _) | EK::Deref(a) | EK::Borrow(a) | EK::BoxVal(a) | EK::Len(a) | EK::ExternFn(a) | EK::Unary(_, a) | EK::Cast(a) | EK::Some(a) | EK::IsNull(a) | EK::Unwrap(a) | EK::NewArr(_, a) => vec![&**a],
        EK::Index(a, b) | EK::Binary(_, a, b) => vec![&**a, &**b],
        EK::Slice(a, lo, hi) => {
            let mut v = vec![&**a];
            v.extend(lo.iter().map(|x| &**x));
            v.extend(hi.iter().map(|x| &**x));
            v
        }
        EK::Call(_, args) | EK::Lambda(_, args) | EK::Builtin(_, args) | EK::ArrLit(_, args) | EK::RtCall(_, args) => args.iter().collect(),
        EK::CallInd(f, args) => std::iter::once(&**f).chain(args.iter()).collect(),
        EK::Method(_, r, args) => std::iter::once(&**r).chain(args.iter()).collect(),
        EK::StructLit(_, fs) | EK::NewObj(_, fs) => fs.iter().map(|(_, x)| x).collect(),
        EK::Json(_, b, l) => std::iter::once(&**b).chain(l.iter().map(|x| &**x)).collect(),
        EK::Cond(a, b, c) => vec![&**a, &**b, &**c],
        _ => vec![],
    }
}

/// Calls `f` on `e` and every expression inside it.
pub(crate) fn visit_expr(e: &TExpr, f: &mut dyn FnMut(&TExpr)) {
    f(e);
    for c in children(e) {
        visit_expr(c, f);
    }
}

/// Does the handle held by variable `pid` get used in `e` in a way that lets it outlive the call?
/// Fine: reading through it, comparing it, giving it to a parameter that itself only borrows.
pub(crate) fn escapes(e: &TExpr, pid: VarId, ok_here: bool, borrowed: &[Vec<bool>]) -> bool {
    let is_p = |x: &TExpr| matches!(x.kind, EK::Var(v) | EK::ObjRef(v) if v == pid);
    match &e.kind {
        EK::Var(v) | EK::ObjRef(v) if *v == pid => !ok_here,
        EK::Deref(a) => escapes(a, pid, true, borrowed),
        EK::Binary(BinOp::Eq | BinOp::Ne, a, b) => escapes(a, pid, true, borrowed) || escapes(b, pid, true, borrowed),
        EK::Call(fid, args) => args.iter().enumerate().any(|(i, a)| escapes(a, pid, is_p(a) && borrowed[*fid].get(i).copied().unwrap_or(false), borrowed)),
        EK::Method(fid, r, args) => escapes(r, pid, false, borrowed) || args.iter().enumerate().any(|(i, a)| escapes(a, pid, is_p(a) && borrowed[*fid].get(i).copied().unwrap_or(false), borrowed)),
        _ => children(e).into_iter().any(|c| escapes(c, pid, false, borrowed)),
    }
}

/// Collects the expressions that `name` meets in an arithmetic, comparison or assignment, in source order.
fn hint_stmt<'a>(s: &'a ast::Stmt, name: &str, out: &mut Vec<Option<&'a Expr>>) {
    let blk = |b: &'a ast::Block, out: &mut Vec<Option<&'a Expr>>| {
        for s in &b.stmts {
            hint_stmt(s, name, out);
        }
    };
    match &s.kind {
        StmtKind::Let { init, .. } => hint_expr(init, name, out),
        StmtKind::Assign { target, value, .. } => {
            if matches!(&target.kind, ExprKind::Ident(n) if n == name) && simple(value) {
                out.push(Some(value));
            }
            hint_expr(target, name, out);
            hint_expr(value, name, out);
        }
        StmtKind::Expr(e) | StmtKind::Throw(e) => hint_expr(e, name, out),
        StmtKind::Return(e) => {
            if let Some(e) = e {
                if matches!(&e.kind, ExprKind::Ident(n) if n == name) {
                    out.push(None);
                }
                hint_expr(e, name, out)
            }
        }
        StmtKind::If { cond, then, els } => {
            hint_expr(cond, name, out);
            blk(then, out);
            if let Some(e) = els {
                blk(e, out)
            }
        }
        StmtKind::While { cond, body } => {
            hint_expr(cond, name, out);
            blk(body, out);
        }
        StmtKind::For { iter, body, .. } => {
            hint_expr(iter, name, out);
            blk(body, out);
        }
        StmtKind::Loop(b) | StmtKind::Block(b) | StmtKind::Sync { body: b, .. } => blk(b, out),
        StmtKind::Labeled(_, s) => hint_stmt(s, name, out),
        StmtKind::Switch { subject, arms } => {
            hint_expr(subject, name, out);
            for a in arms {
                blk(&a.body, out);
            }
        }
        StmtKind::Try { body, catches } => {
            blk(body, out);
            for c in catches {
                blk(&c.body, out);
            }
        }
        _ => {}
    }
}

fn hint_expr<'a>(e: &'a Expr, name: &str, out: &mut Vec<Option<&'a Expr>>) {
    let is_x = |x: &Expr| matches!(&x.kind, ExprKind::Ident(n) if n == name);
    match &e.kind {
        ExprKind::Binary(op, a, b) => {
            if !matches!(op, BinOp::And | BinOp::Or) {
                if is_x(a) && simple(b) && !is_x(b) {
                    out.push(Some(b));
                }
                if is_x(b) && simple(a) && !is_x(a) {
                    out.push(Some(a));
                }
            }
            hint_expr(a, name, out);
            hint_expr(b, name, out);
        }
        ExprKind::Field(a, _) | ExprKind::Unary(_, a) | ExprKind::Cast(a, _) => hint_expr(a, name, out),
        ExprKind::Call(f, args) => {
            hint_expr(f, name, out);
            for a in args {
                hint_expr(a, name, out);
            }
        }
        ExprKind::Index(a, b) => {
            hint_expr(a, name, out);
            hint_expr(b, name, out);
        }
        ExprKind::Cond(a, b, c) => {
            hint_expr(a, name, out);
            hint_expr(b, name, out);
            hint_expr(c, name, out);
        }
        _ => {}
    }
}

/// An operand worth typing: names, fields and indexing only, so typing it has no side effects.
fn simple(x: &Expr) -> bool {
        match &x.kind {
            ExprKind::Ident(_) => true,
            ExprKind::Field(b, _) => simple(b),
            ExprKind::Index(b, i) => simple(b) && simple(i),
            ExprKind::Call(f, a) => a.is_empty() && simple(f),
            ExprKind::Binary(_, a, b) => simple(a) && simple(b),
            _ => false,
        }
    }

/// A place that can be named again without side effects: a variable, `self`, a field of one, a handle's target.
fn pure_place(e: &TExpr) -> bool {
    match &e.kind {
        EK::Var(_) | EK::SelfPlace | EK::ObjPlace(_) => true,
        EK::Field(b, _) => pure_place(b),
        EK::Deref(h) => matches!(h.kind, EK::Var(_) | EK::SelfPlace) || pure_place(h) && matches!(h.kind, EK::Field(..)),
        _ => false,
    }
}

/// Does the block assign to `name` (or a part of it) or call a method on it, so it must be the payload itself and not a copy?
fn block_mutates(b: &ast::Block, name: &str) -> bool {
    b.stmts.iter().any(|s| stmt_mutates(s, name))
}

fn root_is(e: &Expr, name: &str) -> bool {
    match &e.kind {
        ExprKind::Ident(n) => n == name,
        ExprKind::Field(b, _) | ExprKind::Index(b, _) => root_is(b, name),
        _ => false,
    }
}

fn stmt_mutates(s: &ast::Stmt, name: &str) -> bool {
    let blk = |b: &ast::Block| block_mutates(b, name);
    let ex = |e: &Expr| expr_mutates(e, name);
    match &s.kind {
        StmtKind::Let { init, .. } => ex(init),
        StmtKind::Assign { target, value, .. } => root_is(target, name) || ex(value),
        StmtKind::Expr(e) | StmtKind::Throw(e) => ex(e),
        StmtKind::Return(e) => e.as_ref().map_or(false, ex),
        StmtKind::If { cond, then, els } => ex(cond) || blk(then) || els.as_ref().map_or(false, blk),
        StmtKind::While { cond, body } => ex(cond) || blk(body),
        StmtKind::For { iter, body, .. } => ex(iter) || blk(body),
        StmtKind::Loop(b) | StmtKind::Block(b) | StmtKind::Sync { body: b, .. } => blk(b),
        StmtKind::Labeled(_, s) => stmt_mutates(s, name),
        StmtKind::Switch { subject, arms } => ex(subject) || arms.iter().any(|a| blk(&a.body)),
        StmtKind::Try { body, catches } => blk(body) || catches.iter().any(|c| blk(&c.body)),
        _ => false,
    }
}

fn expr_mutates(e: &Expr, name: &str) -> bool {
    let ex = |x: &Expr| expr_mutates(x, name);
    match &e.kind {
        // `name.m(..)`: the method may change what it is called on
        ExprKind::Call(f, args) => matches!(&f.kind, ExprKind::Field(b, _) if root_is(b, name)) || ex(f) || args.iter().any(ex),
        ExprKind::Field(a, _) | ExprKind::Unary(_, a) | ExprKind::Cast(a, _) => ex(a),
        ExprKind::Index(a, b) | ExprKind::Binary(_, a, b) => ex(a) || ex(b),
        ExprKind::Cond(a, b, c) => ex(a) || ex(b) || ex(c),
        _ => false,
    }
}
