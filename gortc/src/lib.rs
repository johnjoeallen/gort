//! gortc: slice 0 of the Gort compiler. A subset front end (lexer, parser, sema) and a C emitter.

pub mod ast;
pub mod codegen;
pub mod diag;
pub mod hir;
pub mod lexer;
pub mod parser;
pub mod sema;
pub mod share;

use diag::Diag;

pub struct Output {
    pub c: String,
    pub notes: Vec<String>,
    /// the generic instances of the program, one per line (`--instances`)
    pub instances: Vec<String>,
    /// the functions that may park, one per line (`--effects`)
    pub effects: Vec<String>,
    /// accepted, but worth a look (printed as `warning:`)
    pub warnings: Vec<Diag>,
}

/// Compiles the files of one package (name, source) to a single C translation unit.
pub fn compile(files: &[(String, String)]) -> Result<Output, Vec<Diag>> {
    compile_with(files, true)
}

/// `require_main: false` checks a library: no `main` is needed, and none is emitted.
pub fn compile_with(files: &[(String, String)], require_main: bool) -> Result<Output, Vec<Diag>> {
    let mut parsed = Vec::new();
    let mut errs = Vec::new();
    // the prelude: types every program has
    match parser::parse_file("<prelude>", PRELUDE) {
        Ok(f) => parsed.push(f),
        Err(d) => return Err(vec![d]),
    }
    for (name, src) in files {
        match parser::parse_file(name, src) {
            Ok(f) => parsed.push(f),
            Err(d) => errs.push(d),
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    // `main(cfg: Config)`: a function that builds the defaults of Config, so the C entry point can start from them
    let cfg_src = parsed.iter().find_map(|f| {
        f.items.iter().find_map(|i| match i {
            ast::Item::Fn(d) if d.name == "main" && d.owner.is_none() && d.params.len() == 1 && d.params[0].ty.args.is_empty() => {
                Some((f.package.clone(), d.params[0].ty.name.clone()))
            }
            _ => None,
        })
    });
    if let Some((pkg, ty)) = cfg_src {
        let src = format!("package {}\nfn __main_cfg(): {} {{ return {} {{}} }}\n", pkg.unwrap_or_else(|| "main".into()), ty, ty);
        match parser::parse_file("<main>", &src) {
            Ok(f) => parsed.push(f),
            Err(d) => return Err(vec![d]),
        }
    }
    let prog = sema::analyze(&parsed)?;
    if require_main && prog.main.is_none() {
        return Err(vec![Diag { file: files.first().map(|f| f.0.clone()).unwrap_or_default(), line: 1, col: 1, msg: "no `main` function".into() }]);
    }
    let c = codegen::generate(&prog);
    let instances = prog
        .instances
        .iter()
        .map(|i| format!("{}{}  =>  G_{}", i.shown, if i.is_extern { " [extern]" } else { "" }, prog.structs[i.sid].name))
        .collect();
    let effects = prog.fns.iter().filter(|f| f.may_park && !f.is_extern && f.body.is_some() && !f.name.starts_with('_')).map(|f| format!("may park: {}", f.name)).collect();
    Ok(Output { c, notes: prog.notes, instances, effects, warnings: prog.warnings })
}

/// Built-in types written in Gort (src/prelude.gort): `Range`, `Map`, `JsonLimits`, `JsonError`.
const PRELUDE: &str = include_str!("prelude.gort");

