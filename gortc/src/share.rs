//! Access checking for shared objects (docs/sharing.md, R1 to R4).
//!
//! Reading or writing through a `Ref<T>` needs a `sync` / `mutate` block naming that reference when the object is not provably local
//! and the field that is touched is mutable. This pass finds the mutable fields (any field written outside construction), follows which
//! locally created objects have not escaped yet, and checks every access against the blocks that enclose it.
//!
//! Approximations (all in the direction of reporting too much, except the last):
//! - a field is mutable if any code anywhere assigns it, grows it, or calls something that writes it; filling a field in after `new` counts;
//! - an object is local while it was made with `new` in this function and its handle has not been stored, captured, passed to a function
//!   that keeps it, or copied; a loop that escapes it anywhere counts as escaping it throughout;
//! - a function result is never `fresh`, so an object from `Db.make()` is incoming until a block is written;
//! - a block covers an access if it names the reference, or any reference on the way to it (`sync(store)` covers `store.entries.get(k)`),
//!   which is the documented rule, though the inner object has a lock of its own that this does not take;
//! - objects reached through a `Ref` field of a local object are taken as local while the root is (not tracked: a field assigned from an
//!   incoming reference).

use crate::hir::*;
use crate::sema::{escapes, param_writes, place_name, same_place, visit_block, visit_expr, walk_stmts};
use std::collections::{HashMap, HashSet};

pub struct Finding {
    pub file: String,
    pub line: u32,
    pub msg: String,
    /// a warning, not an error: the program is accepted
    pub warn: bool,
}

fn sid_of(t: &Ty) -> Option<usize> {
    match t {
        Ty::Struct(s) | Ty::Ref(s) | Ty::Obj(s) => Some(*s),
        Ty::Opt(i) => sid_of(i),
        _ => None,
    }
}

pub fn check(prog: &Program) -> Vec<Finding> {
    let writes = param_writes(prog);
    let (mutable, arr_mutable, garr_mutable) = mutable_fields(prog, &writes);
    let borrowed: Vec<Vec<bool>> = prog.fns.iter().map(|f| f.borrowed.clone()).collect();
    // needs[f][k]: what f needs its caller to hold for its receiver (k = 0) and parameter k - 1: 0 nothing, 1 a sync, 2 a mutate.
    // An access through a parameter that no block of f's own covers raises it; calls pass it on. Iterate until nothing grows.
    let mut needs: Vec<Vec<u8>> = prog.fns.iter().map(|f| vec![0; f.params.len() + 1]).collect();
    let run = |needs: &Vec<Vec<u8>>, fresh: &Vec<bool>, report: bool| -> (Vec<Finding>, Vec<(FnId, usize, u8)>, Vec<bool>, Vec<Style>) {
        let mut out: Vec<Finding> = Vec::new();
        let mut hits: Vec<(FnId, usize, u8)> = Vec::new();
        let mut fresh_out: Vec<bool> = vec![false; prog.fns.len()];
        let mut styles_out: Vec<Style> = Vec::new();
        for (fid, f) in prog.fns.iter().enumerate() {
            let Some(body) = &f.body else { continue };
            if f.name.starts_with("init ") || f.name.starts_with("__") {
                continue;
            }
            let mut cx = Cx { prog, fid, mutable: &mutable, arr_mutable: &arr_mutable, garr_mutable: &garr_mutable, needs, fresh, ret_fresh: None, borrowed: &borrowed, open: Vec::new(), locals: HashMap::new(), shared_arrs: HashMap::new(), var_field: HashMap::new(), styles: Vec::new(), out: Vec::new(), hits: Vec::new() };
            cx.block(body);
            fresh_out[fid] = matches!(f.ret, Ty::Ref(_) | Ty::Arr(_)) && cx.ret_fresh == Some(true);
            hits.extend(cx.hits);
            styles_out.extend(cx.styles);
            if report {
                let mut seen: HashSet<(u32, String)> = HashSet::new();
                for fi in cx.out {
                    if seen.insert((fi.line, fi.msg.clone())) {
                        out.push(fi);
                    }
                }
            }
        }
        (out, hits, fresh_out, styles_out)
    };
    // fresh results first: a function is fresh if every return gives back a new object nothing else refers to (or a fresh call's result)
    let mut fresh: Vec<bool> = vec![false; prog.fns.len()];
    loop {
        let (_, _, f2, _) = run(&needs, &fresh, false);
        let mut changed = false;
        for i in 0..fresh.len() {
            if f2[i] && !fresh[i] {
                fresh[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    loop {
        let (_, hits, _, _) = run(&needs, &fresh, false);
        let mut changed = false;
        for (f, k, l) in hits {
            if needs[f][k] < l {
                needs[f][k] = l;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let (mut out, _, _, styles) = run(&needs, &fresh, true);
    // one array, two locks: a block on the holder (a receiver or parameter, covered by the caller) and a block on the array itself do not
    // exclude each other, so an array field must be locked one way throughout
    let owner_keys: HashSet<(usize, usize)> = styles.iter().filter(|x| !x.direct).map(|x| x.key).collect();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    for st in styles.iter().filter(|x| x.direct && owner_keys.contains(&x.key)) {
        if seen.insert(st.key) {
            let (sid, idx) = st.key;
            out.push(Finding {
                file: st.file.clone(),
                line: st.line,
                msg: format!("`{0}.{1}` is locked through its holder elsewhere (a receiver or parameter) and by a block on the array itself here; the two locks do not exclude each other: use `sync` on the holder everywhere, or on the array everywhere", prog.structs[sid].name, prog.structs[sid].fields[idx].name),
                warn: false,
            });
        }
    }
    out
}

/// How an access to an array field was covered: by a block on the array itself (`direct`) or by the caller's block on the holder.
pub struct Style {
    key: (usize, usize),
    direct: bool,
    line: u32,
    file: String,
}

/// Fields written outside construction: `(struct, field index)`, and separately the array fields whose array is changed in place.
fn mutable_fields(prog: &Program, writes: &[Vec<bool>]) -> (HashSet<(usize, usize)>, HashSet<(usize, usize)>, HashSet<usize>) {
    struct M {
        fields: HashSet<(usize, usize)>,
        arrs: HashSet<(usize, usize)>,
        garrs: HashSet<usize>,
    }
    /// `e` is assigned, or something inside it is
    fn assign(e: &TExpr, m: &mut M) {
        match &e.kind {
            EK::Field(b, idx) => {
                if let Some(s) = sid_of(&b.ty) {
                    m.fields.insert((s, *idx));
                }
                assign(b, m);
            }
            EK::Index(b, _) | EK::Len(b) if matches!(b.ty, Ty::Arr(_)) => arr(b, m),
            EK::Index(b, _) | EK::Deref(b) | EK::Len(b) | EK::Unwrap(b) => assign(b, m),
            _ => {}
        }
    }
    /// the array `e` points to is changed in place (the field that holds it is not)
    fn arr(e: &TExpr, m: &mut M) {
        match &strip(e).kind {
            EK::Field(b, idx) => {
                if let Some(s) = sid_of(&b.ty) {
                    m.arrs.insert((s, *idx));
                }
            }
            EK::Global(g) => {
                m.garrs.insert(*g);
            }
            _ => {}
        }
    }
    fn touch(e: &TExpr, m: &mut M) {
        if matches!(e.ty, Ty::Arr(_)) { arr(e, m) } else { assign(e, m) }
    }
    let mut m = M { fields: HashSet::new(), arrs: HashSet::new(), garrs: HashSet::new() };
    for f in &prog.fns {
        let Some(body) = &f.body else { continue };
        // a handle copied out of a field and then written (`let xs = g.xs; xs[0] = 1`) writes that field's array
        let mut written: HashSet<VarId> = HashSet::new();
        fn root_arr(e: &TExpr) -> Option<VarId> {
            match &e.kind {
                EK::Index(b, _) | EK::Len(b) if matches!(b.ty, Ty::Arr(_)) => match &b.kind {
                    EK::Var(v) => Some(*v),
                    _ => None,
                },
                EK::Field(b, _) | EK::Index(b, _) | EK::Deref(b) | EK::Len(b) | EK::Unwrap(b) => root_arr(b),
                _ => None,
            }
        }
        walk_stmts(body, &mut |s| {
            if let TS::Assign(t, _) | TS::AssignOp(_, t, _) = &s.kind {
                assign(t, &mut m);
                if let Some(v) = root_arr(t) {
                    written.insert(v);
                }
            }
        });
        visit_block(body, &mut |e| {
            visit_expr(e, &mut |x| match &x.kind {
                EK::RtCall(n, args) if matches!(*n, "appendArr" | "appendOne" | "grow" | "moveWithin" | "copyFrom") => {
                    if let Some(a) = args.first() {
                        touch(a, &mut m);
                        if let EK::Var(v) = &strip(a).kind {
                            written.insert(*v);
                        }
                    }
                }
                EK::Call(c, args) => {
                    for (k, a) in args.iter().enumerate() {
                        if writes[*c].get(k + 1).copied().unwrap_or(false) {
                            touch(a, &mut m);
                        }
                    }
                }
                EK::Method(c, r, args) => {
                    if writes[*c].first().copied().unwrap_or(false) {
                        touch(r, &mut m);
                    }
                    for (k, a) in args.iter().enumerate() {
                        if writes[*c].get(k + 1).copied().unwrap_or(false) {
                            touch(a, &mut m);
                        }
                    }
                }
                _ => {}
            })
        });
        walk_stmts(body, &mut |s| {
            if let TS::Let(id, e) = &s.kind {
                if written.contains(id) {
                    arr(e, &mut m);
                }
            }
        });
    }
    (m.fields, m.arrs, m.garrs)
}

#[derive(Clone, Copy, PartialEq)]
enum Need {
    Read,
    Write,
}

struct Cx<'a> {
    prog: &'a Program,
    fid: FnId,
    mutable: &'a HashSet<(usize, usize)>,
    /// array fields whose array is changed in place somewhere
    arr_mutable: &'a HashSet<(usize, usize)>,
    /// global arrays changed in place somewhere
    garr_mutable: &'a HashSet<usize>,
    needs: &'a [Vec<u8>],
    /// functions whose result is a new object nothing else refers to
    fresh: &'a [bool],
    /// every `return` so far returned a fresh value (None: no return seen yet)
    ret_fresh: Option<bool>,
    borrowed: &'a [Vec<bool>],
    /// the enclosing sync / mutate blocks: (is mutate, target)
    open: Vec<(bool, TExpr)>,
    /// variables bound to a fresh `new` in this function: has the handle escaped yet?
    locals: HashMap<VarId, bool>,
    /// variables holding a copy of an array handle taken from a place that other code can reach (a field, a global, a parameter, another such copy)
    shared_arrs: HashMap<VarId, Option<usize>>,
    /// the array field a copied-out handle came from
    var_field: HashMap<VarId, (usize, usize)>,
    styles: Vec<Style>,
    out: Vec<Finding>,
    /// accesses through this function's own receiver or parameters that no block of its own covers: (function, slot, level)
    hits: Vec<(FnId, usize, u8)>,
}

impl<'a> Cx<'a> {
    fn f(&self) -> &'a FnInfo {
        &self.prog.fns[self.fid]
    }

    fn name_of(&self, e: &TExpr) -> String {
        place_name(self.prog, self.f(), e)
    }

    // ---- statements, in order ----

    fn block(&mut self, b: &TBlock) {
        for s in &b.stmts {
            self.stmt(s);
        }
    }

    /// Marks every local whose handle escapes somewhere in `exprs`.
    fn escape_in(&mut self, exprs: &[&TExpr]) {
        let ids: Vec<VarId> = self.locals.iter().filter(|(_, e)| !**e).map(|(i, _)| *i).collect();
        for id in ids {
            let mut esc = false;
            for e in exprs {
                if escapes(e, id, false, self.borrowed) {
                    esc = true;
                }
            }
            if esc {
                self.locals.insert(id, true);
            }
        }
    }

    /// Marks every local whose handle escapes anywhere in the block (used before a loop: an escape on a later turn reaches the earlier part).
    fn escape_in_block(&mut self, b: &TBlock, extra: &[&TExpr]) {
        let ids: Vec<VarId> = self.locals.iter().filter(|(_, e)| !**e).map(|(i, _)| *i).collect();
        for id in ids {
            let mut esc = false;
            visit_block(b, &mut |e| {
                if escapes(e, id, false, self.borrowed) {
                    esc = true;
                }
            });
            for e in extra {
                if escapes(e, id, false, self.borrowed) {
                    esc = true;
                }
            }
            if esc {
                self.locals.insert(id, true);
            }
        }
    }

    /// Is `e` a new object that nothing else refers to? A `new`, the result of a fresh function, or a local that has not escaped.
    fn is_fresh_value(&self, e: &TExpr) -> bool {
        if matches!(e.ty, Ty::Arr(_)) {
            return self.is_fresh_arr(e);
        }
        match &e.kind {
            EK::NewObj(..) => true,
            EK::Call(f, _) | EK::Method(f, _, _) => self.fresh[*f],
            EK::Var(id) | EK::ObjRef(id) => self.locals.get(id) == Some(&false),
            EK::BoxVal(i) | EK::Borrow(i) => self.is_fresh_value(i),
            _ => false,
        }
    }

    /// Is the array handle `e` a new array that nothing else refers to? A function that returns a stored array (`return self.xs`) is not
    /// fresh, so its result is as shared as the field it came from.
    fn is_fresh_arr(&self, e: &TExpr) -> bool {
        match &strip(e).kind {
            EK::NewArr(..) | EK::ArrLit(..) | EK::RtCall(..) | EK::Builtin(..) | EK::Json(..) => true,
            EK::Call(f, args) | EK::Method(f, _, args) => {
                // an extern function gets a raw pointer, so it may hand back the array it was given or a new one: the result is fresh only
                // when every array argument is local too
                if self.prog.fns[*f].body.is_none() {
                    return args.iter().all(|a| !matches!(a.ty, Ty::Arr(_)) || self.arr_origin(a).is_none());
                }
                // a function that returns (a view of) its parameter gives back an array that is as local as its arguments
                self.fresh[*f] && (!self.returns_param_array(*f) || args.iter().all(|a| !matches!(a.ty, Ty::Arr(_)) || self.arr_origin(a).is_none()))
            }
            EK::Var(v) => !self.f().params.contains(v) && !self.shared_arrs.contains_key(v),
            EK::Slice(b, ..) => self.is_fresh_arr(b),
            EK::Cond(_, a, b) => self.is_fresh_arr(a) && self.is_fresh_arr(b),
            _ => false,
        }
    }

    /// Is `e` a parameter's array, or a view of it?
    fn arr_param_derived(&self, e: &TExpr) -> bool {
        match &strip(e).kind {
            EK::Var(v) => self.f().params.contains(v),
            EK::Slice(b, ..) => self.arr_param_derived(b),
            _ => false,
        }
    }

    /// Does any `return` in function `f` give back an array derived from one of its parameters?
    fn returns_param_array(&self, f: FnId) -> bool {
        fn derived(e: &TExpr, params: &[VarId]) -> bool {
            match &strip(e).kind {
                EK::Var(v) => params.contains(v),
                EK::Slice(b, ..) => derived(b, params),
                _ => false,
            }
        }
        let info = &self.prog.fns[f];
        let Some(body) = &info.body else { return false };
        let mut hit = false;
        walk_stmts(body, &mut |s| {
            if let TS::Return(Some(e)) = &s.kind {
                if matches!(e.ty, Ty::Arr(_)) && derived(e, &info.params) {
                    hit = true;
                }
            }
        });
        hit
    }

    fn stmt(&mut self, s: &TStmt) {
        match &s.kind {
            TS::Let(id, e) => {
                self.expr(e, s.line, Need::Read);
                if matches!(e.ty, Ty::Arr(_)) {
                    if let Some(o) = self.arr_origin(e) {
                        self.shared_arrs.insert(*id, o);
                        if let Some(k) = self.arr_key(e) {
                            self.var_field.insert(*id, k);
                        }
                    }
                }
                // a new name for an existing local is a second handle on the object, not a new object: only an allocation starts a local
                let fresh = self.is_fresh_value(e) && !matches!(strip(e).kind, EK::Var(_) | EK::ObjRef(_));
                self.escape_in(&[e]);
                if fresh {
                    self.locals.insert(*id, false);
                }
            }
            TS::LetObj(id, _, fs) => {
                let es: Vec<&TExpr> = fs.iter().map(|(_, e)| e).collect();
                for e in &es {
                    self.expr(e, s.line, Need::Read);
                }
                self.escape_in(&es);
                self.locals.insert(*id, false);
            }
            TS::Return(Some(e)) => {
                self.expr(e, s.line, Need::Read);
                // an array derived from a parameter (`return a[lo..hi]`) is as local as the argument was: checked at the call
                let fr = self.is_fresh_value(e) || (matches!(e.ty, Ty::Arr(_)) && self.arr_param_derived(e));
                self.ret_fresh = Some(self.ret_fresh.unwrap_or(true) && fr);
                self.escape_in(&[e]);
            }
            TS::Expr(e) | TS::SetGlobal(_, e) => {
                self.expr(e, s.line, Need::Read);
                self.escape_in(&[e]);
            }
            TS::Throw(_, fs) => {
                let es: Vec<&TExpr> = fs.iter().map(|(_, e)| e).collect();
                for e in &es {
                    self.expr(e, s.line, Need::Read);
                }
                self.escape_in(&es);
            }
            TS::Assign(t, v) | TS::AssignOp(_, t, v) => {
                if let EK::Var(id) = &t.kind {
                    if matches!(t.ty, Ty::Arr(_)) {
                        if let Some(o) = self.arr_origin(v) {
                            self.shared_arrs.insert(*id, o);
                            if let Some(k) = self.arr_key(v) {
                                self.var_field.insert(*id, k);
                            }
                        }
                    }
                }
                self.expr(v, s.line, Need::Read);
                self.expr(t, s.line, Need::Write);
                self.escape_in(&[v, t]);
            }
            TS::If(c, t, e) => {
                self.expr(c, s.line, Need::Read);
                self.escape_in(&[c]);
                let before = self.locals.clone();
                self.block(t);
                let after_then = std::mem::replace(&mut self.locals, before);
                if let Some(e) = e {
                    self.block(e);
                }
                for (id, esc) in after_then {
                    if esc {
                        self.locals.insert(id, true);
                    }
                }
            }
            TS::While(c, b) => {
                self.expr(c, s.line, Need::Read);
                self.escape_in_block(b, &[c]);
                self.block(b);
            }
            TS::Loop(b) => {
                self.escape_in_block(b, &[]);
                self.block(b);
            }
            TS::Block(b) => self.block(b),
            TS::Labeled(i) => self.stmt(i),
            TS::Try(b, cs) => {
                self.block(b);
                for c in cs {
                    self.block(&c.body);
                }
            }
            TS::Sync(ts, b) => {
                self.warn_nested(ts, s.line);
                for (m, t) in ts {
                    self.expr(t, s.line, Need::Read);      // naming an object to lock it does not let it escape
                    self.open.push((*m, t.clone()));
                }
                self.block(b);
                for _ in ts {
                    self.open.pop();
                }
            }
            TS::Return(None) | TS::Break | TS::Continue | TS::BreakL(..) => {}
        }
    }

    /// A block nested inside another block on a different object: two threads that nest the same two objects in opposite orders deadlock.
    /// One block naming both locks them in a fixed order, no matter how they are written.
    fn warn_nested(&mut self, targets: &[(bool, TExpr)], line: u32) {
        for (mode, t) in targets {
            let Some((om, o)) = self.open.iter().find(|(_, o)| !same_place(o, t)) else { continue };
            let (inner, outer) = (self.name_of(t), self.name_of(o));
            let one = |m: bool, n: &str| format!("sync ({} {})", if m { "writes" } else { "reads" }, n);
            // the grouped form: one clause when the modes agree, reads and writes clauses when they differ
            let group = if *om == *mode {
                format!("sync ({} {}, {})", if *mode { "writes" } else { "reads" }, outer, inner)
            } else if *om {
                format!("sync (reads {}; writes {})", inner, outer)
            } else {
                format!("sync (reads {}; writes {})", outer, inner)
            };
            let msg = format!(
                "`{}` is nested inside `{}`, two different objects: a thread that nests them the other way round deadlocks; lock both in one block, `{}`, so the order is the same no matter how they are written",
                one(*mode, &inner), one(*om, &outer), group
            );
            self.out.push(Finding { file: self.f().file.clone(), line, msg, warn: true });
            return;
        }
    }

    // ---- accesses ----

    fn expr(&mut self, e: &TExpr, line: u32, need: Need) {
        // a write target: the whole spine is written; everything else in it (indices, calls) is read
        if need == Need::Write {
            self.access_spine(e, Need::Write, line);
            self.write_operands(e, line);
            return;
        }
        let mut stack: Vec<&TExpr> = vec![e];
        while let Some(x) = stack.pop() {
            match &x.kind {
                EK::Field(b, idx) => {
                    if let Some(s) = sid_of(&b.ty) {
                        if !self.prog.structs[s].is_extern && self.mutable.contains(&(s, *idx)) {
                            self.access(b, Need::Read, line, &format!("reading `{}.{}`", self.prog.structs[s].name, self.prog.structs[s].fields[*idx].name));
                        }
                    }
                }
                EK::Index(b, _) | EK::Len(b) | EK::Slice(b, ..) if matches!(b.ty, Ty::Arr(_)) => {
                    self.access_arr(b, Need::Read, line, "reading an array element");
                }
                EK::RtCall(n, args) => {
                    let mutator = matches!(*n, "appendArr" | "appendOne" | "grow" | "moveWithin" | "copyFrom");
                    for (k, a) in args.iter().enumerate() {
                        if matches!(a.ty, Ty::Arr(_)) {
                            let need = if mutator && k == 0 { Need::Write } else { Need::Read };
                            self.access_arr(a, need, line, if need == Need::Write { "changing an array" } else { "reading an array" });
                        }
                    }
                }
                EK::Call(fid, args) => {
                    for (j, a) in args.iter().enumerate() {
                        self.call_arg(*fid, j + 1, a, line);
                    }
                }
                EK::Method(fid, r, args) => {
                    for (j, a) in args.iter().enumerate() {
                        self.call_arg(*fid, j + 1, a, line);
                    }
                    if let Some(s) = sid_of(&r.ty) {
                        if !self.prog.structs[s].is_extern {
                            let lvl = self.needs[*fid].first().copied().unwrap_or(0);
                            let desc = |w: &str| format!("calling `{}.{}`, which {} its receiver,", self.prog.structs[s].name, self.prog.fns[*fid].name, w);
                            if lvl == 2 {
                                self.access(r, Need::Write, line, &desc("writes"));
                            } else if lvl == 1 {
                                self.access(r, Need::Read, line, &desc("reads mutable fields of"));
                            }
                        }
                    }
                }
                _ => {}
            }
            // children
            match &x.kind {
                EK::Field(a, _) | EK::Deref(a) | EK::Borrow(a) | EK::BoxVal(a) | EK::Len(a) | EK::ExternFn(a) | EK::Unary(_, a) | EK::Cast(a) | EK::Some(a) | EK::IsNull(a) | EK::Unwrap(a) | EK::NewArr(_, a) => stack.push(a),
                EK::Index(a, b) | EK::Binary(_, a, b) => {
                    stack.push(a);
                    stack.push(b);
                }
                EK::Slice(a, lo, hi) => {
                    stack.push(a);
                    stack.extend(lo.iter().map(|y| &**y));
                    stack.extend(hi.iter().map(|y| &**y));
                }
                EK::Call(_, args) | EK::Lambda(_, args) | EK::Builtin(_, args) | EK::ArrLit(_, args) | EK::RtCall(_, args) => stack.extend(args.iter()),
                EK::CallInd(f, args) => {
                    stack.push(f);
                    stack.extend(args.iter());
                }
                EK::Method(_, r, args) => {
                    stack.push(r);
                    stack.extend(args.iter());
                }
                EK::StructLit(_, fs) | EK::NewObj(_, fs) => stack.extend(fs.iter().map(|(_, y)| y)),
                EK::Json(_, b, l) => {
                    stack.push(b);
                    stack.extend(l.iter().map(|y| &**y));
                }
                EK::Cond(a, b, c) => {
                    stack.push(a);
                    stack.push(b);
                    stack.push(c);
                }
                _ => {}
            }
        }
    }

    /// An argument that is a `Ref`: the callee reads or writes through it as its summary says, so the caller must hold a block of that strength.
    fn call_arg(&mut self, fid: FnId, slot: usize, a: &TExpr, line: u32) {
        if matches!(a.ty, Ty::Arr(_)) {
            let lvl = self.needs[fid].get(slot).copied().unwrap_or(0);
            if lvl > 0 {
                let what = format!("passing it to `{}`, which {} through it,", self.prog.fns[fid].name, if lvl == 2 { "writes" } else { "reads" });
                self.access_arr(a, if lvl == 2 { Need::Write } else { Need::Read }, line, &what);
            }
            return;
        }
        let Some(sid) = (match &a.ty {
            Ty::Ref(s) | Ty::Obj(s) => Some(*s),
            _ => None,
        }) else { return };
        // a value lent by address: the place itself is reached (a local variable is nobody else's; a field of a shared object is)
        let lent = match &a.kind {
            EK::Borrow(i) | EK::BoxVal(i) if matches!(i.ty, Ty::Struct(_)) => Some(&**i),
            _ => None,
        };
        if self.prog.structs[sid].is_extern {
            return;
        }
        let lvl = self.needs[fid].get(slot).copied().unwrap_or(0);
        if lvl == 0 || self.is_fresh_value(a) {
            return;                               // nothing needed, or a new object nobody else has
        }
        let what = format!("passing it to `{}`, which {} through it,", self.prog.fns[fid].name, if lvl == 2 { "writes" } else { "reads mutable fields" });
        let need = if lvl == 2 { Need::Write } else { Need::Read };
        match lent {
            Some(i) => self.access(i, need, line, &what),
            None => self.access_ref(a, need, line, &what),
        }
    }

    /// The parts of an assignment target that are read, not written: index operands, and a root that is not a place.
    fn write_operands(&mut self, e: &TExpr, line: u32) {
        match &e.kind {
            EK::Field(b, _) | EK::Deref(b) | EK::Len(b) | EK::Unwrap(b) => self.write_operands(b, line),
            EK::Index(b, i) => {
                self.write_operands(b, line);
                self.expr(i, line, Need::Read);
            }
            EK::Var(_) | EK::SelfPlace | EK::ObjPlace(_) | EK::ObjRef(_) | EK::Global(_) => {}
            _ => self.expr(e, line, Need::Read),
        }
    }

    /// An assignment target: each mutable field along its spine needs the object locked for writing.
    fn access_spine(&mut self, e: &TExpr, need: Need, line: u32) {
        match &e.kind {
            EK::Field(b, idx) => {
                if let Some(s) = sid_of(&b.ty) {
                    if !self.prog.structs[s].is_extern && self.mutable.contains(&(s, *idx)) {
                        self.access(b, need, line, &format!("writing `{}.{}`", self.prog.structs[s].name, self.prog.structs[s].fields[*idx].name));
                    }
                }
                self.access_spine(b, need, line);
            }
            EK::Index(b, _) | EK::Deref(b) | EK::Len(b) | EK::Unwrap(b) => {
                if matches!(b.ty, Ty::Arr(_)) {
                    // storing into an array changes it; picking a handle out of one (`xs[i].n = 1`) only reads it
                    let n = if matches!(e.ty, Ty::Ref(_) | Ty::Obj(_)) { Need::Read } else { need };
                    self.access_arr(b, n, line, if n == Need::Write { "changing an array" } else { "reading an array element" });
                    // the array is its own object: the object that holds it is only read to get the handle
                    return self.access_spine(b, Need::Read, line);
                }
                self.access_spine(b, need, line)
            }
            _ => {}
        }
    }

    /// The array field `e` is, or was copied from.
    fn arr_key(&self, e: &TExpr) -> Option<(usize, usize)> {
        match &strip(e).kind {
            EK::Field(b, idx) => sid_of(&b.ty).map(|s| (s, *idx)),
            EK::Var(v) => self.var_field.get(v).copied(),
            EK::Slice(b, ..) => self.arr_key(b),
            _ => None,
        }
    }

    /// Can other code reach the array handle `e` too? None: no, it is this function's own. Some(None): yes, and this function must lock the
    /// array itself. Some(Some(slot)): it comes from this function's receiver (slot 0) or parameter (slot k + 1); the caller's block covers it.
    fn arr_origin(&self, e: &TExpr) -> Option<Option<usize>> {
        match &strip(e).kind {
            EK::Field(b, _) => {
                // a struct held by value is nobody else's: the owner must be reached through a handle
                let mut n: &TExpr = b;
                let mut hop = false;
                loop {
                    hop |= matches!(n.ty, Ty::Ref(_) | Ty::Obj(_)) || matches!(n.kind, EK::Deref(_) | EK::ObjRef(_) | EK::ObjPlace(_));
                    match &n.kind {
                        EK::Field(x, _) | EK::Index(x, _) | EK::Len(x) | EK::Unwrap(x) | EK::Borrow(x) | EK::BoxVal(x) | EK::Deref(x) => n = x,
                        _ => break,
                    }
                }
                match &n.kind {
                    EK::SelfPlace => Some(Some(0)),
                    EK::Var(v) | EK::ObjRef(v) | EK::ObjPlace(v) => {
                        if !hop || self.locals.get(v) == Some(&false) {
                            None
                        } else if let Some(k) = self.f().params.iter().position(|p| p == v) {
                            Some(Some(k + 1))
                        } else {
                            Some(None)
                        }
                    }
                    _ => if hop { Some(None) } else { None },
                }
            }
            EK::Global(_) => Some(None),
            EK::Var(v) => match self.shared_arrs.get(v) {
                Some(o) => Some(*o),
                None => self.f().params.iter().position(|p| p == v).map(|k| Some(k + 1)),
            },
            EK::Slice(b, ..) => self.arr_origin(b),
            EK::Call(..) | EK::Method(..) | EK::Cond(..) => if self.is_fresh_arr(e) { None } else { Some(None) },
            _ => None,
        }
    }

    /// The array `a` points to is read or written. An array is an object of its own: a block must name it (a block on the object that holds it
    /// does not cover it), unless it came in through this function's receiver or parameter (the caller covers it, as for a `Ref` field), or
    /// is not reachable from anywhere else.
    fn access_arr(&mut self, a: &TExpr, need: Need, line: u32, what: &str) {
        let a = strip(a);
        let want_mutate = need == Need::Write;
        if let EK::Slice(b, ..) = &a.kind {
            return self.access_arr(b, need, line, what);
        }
        match &a.kind {
            EK::Field(b, idx) if !want_mutate => match sid_of(&b.ty) {
                Some(s) if self.arr_mutable.contains(&(s, *idx)) || self.mutable.contains(&(s, *idx)) => {}
                _ => return,                              // nothing ever writes this field
            },
            EK::Global(g) if !want_mutate && !self.garr_mutable.contains(g) => return,
            EK::Field(..) | EK::Global(_) | EK::Var(_) => {}
            _ => return,
        }
        let Some(origin) = self.arr_origin(a) else { return };
        let key = self.arr_key(a);
        let mut weak: Option<String> = None;
        for (m, t) in &self.open {
            if same_place(a, t) {
                if *m || !want_mutate {
                    if let Some(key) = key {
                        self.styles.push(Style { key, direct: true, line, file: self.f().file.clone() });
                    }
                    return;
                }
                weak = Some(self.name_of(t));
            }
        }
        if let (None, Some(slot)) = (&weak, origin) {
            if let Some(key) = key {
                self.styles.push(Style { key, direct: false, line, file: self.f().file.clone() });
            }
            self.hits.push((self.fid, slot, if want_mutate { 2 } else { 1 }));
            return;
        }
        self.arr_error(a, want_mutate, weak, line, what);
    }

    fn arr_error(&mut self, a: &TExpr, want_mutate: bool, weak: Option<String>, line: u32, what: &str) {
        let kw = if want_mutate { "writes" } else { "reads" };
        let msg = match weak {
            Some(w) => format!("{} needs writes {}, but it is listed under reads", what, w),
            None => format!("{} needs `sync ({} {})`: the array is not local and is mutable", what, kw, self.name_of(a)),
        };
        self.out.push(Finding { file: self.f().file.clone(), line, msg, warn: false });
    }

    /// The body reached through place `p` is read or written. Is that allowed here?
    fn access(&mut self, p: &TExpr, need: Need, line: u32, what: &str) {
        self.access_from(p, false, need, line, what)
    }

    /// `r` is a reference expression: the object it points to is read or written.
    fn access_ref(&mut self, r: &TExpr, need: Need, line: u32, what: &str) {
        self.access_from(r, true, need, line, what)
    }

    fn access_from(&mut self, p: &TExpr, via_ref: bool, need: Need, line: u32, what: &str) {
        // walk down to the root, collecting the places on the way
        let mut nodes: Vec<&TExpr> = Vec::new();
        let mut hop = via_ref;
        let mut owner_expr: Option<&TExpr> = if via_ref { Some(p) } else { None };
        let mut n = p;
        loop {
            nodes.push(n);
            match &n.kind {
                EK::Field(b, _) | EK::Index(b, _) | EK::Len(b) | EK::Unwrap(b) | EK::Borrow(b) | EK::BoxVal(b) => n = b,
                EK::Deref(b) => {
                    hop = true;
                    if owner_expr.is_none() {
                        owner_expr = Some(b);
                    }
                    n = b;
                }
                _ => break,
            }
        }
        match &n.kind {
            EK::SelfPlace => {
                // inside a method the caller holds the block: the method needs it of its caller
                self.hits.push((self.fid, 0, if need == Need::Write { 2 } else { 1 }));
                return;
            }
            EK::ObjPlace(_) | EK::ObjRef(_) => hop = true, // an object variable is reached through its handle
            _ => {}
        }
        if !hop {
            return;                                       // a plain value place: no other holder can reach it
        }
        let root_id = match &n.kind {
            EK::Var(v) | EK::ObjRef(v) | EK::ObjPlace(v) => Some(*v),
            _ => None,
        };
        if let Some(id) = root_id {
            if self.locals.get(&id) == Some(&false) {
                return;                                   // provably local so far
            }
            if self.f().vars[id].is_obj && !self.f().vars[id].converted && !self.locals.contains_key(&id) {
                return;                                   // a scope-owned body: never a Ref, so never shared
            }
        }
        let want_mutate = need == Need::Write;
        let mut weak: Option<String> = None;
        for (m, t) in &self.open {
            // A block covers the object its reference names and the by-value places inside it, not another object reached through a field that
            // holds a reference: that one may be held by someone else too, who locks a different object. So no further reference hop may lie
            // between the access and the block's reference (the one hop that opens the named object itself is the allowed one).
            let j = nodes.iter().position(|x| same_place(x, t)).or_else(|| if same_place(n, t) { Some(nodes.len()) } else { None });
            let Some(j) = j else { continue };
            let hops = nodes[..j].iter().filter(|x| matches!(x.kind, EK::Deref(_))).count() + usize::from(via_ref);
            if hops > 1 {
                continue;
            }
            {
                if *m || !want_mutate {
                    return;
                }
                weak = Some(self.name_of(t));
            }
        }
        // the object that is reached first is the caller's when it is this function's own parameter: the caller holds the block
        if weak.is_none() {
            if let Some(o) = owner_expr {
                let mut o = o;
                while let EK::Unwrap(i) = &o.kind {
                    o = i;
                }
                let mut o2 = o;
                while let EK::Borrow(i) | EK::BoxVal(i) = &o2.kind {
                    o2 = i;
                }
                if let EK::Var(v) = &o2.kind {
                    if let Some(k) = self.f().params.iter().position(|p| p == v) {
                        self.hits.push((self.fid, k + 1, if want_mutate { 2 } else { 1 }));
                        return;
                    }
                }
            }
        }
        let owner = self.name_of(owner_expr.unwrap_or(p));
        let kw = if want_mutate { "writes" } else { "reads" };
        let msg = match weak {
            Some(w) => format!("{} needs writes {}, but it is listed under reads", what, w),
            None => format!("{} needs `sync ({} {})`: the object is not local and is mutable", what, kw, owner),
        };
        self.out.push(Finding { file: self.f().file.clone(), line, msg, warn: false });
    }
}

fn strip(mut e: &TExpr) -> &TExpr {
    while let EK::BoxVal(i) | EK::Borrow(i) = &e.kind {
        e = i;
    }
    e
}
