use rcmlib::*;
use std::sync::{Arc, Barrier};
use std::time::Instant;

fn geti(a: &[String], k: &str, d: i64) -> i64 {
    for s in a {
        if let Some(v) = s.strip_prefix(&format!("{k}=")) {
            return v.parse().unwrap();
        }
    }
    d
}
fn gets(a: &[String], k: &str, d: &str) -> String {
    for s in a {
        if let Some(v) = s.strip_prefix(&format!("{k}=")) {
            return v.to_string();
        }
    }
    d.to_string()
}
struct Out {
    variant: String,
    params: String,
}
impl Out {
    fn row(&self, w: &str, m: &str, v: f64) {
        println!("{w},rust,{},{},{m},{v:.6}", self.variant, self.params);
    }
}
fn vmhwm_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for l in s.lines() {
        if let Some(r) = l.strip_prefix("VmHWM:") {
            return r.trim().trim_end_matches("kB").trim().parse::<f64>().unwrap_or(0.0) / 1024.0;
        }
    }
    0.0
}
fn cpu_s() -> f64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let rest = &s[s.rfind(')').unwrap() + 2..];
    let f: Vec<&str> = rest.split(' ').collect();
    (f[11].parse::<f64>().unwrap_or(0.0) + f[12].parse::<f64>().unwrap_or(0.0)) / 100.0
}

fn mk_sess_table() -> Arc<Table<idio::Sess>> {
    Arc::new(Table::new(NSESS, |i| idio::new_sess(i as u64, i as u64 * 7)))
}

fn w3(args: &[String]) {
    let t = geti(args, "workers", 1) as usize;
    let total = geti(args, "requests", 5_000_000) as usize;
    let warm = geti(args, "warmup", 300_000) as usize;
    let work = geti(args, "work", 1) as u32;
    let tbm = gets(args, "table", "shared");
    let variant = gets(args, "variant", "rust-idiomatic");
    let arc = variant == "rust-arc";
    let out = Out { variant, params: format!("workers={t};table={tbm};work={work}") };
    let private = tbm == "private";
    let shared = mk_sess_table();
    let n = total / t;
    let bar = Arc::new(Barrier::new(t));
    let epoch = Instant::now();
    let hs: Vec<_> = (0..t)
        .map(|id| {
            let shared = Arc::clone(&shared);
            let bar = Arc::clone(&bar);
            std::thread::spawn(move || {
                let mut rs = MIX ^ ((id as u64 + 1) * 0x1234567);
                let tb = if private { mk_sess_table() } else { shared };
                let mut ws = rs ^ 0xABCDEF;
                bar.wait();
                for _ in 0..warm {
                    std::hint::black_box(if arc { arcv::one_request(&mut ws, &tb, work) } else { idio::one_request(&mut ws, &tb, work) });
                }
                bar.wait();
                let mut lat = Vec::with_capacity(n);
                let mut cs = 0u64;
                let t0 = epoch.elapsed().as_nanos() as u64;
                for _ in 0..n {
                    let a = Instant::now();
                    cs = cs.wrapping_add(if arc { arcv::one_request(&mut rs, &tb, work) } else { idio::one_request(&mut rs, &tb, work) });
                    lat.push(a.elapsed().as_nanos() as u32);
                }
                let t1 = epoch.elapsed().as_nanos() as u64;
                (t0, t1, cs, lat)
            })
        })
        .collect();
    let rs: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    let a = rs.iter().map(|x| x.0).min().unwrap();
    let b = rs.iter().map(|x| x.1).max().unwrap();
    let mut lat: Vec<u32> = rs.iter().flat_map(|x| x.3.iter().copied()).collect();
    lat.sort_unstable();
    let nn = lat.len();
    out.row("W3", "rps", nn as f64 / ((b - a) as f64 / 1e9));
    out.row("W3", "p50_ns", lat[nn / 2] as f64);
    out.row("W3", "p99_ns", lat[(nn as f64 * 0.99) as usize] as f64);
    out.row("W3", "p999_ns", lat[(nn as f64 * 0.999) as usize] as f64);
    out.row("W3", "rss_mb", vmhwm_mb());
    out.row("W3", "cpu_s", cpu_s());
    if t == 1 {
        out.row("W3", "checksum", (rs[0].2 & 0xffff_ffff) as f64);
    }
}

fn median(mut x: Vec<f64>) -> f64 {
    x.sort_by(|a, b| a.partial_cmp(b).unwrap());
    x[x.len() / 2]
}

fn w4(args: &[String]) {
    let t = geti(args, "threads", 1) as usize;
    let n = geti(args, "nodes", 1_000_000);
    let cycles = geti(args, "cycles", 6) as usize;
    let weak = geti(args, "weak", 0) != 0;
    let variant = gets(args, "variant", "rust-box");
    let out = Out { variant: variant.clone(), params: format!("threads={t};weak={}", weak as i32) };
    if variant == "rust-box" && weak {
        eprintln!("rust-box cannot express a weak parent pointer; skipped");
        return;
    }
    let hs: Vec<_> = (0..t)
        .map(|_| {
            let variant = variant.clone();
            std::thread::spawn(move || {
                let (mut bs, mut ts, mut fs) = (vec![], vec![], vec![]);
                let mut sink = 0i64;
                for _ in 0..cycles {
                    let (b, tr, f);
                    match variant.as_str() {
                        "rust-box" => {
                            let t0 = Instant::now();
                            let root = w4::build_box(0, n);
                            let t1 = Instant::now();
                            sink += w4::sum_box(&root);
                            let t2 = Instant::now();
                            drop(root);
                            let t3 = Instant::now();
                            (b, tr, f) = (t1 - t0, t2 - t1, t3 - t2);
                        }
                        "rust-rc" => {
                            let t0 = Instant::now();
                            let root = w4::rc::build(0, n, std::rc::Weak::new(), weak);
                            let t1 = Instant::now();
                            sink += w4::rc::sum(std::rc::Rc::clone(&root), weak);
                            let t2 = Instant::now();
                            drop(root);
                            let t3 = Instant::now();
                            (b, tr, f) = (t1 - t0, t2 - t1, t3 - t2);
                        }
                        _ => {
                            let t0 = Instant::now();
                            let root = w4::sync::build(0, n, std::sync::Weak::new(), weak);
                            let t1 = Instant::now();
                            sink += w4::sync::sum(Arc::clone(&root), weak);
                            let t2 = Instant::now();
                            drop(root);
                            let t3 = Instant::now();
                            (b, tr, f) = (t1 - t0, t2 - t1, t3 - t2);
                        }
                    }
                    bs.push(b.as_nanos() as f64 / n as f64);
                    ts.push(tr.as_nanos() as f64 / n as f64);
                    fs.push(f.as_nanos() as f64 / n as f64);
                }
                std::hint::black_box(sink);
                (median(bs[1..].to_vec()), median(ts[1..].to_vec()), median(fs[1..].to_vec()))
            })
        })
        .collect();
    let r: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    let k = t as f64;
    let (b, tr, f) = (r.iter().map(|x| x.0).sum::<f64>() / k, r.iter().map(|x| x.1).sum::<f64>() / k, r.iter().map(|x| x.2).sum::<f64>() / k);
    out.row("W4", "build_ns_per_node", b);
    out.row("W4", "traverse_ns_per_node", tr);
    out.row("W4", "free_ns_per_node", f);
    out.row("W4", "cycle_ns_per_node", b + tr + f);
    out.row("W4", "cpu_ns_per_node_cycle", cpu_s() * 1e9 / (n as f64 * cycles as f64 * k));
    out.row("W4", "rss_mb", vmhwm_mb());
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("w3") => w3(&a[2..]),
        Some("w4") => w4(&a[2..]),
        _ => {
            eprintln!("usage: rcm w3|w4 k=v...");
            std::process::exit(2)
        }
    }
}
