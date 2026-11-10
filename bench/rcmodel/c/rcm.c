// rcm.c - hand-emulation of the Gort memory model in C. One source, five variants (-DMODE=n):
//   0 raw      plain pointers, no counts (ownership explicit; upper bound)
//   1 local    every pass inc/dec, NON-atomic counts (only sound single-threaded or on unshared objects)
//   2 flagged  every pass tests obj->shared, then non-atomic or atomic   <- the real Gort model
//   3 atomic   always atomic
//   4 elided   MODE 2 + hand-applied elisions (PASS_B/DROP_B borrows, PASS_L/DROP_L last-use moves)
// Workloads: w1 pass-down, w2 shared table contention, w3 request path, w4 graph alloc/free.
// Output: CSV rows  workload,impl,variant,params,metric,value   (run index is added by run_all.sh)
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <pthread.h>
#include <time.h>
#include <unistd.h>
#include <sys/resource.h>

#include "rc.h"

// ------------------------------------------------------------------ util
static inline uint64_t nowns(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec * 1000000000ull + t.tv_nsec; }
static inline uint64_t rng(uint64_t *s) { uint64_t x = *s; x ^= x >> 12; x ^= x << 25; x ^= x >> 27; *s = x; return x * 2685821657736338717ull; }
#define FNV 0x100000001b3ull
#define MIX 0x9E3779B97F4A7C15ull
static volatile uint64_t g_sink;
static const char *g_params = "";
static void row(const char *w, const char *metric, double v) {
  printf("%s,%s,%s,%s,%s,%.12g\n", w, IMPL, VARIANT, g_params, metric, v); fflush(stdout);
}
static double cpu_s(void) { struct rusage r; getrusage(RUSAGE_SELF, &r); return r.ru_utime.tv_sec + r.ru_stime.tv_sec + (r.ru_utime.tv_usec + r.ru_stime.tv_usec) / 1e6; }
static double rss_mb(void) { struct rusage r; getrusage(RUSAGE_SELF, &r); return r.ru_maxrss / 1024.0; }
static long geti(int ac, char **av, const char *k, long d) { size_t n = strlen(k); for (int i = 2; i < ac; i++) if (!strncmp(av[i], k, n) && av[i][n] == '=') return atol(av[i] + n + 1); return d; }
static const char *gets_(int ac, char **av, const char *k, const char *d) { size_t n = strlen(k); for (int i = 2; i < ac; i++) if (!strncmp(av[i], k, n) && av[i][n] == '=') return av[i] + n + 1; return d; }
static int cmpu32(const void *a, const void *b) { uint32_t x = *(const uint32_t *)a, y = *(const uint32_t *)b; return x < y ? -1 : x > y; }

typedef struct { pthread_t th; int id; void *arg; uint64_t t0, t1; double v[8]; } Th;
static volatile int g_go; static int g_ready;
static void *th_tramp(void *p);
static void (*g_fn)(Th *);
static void run_threads(int T, void (*fn)(Th *), void *arg, Th *ths, uint64_t *wall) {
  g_fn = fn; g_go = 0; g_ready = 0;
  for (int i = 0; i < T; i++) { ths[i].id = i; ths[i].arg = arg; pthread_create(&ths[i].th, NULL, th_tramp, &ths[i]); }
  while (__atomic_load_n(&g_ready, __ATOMIC_ACQUIRE) < T) { }
  __atomic_store_n(&g_go, 1, __ATOMIC_RELEASE);
  for (int i = 0; i < T; i++) pthread_join(ths[i].th, NULL);
  uint64_t a = ths[0].t0, b = ths[0].t1;
  for (int i = 1; i < T; i++) { if (ths[i].t0 < a) a = ths[i].t0; if (ths[i].t1 > b) b = ths[i].t1; }
  *wall = b - a;
}
static void *th_tramp(void *p) {
  Th *t = p;
  __atomic_fetch_add(&g_ready, 1, __ATOMIC_ACQ_REL);
  while (!__atomic_load_n(&g_go, __ATOMIC_ACQUIRE)) { }
  g_fn(t);
  return NULL;
}

// ------------------------------------------------------------------ W1/W2 objects and shared table
typedef struct Entry { Obj h; int64_t key, val; uint8_t payload[48]; uint8_t pad[48]; } Entry;  // 128 bytes
static Entry *entry_new(int64_t key, int64_t val) {
  Entry *e = aligned_alloc(64, sizeof(Entry));
  e->h.rc = 1; e->h.weak = 1; e->h.shared = 0; e->h.type = T_ENTRY; t_allocs++;
  e->key = key; e->val = val; memset(e->payload, (int)key, sizeof e->payload);
  return e;
}

// callee owns one reference ("passing a Ref increments, the drop decrements"); returns the field at the bottom
static NOINLINE int64_t walk(Entry *e, int d) {
  int64_t v;
  if (d <= 1) v = e->val;
  else { v = walk(PASS_B(e), d - 1); }
  DROP_B(e);
  return v;
}

typedef struct { volatile uint32_t l; } __attribute__((aligned(64))) Stripe;
#define NSTR 256
typedef struct Table { Stripe st[NSTR]; Obj **slot; int n; } Table;
AINLINE void lock_(Stripe *s) {
  while (__atomic_exchange_n(&s->l, 1, __ATOMIC_ACQUIRE)) while (__atomic_load_n(&s->l, __ATOMIC_RELAXED)) __builtin_ia32_pause();
}
AINLINE void unlock_(Stripe *s) { __atomic_store_n(&s->l, 0, __ATOMIC_RELEASE); }
static Table *tbl_new(int n) {
  Table *t = aligned_alloc(64, sizeof(Table)); memset(t, 0, sizeof(Table));
  t->slot = calloc(n, sizeof(Obj *)); t->n = n; return t;
}
// returns a counted reference (the lookup clones under the stripe lock)
AINLINE Obj *tbl_get(Table *t, int k) {
  Stripe *s = &t->st[k & (NSTR - 1)];
  lock_(s); Obj *o = t->slot[k]; CLONE_IF_COUNTED(o); unlock_(s);
  return o;
}
AINLINE Obj *tbl_swap(Table *t, int k, Obj *n) {
  Stripe *s = &t->st[k & (NSTR - 1)];
  lock_(s); Obj *o = t->slot[k]; t->slot[k] = n; unlock_(s);
  return o;
}
// raw cannot free a replaced object that readers may still hold; it parks it until the run is over
static __thread Obj **t_ret; static __thread size_t t_nret, t_capret;
static void retire(Obj *old) {
#if MODE == 0
  if (t_nret == t_capret) { t_capret = t_capret ? t_capret * 2 : 1024; t_ret = realloc(t_ret, t_capret * sizeof(Obj *)); }
  t_ret[t_nret++] = old;
#else
  rc_release(old);
#endif
}
static void retire_flush(void) { for (size_t i = 0; i < t_nret; i++) obj_free(t_ret[i]); t_nret = 0; }

// ------------------------------------------------------------------ W1
static void w1(int ac, char **av) {
  int depth = geti(ac, av, "depth", 4); long iters = geti(ac, av, "iters", 100000000); int shared = geti(ac, av, "shared", 0);
  static char ps[64]; snprintf(ps, sizeof ps, "depth=%d;shared=%d", depth, shared); g_params = ps;
  Entry *pool[64];
  for (int i = 0; i < 64; i++) { pool[i] = entry_new(i, i * 3 + 1); pool[i]->h.shared = shared; }
  int64_t sink = 0;
  for (long i = 0; i < iters / 10; i++) { Entry *e = pool[i & 63]; sink += walk(PASS_B(e), depth); }  // warm-up
  uint64_t t0 = nowns();
  for (long i = 0; i < iters; i++) { Entry *e = pool[i & 63]; sink += walk(PASS_B(e), depth); }
  uint64_t t1 = nowns();
  g_sink = sink;
  row("W1", "ns_per_pass", (double)(t1 - t0) / ((double)iters * depth));
  row("W1", "checksum", (double)(sink & 0xffffff));
}

// ------------------------------------------------------------------ W2
typedef struct { Table *t; int mode; long ops; int nent; } W2a;  // mode 0 uniform, 1 hot_same, 2 hot_diff
static void w2_worker(Th *th) {
  W2a *a = th->arg; Table *t = a->t; uint64_t s = 0x9E3779B97F4A7C15ull ^ ((uint64_t)(th->id + 1) * 0x1234567ull);
  int64_t sink = 0; int hot = a->mode == 1 ? 17 : (int)(((long)th->id * 601 + 7) % a->nent);
  th->t0 = nowns();
  for (long i = 0; i < a->ops; i++) {
    uint64_t r = rng(&s); int k = a->mode == 0 ? (int)((r >> 11) % a->nent) : hot;
    uint64_t r2 = rng(&s);
    if ((r2 >> 33) % 100 >= 5) {
      Entry *e = (Entry *)tbl_get(t, k);
      sink += walk(PASS_B(e), 3);
      DROP(e);
    } else {
      Entry *n = entry_new(k, (int64_t)(r2 & 0xffff)); n->h.shared = 1;   // publish: flag set before it becomes visible
      retire(tbl_swap(t, k, &n->h));
    }
  }
  th->t1 = nowns();
  g_sink += sink; retire_flush();
}
static void w2(int ac, char **av) {
  int T = geti(ac, av, "threads", 1), nent = geti(ac, av, "entries", 10000); long ops = geti(ac, av, "ops", 10000000);
  const char *m = gets_(ac, av, "mode", "uniform");
  static char ps[96]; snprintf(ps, sizeof ps, "threads=%d;mode=%s", T, m); g_params = ps;
  W2a a = { tbl_new(nent), !strcmp(m, "hot_same") ? 1 : !strcmp(m, "hot_diff") ? 2 : 0, ops, nent };
  for (int i = 0; i < nent; i++) { Entry *e = entry_new(i, i * 5 + 1); e->h.shared = 1; a.t->slot[i] = &e->h; }
  Th ths[64]; uint64_t wall; run_threads(T, w2_worker, &a, ths, &wall);
  row("W2", "mops_total", (double)T * ops / (wall / 1e9) / 1e6);
  row("W2", "ns_per_op_per_thread", (double)wall / ops);
}

// ------------------------------------------------------------------ W3: request path
typedef struct { Obj h; uint32_t n, pad; struct { uint32_t name, pad; uint64_t val[3]; } hs[8]; } Arr;     // header array (8 headers)
typedef struct { Obj h; uint64_t data[64]; } Buf512;
typedef struct { Obj h; uint64_t data[32]; } Buf256;
typedef struct { Obj h; uint64_t id; Arr *headers; Buf512 *body; } Req;
typedef struct { Obj h; uint64_t uid, counter; uint64_t data[5]; } Sess;
typedef struct { Obj h; uint32_t status, pad; Buf256 *body; } Resp;
typedef struct { Obj h; uint64_t acc; uint32_t sid, pad; Sess *sess; Resp *resp; } Ctx;
#define NSESS 1024

static void obj_free(Obj *o) {
  switch (o->type) {
    case T_REQ: { Req *r = (Req *)o; OWN_DROP(r->headers); OWN_DROP(r->body); break; }
    case T_RESP: { Resp *r = (Resp *)o; OWN_DROP(r->body); break; }
    case T_CTX: { Ctx *c = (Ctx *)o; if (c->sess) DROP(c->sess); if (c->resp) OWN_DROP(c->resp); break; }
    case T_NODE: break;  // handled by node_free below
    default: break;
  }
  if (o->type == T_NODE) { extern void node_free(Obj *); node_free(o); return; }
  t_frees++;
  if (o->type == T_ENTRY) free(o); else free(o);
}

static Sess *sess_new(uint64_t uid, uint64_t counter) {
  Sess *s = oalloc(sizeof(Sess), T_SESS); s->uid = uid; s->counter = counter;
  for (int i = 0; i < 5; i++) s->data[i] = uid * (i + 1);
  return s;
}
static Req *req_new(uint64_t *rs) {
  Req *rq = oalloc(sizeof(Req), T_REQ); rq->id = rng(rs);
  Arr *a = oalloc(sizeof(Arr), T_ARR); a->n = 8;
  for (int j = 0; j < 8; j++) { a->hs[j].name = j; for (int w = 0; w < 3; w++) a->hs[j].val[w] = rng(rs); }
  Buf512 *b = oalloc(sizeof(Buf512), T_BUF);
  for (int i = 0; i < 64; i++) b->data[i] = rng(rs);
  rq->headers = PASS_L(a); DROP_L(a);   // store into field: clone, then the local dies (elided as a move in C4)
  rq->body = PASS_L(b); DROP_L(b);
  return rq;
}
static int g_work = 1;   // W3 sensitivity knob: how many times the body/response hash loops run (default 1)
typedef struct { Table *t; long total, warm; int private_; int nthr; uint32_t *lat; uint64_t checksum; } W3a;

static NOINLINE uint64_t layer(int k, Req *rq, Ctx *cx, Table *tb) {
  uint64_t r;
  switch (k) {
    case 0: { Arr *h = PASS_B(rq->headers); cx->acc = h->hs[0].val[0] ^ h->hs[0].val[1]; DROP_B(h); break; }
    case 1: cx->acc += rq->id; break;
    case 2: { Arr *h = PASS_B(rq->headers); for (int j = 0; j < 8; j++) if (h->hs[j].name == 3) cx->acc ^= h->hs[j].val[0]; DROP_B(h); break; }
    case 3: cx->acc = cx->acc * MIX + 1; break;
    case 4: { Arr *h = PASS_B(rq->headers); cx->sid = (uint32_t)((h->hs[5].val[0] ^ h->hs[5].val[1] ^ h->hs[5].val[2]) % NSESS); cx->acc += cx->sid; DROP_B(h); break; }
    case 5: {
      Sess *s = (Sess *)tbl_get(tb, cx->sid);            // counted clone out of the shared table (kept in all variants)
      cx->acc += s->counter;
      if (((rq->id >> 40) % 20) == 0) {                   // 5% of requests update the session
        Sess *n = sess_new(s->uid, s->counter + 1); n->h.shared = 1;
        retire(tbl_swap(tb, cx->sid, &n->h));
      }
      cx->sess = PASS_L(s); DROP_L(s);                    // moved into ctx (elided as a move in C4)
      break;
    }
    case 6: cx->acc ^= (rq->id >> 8) & 0xff; break;
    case 7: { Buf512 *b = PASS_B(rq->body); uint64_t a = cx->acc; for (int w = 0; w < g_work; w++) for (int i = 0; i < 64; i++) a = (a ^ b->data[i]) * FNV; cx->acc = a; DROP_B(b); break; }
    case 8: {
      Sess *s = PASS_B(cx->sess);
      Resp *rp = oalloc(sizeof(Resp), T_RESP); rp->status = 200;
      Buf256 *b = oalloc(sizeof(Buf256), T_BUF);
      for (int i = 0; i < 32; i++) b->data[i] = cx->acc + (uint64_t)i * s->counter + s->uid;
      cx->acc ^= b->data[31];
      rp->body = PASS_L(b); DROP_L(b);
      cx->resp = PASS_L(rp); DROP_L(rp);
      DROP_B(s); break;
    }
    case 9: { Resp *rp = PASS_B(cx->resp); Buf256 *b = PASS_B(rp->body); uint64_t a = cx->acc; for (int w = 0; w < g_work; w++) for (int i = 0; i < 32; i++) a = (a ^ b->data[i]) * FNV; cx->acc = a + rp->status; DROP_B(b); DROP_B(rp); break; }
  }
  if (k < 9) r = layer(k + 1, PASS_B(rq), PASS_B(cx), tb); else r = cx->acc;
  DROP_B(rq); DROP_B(cx);
  return r;
}
static uint64_t one_request(uint64_t *rs, Table *tb) {
  Req *rq = req_new(rs);
  Ctx *cx = oalloc(sizeof(Ctx), T_CTX); cx->acc = 0; cx->sess = NULL; cx->resp = NULL;
  uint64_t v = layer(0, PASS_B(rq), PASS_B(cx), tb);
  OWN_DROP(cx); OWN_DROP(rq);
  return v;
}
static void w3_worker(Th *th) {
  W3a *a = th->arg; uint64_t rs = 0x9E3779B97F4A7C15ull ^ ((uint64_t)(th->id + 1) * 0x1234567ull);
  Table *tb = a->t;
  if (a->private_) {
    tb = tbl_new(NSESS);
    for (int i = 0; i < NSESS; i++) { Sess *s = sess_new(i, i * 7); s->h.shared = 0; tb->slot[i] = &s->h; }
  }
  long n = a->total / a->nthr; uint32_t *lat = a->lat + (size_t)th->id * n; uint64_t cs = 0;
  uint64_t ws = rs ^ 0xABCDEF;
  for (long i = 0; i < a->warm; i++) cs += one_request(&ws, tb);
  __atomic_fetch_add(&g_ready, 1, __ATOMIC_ACQ_REL);   // (re-sync after warm-up: workers wait for each other)
  while (__atomic_load_n(&g_ready, __ATOMIC_ACQUIRE) < 2 * a->nthr) { }
  cs = 0; t_allocs = t_frees = 0;
#ifdef COUNT
  c_inc = c_atomic = c_dec = 0;
#endif
  th->t0 = nowns();
  for (long i = 0; i < n; i++) {
    uint64_t t = nowns(); cs += one_request(&rs, tb); lat[i] = (uint32_t)(nowns() - t);
  }
  th->t1 = nowns();
  th->v[0] = (double)cs; th->v[1] = (double)t_allocs; th->v[2] = (double)t_frees;
#ifdef COUNT
  th->v[3] = c_inc; th->v[4] = c_atomic; th->v[5] = c_dec;
#endif
  retire_flush();
  if (a->private_) { /* leave the private table; process exits */ }
}
static void w3(int ac, char **av) {
  int T = geti(ac, av, "workers", 1); long total = geti(ac, av, "requests", 5000000), warm = geti(ac, av, "warmup", 300000);
  const char *tbm = gets_(ac, av, "table", "shared"); g_work = geti(ac, av, "work", 1);
  static char ps[96]; snprintf(ps, sizeof ps, "workers=%d;table=%s;work=%d", T, tbm, g_work); g_params = ps;
  W3a a = { NULL, total, warm, !strcmp(tbm, "private"), T, malloc((size_t)(total / T) * T * sizeof(uint32_t)), 0 };
  if (!a.private_) {
    a.t = tbl_new(NSESS);
    for (int i = 0; i < NSESS; i++) { Sess *s = sess_new(i, i * 7); s->h.shared = 1; a.t->slot[i] = &s->h; }
  }
  Th ths[64]; uint64_t wall;
  g_fn = NULL;
  // run_threads resets g_ready to 0 and waits for T; the worker adds a second round after warm-up
  run_threads(T, w3_worker, &a, ths, &wall);
  long n = total / T; size_t N = (size_t)n * T;
  qsort(a.lat, N, sizeof(uint32_t), cmpu32);
  double cs = 0, al = 0, fr = 0; for (int i = 0; i < T; i++) { cs += ths[i].v[0]; al += ths[i].v[1]; fr += ths[i].v[2]; }
  row("W3", "rps", (double)N / (wall / 1e9));
  row("W3", "p50_ns", a.lat[N / 2]); row("W3", "p99_ns", a.lat[(size_t)(N * 0.99)]); row("W3", "p999_ns", a.lat[(size_t)(N * 0.999)]);
  row("W3", "rss_mb", rss_mb()); row("W3", "cpu_s", cpu_s());
  row("W3", "allocs_per_req", al / N); row("W3", "alloc_minus_free", al - fr);
  if (T == 1) row("W3", "checksum", (double)((uint64_t)cs & 0xffffffffull));
#ifdef COUNT
  { double ci = 0, ca = 0, cd = 0; for (int i = 0; i < T; i++) { ci += ths[i].v[3]; ca += ths[i].v[4]; cd += ths[i].v[5]; }
    row("W3", "incs_per_req", ci / N); row("W3", "atomic_incs_per_req", ca / N); row("W3", "decs_per_req", cd / N); }
#endif
}

// ------------------------------------------------------------------ W4: tree alloc/free
typedef struct Node { Obj h; int64_t val; struct Node *ch[4]; struct Node *parent; } Node;
static void weak_release(Node *p) { if (weak_dec(&p->h)) { t_frees++; free(p); } }
void node_free(Obj *o) {
  Node *n = (Node *)o;
  for (int i = 0; i < 4; i++) if (n->ch[i]) OWN_DROP(n->ch[i]);
#if MODE != 0
  if (g_weak && n->parent) weak_release(n->parent);
  if (n->h.weak == 1) { t_frees++; free(n); } else weak_release(n);   // fast path: no weak refs outstanding
#else
  t_frees++; free(n);
#endif
}
static Node *build(int64_t i, int64_t N, Node *parent) {
  Node *n = oalloc(sizeof(Node), T_NODE); n->val = i; n->parent = NULL;
  if (g_weak && parent) {
    n->parent = parent;
#if MODE != 0
    weak_inc(&parent->h);
#endif
  }
  for (int c = 0; c < 4; c++) { int64_t ci = 4 * i + 1 + c; n->ch[c] = ci < N ? build(ci, N, n) : NULL; }
  return n;
}
static NOINLINE int64_t tsum(Node *n) {
  int64_t s = n->val;
  if (g_weak && n->parent) {
#if MODE == 0
    s += n->parent->val;
#else
    Node *p = n->parent;   // upgrade: fails if rc==0; unshared here, so a plain check suffices when !shared
    if (p->h.shared) { uint32_t c = __atomic_load_n(&p->h.rc, __ATOMIC_RELAXED); while (c && !__atomic_compare_exchange_n(&p->h.rc, &c, c + 1, 1, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) { } if (c) { s += p->val; DROP(p); } }
    else if (p->h.rc) { rc_inc(&p->h); s += p->val; DROP(p); }
#endif
  }
  for (int c = 0; c < 4; c++) if (n->ch[c]) { Node *ch = n->ch[c]; s += tsum(PASS_B(ch)); }
  DROP_B(n);
  return s;
}
typedef struct { int64_t N; int cycles; } W4a;
static int cmpd(const void *a, const void *b) { double x = *(const double *)a, y = *(const double *)b; return x < y ? -1 : x > y; }
static void w4_worker(Th *th) {
  W4a *a = th->arg; double b[64], t[64], f[64];
  th->t0 = nowns();
  int64_t sum = 0;
  for (int c = 0; c < a->cycles; c++) {
    uint64_t t0 = nowns(); Node *root = build(0, a->N, NULL);
    uint64_t t1 = nowns(); sum += tsum(PASS_B(root));
    uint64_t t2 = nowns(); OWN_DROP(root);
    uint64_t t3 = nowns();
    b[c] = (double)(t1 - t0) / a->N; t[c] = (double)(t2 - t1) / a->N; f[c] = (double)(t3 - t2) / a->N;
  }
  th->t1 = nowns(); g_sink += sum;
  // drop first cycle (page faults / lazy init), median of the rest
  qsort(b + 1, a->cycles - 1, sizeof(double), cmpd); qsort(t + 1, a->cycles - 1, sizeof(double), cmpd); qsort(f + 1, a->cycles - 1, sizeof(double), cmpd);
  int m = 1 + (a->cycles - 1) / 2; th->v[0] = b[m]; th->v[1] = t[m]; th->v[2] = f[m];
}
static void w4(int ac, char **av) {
  int T = geti(ac, av, "threads", 1); g_weak = geti(ac, av, "weak", 0);
  W4a a = { geti(ac, av, "nodes", 1000000), geti(ac, av, "cycles", 6) };
  static char ps[64]; snprintf(ps, sizeof ps, "threads=%d;weak=%d", T, g_weak); g_params = ps;
  Th ths[64]; uint64_t wall; run_threads(T, w4_worker, &a, ths, &wall);
  double b = 0, t = 0, f = 0; for (int i = 0; i < T; i++) { b += ths[i].v[0]; t += ths[i].v[1]; f += ths[i].v[2]; }
  b /= T; t /= T; f /= T;
  row("W4", "build_ns_per_node", b); row("W4", "traverse_ns_per_node", t); row("W4", "free_ns_per_node", f);
  row("W4", "cycle_ns_per_node", b + t + f);
  row("W4", "cpu_ns_per_node_cycle", cpu_s() * 1e9 / ((double)a.N * a.cycles * T)); row("W4", "rss_mb", rss_mb());
}

int main(int ac, char **av) {
  if (ac < 2) { fprintf(stderr, "usage: rcm w1|w2|w3|w4 key=val...\n"); return 2; }
  if (!strcmp(av[1], "w1")) w1(ac, av); else if (!strcmp(av[1], "w2")) w2(ac, av);
  else if (!strcmp(av[1], "w3")) w3(ac, av); else if (!strcmp(av[1], "w4")) w4(ac, av);
  else return 2;
  return 0;
}
