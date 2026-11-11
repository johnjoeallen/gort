//! Comparative W2 driver (same protocol as c/w2sync.c with warm_ms, go/w2x, java rcm.W2X).
//!  impl=rwlock   striped RwLock<Vec<Arc<Entry>>>, Arc cloned out under the read lock (as rcmlib::Table)
//!  impl=arcswap  Vec<ArcSwap<Entry>>, `load()` guard (debt-list borrow, no count on the fast path)
//!  impl=epoch    crossbeam-epoch Atomic<Entry>, pinned guard per read, defer_destroy on swap (epoch-based reclamation)
use arc_swap::ArcSwap;
use crossbeam_epoch::{self as epoch, Atomic, Owned};
use rcmlib::{new_entry, rng, walk_ref, Entry, Table, MIX};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering::*};
use std::sync::Arc;
use std::time::{Duration, Instant};

const NENT: usize = 10000;
const HBUF: usize = 16384;

#[inline(never)]
fn fnv(b: &[u8], seed: u64) -> u64 {
    let mut h = 0xcbf29ce484222325u64 ^ seed;
    for &c in b {
        h = (h ^ c as u64).wrapping_mul(0x100000001b3);
    }
    h
}

enum Tbl {
    Rw(Table<Entry>),
    Swap(Vec<ArcSwap<Entry>>),
    Epoch(Vec<Atomic<Entry>>),
}

#[repr(align(128))]
struct Prog(AtomicI64);

fn arg(k: &str, d: &str) -> String {
    std::env::args().skip(1).find_map(|a| a.strip_prefix(&format!("{k}=")).map(str::to_string)).unwrap_or_else(|| d.to_string())
}
fn argi(k: &str, d: i64) -> i64 {
    arg(k, &d.to_string()).parse().expect(k)
}

fn main() {
    let (imp, mode) = (arg("impl", "rwlock"), arg("mode", "hot_same"));
    let (t_n, wpct, hash) = (argi("threads", 1) as usize, argi("wpct", 5) as u64, argi("hash", 0) as usize);
    let (warm, dur) = (argi("warm_ms", 500) as u64, argi("dur_ms", 1000) as u64);
    assert!(hash <= HBUF, "hash out of range");
    let mk = |k: usize| new_entry(k as i64, k as i64 * 5 + 1);
    let tbl = Arc::new(match imp.as_str() {
        "rwlock" => Tbl::Rw(Table::new(NENT, mk)),
        "arcswap" => Tbl::Swap((0..NENT).map(|k| ArcSwap::from_pointee(mk(k))).collect()),
        "epoch" => Tbl::Epoch((0..NENT).map(|k| Atomic::new(mk(k))).collect()),
        _ => panic!("impl=rwlock|arcswap|epoch"),
    });
    let progs: Arc<Vec<Prog>> = Arc::new((0..t_n).map(|_| Prog(AtomicI64::new(0))).collect());
    let stop = Arc::new(AtomicBool::new(false));
    let hs: Vec<_> = (0..t_n)
        .map(|id| {
            let (tbl, progs, stop, mode) = (Arc::clone(&tbl), Arc::clone(&progs), Arc::clone(&stop), mode.clone());
            std::thread::spawn(move || {
                let mut s = MIX ^ ((id as u64 + 1) * 0x1234567);
                let mut s2 = s ^ 0x5555;
                let hb: Vec<u8> = (0..HBUF).map(|_| rng(&mut s2) as u8).collect();
                let hot = if mode == "hot_diff" { (id * 601 + 7) % NENT } else { 17 };
                let uni = mode == "uniform";
                let mut sink = 0i64;
                let mut i: i64 = 0;
                loop {
                    if i & 63 == 0 {
                        progs[id].0.store(i, Relaxed);
                        if stop.load(Relaxed) {
                            break;
                        }
                    }
                    i += 1;
                    let r = rng(&mut s);
                    let k = if uni { ((r >> 11) % NENT as u64) as usize } else { hot };
                    let r2 = rng(&mut s);
                    if (r2 >> 33) % 100 < wpct {
                        let e = new_entry(k as i64, (r2 & 0xffff) as i64);
                        match &*tbl {
                            Tbl::Rw(t) => drop(t.swap(k, Arc::new(e))),
                            Tbl::Swap(v) => v[k].store(Arc::new(e)),
                            Tbl::Epoch(v) => {
                                let g = epoch::pin();
                                let old = v[k].swap(Owned::new(e), AcqRel, &g);
                                unsafe { g.defer_destroy(old) };
                            }
                        }
                        continue;
                    }
                    let v = match &*tbl {
                        Tbl::Rw(t) => walk_ref(&t.get(k), 3),
                        Tbl::Swap(v) => walk_ref(&v[k].load(), 3),
                        Tbl::Epoch(v) => {
                            let g = epoch::pin();
                            walk_ref(unsafe { v[k].load(Acquire, &g).deref() }, 3)
                        }
                    };
                    sink += v;
                    if hash > 0 {
                        sink = sink.wrapping_add(fnv(&hb[..hash], v as u64) as i64);
                    }
                }
                std::hint::black_box(sink);
            })
        })
        .collect();
    let sum = || progs.iter().map(|p| p.0.load(Relaxed)).sum::<i64>();
    std::thread::sleep(Duration::from_millis(warm));
    let (c0, w0) = (sum(), Instant::now());
    std::thread::sleep(Duration::from_millis(dur));
    let (c1, w1) = (sum(), Instant::now());
    stop.store(true, Relaxed);
    for h in hs {
        h.join().unwrap();
    }
    let mops = (c1 - c0) as f64 / (w1 - w0).as_secs_f64() / 1e6;
    let params = format!("threads={t_n};mode={mode};wpct={wpct};ppct=0;hash={hash}");
    println!("W2X,rust,{imp},{params},mops_total,{mops:.6}");
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).map(|l| l[6..].trim().trim_end_matches("kB").trim().parse::<f64>().unwrap_or(0.0)))
        .unwrap_or(0.0);
    println!("W2X,rust,{imp},{params},rss_mb,{:.1}", rss / 1024.0);
}
