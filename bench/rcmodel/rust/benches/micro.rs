use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rcmlib::*;
use std::sync::Arc;
use std::time::Duration;

/// W1: group "w1/<variant>", param depth. One iteration = one chain of `depth` calls (ns/pass = time / depth).
fn w1(c: &mut Criterion) {
    let pool_ref: Vec<Entry> = (0..64).map(|i| new_entry(i, i * 3 + 1)).collect();
    let pool_arc: Vec<Arc<Entry>> = (0..64).map(|i| Arc::new(new_entry(i, i * 3 + 1))).collect();
    for d in [1, 4, 16] {
        let mut g = c.benchmark_group("w1/idiomatic");
        g.sample_size(10).warm_up_time(Duration::from_secs(1)).measurement_time(Duration::from_secs(3));
        g.bench_function(format!("depth={d}"), |b| {
            let mut i = 0usize;
            b.iter(|| {
                i = i.wrapping_add(1);
                black_box(walk_ref(black_box(&pool_ref[i & 63]), d))
            })
        });
        g.finish();
        let mut g = c.benchmark_group("w1/arc");
        g.sample_size(10).warm_up_time(Duration::from_secs(1)).measurement_time(Duration::from_secs(3));
        g.bench_function(format!("depth={d}"), |b| {
            let mut i = 0usize;
            b.iter(|| {
                i = i.wrapping_add(1);
                black_box(walk_arc(Arc::clone(&pool_arc[i & 63]), d))
            })
        });
        g.finish();
    }
}

/// W2: one criterion iteration = one op per thread; iter_custom runs `iters` ops on each of T threads and returns wall time.
/// ns/op reported = wall / ops-per-thread  (total Mops/s = T * 1000 / ns).
fn w2(c: &mut Criterion) {
    let t = Arc::new(w2_table());
    let threads: Vec<usize> = std::env::var("W2_THREADS").unwrap_or("1,2,4,8,16".into()).split(',').map(|x| x.parse().unwrap()).collect();
    for tn in threads {
        for (mname, mode) in [("uniform", Mode::Uniform), ("hot_same", Mode::HotSame), ("hot_diff", Mode::HotDiff)] {
            for (vname, arc_pass) in [("idiomatic", false), ("arc", true)] {
                let mut g = c.benchmark_group(format!("w2/{vname}/{mname}"));
                g.sample_size(10).warm_up_time(Duration::from_millis(500)).measurement_time(Duration::from_secs(3));
                let tt = Arc::clone(&t);
                g.bench_function(format!("t={tn}"), move |b| {
                    b.iter_custom(|iters| Duration::from_nanos(w2_run(&tt, mode, tn, iters, arc_pass)))
                });
                g.finish();
            }
        }
    }
}
criterion_group!(benches, w1, w2);
criterion_main!(benches);
