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
    let mutable = mutable_fields(prog, &writes);
    let borrowed: Vec<Vec<bool>> = prog.fns.iter().map(|f| f.borrowed.clone()).collect();
    // needs[f][k]: what f needs its caller to hold for its receiver (k = 0) and parameter k - 1: 0 nothing, 1 a sync, 2 a mutate.
    // An access through a parameter that no block of f's own covers raises it; calls pass it on. Iterate until nothing grows.
    let mut needs: Vec<Vec<u8>> = prog.fns.iter().map(|f| vec![0; f.params.len() + 1]).collect();
    let run = |needs: &Vec<Vec<u8>>, fresh: &Vec<bool>, report: bool| -> (Vec<Finding>, Vec<(FnId, usize, u8)>, Vec<bool>) {
        let mut out: Vec<Finding> = Vec::new();
        let mut hits: Vec<(FnId, usize, u8)> = Vec::new();
        let mut fresh_out: Vec<bool> = vec![false; prog.fns.len()];
        for (fid, f) in prog.fns.iter().enumerate() {
            let Some(body) = &f.body else { continue };
            if f.name.starts_with("init ") || f.name.starts_with("__") {
                continue;
            }
            let mut cx = Cx { prog, fid, mutable: &mutable, needs, fresh, ret_fresh: None, borrowed: &borrowed, open: Vec::new(), locals: HashMap::new(), out: Vec::new(), hits: Vec::new() };
            cx.block(body);
            fresh_out[fid] = matches!(f.ret, Ty::Ref(_)) && cx.ret_fresh == Some(true);
            hits.extend(cx.hits);
            if report {
                let mut seen: HashSet<(u32, String)> = HashSet::new();
                for fi in cx.out {
                    if seen.insert((fi.line, fi.msg.clone())) {
                        out.push(fi);
                    }
                }
            }
        }
        (out, hits, fresh_out)
    };
    // fresh results first: a function is fresh if every return gives back a new object nothing else refers to (or a fresh call's result)
    let mut fresh: Vec<bool> = vec![false; prog.fns.len()];
    loop {
        let (_, _, f2) = run(&needs, &fresh, false);
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
        let (_, hits, _) = run(&needs, &fresh, false);
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
    run(&needs, &fresh, true).0
}

/// Fields written outside construction: `(struct, field index)`.
fn mutable_fields(prog: &Program, writes: &[Vec<bool>]) -> HashSet<(usize, usize)> {
    let mut m: HashSet<(usize, usize)> = HashSet::new();
    fn mark(e: &TExpr, m: &mut HashSet<(usize, usize)>) {
        match &e.kind {
            EK::Field(b, idx) => {
                if let Some(s) = sid_of(&b.ty) {
                    m.insert((s, *idx));
                }
                mark(b, m);
            }
            EK::Index(b, _) | EK::Deref(b) | EK::Len(b) | EK::Unwrap(b) => mark(b, m),
            _ => {}
        }
    }
    for f in &prog.fns {
        let Some(body) = &f.body else { continue };
        walk_stmts(body, &mut |s| {
            if let TS::Assign(t, _) | TS::AssignOp(_, t, _) = &s.kind {
                mark(t, &mut m);
            }
        });
        visit_block(body, &mut |e| {
            visit_expr(e, &mut |x| match &x.kind {
                EK::RtCall(n, args) if matches!(*n, "appendArr" | "appendOne" | "grow" | "moveWithin" | "copyFrom") => {
                    if let Some(a) = args.first() {
                        mark(a, &mut m);
                    }
                }
                EK::Call(c, args) => {
                    for (k, a) in args.iter().enumerate() {
                        if writes[*c].get(k + 1).copied().unwrap_or(false) {
                            mark(a, &mut m);
                        }
                    }
                }
                EK::Method(c, r, args) => {
                    if writes[*c].first().copied().unwrap_or(false) {
                        mark(r, &mut m);
                    }
                    for (k, a) in args.iter().enumerate() {
                        if writes[*c].get(k + 1).copied().unwrap_or(false) {
                            mark(a, &mut m);
                        }
                    }
                }
                _ => {}
            })
        });
    }
    m
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
        match &e.kind {
            EK::NewObj(..) => true,
            EK::Call(f, _) | EK::Method(f, _, _) => self.fresh[*f],
            EK::Var(id) | EK::ObjRef(id) => self.locals.get(id) == Some(&false),
            EK::BoxVal(i) | EK::Borrow(i) => self.is_fresh_value(i),
            _ => false,
        }
    }

    fn stmt(&mut self, s: &TStmt) {
        match &s.kind {
            TS::Let(id, e) => {
                self.expr(e, s.line, Need::Read);
                let fresh = self.is_fresh_value(e);
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
                let fr = self.is_fresh_value(e);
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
                format!("sync (reads {}, writes {})", inner, outer)
            } else {
                format!("sync (reads {}, writes {})", outer, inner)
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
            EK::Index(b, _) | EK::Deref(b) | EK::Len(b) | EK::Unwrap(b) => self.access_spine(b, need, line),
            _ => {}
        }
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
            if nodes.iter().any(|x| same_place(x, t)) || same_place(n, t) {
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
