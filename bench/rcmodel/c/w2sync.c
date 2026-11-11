// w2sync.c - W2 (shared-table read contention) under reader-friendly synchronisation schemes.
// Built with MODE=2 (the real Gort model: shared objects take the atomic count path). The scheme is a runtime parameter:
//   spin         L0  exclusive spinlock per stripe, reader clones under the lock (= rcm.c W2, the baseline)
//   rw           L1  reader-writer lock per stripe, reader clones under the read lock (control)
//   qsbr_clone   L2  lock-free read + QSBR reclamation, reader still clones and counts every pass
//   qsbr_borrow  L3  lock-free read + QSBR, reader BORROWS: no clone, passes inside the borrow are uncounted (shipping design)
//   hp           L4  hazard pointer + borrow, reader fences (mfence) on publish
//   hp_asym      L4' hazard pointer + borrow, reader uses a compiler barrier only; reclaimer issues membarrier(2)
// hpt= sets the hazard-pointer scan threshold (default 2T+64); hp_asym needs ~1024, or the membarrier IPIs dominate.
// Per read: look up key k, walk the entry 3 frames deep (as rcm.c W2), optional "promotion" (the borrow escapes: the
// compiler-inserted clone, kept until the next promotion drops it), optional thread-local work after the read.
// Writes (wpct %) allocate a new entry and swap it in; the table's reference to the old one is released at once under a
// lock, or after a grace period (QSBR) / when no hazard pointer covers it (HP).
// Quiescent states (QSBR) are announced every q ops, modelling a fiber yield / request boundary every q reads.
// Output: CSV rows  workload,impl,variant,params,metric,value  (variant = scheme)
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <pthread.h>
#include <time.h>
#include <unistd.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <linux/membarrier.h>

#include "rc.h"

#if MODE != 2
#error "w2sync models the flagged (MODE=2) Gort variant only"
#endif

static inline uint64_t nowns(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec * 1000000000ull + t.tv_nsec; }
static inline uint64_t rng(uint64_t *s) { uint64_t x = *s; x ^= x >> 12; x ^= x << 25; x ^= x >> 27; *s = x; return x * 2685821657736338717ull; }
static double rss_mb(void) { struct rusage r; getrusage(RUSAGE_SELF, &r); return r.ru_maxrss / 1024.0; }
static long geti(int ac, char **av, const char *k, long d) { size_t n = strlen(k); for (int i = 1; i < ac; i++) if (!strncmp(av[i], k, n) && av[i][n] == '=') return atol(av[i] + n + 1); return d; }
static const char *gets_(int ac, char **av, const char *k, const char *d) { size_t n = strlen(k); for (int i = 1; i < ac; i++) if (!strncmp(av[i], k, n) && av[i][n] == '=') return av[i] + n + 1; return d; }

static uint64_t g_allocs, g_frees, g_sink; static char g_params[160];   // summed from the per-thread counters in rc.h at thread exit
static void obj_free(Obj *o) { free(o); t_frees++; }

enum { S_SPIN, S_RW, S_QSBR_CLONE, S_QSBR_BORROW, S_HP, S_HP_ASYM, S_N };
static const char *snames[S_N] = { "spin", "rw", "qsbr_clone", "qsbr_borrow", "hp", "hp_asym" };

// 192-byte entry. split=0 reads key/val from the header's cache line (as rcm.c); split=1 reads key2/val2 on the next line,
// so a reader's data load does not share a line with the count other readers hammer.
typedef struct Entry { Obj h; int64_t key, val; uint8_t pad1[32]; int64_t key2, val2; uint8_t payload[48]; uint8_t pad2[64]; } Entry;
_Static_assert(sizeof(Entry) == 192, "Entry layout");
_Static_assert(__builtin_offsetof(Entry, val2) == 72, "Entry layout");

static volatile int g_stop; static int g_dur_ms, g_warm_ms = -1, g_hash;
// comparative protocol (warm_ms >= 0): threads publish their op count every 64 ops; main measures the count delta over
// [warm_ms, warm_ms + dur_ms]. Same in the Go, Java and Rust w2x drivers.
typedef struct { volatile long n; uint8_t pad[120]; } __attribute__((aligned(128))) Prog;
static Prog g_prog[64];
#define HBUF 16384
// "real work": FNV-1a over hash bytes of a thread-local request buffer, seeded by the value read (as a handler parsing or serialising)
static NOINLINE uint64_t fnv(const uint8_t *b, int n, uint64_t seed) {
  uint64_t h = 0xcbf29ce484222325ull ^ seed;
  for (int j = 0; j < n; j++) h = (h ^ b[j]) * 0x100000001b3ull;
  return h;
}
static int g_scheme, g_T, g_voff, g_wpct, g_ppct, g_work, g_q, g_mode, g_nent, g_stall_ms; static long g_ops;

static Entry *entry_new(int64_t key, int64_t val) {
  Entry *e = aligned_alloc(64, sizeof(Entry));
  if (!e) { perror("aligned_alloc"); exit(1); }
  e->h.rc = 1; e->h.weak = 1; e->h.shared = 1; e->h.type = T_ENTRY; t_allocs++;   // published entries are shared from birth
  e->key = e->key2 = key; e->val = e->val2 = val; memset(e->payload, (int)key, sizeof e->payload);
  return e;
}
#define VAL(e) (*(volatile int64_t *)((char *)(e) + g_voff))

// counted walk: every frame owns a reference (PASS = inc, DROP = dec; atomic because the entry is shared)
static NOINLINE int64_t walk_c(Entry *e, int d) {
  int64_t v = d <= 1 ? VAL(e) : walk_c(PASS(e), d - 1);
  DROP(e);
  return v;
}
// borrowed walk: the caller's borrow outlives the callee, so no counts at all
static NOINLINE int64_t walk_b(Entry *e, int d) {
  int64_t v = d <= 1 ? VAL(e) : walk_b(e, d - 1);
  __asm__ volatile("" ::: "memory");
  return v;
}

// ------------------------------------------------------------------ table and stripe locks
typedef struct { volatile uint32_t l; } __attribute__((aligned(64))) Stripe;
#define NSTR 256
static Stripe g_st[NSTR];
static Obj **g_slot;

AINLINE void spin_lock(Stripe *s) {
  while (__atomic_exchange_n(&s->l, 1, __ATOMIC_ACQUIRE)) while (__atomic_load_n(&s->l, __ATOMIC_RELAXED)) __builtin_ia32_pause();
}
AINLINE void spin_unlock(Stripe *s) { __atomic_store_n(&s->l, 0, __ATOMIC_RELEASE); }
// rw: bit 0 = writer, readers add 2. A reader that sees the writer bit backs out and waits; the writer sets the bit, then waits for readers to drain.
AINLINE void rd_lock(Stripe *s) {
  for (;;) {
    if (!(__atomic_fetch_add(&s->l, 2, __ATOMIC_ACQUIRE) & 1)) return;
    __atomic_fetch_sub(&s->l, 2, __ATOMIC_RELAXED);
    while (__atomic_load_n(&s->l, __ATOMIC_RELAXED) & 1) __builtin_ia32_pause();
  }
}
AINLINE void rd_unlock(Stripe *s) { __atomic_fetch_sub(&s->l, 2, __ATOMIC_RELEASE); }
AINLINE void wr_lock(Stripe *s) {
  while (__atomic_fetch_or(&s->l, 1, __ATOMIC_ACQUIRE) & 1) while (__atomic_load_n(&s->l, __ATOMIC_RELAXED) & 1) __builtin_ia32_pause();
  while (__atomic_load_n(&s->l, __ATOMIC_ACQUIRE) != 1) __builtin_ia32_pause();
}
AINLINE void wr_unlock(Stripe *s) { __atomic_fetch_and(&s->l, ~1u, __ATOMIC_RELEASE); }

// ------------------------------------------------------------------ per-thread shared state (one 128-byte block each)
#define MAXT 64
#define OFFLINE (1ull << 63)
typedef struct { uint64_t ctr; Obj *hp; uint8_t pad[112]; } __attribute__((aligned(128))) PT;
static PT g_pt[MAXT];

// QSBR (liburcu-qsbr style): a thread announces a quiescent state by bumping its own counter between full fences.
// A retired batch records every other thread's counter; it is freed once each has moved on (or gone offline).
#define BATCH 32
typedef struct Batch { struct Batch *next; int n; uint64_t snap[MAXT]; Obj *o[BATCH]; } Batch;
static __thread Batch *t_cur, *t_head, *t_tail;
static __thread long t_pending, t_pending_max;
static __thread int t_id;

AINLINE void qs_announce(int id) {
  __atomic_thread_fence(__ATOMIC_SEQ_CST);
  __atomic_store_n(&g_pt[id].ctr, g_pt[id].ctr + 1, __ATOMIC_RELAXED);
  __atomic_thread_fence(__ATOMIC_SEQ_CST);
}
static int batch_ready(Batch *b) {
  for (int i = 0; i < g_T; i++) {
    if (i == t_id) continue;   // the retiring thread holds no borrow across ops
    uint64_t c = __atomic_load_n(&g_pt[i].ctr, __ATOMIC_ACQUIRE);
    if (!(c & OFFLINE) && c == b->snap[i]) return 0;
  }
  return 1;
}
static void qsbr_reclaim(void) {
  while (t_head && batch_ready(t_head)) {
    Batch *b = t_head; t_head = b->next; if (!t_head) t_tail = NULL;
    for (int i = 0; i < b->n; i++) rc_release(b->o[i]);   // drop the table's reference
    t_pending -= b->n; free(b);
  }
}
static void qsbr_retire(Obj *o) {
  if (!t_cur) { t_cur = malloc(sizeof(Batch)); t_cur->n = 0; t_cur->next = NULL; }
  t_cur->o[t_cur->n++] = o;
  if (++t_pending > t_pending_max) t_pending_max = t_pending;
  if (t_cur->n == BATCH) {
    __atomic_thread_fence(__ATOMIC_SEQ_CST);
    for (int i = 0; i < g_T; i++) t_cur->snap[i] = __atomic_load_n(&g_pt[i].ctr, __ATOMIC_ACQUIRE);
    if (t_tail) t_tail->next = t_cur; else t_head = t_cur;
    t_tail = t_cur; t_cur = NULL;
  }
  qsbr_reclaim();
}

// Hazard pointers: one slot per thread. Retired objects are scanned against every slot once the list reaches the threshold.
static __thread Obj **t_hret; static __thread int t_hn, t_hcap;
static int g_hp_thresh;
static void membarrier_all(void) {
  if (syscall(__NR_membarrier, MEMBARRIER_CMD_PRIVATE_EXPEDITED, 0, 0)) { perror("membarrier"); exit(1); }
}
static void hp_scan(void) {
  Obj *hz[MAXT];
  if (g_scheme == S_HP_ASYM) membarrier_all(); else __atomic_thread_fence(__ATOMIC_SEQ_CST);
  for (int i = 0; i < g_T; i++) hz[i] = __atomic_load_n(&g_pt[i].hp, __ATOMIC_ACQUIRE);
  int w = 0;
  for (int j = 0; j < t_hn; j++) {
    Obj *o = t_hret[j]; int held = 0;
    for (int i = 0; i < g_T; i++) if (hz[i] == o) { held = 1; break; }
    if (held) t_hret[w++] = o; else { rc_release(o); t_pending--; }
  }
  t_hn = w;
}
static void hp_retire(Obj *o) {
  if (t_hn == t_hcap) { t_hcap = t_hcap ? t_hcap * 2 : 256; t_hret = realloc(t_hret, t_hcap * sizeof(Obj *)); }
  t_hret[t_hn++] = o;
  if (++t_pending > t_pending_max) t_pending_max = t_pending;
  if (t_hn >= g_hp_thresh) hp_scan();
}
AINLINE Entry *hp_protect(int id, int k) {
  Obj *p;
  do {
    p = __atomic_load_n(&g_slot[k], __ATOMIC_ACQUIRE);
    __atomic_store_n(&g_pt[id].hp, p, __ATOMIC_RELAXED);
    if (g_scheme == S_HP_ASYM) __asm__ volatile("" ::: "memory");   // the reclaimer's membarrier supplies the fence
    else __atomic_thread_fence(__ATOMIC_SEQ_CST);
  } while (__atomic_load_n(&g_slot[k], __ATOMIC_ACQUIRE) != p);
  return (Entry *)p;
}
AINLINE void hp_clear(int id) { __atomic_store_n(&g_pt[id].hp, NULL, __ATOMIC_RELEASE); }

// ------------------------------------------------------------------ threads
typedef struct { pthread_t th; int id; long done; uint64_t t0, t1; long pending_max; Batch *left_head, *left_cur; Obj **left_hp; int left_hn; } Th;
static volatile int g_go; static int g_ready;

static void stall(void) { struct timespec ts = { g_stall_ms / 1000, (long)(g_stall_ms % 1000) * 1000000L }; nanosleep(&ts, NULL); }

static void *worker(void *p) {
  Th *th = p; int id = th->id; t_id = id;
  uint64_t s = 0x9E3779B97F4A7C15ull ^ ((uint64_t)(id + 1) * 0x1234567ull), s2 = s ^ 0x5555;
  int64_t sink = 0; Obj *keep = NULL;
  uint8_t *hb = malloc(HBUF); for (int j = 0; j < HBUF; j++) hb[j] = (uint8_t)rng(&s2);
  int hot = g_mode == 1 ? 17 : (int)(((long)id * 601 + 7) % g_nent);
  long stall_at = (g_stall_ms > 0 && id == 0) ? (g_dur_ms ? 100000 : g_ops / 2) : -1;
  int sc = g_scheme, qsbr = sc == S_QSBR_CLONE || sc == S_QSBR_BORROW, hp = sc == S_HP || sc == S_HP_ASYM;
  __atomic_fetch_add(&g_ready, 1, __ATOMIC_ACQ_REL);
  while (!__atomic_load_n(&g_go, __ATOMIC_ACQUIRE)) { }
  th->t0 = nowns();
  long i;
  for (i = 0; i < g_ops; i++) {
    if (g_dur_ms && (i & 63) == 0) { g_prog[id].n = i; if (__atomic_load_n(&g_stop, __ATOMIC_RELAXED)) break; }
    if (qsbr && i % g_q == 0) qs_announce(id);
    uint64_t r = rng(&s); int k = g_mode == 0 ? (int)((r >> 11) % g_nent) : hot;
    uint64_t r2 = rng(&s);
    int is_write = (int)((r2 >> 33) % 100) < g_wpct && i != stall_at;
    if (is_write) {
      Entry *n = entry_new(k, (int64_t)(r2 & 0xffff));
      Stripe *st = &g_st[k & (NSTR - 1)];
      Obj *old;
      switch (sc) {
      case S_SPIN: spin_lock(st); old = g_slot[k]; g_slot[k] = &n->h; spin_unlock(st); rc_release(old); break;
      case S_RW:   wr_lock(st);   old = g_slot[k]; g_slot[k] = &n->h; wr_unlock(st);   rc_release(old); break;
      default:
        old = __atomic_exchange_n(&g_slot[k], &n->h, __ATOMIC_SEQ_CST);
        if (qsbr) qsbr_retire(old); else hp_retire(old);
      }
      continue;
    }
    Stripe *st = &g_st[k & (NSTR - 1)];
    Entry *e; int64_t v;
    switch (sc) {
    case S_SPIN: spin_lock(st); e = (Entry *)g_slot[k]; rc_inc(&e->h); spin_unlock(st); break;
    case S_RW:   rd_lock(st);   e = (Entry *)g_slot[k]; rc_inc(&e->h); rd_unlock(st);   break;
    case S_QSBR_CLONE: e = (Entry *)__atomic_load_n(&g_slot[k], __ATOMIC_ACQUIRE); rc_inc(&e->h); break;
    case S_QSBR_BORROW: e = (Entry *)__atomic_load_n(&g_slot[k], __ATOMIC_ACQUIRE); break;
    default: e = hp_protect(id, k);
    }
    if (i == stall_at) stall();   // thread 0 parks mid-read: holds a borrow (or a counted ref) and announces nothing
    if (sc <= S_QSBR_CLONE) {     // counted reader (as rcm.c W2): clone at lookup, every pass counted, drop
      v = walk_c(PASS(e), 3);
      DROP(e);
    } else {
      v = walk_b(e, 3);
      if (g_ppct && (int)((r2 >> 13) % 100) < g_ppct) {   // the borrow escapes: compiler-inserted clone, taken while the borrow is valid
        rc_inc(&e->h); Obj *o = keep; keep = &e->h; if (o) rc_release(o);
      }
      if (hp) hp_clear(id);
    }
    sink += v;
    if (g_hash) sink += (int64_t)fnv(hb, g_hash, (uint64_t)v);
    for (int j = 0; j < g_work; j++) sink += (int64_t)rng(&s2);   // thread-local work after the read (handler compute / I/O stand-in)
  }
  th->t1 = nowns(); th->done = i; free(hb);
  if (keep) rc_release(keep);
  if (qsbr) { __atomic_thread_fence(__ATOMIC_SEQ_CST); __atomic_store_n(&g_pt[id].ctr, g_pt[id].ctr | OFFLINE, __ATOMIC_RELEASE); }
  th->pending_max = t_pending_max; th->left_head = t_head; th->left_cur = t_cur; th->left_hp = t_hret; th->left_hn = t_hn;
  __atomic_fetch_add(&g_allocs, t_allocs, __ATOMIC_RELAXED); __atomic_fetch_add(&g_frees, t_frees, __ATOMIC_RELAXED);
  __atomic_fetch_add(&g_sink, (uint64_t)sink, __ATOMIC_RELAXED);
  return NULL;
}

static void row(const char *metric, double v) { printf("W2S,%s,%s,%s,%s,%.12g\n", IMPL, snames[g_scheme], g_params, metric, v); fflush(stdout); }

int main(int ac, char **av) {
  g_T = geti(ac, av, "threads", 1); g_nent = geti(ac, av, "entries", 10000); g_ops = geti(ac, av, "ops", 3000000);
  g_wpct = geti(ac, av, "wpct", 5); g_ppct = geti(ac, av, "ppct", 0); g_work = geti(ac, av, "work", 0);
  g_q = geti(ac, av, "q", 16); g_stall_ms = geti(ac, av, "stall_ms", 0); g_dur_ms = geti(ac, av, "dur_ms", 0); if (g_dur_ms) g_ops = 1L << 62;
  g_warm_ms = geti(ac, av, "warm_ms", -1); g_hash = geti(ac, av, "hash", 0);
  if (g_hash < 0 || g_hash > HBUF || (g_warm_ms >= 0 && !g_dur_ms)) { fprintf(stderr, "hash must be 0..%d; warm_ms needs dur_ms\n", HBUF); return 2; }
  int split = geti(ac, av, "split", 0); g_voff = split ? 72 : 24;
  const char *m = gets_(ac, av, "mode", "hot_same"), *sn = gets_(ac, av, "scheme", "spin");
  g_mode = !strcmp(m, "hot_same") ? 1 : !strcmp(m, "hot_diff") ? 2 : !strcmp(m, "uniform") ? 0 : -1;
  g_scheme = -1; for (int i = 0; i < S_N; i++) if (!strcmp(sn, snames[i])) g_scheme = i;
  if (g_scheme < 0 || g_mode < 0 || g_T < 1 || g_T > MAXT || g_q < 1) { fprintf(stderr, "usage: w2sync scheme=spin|rw|qsbr_clone|qsbr_borrow|hp|hp_asym mode=uniform|hot_same|hot_diff threads= ops= wpct= ppct= work= q= split= stall_ms=\n"); return 2; }
  g_hp_thresh = geti(ac, av, "hpt", 2 * g_T + 64);   // hazard-pointer scan threshold (retired objects per thread)
  snprintf(g_params, sizeof g_params, "threads=%d;mode=%s;wpct=%d;ppct=%d;work=%d;q=%d;split=%d;stall_ms=%d", g_T, m, g_wpct, g_ppct, g_work, g_q, split, g_stall_ms);
  if (g_hash) snprintf(g_params + strlen(g_params), sizeof g_params - strlen(g_params), ";hash=%d", g_hash);
  if (geti(ac, av, "hpt", 0)) snprintf(g_params + strlen(g_params), sizeof g_params - strlen(g_params), ";hpt=%d", g_hp_thresh);
  if (g_scheme == S_HP_ASYM && syscall(__NR_membarrier, MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED, 0, 0)) { perror("membarrier register"); return 1; }

  g_slot = calloc(g_nent, sizeof(Obj *));
  for (int i = 0; i < g_nent; i++) g_slot[i] = &entry_new(i, i * 5 + 1)->h;
  Th ths[MAXT]; memset(ths, 0, sizeof ths);
  for (int i = 0; i < g_T; i++) { ths[i].id = i; pthread_create(&ths[i].th, NULL, worker, &ths[i]); }
  while (__atomic_load_n(&g_ready, __ATOMIC_ACQUIRE) < g_T) { }
  __atomic_store_n(&g_go, 1, __ATOMIC_RELEASE);
  double win_mops = -1;
  if (g_dur_ms && g_warm_ms >= 0) {
    struct timespec w = { g_warm_ms / 1000, (long)(g_warm_ms % 1000) * 1000000L }, d = { g_dur_ms / 1000, (long)(g_dur_ms % 1000) * 1000000L };
    nanosleep(&w, NULL); long c0 = 0; for (int i = 0; i < g_T; i++) c0 += g_prog[i].n; uint64_t w0 = nowns();
    nanosleep(&d, NULL); long c1 = 0; for (int i = 0; i < g_T; i++) c1 += g_prog[i].n; uint64_t w1 = nowns();
    __atomic_store_n(&g_stop, 1, __ATOMIC_RELAXED);
    win_mops = (double)(c1 - c0) / ((w1 - w0) / 1e9) / 1e6;
  } else if (g_dur_ms) { struct timespec ts = { g_dur_ms / 1000, (long)(g_dur_ms % 1000) * 1000000L }; nanosleep(&ts, NULL); __atomic_store_n(&g_stop, 1, __ATOMIC_RELAXED); }
  for (int i = 0; i < g_T; i++) pthread_join(ths[i].th, NULL);
  uint64_t a = ths[0].t0, b = ths[0].t1; long gmax = 0; double total = 0;
  for (int i = 0; i < g_T; i++) { if (ths[i].t0 < a) a = ths[i].t0; if (ths[i].t1 > b) b = ths[i].t1; gmax += ths[i].pending_max; total += ths[i].done; }
  double wall = (double)(b - a);
  double rss = rss_mb();

  // all threads are gone: every retired object is past its grace period
  for (int i = 0; i < g_T; i++) {
    for (Batch *x = ths[i].left_head, *nx; x; x = nx) { nx = x->next; for (int j = 0; j < x->n; j++) rc_release(x->o[j]); free(x); }
    if (ths[i].left_cur) { for (int j = 0; j < ths[i].left_cur->n; j++) rc_release(ths[i].left_cur->o[j]); free(ths[i].left_cur); }
    for (int j = 0; j < ths[i].left_hn; j++) rc_release(ths[i].left_hp[j]);
    free(ths[i].left_hp);
  }
  for (int i = 0; i < g_nent; i++) rc_release(g_slot[i]);
  g_allocs += t_allocs; g_frees += t_frees;

  if (win_mops >= 0) { row("mops_total", win_mops); row("ns_per_op_per_thread", g_T * 1e3 / win_mops); }
  else { row("mops_total", total / (wall / 1e9) / 1e6); row("ns_per_op_per_thread", wall * g_T / total); }
  row("garbage_max_objs", (double)gmax);
  row("rss_mb", rss);
  row("leaked_objs", (double)((int64_t)g_allocs - (int64_t)g_frees));
  return g_allocs == g_frees ? 0 : 3;
}
