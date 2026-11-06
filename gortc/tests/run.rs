//! Runs every program under tests/: ok/ must build under ASan+UBSan and print the .out file (the runtime also fails
//! the run if any counted object is still live at exit), err/ must be rejected with the message named on its first
//! line (`// error: ...`), warn/ must be accepted and print each `// warning:` text (and none that contains a `// no-warning:` text), trap/ must exit with the code named in `// expect-exit:` and print `// stderr:` text.

use std::path::{Path, PathBuf};
use std::process::Command;

fn gortc() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gortc"))
}

fn files(dir: &str) -> Vec<PathBuf> {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join(dir);
    let mut v: Vec<PathBuf> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().map(|e| e == "gort").unwrap_or(false)).collect();
    v.sort();
    v
}

fn header(p: &Path, key: &str) -> Vec<String> {
    let src = std::fs::read_to_string(p).unwrap();
    src.lines().take_while(|l| l.starts_with("//")).filter_map(|l| l.strip_prefix(&format!("// {}:", key)).map(|s| s.trim().to_string())).collect()
}

fn run_program(p: &Path) -> std::process::Output {
    let out = std::env::temp_dir().join(format!("gortc-test-{}-{}", std::process::id(), p.file_stem().unwrap().to_string_lossy()));
    let mut cmd = Command::new(gortc());
    cmd.args(["build", "--sanitize"]);
    // a sibling .c file supplies the runtime side of the program's `extern` declarations
    let c = p.with_extension("c");
    if c.exists() {
        cmd.arg("--link").arg(&c);
    }
    let b = cmd.arg("-o").arg(&out).arg(p).output().unwrap();
    assert!(b.status.success(), "{}: build failed\n{}", p.display(), String::from_utf8_lossy(&b.stderr));
    let r = Command::new(&out).output().unwrap();
    let _ = std::fs::remove_file(&out);
    r
}

#[test]
fn ok_programs() {
    for p in files("ok") {
        let r = run_program(&p);
        let want = std::fs::read_to_string(p.with_extension("out")).unwrap();
        assert!(r.status.success(), "{}: exit {:?}\nstderr: {}", p.display(), r.status.code(), String::from_utf8_lossy(&r.stderr));
        assert_eq!(String::from_utf8_lossy(&r.stdout), want, "{}: output differs", p.display());
    }
}

#[test]
fn rejected_programs() {
    for p in files("err") {
        let want = header(&p, "error");
        assert!(!want.is_empty(), "{}: no `// error:` line", p.display());
        // `emit` accepts a library; a missing `main` is only an error when building, so that case is checked with `build`
        let mode = if std::fs::read_to_string(&p).unwrap().contains("no `main`") { "build" } else { "emit" };
        let r = Command::new(gortc()).arg(mode).arg(&p).output().unwrap();
        assert_eq!(r.status.code(), Some(1), "{}: should fail to compile", p.display());
        let err = String::from_utf8_lossy(&r.stderr);
        for w in want {
            assert!(err.contains(&w), "{}: expected `{}` in\n{}", p.display(), w, err);
        }
    }
}

#[test]
fn trapping_programs() {
    for p in files("trap") {
        let code: i32 = header(&p, "expect-exit")[0].parse().unwrap();
        let r = run_program(&p);
        assert_eq!(r.status.code(), Some(code), "{}", p.display());
        let want = std::fs::read_to_string(p.with_extension("out")).unwrap();
        assert_eq!(String::from_utf8_lossy(&r.stdout), want, "{}: output differs", p.display());
        for w in header(&p, "stderr") {
            assert!(String::from_utf8_lossy(&r.stderr).contains(&w), "{}: stderr lacks `{}`", p.display(), w);
        }
    }
}

#[test]
fn warned_programs() {
    for p in files("warn") {
        let r = Command::new(gortc()).arg("check").arg(&p).output().unwrap();
        let err = String::from_utf8_lossy(&r.stderr);
        assert_eq!(r.status.code(), Some(0), "{}: a warning must not fail the build\n{}", p.display(), err);
        for w in header(&p, "warning") {
            assert!(err.contains(&format!("warning: {}", w)) || err.contains(&w), "{}: expected warning `{}` in\n{}", p.display(), w, err);
        }
        for w in header(&p, "no-warning") {
            assert!(!err.contains(&w), "{}: unexpected warning `{}` in\n{}", p.display(), w, err);
        }
    }
}
