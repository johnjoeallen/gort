//! Typed, name-resolved tree produced by sema and consumed by codegen.
//! Implicit conversions are explicit nodes (ObjRef, ObjPlace, Deref) so codegen never guesses.

use crate::ast::{BinOp, Expr as AstExpr, UnOp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntTy {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    Usize,
    Isize,
}

impl IntTy {
    pub fn from_name(s: &str) -> Option<IntTy> {
        Some(match s {
            "i8" => IntTy::I8,
            "i16" => IntTy::I16,
            "i32" => IntTy::I32,
            "i64" => IntTy::I64,
            "u8" => IntTy::U8,
            "u16" => IntTy::U16,
            "u32" => IntTy::U32,
            "u64" => IntTy::U64,
            "usize" => IntTy::Usize,
            "isize" => IntTy::Isize,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            IntTy::I8 => "i8",
            IntTy::I16 => "i16",
            IntTy::I32 => "i32",
            IntTy::I64 => "i64",
            IntTy::U8 => "u8",
            IntTy::U16 => "u16",
            IntTy::U32 => "u32",
            IntTy::U64 => "u64",
            IntTy::Usize => "usize",
            IntTy::Isize => "isize",
        }
    }
    pub fn c_name(self) -> &'static str {
        match self {
            IntTy::I8 => "int8_t",
            IntTy::I16 => "int16_t",
            IntTy::I32 => "int32_t",
            IntTy::I64 => "int64_t",
            IntTy::U8 => "uint8_t",
            IntTy::U16 => "uint16_t",
            IntTy::U32 => "uint32_t",
            IntTy::U64 => "uint64_t",
            IntTy::Usize => "size_t",
            IntTy::Isize => "ptrdiff_t",
        }
    }
    /// (min, max) as i128
    pub fn range(self) -> (i128, i128) {
        match self {
            IntTy::I8 => (i8::MIN as i128, i8::MAX as i128),
            IntTy::I16 => (i16::MIN as i128, i16::MAX as i128),
            IntTy::I32 => (i32::MIN as i128, i32::MAX as i128),
            IntTy::I64 | IntTy::Isize => (i64::MIN as i128, i64::MAX as i128),
            IntTy::U8 => (0, u8::MAX as i128),
            IntTy::U16 => (0, u16::MAX as i128),
            IntTy::U32 => (0, u32::MAX as i128),
            IntTy::U64 | IntTy::Usize => (0, u64::MAX as i128),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Unit,
    Bool,
    Int(IntTy),
    F64,
    Str,
    /// `Duration` and `Instant`: signed nanoseconds, so they add, subtract and compare as integers
    Dur,
    Inst,
    /// a struct value
    Struct(usize),
    /// `Ref<T>`, a counted handle to a struct object
    Ref(usize),
    /// `Array<T>`, a counted handle (slice 0: scalar elements only)
    Arr(Box<Ty>),
    /// the type of a variable bound to `new T {..}` before it is known whether it is counted (D18 item 5)
    Obj(usize),
    /// `T?`: a handle that may be null, or a scalar or value with a presence flag
    Opt(Box<Ty>),
    /// the type of the literal `null`, before it meets an optional
    Null,
    /// an exception caught by `catch (e: Exception)`: any exception, type erased
    Exc,
    /// a function value: parameter types, result type, whether it throws (a plain pointer; no closures)
    Fn(Vec<Ty>, Box<Ty>, bool),
    Error,
}

/// In a `throws` list or a `catch`: any exception.
pub const ANY_EXC: usize = usize::MAX;

pub type VarId = usize;
pub type FnId = usize;

#[derive(Clone, Debug)]
pub struct VarInfo {
    pub name: String,
    pub ty: Ty,
    pub mutable: bool,
    /// a variable bound to `new T {..}`: true once any use converts it to a `Ref`
    pub converted: bool,
    pub converted_line: u32,
    pub is_obj: bool,
    pub line: u32,
    /// a switch binding that names the payload in place: uses of the name are this place
    pub alias: Option<TExpr>,
}

#[derive(Clone, Debug)]
pub struct TExpr {
    pub kind: EK,
    pub ty: Ty,
    pub line: u32,
    /// an untyped literal (or constant arithmetic on literals), still free to take any numeric type
    pub lit: bool,
}

#[derive(Clone, Debug, Copy, PartialEq)]
pub enum Builtin {
    Print,
    Println,
    Assert,
    Str,
    /// `hash(x)`: a u64 hash of an integer, bool or String
    Hash,
    /// `Instant.now()`
    Now,
}

#[derive(Clone, Debug)]
pub enum EK {
    Int(i128),
    Float(f64),
    Bool(bool),
    /// index into Program::strings
    StrLit(usize),
    Var(VarId),
    /// the receiver of a method: a place
    SelfPlace,
    Field(Box<TExpr>, usize),
    /// `Ref<T>` to the place of its body
    Deref(Box<TExpr>),
    /// a value lent to a `Ref<T>` parameter that only borrows: the address of the place, no count
    Borrow(Box<TExpr>),
    /// a value copied into a new counted object (passed through a function value, whose callee is unknown)
    BoxVal(Box<TExpr>),
    /// an obj variable used as a `Ref<T>` (it is counted)
    ObjRef(VarId),
    /// an obj variable used as the place of its body
    ObjPlace(VarId),
    Index(Box<TExpr>, Box<TExpr>),
    Len(Box<TExpr>),
    /// a view of an array: shares the run, counts the array it was cut from
    Slice(Box<TExpr>, Option<Box<TExpr>>, Option<Box<TExpr>>),
    Call(FnId, Vec<TExpr>),
    /// a function used as a value; the flag asks for the throwing calling convention (a thunk if the function does not throw)
    FnRef(FnId, bool),
    /// a lifted anonymous function with the values it captured (copies, owned by its environment)
    Lambda(FnId, Vec<TExpr>),
    /// a function value given to an `extern` function: passed as (code, environment)
    ExternFn(Box<TExpr>),
    /// a call through a function value
    CallInd(Box<TExpr>, Vec<TExpr>),
    /// a method call: the receiver is a place
    Method(FnId, Box<TExpr>, Vec<TExpr>),
    Builtin(Builtin, Vec<TExpr>),
    Unary(UnOp, Box<TExpr>),
    Binary(BinOp, Box<TExpr>, Box<TExpr>),
    /// all fields, in evaluation order, defaults included
    StructLit(usize, Vec<(usize, TExpr)>),
    NewObj(usize, Vec<(usize, TExpr)>),
    NewArr(Ty, Box<TExpr>),
    Cast(Box<TExpr>),
    /// `null` as an optional of the expression's type
    Null,
    /// a value wrapped as present
    Some(Box<TExpr>),
    IsNull(Box<TExpr>),
    /// the value of an optional; traps when it is null (flow narrowing will replace most of these)
    Unwrap(Box<TExpr>),
    /// the arguments of a variadic call, collected into a new array
    ArrLit(Ty, Vec<TExpr>),
    /// the all-zero value of its type: null handles, absent optionals, zeroed structs (the payloads of the other variants of an enum)
    Zero,
    /// a C variable already holding a value (codegen-only: an owner pinned for a lend)
    CName(String),
    /// a built-in array operation: "grow" or "moveWithin"; the first argument is the array
    RtCall(&'static str, Vec<TExpr>),
    /// `json.parse<T>(bytes[, limits])`: the struct, the bytes, the limits
    Json(usize, Box<TExpr>, Option<Box<TExpr>>),
    Cond(Box<TExpr>, Box<TExpr>, Box<TExpr>),
    /// a global provided by the runtime (`extern let`)
    Global(usize),
}

#[derive(Clone, Debug, Default)]
pub struct TBlock {
    pub stmts: Vec<TStmt>,
}

#[derive(Clone, Debug)]
pub struct TStmt {
    pub kind: TS,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub enum TS {
    Let(VarId, TExpr),
    /// `let a = new T {..}`: counted or scope-owned, decided by VarInfo::converted
    LetObj(VarId, usize, Vec<(usize, TExpr)>),
    Assign(TExpr, TExpr),
    AssignOp(BinOp, TExpr, TExpr),
    Expr(TExpr),
    If(TExpr, TBlock, Option<TBlock>),
    While(TExpr, TBlock),
    Loop(TBlock),
    Break,
    Continue,
    /// a labeled loop: the next loop emitted gets labels to jump to
    Labeled(Box<TStmt>),
    /// `break label` / `continue label` (true): the loop's nesting level
    BreakL(usize, bool),
    Return(Option<TExpr>),
    Block(TBlock),
    /// the first assignment of a global, which holds nothing to drop yet
    SetGlobal(usize, TExpr),
    /// throw a new exception object of this exception struct
    Throw(usize, Vec<(usize, TExpr)>),
    Try(TBlock, Vec<TCatch>),
    /// `sync(x) { }` (false) / `mutate(x) { }` (true): `x` is a `Ref` to a counted object whose lock is held for the body
    Sync(bool, Vec<TExpr>, TBlock),
}

#[derive(Clone, Debug)]
pub struct TCatch {
    /// an exception struct, or ANY_EXC
    pub exc: usize,
    pub var: VarId,
    pub body: TBlock,
}

#[derive(Clone, Debug)]
pub struct FieldInfo {
    pub name: String,
    pub ty: Ty,
    pub default: Option<AstExpr>,
}

/// A variant of an enum. An enum is a struct whose field 0 is the tag; each variant's payload fields follow.
#[derive(Clone, Debug)]
pub struct GlobalInfo {
    pub name: String,
    pub ty: Ty,
    pub is_extern: bool,
}

#[derive(Clone, Debug)]
pub struct VariantInfo {
    pub name: String,
    /// indices into StructInfo::fields, one per payload field, in declaration order
    pub fields: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct StructInfo {
    pub name: String,
    pub file: String,
    pub fields: Vec<FieldInfo>,
    pub methods: Vec<(String, FnId)>,
    pub variants: Vec<VariantInfo>,
    pub is_exception: bool,
    pub is_extern: bool,
    pub cleanup: Option<FnId>,
    pub needs_drop: bool,
    /// holds a cleanup() (directly or inline): copying it would run the cleanup twice
    pub no_copy: bool,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct FnInfo {
    pub name: String,
    pub cname: String,
    pub owner: Option<usize>,
    pub is_static: bool,
    pub params: Vec<VarId>,
    pub param_tys: Vec<Ty>,
    pub ret: Ty,
    /// exception structs the function may throw (ANY_EXC for `Exception`)
    pub throws: Vec<usize>,
    pub is_extern: bool,
    /// declared: an `extern` function that may wait (`parks fn`)
    pub parks: bool,
    /// `fn[nopark]`: asserted not to park (checked)
    pub nopark: bool,
    /// inferred: this function, or something it calls, may park
    pub may_park: bool,
    /// the last parameter takes any number of arguments, collected into an array
    pub variadic: bool,
    /// per parameter: a `Ref<T>` the function only reads through (never stores, returns or throws), so the caller lends it
    /// without giving a count away, and may lend a plain value it owns (`EK::Borrow`)
    pub borrowed: Vec<bool>,
    pub vars: Vec<VarInfo>,
    pub body: Option<TBlock>,
    pub line: u32,
    pub file: String,
}

#[derive(Clone, Debug, Default)]
pub struct Program {
    pub structs: Vec<StructInfo>,
    pub fns: Vec<FnInfo>,
    pub strings: Vec<Vec<u8>>,
    pub main: Option<FnId>,
    /// `main(cfg: Config)`: the config struct and the generated function that builds its defaults
    pub main_cfg: Option<(usize, FnId)>,
    /// the built-in exception thrown by the use of a null optional
    pub null_exc: Option<usize>,
    pub globals: Vec<GlobalInfo>,
    /// functions that initialise the globals, in declaration order
    pub global_inits: Vec<FnId>,
    /// every generic instance the program uses: (template, struct id, declared `extern`)
    pub instances: Vec<Instance>,
    /// "counted because of line N" notes, one per obj variable that is counted
    pub notes: Vec<String>,
}

impl Program {
    pub fn is_enum(&self, t: &Ty) -> Option<usize> {
        match t {
            Ty::Struct(s) if !self.structs[*s].variants.is_empty() => Some(*s),
            _ => None,
        }
    }
    pub fn needs_drop(&self, t: &Ty) -> bool {
        match t {
            Ty::Ref(_) | Ty::Str | Ty::Arr(_) | Ty::Exc => true,
            Ty::Struct(s) => self.structs[*s].needs_drop,
            Ty::Opt(i) => self.needs_drop(i),
            _ => false,
        }
    }
    pub fn type_name(&self, t: &Ty) -> String {
        match t {
            Ty::Unit => "()".into(),
            Ty::Bool => "bool".into(),
            Ty::Int(i) => i.name().into(),
            Ty::F64 => "f64".into(),
            Ty::Str => "String".into(),
            Ty::Dur => "Duration".into(),
            Ty::Inst => "Instant".into(),
            Ty::Struct(s) => self.structs[*s].name.clone(),
            Ty::Ref(s) => format!("Ref<{}>", self.structs[*s].name),
            Ty::Arr(e) => format!("Array<{}>", self.type_name(e)),
            Ty::Obj(s) => self.structs[*s].name.clone(),
            Ty::Opt(i) => format!("{}?", self.type_name(i)),
            Ty::Null => "null".into(),
            Ty::Exc => "Exception".into(),
            Ty::Fn(ps, r, th) => format!("fn({}): {}{}", ps.iter().map(|p| self.type_name(p)).collect::<Vec<_>>().join(", "), self.type_name(r), if *th { " throws" } else { "" }),
            Ty::Error => "<error>".into(),
        }
    }
}

impl Ty {
    /// a counted handle, held as a nullable pointer when optional
    pub fn is_handle_opt(&self) -> bool {
        matches!(self, Ty::Opt(i) if i.is_handle())
    }
    pub fn is_handle(&self) -> bool {
        matches!(self, Ty::Ref(_) | Ty::Str | Ty::Arr(_))
    }
    /// the struct held by value, directly or as an optional
    pub fn inline_struct(&self) -> Option<usize> {
        match self {
            Ty::Struct(s) => Some(*s),
            Ty::Opt(i) => i.inline_struct(),
            _ => None,
        }
    }
}

/// A generic instance: the template, the use as written (`Ring<String>`), the struct made for it, and whether the template is `extern`.
#[derive(Clone, Debug)]
pub struct Instance {
    pub template: String,
    pub shown: String,
    pub sid: usize,
    pub is_extern: bool,
}

impl TExpr {
    /// Is this handle held by something the callee cannot reach: a local, a parameter, a temporary? A handle read out of
    /// a field, an element or a global sits in shared storage; a callee could replace it and free what it points to.
    pub fn handle_stable(&self) -> bool {
        match &self.kind {
            EK::Field(..) | EK::Index(..) | EK::Global(..) => false,
            EK::Unwrap(x) => x.handle_stable(),
            EK::Cond(_, a, b) => a.handle_stable() && b.handle_stable(),
            _ => true,
        }
    }

    /// Is this place safe to lend: inline in a local, or inside an object that a local or a temporary keeps alive?
    /// A place reached through a handle stored in a field or an element is not.
    pub fn place_stable(&self) -> bool {
        match &self.kind {
            EK::Field(b, _) => b.place_stable(),
            EK::Deref(h) => h.handle_stable(),
            EK::Index(a, _) => a.handle_stable(),
            _ => true,
        }
    }
}
