use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn usage() -> ExitCode {
    eprintln!(
        "usage: gortc <check|emit|build|run> [options] FILE.gort...\n\n  check           type-check only; no `main` needed\n  emit            print the generated C\n  build -o OUT    compile to an executable (needs cc)\n  run             build and run\n\noptions:\n  --instances     list the generic instances of the program\n  --notes         say which `new` objects are counted and why\n  --sanitize      build with ASan and UBSan\n  --keep-c        keep the generated C next to the output\n  --link FILE.c   also compile and link a C file (the runtime's side of `extern` declarations)"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else { return usage() };
    let mut files: Vec<String> = Vec::new();
    let mut out: Option<PathBuf> = None;
    let (mut notes, mut sanitize, mut keep_c, mut instances) = (false, false, false, false);
    let mut rest_args: Vec<String> = Vec::new();
    let mut links: Vec<String> = Vec::new();
    let mut after_dd = false;
    while let Some(a) = args.next() {
        if after_dd {
            rest_args.push(a);
            continue;
        }
        match a.as_str() {
            "--" => after_dd = true,
            "-o" => out = args.next().map(PathBuf::from),
            "--notes" => notes = true,
            "--instances" => instances = true,
            "--sanitize" => sanitize = true,
            "--keep-c" => keep_c = true,
            "--link" => links.extend(args.next()),
            _ if a.starts_with('-') => return usage(),
            _ => files.push(a),
        }
    }
    if !matches!(cmd.as_str(), "check" | "emit" | "build" | "run") || files.is_empty() {
        return usage();
    }
    let mut srcs = Vec::new();
    for f in &files {
        match std::fs::read_to_string(f) {
            Ok(s) => srcs.push((f.clone(), s)),
            Err(e) => {
                eprintln!("gortc: {}: {}", f, e);
                return ExitCode::from(2);
            }
        }
    }
    let o = match gortc::compile_with(&srcs, cmd == "build" || cmd == "run") {
        Ok(o) => o,
        Err(ds) => {
            for d in &ds {
                eprintln!("{}", d);
            }
            eprintln!("gortc: {} error(s)", ds.len());
            return ExitCode::from(1);
        }
    };
    if instances {
        for i in &o.instances {
            eprintln!("{}", i);
        }
    }
    if notes {
        for n in &o.notes {
            eprintln!("{}", n);
        }
    }
    if cmd == "check" {
        return ExitCode::SUCCESS;
    }
    if cmd == "emit" {
        print!("{}", o.c);
        return ExitCode::SUCCESS;
    }
    let exe = out.unwrap_or_else(|| std::env::temp_dir().join(format!("gortc-{}", std::process::id())));
    let cfile = exe.with_extension("c");
    if let Err(e) = std::fs::write(&cfile, &o.c) {
        eprintln!("gortc: {}: {}", cfile.display(), e);
        return ExitCode::from(2);
    }
    let mut cc = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()));
    cc.args(["-std=gnu11", "-g", "-O1", "-fwrapv", "-Wall", "-Wno-unused-function", "-Wno-unused-variable", "-Wno-unused-but-set-variable"]);
    if sanitize {
        cc.arg("-fsanitize=address,undefined");
    }
    cc.arg(&cfile).args(&links).arg("-o").arg(&exe);
    match cc.status() {
        Ok(s) if s.success() => {}
        Ok(_) => {
            eprintln!("gortc: the C compiler rejected the generated code ({} kept)", cfile.display());
            return ExitCode::from(3);
        }
        Err(e) => {
            eprintln!("gortc: cannot run the C compiler: {}", e);
            return ExitCode::from(2);
        }
    }
    if cmd == "build" {
        if !keep_c {
            let _ = std::fs::remove_file(&cfile);
        }
        return ExitCode::SUCCESS;
    }
    let status = Command::new(&exe).args(&rest_args).status();
    let _ = std::fs::remove_file(&exe);
    if !keep_c {
        let _ = std::fs::remove_file(&cfile);
    }
    match status {
        Ok(s) => ExitCode::from(s.code().unwrap_or(1) as u8),
        Err(e) => {
            eprintln!("gortc: {}", e);
            ExitCode::from(2)
        }
    }
}
