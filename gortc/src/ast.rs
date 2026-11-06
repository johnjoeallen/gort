//! Syntax tree for the slice-0 subset.

#[derive(Debug, Clone)]
pub struct TypeExpr {
    pub name: String,
    pub args: Vec<TypeExpr>,
    /// `T?`
    pub optional: bool,
    /// `T[N]`: a fixed-capacity run of N elements (the size is recorded, not yet used)
    pub fixed: Option<u64>,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(u64),
    Float(f64),
    Str(Vec<u8>),
    Char(u8),
    Bool(bool),
    Ident(String),
    Field(Box<Expr>, String),
    Call(Box<Expr>, Vec<Expr>),
    Index(Box<Expr>, Box<Expr>),
    /// `a[lo..hi]`, either bound optional: a view of the array
    Slice(Box<Expr>, Option<Box<Expr>>, Option<Box<Expr>>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `Name { f: e, ... }`, a struct value
    StructLit(TypeExpr, Vec<(String, Expr)>),
    /// `new Name { ... }` or `new Name()`
    New(TypeExpr, Vec<(String, Expr)>),
    /// `new T[n]`
    NewArray(TypeExpr, Box<Expr>),
    Cast(Box<Expr>, TypeExpr),
    /// `fn(params) { body }`: an anonymous function
    Lambda(Box<LambdaDecl>),
    /// `f<T>` : a function named with explicit type arguments, as the callee of a call
    TypeArgs(Box<Expr>, Vec<TypeExpr>),
    /// `[a, b, c]`: an array of the items
    ArrayLit(Vec<Expr>),
    /// `c ? a : b`
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// a type with arguments used as a qualifier: `Mailbox<Reap>.make(..)`
    TypeName(TypeExpr),
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Let { name: String, mutable: bool, ty: Option<TypeExpr>, init: Expr },
    Assign { target: Expr, op: Option<BinOp>, value: Expr },
    Expr(Expr),
    If { cond: Expr, then: Block, els: Option<Block> },
    While { cond: Expr, body: Block },
    Loop(Block),
    /// `for (x in iterable) body`
    For { var: String, iter: Expr, body: Block },
    Break,
    Continue,
    /// `label: loop`
    Labeled(String, Box<Stmt>),
    /// `break label` and `continue label`
    BreakTo(String),
    ContinueTo(String),
    Return(Option<Expr>),
    Block(Block),
    Switch { subject: Expr, arms: Vec<Arm> },
    Throw(Expr),
    Try { body: Block, catches: Vec<Catch> },
    /// `sync (reads a, b; writes c) { }`: each target is a `Ref<T>` with its mode (true: written); clauses end with `;`; all are locked together in one canonical order
    Sync { targets: Vec<(bool, Expr)>, body: Block },
}

#[derive(Debug, Clone)]
pub struct Catch {
    pub name: String,
    pub ty: TypeExpr,
    pub body: Block,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub enum PatKind {
    Default,
    /// a variant name (optionally `Enum.Variant`) with binders; `_` is a binder that binds nothing
    Variant { qual: Option<String>, name: String, binds: Option<Vec<String>> },
    Lit(Expr),
}

#[derive(Debug, Clone)]
pub struct Pat {
    pub kind: PatKind,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct Arm {
    pub pats: Vec<Pat>,
    pub body: Block,
}

#[derive(Debug, Clone)]
pub struct VariantDecl {
    pub name: String,
    pub fields: Vec<(String, TypeExpr)>,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    /// `name: T...`: the call may pass any number of `T`, collected into an array
    pub variadic: bool,
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    /// `fn T::name` out of line
    pub owner: Option<String>,
    pub is_static: bool,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub throws: Vec<TypeExpr>,
    pub body: Option<Block>,
    /// declared `extern`: implemented by the C runtime
    pub is_extern: bool,
    /// `parks`: may wait (the runtime's effect, stated because there is no body to analyse)
    pub parks: bool,
    /// `fn[nopark]` / `fn[yield]`: the effect attribute, if written
    pub attr: Option<String>,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<FieldDecl>,
    pub methods: Vec<FnDecl>,
    /// non-empty for `enum`
    pub variants: Vec<VariantDecl>,
    pub is_enum: bool,
    pub is_exception: bool,
    pub is_extern: bool,
    /// `struct Ring<T>`: a template, instantiated per use
    pub type_params: Vec<String>,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub value: Expr,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub enum Item {
    Import(String),
    Const(ConstDecl),
    Fn(FnDecl),
    Struct(StructDecl),
    /// `extern let name: T`, a value the runtime provides
    ExternLet(ConstDecl),
    /// a top-level `let`: one binding for the whole run, initialised before `main`
    Global { name: String, ty: Option<TypeExpr>, init: Expr, line: u32, col: u32 },
    /// `extern impl Type { fn ... }`: more functions for a type declared elsewhere
    Impl(TypeExpr, Vec<FnDecl>),
}

#[derive(Debug, Clone)]
pub struct File {
    pub name: String,
    pub package: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub struct LambdaDecl {
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Block,
    /// every name written inside it (the captures are the ones that name a local of the enclosing function)
    pub idents: Vec<String>,
}
