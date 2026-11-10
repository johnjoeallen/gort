//! Same workloads as c/rcm.c.
//!  * `idio`: idiomatic Rust, owned Boxes, `&T` borrows through the call chain.
//!  * `arcv`: pessimal: every object is an Arc, cloned on every pass and every field load (closest analogue of the Gort model;
//!    Arc is always atomic, so it corresponds to C3).
use std::sync::{Arc, RwLock};

pub const NSTR: usize = 256;
pub const NSESS: usize = 1024;
pub const FNV: u64 = 0x100000001b3;
pub const MIX: u64 = 0x9E3779B97F4A7C15;

#[inline(always)]
pub fn rng(s: &mut u64) -> u64 {
    let mut x = *s;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *s = x;
    x.wrapping_mul(2685821657736338717)
}

// ---------------------------------------------------------------- shared striped table (Arc values, RwLock stripes)
#[repr(align(64))]
pub struct Stripe<T>(pub RwLock<Vec<Arc<T>>>);
pub struct Table<T> {
    pub st: Vec<Stripe<T>>,
}
impl<T> Table<T> {
    pub fn new(n: usize, mk: impl Fn(usize) -> T) -> Self {
        let mut v: Vec<Vec<Arc<T>>> = (0..NSTR).map(|_| Vec::new()).collect();
        for k in 0..n {
            v[k & (NSTR - 1)].push(Arc::new(mk(k)));
        }
        Table { st: v.into_iter().map(|x| Stripe(RwLock::new(x))).collect() }
    }
    #[inline]
    pub fn get(&self, k: usize) -> Arc<T> {
        let g = self.st[k & (NSTR - 1)].0.read().unwrap();
        Arc::clone(&g[k >> 8])
    }
    #[inline]
    pub fn swap(&self, k: usize, n: Arc<T>) -> Arc<T> {
        let mut g = self.st[k & (NSTR - 1)].0.write().unwrap();
        std::mem::replace(&mut g[k >> 8], n)
    }
}

// ---------------------------------------------------------------- W1/W2
#[repr(align(64))]
pub struct Entry {
    pub key: i64,
    pub val: i64,
    pub payload: [u8; 48],
    pub pad: [u8; 64],
}
pub fn new_entry(key: i64, val: i64) -> Entry {
    Entry { key, val, payload: [key as u8; 48], pad: [0; 64] }
}
#[inline(never)]
pub fn walk_ref(e: &Entry, d: i32) -> i64 {
    if d <= 1 {
        return std::hint::black_box(e).val;
    }
    let v = walk_ref(e, d - 1);
    std::hint::black_box(v) // keeps a real call chain: without it LLVM turns the recursion into a loop/constant
}
#[inline(never)]
pub fn walk_arc(e: Arc<Entry>, d: i32) -> i64 {
    if d <= 1 {
        return e.val;
    }
    walk_arc(Arc::clone(&e), d - 1)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Uniform,
    HotSame,
    HotDiff,
}
impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "hot_same" => Mode::HotSame,
            "hot_diff" => Mode::HotDiff,
            _ => Mode::Uniform,
        }
    }
}
/// returns wall ns (max end - min start) for T threads doing `ops` each. arc_pass=false: Arc cloned once out of the table, then `&Entry`.
pub fn w2_run(t: &Arc<Table<Entry>>, mode: Mode, threads: usize, ops: u64, arc_pass: bool) -> u64 {
    let nent = 10000usize;
    let bar = Arc::new(std::sync::Barrier::new(threads));
    let epoch = std::time::Instant::now();
    let hs: Vec<_> = (0..threads)
        .map(|id| {
            let t = Arc::clone(t);
            let bar = Arc::clone(&bar);
            std::thread::spawn(move || {
                let mut s = MIX ^ ((id as u64 + 1) * 0x1234567);
                let hot = if mode == Mode::HotSame { 17 } else { (id * 601 + 7) % nent };
                let mut sink = 0i64;
                bar.wait();
                let t0 = epoch.elapsed().as_nanos() as u64;
                for _ in 0..ops {
                    let r = rng(&mut s);
                    let k = if mode == Mode::Uniform { ((r >> 11) % nent as u64) as usize } else { hot };
                    let r2 = rng(&mut s);
                    if (r2 >> 33) % 100 >= 5 {
                        let e = t.get(k);
                        sink += if arc_pass { walk_arc(Arc::clone(&e), 3) } else { walk_ref(&e, 3) };
                    } else {
                        let _old = t.swap(k, Arc::new(new_entry(k as i64, (r2 & 0xffff) as i64)));
                    }
                }
                let t1 = epoch.elapsed().as_nanos() as u64;
                std::hint::black_box(sink);
                (t0, t1)
            })
        })
        .collect();
    let r: Vec<(u64, u64)> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    r.iter().map(|x| x.1).max().unwrap() - r.iter().map(|x| x.0).min().unwrap()
}
pub fn w2_table() -> Arc<Table<Entry>> {
    Arc::new(Table::new(10000, |k| new_entry(k as i64, k as i64 * 5 + 1)))
}

// ---------------------------------------------------------------- W3 idiomatic
pub mod idio {
    use super::*;
    pub struct Hdr {
        pub name: u32,
        pub val: [u64; 3],
    }
    pub struct Arr {
        pub hs: [Hdr; 8],
    }
    pub struct Req {
        pub id: u64,
        pub headers: Box<Arr>,
        pub body: Box<[u64; 64]>,
    }
    pub struct Sess {
        pub uid: u64,
        pub counter: u64,
        pub data: [u64; 5],
    }
    pub struct Resp {
        pub status: u32,
        pub body: Box<[u64; 32]>,
    }
    pub struct Ctx {
        pub acc: u64,
        pub sid: u32,
        pub sess: Option<Arc<Sess>>,
        pub resp: Option<Box<Resp>>,
    }
    pub fn new_sess(uid: u64, counter: u64) -> Sess {
        let mut data = [0u64; 5];
        for i in 0..5 {
            data[i] = uid * (i as u64 + 1);
        }
        Sess { uid, counter, data }
    }
    #[inline(never)]
    pub fn new_req(rs: &mut u64) -> Box<Req> {
        let id = rng(rs);
        let mut hs: [Hdr; 8] = std::array::from_fn(|j| Hdr { name: j as u32, val: [0; 3] });
        for j in 0..8 {
            for w in 0..3 {
                hs[j].val[w] = rng(rs);
            }
        }
        let mut b = Box::new([0u64; 64]);
        for i in 0..64 {
            b[i] = rng(rs);
        }
        Box::new(Req { id, headers: Box::new(Arr { hs }), body: b })
    }
    #[inline(never)]
    pub fn layer(k: i32, rq: &Req, cx: &mut Ctx, tb: &Table<Sess>, work: u32) -> u64 {
        match k {
            0 => cx.acc = rq.headers.hs[0].val[0] ^ rq.headers.hs[0].val[1],
            1 => cx.acc = cx.acc.wrapping_add(rq.id),
            2 => {
                for j in 0..8 {
                    if rq.headers.hs[j].name == 3 {
                        cx.acc ^= rq.headers.hs[j].val[0];
                    }
                }
            }
            3 => cx.acc = cx.acc.wrapping_mul(MIX).wrapping_add(1),
            4 => {
                let h = &rq.headers.hs[5].val;
                cx.sid = ((h[0] ^ h[1] ^ h[2]) % NSESS as u64) as u32;
                cx.acc = cx.acc.wrapping_add(cx.sid as u64);
            }
            5 => {
                let s = tb.get(cx.sid as usize);
                cx.acc = cx.acc.wrapping_add(s.counter);
                if (rq.id >> 40) % 20 == 0 {
                    tb.swap(cx.sid as usize, Arc::new(new_sess(s.uid, s.counter + 1)));
                }
                cx.sess = Some(s);
            }
            6 => cx.acc ^= (rq.id >> 8) & 0xff,
            7 => {
                let mut a = cx.acc;
                for _ in 0..work {
                    for i in 0..64 {
                        a = (a ^ rq.body[i]).wrapping_mul(FNV);
                    }
                }
                cx.acc = a;
            }
            8 => {
                let s = cx.sess.as_ref().unwrap();
                let mut b = Box::new([0u64; 32]);
                for i in 0..32 {
                    b[i] = cx.acc.wrapping_add((i as u64).wrapping_mul(s.counter)).wrapping_add(s.uid);
                }
                cx.acc ^= b[31];
                cx.resp = Some(Box::new(Resp { status: 200, body: b }));
            }
            9 => {
                let rp = cx.resp.as_ref().unwrap();
                let mut a = cx.acc;
                for _ in 0..work {
                    for i in 0..32 {
                        a = (a ^ rp.body[i]).wrapping_mul(FNV);
                    }
                }
                cx.acc = a.wrapping_add(rp.status as u64);
            }
            _ => {}
        }
        if k < 9 {
            layer(k + 1, rq, cx, tb, work)
        } else {
            cx.acc
        }
    }
    #[inline(never)]
    pub fn one_request(rs: &mut u64, tb: &Table<Sess>, work: u32) -> u64 {
        let rq = new_req(rs);
        let mut cx = Box::new(Ctx { acc: 0, sid: 0, sess: None, resp: None });
        layer(0, &rq, &mut cx, tb, work)
    }
}

// ---------------------------------------------------------------- W3 pessimal: Arc cloned on every pass
pub mod arcv {
    use super::idio::{Arr, Hdr, Resp as _R, Sess};
    use super::*;
    use std::cell::RefCell;
    pub struct Req {
        pub id: u64,
        pub headers: Arc<Arr>,
        pub body: Arc<[u64; 64]>,
    }
    pub struct Resp {
        pub status: u32,
        pub body: Arc<[u64; 32]>,
    }
    pub struct Ctx {
        pub acc: u64,
        pub sid: u32,
        pub sess: Option<Arc<Sess>>,
        pub resp: Option<Arc<Resp>>,
    }
    #[allow(dead_code)]
    type Unused = _R;
    #[inline(never)]
    pub fn new_req(rs: &mut u64) -> Arc<Req> {
        let id = rng(rs);
        let mut hs: [Hdr; 8] = std::array::from_fn(|j| Hdr { name: j as u32, val: [0; 3] });
        for j in 0..8 {
            for w in 0..3 {
                hs[j].val[w] = rng(rs);
            }
        }
        let a = Arc::new(Arr { hs });
        let mut bb = [0u64; 64];
        for i in 0..64 {
            bb[i] = rng(rs);
        }
        let b = Arc::new(bb);
        let rq = Req { id, headers: Arc::clone(&a), body: Arc::clone(&b) };
        drop(a);
        drop(b); // store into field = clone + drop of the local, as in C1-C3
        Arc::new(rq)
    }
    #[inline(never)]
    pub fn layer(k: i32, rq: Arc<Req>, cx: Arc<RefCell<Ctx>>, tb: &Table<Sess>, work: u32) -> u64 {
        match k {
            0 => {
                let h = Arc::clone(&rq.headers);
                cx.borrow_mut().acc = h.hs[0].val[0] ^ h.hs[0].val[1];
            }
            1 => {
                let mut c = cx.borrow_mut();
                c.acc = c.acc.wrapping_add(rq.id);
            }
            2 => {
                let h = Arc::clone(&rq.headers);
                for j in 0..8 {
                    if h.hs[j].name == 3 {
                        cx.borrow_mut().acc ^= h.hs[j].val[0];
                    }
                }
            }
            3 => {
                let mut c = cx.borrow_mut();
                c.acc = c.acc.wrapping_mul(MIX).wrapping_add(1);
            }
            4 => {
                let h = Arc::clone(&rq.headers);
                let v = &h.hs[5].val;
                let sid = ((v[0] ^ v[1] ^ v[2]) % NSESS as u64) as u32;
                let mut c = cx.borrow_mut();
                c.sid = sid;
                c.acc = c.acc.wrapping_add(sid as u64);
            }
            5 => {
                let sid = cx.borrow().sid as usize;
                let s = tb.get(sid);
                let mut c = cx.borrow_mut();
                c.acc = c.acc.wrapping_add(s.counter);
                if (rq.id >> 40) % 20 == 0 {
                    tb.swap(sid, Arc::new(super::idio::new_sess(s.uid, s.counter + 1)));
                }
                c.sess = Some(Arc::clone(&s));
                drop(s);
            }
            6 => cx.borrow_mut().acc ^= (rq.id >> 8) & 0xff,
            7 => {
                let b = Arc::clone(&rq.body);
                let mut c = cx.borrow_mut();
                let mut a = c.acc;
                for _ in 0..work {
                    for i in 0..64 {
                        a = (a ^ b[i]).wrapping_mul(FNV);
                    }
                }
                c.acc = a;
            }
            8 => {
                let s = Arc::clone(cx.borrow().sess.as_ref().unwrap());
                let acc = cx.borrow().acc;
                let mut bb = [0u64; 32];
                for i in 0..32 {
                    bb[i] = acc.wrapping_add((i as u64).wrapping_mul(s.counter)).wrapping_add(s.uid);
                }
                let b = Arc::new(bb);
                let last = b[31];
                let rp = Arc::new(Resp { status: 200, body: Arc::clone(&b) });
                drop(b);
                let mut c = cx.borrow_mut();
                c.acc ^= last;
                c.resp = Some(Arc::clone(&rp));
                drop(rp);
            }
            9 => {
                let rp = Arc::clone(cx.borrow().resp.as_ref().unwrap());
                let b = Arc::clone(&rp.body);
                let mut c = cx.borrow_mut();
                let mut a = c.acc;
                for _ in 0..work {
                    for i in 0..32 {
                        a = (a ^ b[i]).wrapping_mul(FNV);
                    }
                }
                c.acc = a.wrapping_add(rp.status as u64);
            }
            _ => {}
        }
        if k < 9 {
            layer(k + 1, Arc::clone(&rq), Arc::clone(&cx), tb, work)
        } else {
            let a = cx.borrow().acc;
            a
        }
    }
    #[inline(never)]
    pub fn one_request(rs: &mut u64, tb: &Table<Sess>, work: u32) -> u64 {
        let rq = new_req(rs);
        let cx = Arc::new(RefCell::new(Ctx { acc: 0, sid: 0, sess: None, resp: None }));
        layer(0, Arc::clone(&rq), Arc::clone(&cx), tb, work)
    }
}

// ---------------------------------------------------------------- W4
pub mod w4 {
    pub struct BoxNode {
        pub val: i64,
        pub ch: [Option<Box<BoxNode>>; 4],
    }
    pub fn build_box(i: i64, n: i64) -> Box<BoxNode> {
        let mut ch: [Option<Box<BoxNode>>; 4] = [None, None, None, None];
        for c in 0..4 {
            let ci = 4 * i + 1 + c as i64;
            if ci < n {
                ch[c] = Some(build_box(ci, n));
            }
        }
        Box::new(BoxNode { val: i, ch })
    }
    #[inline(never)]
    pub fn sum_box(n: &BoxNode) -> i64 {
        let mut s = n.val;
        for c in n.ch.iter().flatten() {
            s += sum_box(c);
        }
        s
    }
    // Rc (non-atomic, closest to C1) and Arc (atomic, closest to C3) trees with a Weak parent
    macro_rules! rc_tree {
        ($m:ident, $P:ident, $W:ident) => {
            pub mod $m {
                use std::$m::{$P, $W};
                pub struct Node {
                    pub val: i64,
                    pub ch: [Option<$P<Node>>; 4],
                    pub parent: $W<Node>,
                }
                pub fn build(i: i64, n: i64, parent: $W<Node>, weak: bool) -> $P<Node> {
                    $P::new_cyclic(|me| {
                        let mut ch: [Option<$P<Node>>; 4] = [None, None, None, None];
                        for c in 0..4 {
                            let ci = 4 * i + 1 + c as i64;
                            if ci < n {
                                ch[c] = Some(build(ci, n, if weak { me.clone() } else { $W::new() }, weak));
                            }
                        }
                        Node { val: i, ch, parent }
                    })
                }
                #[inline(never)]
                pub fn sum(n: $P<Node>, weak: bool) -> i64 {
                    let mut s = n.val;
                    if weak {
                        if let Some(p) = n.parent.upgrade() {
                            s += p.val;
                        }
                    }
                    for c in n.ch.iter().flatten() {
                        s += sum($P::clone(c), weak);
                    }
                    s
                }
            }
        };
    }
    rc_tree!(rc, Rc, Weak);
    rc_tree!(sync, Arc, Weak);
}
