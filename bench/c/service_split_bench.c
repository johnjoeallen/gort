// B15 second run (D17): the split control block in a whole service. The mixed-request service of mixed_bench.c (parse, authenticate, shared cache under a
// lock, simulated database wait that parks, render, send) with one change: the cache holds Ref<User> objects allocated from arenas instead of 64-byte
// entries stored inline in the table. A hit reads the User's body inside the cache's sync block (the only place a reader exists); a miss builds a new
// User object, and replacing a cache entry frees the old one inside mutate. Two layouts, built from this one file:
//   -DSPLIT=0  combined: Ref -> slot { strong, weak, meta, body[64] }                       (spec 3.3 today)
//   -DSPLIT=1  split:    Ref -> control block { strong, weak, body*, meta } (24 B, never moves), body in a second arena slot { back pointer, body[64] }
// Moving (split only, mover=1): a mover thread evacuates sparse body chunks. It takes the cache lock (mutate) for each chunk, so no reader can be inside
// a window while a body moves; there is nothing else to wait for, and an emptied chunk is unmapped at once.
// Arenas are per worker (64 KB chunks, as in split_bench.c) with a spinlock each; frees of another worker's object take the owner's lock.
//
// Third dimension set (B12 on the D17 model): shards=N splits the cache lock into N cache-line-padded locks (a set belongs to shard set%N);
// clone=1 makes a hit clone the Ref (atomic strong++ inside sync), release the lock, and read the body afterwards (the Ref is dropped after render;
// dropping the last strong frees the object, so an evicted-while-held User dies in the request). With clone=1 a body is read outside any lock, so the mover
// may not move a body whose object has strong>1 (a holder exists) or that is not in the cache (an orphan held by a request): it skips that chunk. A move takes
// every shard lock (ascending) and then the arena lock; workers take one shard lock and then an arena lock, so there is no cycle.
// skew=P: P% of requests draw their key from the 1000 hottest users (so clone's atomic count updates contend on the same cache lines).
//
// usage: service_split_N hit% db_us concurrency workers requests_per_worker [users=1000000] [flush=0|1] [mover=0|1] [thresh=0.5] [shards=1] [clone=0|1] [skew=0]
//   flush=1: when worker 0 reaches 40% of its requests, the main thread evicts 90% of the cache at random; from that point every worker's keys come from
//            a hot set of 1/20 of the users (so the cache is not refilled and the heap stays sparse).
#define _GNU_SOURCE
#include "../../c/runtime/rt.h"
#include <sys/mman.h>
#ifndef SPLIT
#define SPLIT 0
#endif
typedef uint16_t u16;
#define WIN(b) __typeof__((b)->ptr) wp_##b = (b)->ptr
#define RDP(b, i) (&wp_##b[(i)])
#define RDB(b, i) (*RDP(b, i))
typedef struct { u8 *ptr; u32 len, cap; } Buf;
typedef struct { u64 *ptr; u32 len; } Keys;
#define NTOK 1024
#define MAXW 16
static inline u64 mix64(u64 x) { x += 0x9E3779B97F4A7C15ull; x = (x ^ (x >> 30)) * 0xBF58476D1CE4E5B9ull; x = (x ^ (x >> 27)) * 0x94D049BB133111EBull; return x ^ (x >> 31); }
static inline u64 rng(u64 *s) { *s ^= *s << 13; *s ^= *s >> 7; *s ^= *s << 17; return *s; }

// ---------------- arenas ----------------
#define CHUNK (64UL << 10)
#define PAGE 4096UL
typedef struct Arena Arena;
typedef struct Chunk {
    struct Chunk *prev, *next; Arena *owner;
    u32 idx, nslots, live, bump, nfree; u8 in_list;
    u16 *freeidx; u64 *bits; char *slots;
} Chunk;
struct Arena { u8 lock; u32 slot, nslots; size_t o_free, o_bits, o_slots; Chunk *head; Chunk **all; u32 nall, capall; u64 nmapped; };
#define ALOCK(a) while (__atomic_exchange_n(&(a)->lock, 1, __ATOMIC_ACQUIRE)) __builtin_ia32_pause()
#define AUNLOCK(a) __atomic_store_n(&(a)->lock, 0, __ATOMIC_RELEASE)
static size_t al(size_t v, size_t a) { return (v + a - 1) / a * a; }
static void arena_init(Arena *a, u32 slot) {
    memset(a, 0, sizeof *a); a->slot = slot;
    u32 n = (u32)((CHUNK - sizeof(Chunk) - 128) * 8 / ((slot + 2) * 8 + 1));
    for (;; n--) {
        a->o_free = sizeof(Chunk); a->o_bits = al(a->o_free + 2 * n, 8); a->o_slots = al(a->o_bits + 8 * ((n + 63) / 64), 64);
        if (a->o_slots + (size_t)n * slot <= CHUNK) break;
    }
    a->nslots = n;
}
static void list_push(Arena *a, Chunk *c) { c->prev = NULL; c->next = a->head; if (a->head) a->head->prev = c; a->head = c; c->in_list = 1; }
static void list_remove(Arena *a, Chunk *c) {
    if (c->prev) c->prev->next = c->next; else a->head = c->next;
    if (c->next) c->next->prev = c->prev;
    c->in_list = 0;
}
static Chunk *chunk_new(Arena *a) {
    char *m = mmap(NULL, 2 * CHUNK, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (m == MAP_FAILED) { perror("mmap"); exit(1); }
    uintptr_t b = ((uintptr_t)m + CHUNK - 1) & ~(CHUNK - 1), me = (uintptr_t)m + 2 * CHUNK;
    if (b > (uintptr_t)m) munmap(m, b - (uintptr_t)m);
    if (me > b + CHUNK) munmap((void *)(b + CHUNK), me - (b + CHUNK));
    madvise((void *)b, CHUNK, MADV_NOHUGEPAGE);
    Chunk *c = (Chunk *)b; c->owner = a;
    c->nslots = a->nslots; c->freeidx = (u16 *)(b + a->o_free); c->bits = (u64 *)(b + a->o_bits); c->slots = (char *)(b + a->o_slots);
    if (a->nall == a->capall) { a->capall = a->capall ? a->capall * 2 : 1024; a->all = realloc(a->all, a->capall * sizeof *a->all); }
    c->idx = a->nall; a->all[a->nall++] = c; a->nmapped++;
    list_push(a, c);
    return c;
}
static void chunk_unmap(Arena *a, Chunk *c) { a->all[c->idx] = NULL; a->nmapped--; munmap(c, CHUNK); }
static inline void *a_alloc_u(Arena *a) {                 // caller holds a->lock
    Chunk *c = a->head ? a->head : chunk_new(a);
    u32 i = c->nfree ? c->freeidx[--c->nfree] : c->bump++;
    c->live++; c->bits[i >> 6] |= 1ULL << (i & 63);
    if (c->live == c->nslots) list_remove(a, c);
    return c->slots + (size_t)i * a->slot;
}
static inline void a_free_u(Arena *a, void *p) {          // caller holds a->lock
    Chunk *c = (Chunk *)((uintptr_t)p & ~(CHUNK - 1));
    u32 i = (u32)(((char *)p - c->slots) / a->slot);
    c->bits[i >> 6] &= ~(1ULL << (i & 63)); c->freeidx[c->nfree++] = (u16)i;
    int was_full = c->live == c->nslots;
    c->live--;
    if (c->live == 0) { if (!was_full) list_remove(a, c); chunk_unmap(a, c); return; }
    if (was_full) list_push(a, c);
}
static inline int slot_used(Chunk *c, u32 i) { return (c->bits[i >> 6] >> (i & 63)) & 1; }
static inline Chunk *chunk_of(void *p) { return (Chunk *)((uintptr_t)p & ~(CHUNK - 1)); }

// ---------------- User objects ----------------
#define BODY 64
#if SPLIT
typedef struct Obj { u32 strong, weak; char *body; u64 meta; } Obj;      // meta = index of the worker whose body arena holds the body
static Arena A_cb[MAXW], A_body[MAXW];
static void objs_init(u32 w) { for (u32 i = 0; i < w; i++) { arena_init(&A_cb[i], sizeof(Obj)); arena_init(&A_body[i], 8 + BODY); } }
static Obj *obj_new(u32 w) {
    Arena *ac = &A_cb[w], *ab = &A_body[w];
    ALOCK(ac); Obj *o = a_alloc_u(ac); AUNLOCK(ac);
    ALOCK(ab); char *s = a_alloc_u(ab); *(Obj **)s = o; o->body = s + 8; AUNLOCK(ab);        // back pointer set before the arena lock drops: the mover reads it
    o->strong = 1; o->weak = 0; o->meta = w; return o;
}
static inline char *body_of(Obj *o) { return o->body; }
static void obj_free(Obj *o) {                                           // called inside mutate(cache)
    Arena *ab = &A_body[o->meta]; ALOCK(ab); a_free_u(ab, o->body - 8); AUNLOCK(ab);
    Arena *ac = chunk_of(o)->owner; ALOCK(ac); a_free_u(ac, o); AUNLOCK(ac);
}
static u64 mapped_bytes(u32 w) { u64 n = 0; for (u32 i = 0; i < w; i++) n += A_cb[i].nmapped + A_body[i].nmapped; return n * CHUNK; }
#else
typedef struct Obj { u32 strong, weak; u64 meta; char body[]; } Obj;
static Arena A_obj[MAXW];
static void objs_init(u32 w) { for (u32 i = 0; i < w; i++) arena_init(&A_obj[i], sizeof(Obj) + BODY); }
static Obj *obj_new(u32 w) { Arena *a = &A_obj[w]; ALOCK(a); Obj *o = a_alloc_u(a); AUNLOCK(a); o->strong = 1; o->weak = 0; o->meta = w; return o; }
static inline char *body_of(Obj *o) { return o->body; }
static void obj_free(Obj *o) { Arena *a = chunk_of(o)->owner; ALOCK(a); a_free_u(a, o); AUNLOCK(a); }
static u64 mapped_bytes(u32 w) { u64 n = 0; for (u32 i = 0; i < w; i++) n += A_obj[i].nmapped; return n * CHUNK; }
#endif
static inline void obj_release(Obj *o) { if (__atomic_sub_fetch(&o->strong, 1, __ATOMIC_ACQ_REL) == 0) obj_free(o); }

// ---------------- the cache: Refs to User objects, 4-way set-associative, random replacement ----------------
typedef struct { u64 key; Obj *ref; } CEnt;
typedef struct { CEnt *ptr; u32 sets; } Cache;
typedef struct { u8 lk; } __attribute__((aligned(64))) ShardLock;
static Cache g_cache;
static ShardLock g_lock[64];
static u32 g_nsh = 1, g_clone, g_skew;
static Keys g_keys;
static u32 g_db_us, g_conc, g_workers, g_hit, g_users, g_flush, g_mover;
static u64 g_reqs_per_worker, g_shift_at;
static double g_thresh = 0.5;
static __thread u64 tl_spins;                                   // failed lock attempts by this thread
#define SLOCK(i) while (__atomic_exchange_n(&g_lock[i].lk, 1, __ATOMIC_ACQUIRE)) { tl_spins++; __builtin_ia32_pause(); }
#define SUNLOCK(i) __atomic_store_n(&g_lock[i].lk, 0, __ATOMIC_RELEASE)
static inline u32 shard_of_set(u32 set) { return set % g_nsh; }

// ---- growable buffer: append may move the store, so it always goes through the header (all modes) ----
__attribute__((noinline)) static void buf_grow(Buf *b, u32 need) {
    u32 c = b->cap ? b->cap : 32; while (c < need) c *= 2;
    b->ptr = realloc(b->ptr, c); b->cap = c;
}
static inline void app(Buf *b, const void *s, u32 n) {
    if (b->len + n > b->cap) buf_grow(b, b->len + n);
    memcpy(b->ptr + b->len, s, n); b->len += n;
}
#define APPS(b, lit) app(b, lit, (u32)sizeof(lit) - 1)
static void appu(Buf *b, u64 v) { char t[24]; int n = 0; do { t[n++] = (char)('0' + v % 10); v /= 10; } while (v); char o[24]; for (int i = 0; i < n; i++) o[i] = t[n - 1 - i]; app(b, o, (u32)n); }

typedef struct Req {
    Task t;                                    // header: resume, next, w, ...
    int st;
    u64 key, t0, rid, seed;
    Buf in, out, hdr;
    u8 inbytes[320];
    u32 hname_off[16], hname_len[16], hval_off[16], hval_len[16]; int nh;
    u32 path_off, path_len, tok_off, tok_len;
    u8 val[56];
    Obj *ref;                                  // clone mode: the cloned Ref<User> held across render
} Req;



typedef struct { Worker w; u64 done, started, rng, rng2; u64 csum, hits, spins; double cpu_ns, busy_ns; u32 *lat; int idx; } WCtx;
static __thread WCtx *tl_ctx;

// ---- request text (stands in for the bytes a socket read delivered) ----
static void make_request(Req *r, u64 key, u64 rid, u32 tok) {
    Buf *b = &r->in; b->len = 0;
    APPS(b, "GET /users/"); appu(b, key);
    APPS(b, " HTTP/1.1\r\nHost: api.example.com\r\nUser-Agent: bench/1.0\r\nAccept: application/json\r\nAccept-Encoding: gzip, deflate\r\nAuthorization: Bearer tok");
    appu(b, tok);
    APPS(b, "\r\nConnection: keep-alive\r\nX-Request-Id: "); appu(b, rid);
    APPS(b, "\r\n\r\n");
}

static inline u8 lower(u8 c) { return c >= 'A' && c <= 'Z' ? c + 32 : c; }
static int name_is(Req *r, int h, const char *lit, u32 n) {       // case-insensitive header name compare, reading the request buffer
    Buf *b = &r->in; WIN(b);
    if (r->hname_len[h] != n) return 0;
    for (u32 i = 0; i < n; i++) if (lower(RDB(b, r->hname_off[h] + i)) != (u8)lit[i]) return 0;
    return 1;
}

// ---- parse: request line and headers, recorded as offsets into the buffer ----
static u64 parse_request(Req *r) {
    Buf *b = &r->in; WIN(b);
    u32 n = b->len, i = 0;
    while (i < n && RDB(b, i) != ' ') i++;                         // method
    u32 ps = ++i; while (i < n && RDB(b, i) != ' ') i++;
    r->path_off = ps; r->path_len = i - ps;
    while (i < n && RDB(b, i) != '\n') i++;                             // rest of the request line
    i++;
    r->nh = 0;
    while (i < n && RDB(b, i) != '\r' && r->nh < 16) {
        u32 s = i; while (i < n && RDB(b, i) != ':') i++;
        r->hname_off[r->nh] = s; r->hname_len[r->nh] = i - s; i++;
        while (i < n && RDB(b, i) == ' ') i++;
        u32 vs = i; while (i < n && RDB(b, i) != '\r') i++;
        r->hval_off[r->nh] = vs; r->hval_len[r->nh] = i - vs; r->nh++;
        i += 2;
    }
    u64 id = 0; u32 j = r->path_off + r->path_len;                  // id: digits after the last '/'
    u32 k = j; while (k > r->path_off && RDB(b, k - 1) != '/') k--;
    for (; k < j; k++) id = id * 10 + (RDB(b, k) - '0');
    return id;
}

static int authenticate(Req *r) {
    Buf *b = &r->in;
    for (int h = 0; h < r->nh; h++) if (name_is(r, h, "authorization", 13)) {
        WIN(b);
        u64 hash = 1469598103934665603ull;                          // FNV-1a over the token text after "Bearer "
        for (u32 i = r->hval_off[h] + 7; i < r->hval_off[h] + r->hval_len[h]; i++) { hash ^= RDB(b, i); hash *= 1099511628211ull; }
        Keys *k = &g_keys; WIN(k);                                  // write-once table: no lock needed
        u32 mask = k->len - 1, p = (u32)(hash >> 20) & mask;
        for (u32 n = 0; n < k->len; n++, p = (p + 1) & mask) { u64 v = RDB(k, p); if (v == hash) return 1; if (v == 0) return 0; }
    }
    return 0;
}



static inline u32 set_of(u64 key) { return (u32)(mix64(key) % g_cache.sets); }
static int cache_get(Req *r, u64 key) {                              // sync(shard)
    Cache *c = &g_cache; int hit = 0; u32 set = set_of(key), sh = shard_of_set(set);
    SLOCK(sh);
    { u32 base = set * 4;
      for (int w = 0; w < 4; w++) { CEnt *e = &c->ptr[base + w];
          if (e->key == key && e->ref) {
              if (g_clone) { __atomic_add_fetch(&e->ref->strong, 1, __ATOMIC_RELAXED); r->ref = e->ref; }       // clone the Ref; the body is read after the lock is released
              else memcpy(r->val, body_of(e->ref) + 8, 56);                                                     // read the body inside the block
              hit = 1; break; } } }
    SUNLOCK(sh);
    return hit;
}
static void cache_put(u32 wk, u64 key, const u8 *val, u64 *s) {     // mutate(shard): a new User replaces one entry; the evicted User is released here
    Obj *n = obj_new(wk); memcpy(body_of(n), &key, 8); memcpy(body_of(n) + 8, val, 56);
    Cache *c = &g_cache; u32 set = set_of(key), sh = shard_of_set(set);
    SLOCK(sh);
    u32 base = set * 4, w = (u32)(rng(s) & 3);
    for (int i = 0; i < 4; i++) if (c->ptr[base + i].key == 0 || c->ptr[base + i].key == key) { w = (u32)i; break; }
    CEnt *e = &c->ptr[base + w]; Obj *old = e->ref; e->key = key; e->ref = n;
    if (old) obj_release(old);
    SUNLOCK(sh);
}

static void db_decode(u64 key, u8 *val) {                            // the "row": 56 bytes derived from the key (decoding cost stands in for a row parse)
    u64 x = key;
    for (int i = 0; i < 56; i += 8) { x = mix64(x); memcpy(val + i, &x, 8); }
    for (int i = 0; i < 56; i++) val[i] = (u8)('a' + val[i] % 26);
}

static u64 render_and_send(Req *r, u64 key, const u8 *val) {
    Buf *o = &r->out, *h = &r->hdr; o->len = 0; h->len = 0;
    APPS(o, "{\"id\":"); appu(o, key);
    APPS(o, ",\"name\":\""); app(o, val, 24);
    APPS(o, "\",\"email\":\""); app(o, val + 24, 16); APPS(o, "@example.com");
    APPS(o, "\",\"tags\":[\"");
    for (int i = 0; i < 4; i++) { if (i) APPS(o, "\",\""); app(o, val + 40 + i * 4, 4); }
    APPS(o, "\"],\"score\":"); appu(o, mix64(key) % 100000); APPS(o, "}");
    APPS(h, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: "); appu(h, o->len); APPS(h, "\r\n\r\n");
    u64 sum = 0;                                                      // the "send": read every output byte once (a checksum stands in for the copy to the socket)
    { WIN(h); for (u32 i = 0; i < h->len; i++) sum = sum * 31 + RDB(h, i); }
    { WIN(o); for (u32 i = 0; i < o->len; i++) sum = sum * 31 + RDB(o, i); }
    return sum;
}



static int req_run(Task *t) {
    Req *r = (Req *)t; WCtx *cx = tl_ctx; Worker *w = &cx->w;
    switch (r->st) {
    case 0: {
        if (cx->started >= g_reqs_per_worker) { free(r->out.ptr); free(r->hdr.ptr); pool_free(&w->reqs, r); return DONE; }
        u64 pos = cx->started++; r->rid = (u64)cx->idx * g_reqs_per_worker + pos; r->t0 = mono_ns();
        u64 h = mix64(r->rid); r->key = 1 + h % (pos >= g_shift_at ? g_users / 20 : g_users);
        if (g_skew && (h >> 50) % 100 < g_skew) r->key = 1 + (h >> 8) % 1000;   // key and token depend only on the request id: every build serves the same requests
        make_request(r, r->key, r->rid, (u32)((h >> 40) % NTOK));
        u64 id = parse_request(r);
        if (id != r->key || !authenticate(r)) { fprintf(stderr, "request rejected\n"); exit(1); }
        if (cache_get(r, r->key)) { cx->hits++; break; }
        u64 lat = (u64)g_db_us * 1000 * (80 + rng(&cx->rng2) % 41) / 100;
        timer_add(w, t, mono_ns() + lat); r->st = 1; return park(t, W_TIMER);
    }
    case 1:
        db_decode(r->key, r->val); cache_put((u32)cx->idx, r->key, r->val, &cx->rng);
        break;
    }
    if (r->ref) { cx->csum += render_and_send(r, r->key, (const u8 *)body_of(r->ref) + 8); obj_release(r->ref); r->ref = NULL; }   // clone mode: read the body outside the lock, then drop the Ref
    else cx->csum += render_and_send(r, r->key, r->val);
    cx->lat[cx->done++] = (u32)(mono_ns() - r->t0);
    r->st = 0; return yield_now(t);
}

static void *worker_main(void *arg) {
    WCtx *cx = arg; Worker *w = &cx->w; tl_ctx = cx;
    cpu_set_t cs; CPU_ZERO(&cs); CPU_SET(2 + cx->idx, &cs); pthread_setaffinity_np(pthread_self(), sizeof cs, &cs);
    memset(w, 0, sizeof *w); w->id = cx->idx; pool_init(&w->reqs, sizeof(Req));
    cx->rng = 88172645463325252ull + (u64)cx->idx * 7919; cx->rng2 = 1234567ull + (u64)cx->idx * 104729; cx->lat = malloc((g_reqs_per_worker + 1) * sizeof(u32));
    struct timespec c0; clock_gettime(CLOCK_THREAD_CPUTIME_ID, &c0);
    for (u32 i = 0; i < g_conc; i++) {
        Req *r = pool_alloc(&w->reqs); memset(r, 0, sizeof *r);
        r->t.resume = req_run; r->t.w = w; r->in.ptr = r->inbytes; r->in.cap = sizeof r->inbytes;
        r->out.ptr = malloc(64); r->out.cap = 64; r->hdr.ptr = malloc(48); r->hdr.cap = 48;
        enqueue(w, &r->t);
    }
    while (cx->done < g_reqs_per_worker) {
        u64 b0 = mono_ns();
        if (w->tn) { Task *tt; while ((tt = timer_pop_due(w, b0))) wake(w, tt); }
        Task *t = w->qh; w->qh = w->qt = NULL;
        if (!t) {
            if (w->tn) { u64 now = mono_ns(), d = w->th[0].when > now ? w->th[0].when - now : 0; if (d > 30000) { struct timespec ts = { 0, (long)(d - 20000) }; nanosleep(&ts, NULL); } }
            continue;
        }
        while (t) { Task *nx = t->next; t->queued = 0; t->parked = 0; t->resume(t); t = nx; }
        cx->busy_ns += (double)(mono_ns() - b0);
    }
    struct timespec c1; clock_gettime(CLOCK_THREAD_CPUTIME_ID, &c1);
    cx->cpu_ns = (c1.tv_sec - c0.tv_sec) * 1e9 + (c1.tv_nsec - c0.tv_nsec); cx->spins = tl_spins;
    return NULL;
}

// ---------------- the mover (split only): evacuates sparse body chunks inside mutate(cache) ----------------
static volatile int g_stop;
static u64 g_moved, g_cfreed, g_pinned_skips; static double g_mover_cpu_ms, g_maxhold_us; static u64 g_passes;
#if SPLIT
static int chunk_movable(Chunk *c, Arena *a) {                  // caller holds every shard lock and a->lock: no new clone can appear, none can be dropped to zero and freed
    for (u32 i = 0; i < c->bump; i++) {
        if (!slot_used(c, i)) continue;
        char *p = c->slots + (size_t)i * a->slot; Obj *o = *(Obj **)p; u64 key; memcpy(&key, p + 8, 8);
        if (__atomic_load_n(&o->strong, __ATOMIC_ACQUIRE) != 1) return 0;                          // a request holds a clone of it
        u32 set = set_of(key); CEnt *e = &g_cache.ptr[set * 4]; int in = 0;
        for (int w = 0; w < 4; w++) if (e[w].ref == o) in = 1;
        if (!in) return 0;                                                                        // held by a request alone (evicted, or not yet inserted)
    }
    return 1;
}
static void evacuate_chunk(Arena *a, Chunk *c) {                // caller holds every shard lock and a->lock
    if (!chunk_movable(c, a)) { g_pinned_skips++; return; }
    if (c->in_list) list_remove(a, c);
    for (u32 i = 0; i < c->bump; i++) {
        if (!slot_used(c, i)) continue;
        char *p = c->slots + (size_t)i * a->slot, *q = a_alloc_u(a);
        memcpy(q, p, a->slot); (*(Obj **)q)->body = q + 8; g_moved++;
    }
    chunk_unmap(a, c); g_cfreed++;
}
static void *mover_main(void *arg) {
    (void)arg; cpu_set_t cs; CPU_ZERO(&cs); CPU_SET(6, &cs); pthread_setaffinity_np(pthread_self(), sizeof cs, &cs);
    u32 cursor[MAXW] = { 0 };
    while (!g_stop) {
        usleep(1000);
        for (u32 w = 0; w < g_workers && !g_stop; w++) {
            Arena *a = &A_body[w];
            for (u32 k = 0; k < g_nsh; k++) SLOCK(k);
            ALOCK(a);
            u64 t0 = mono_ns();                                            // the hold starts once both locks are ours
            Chunk *c = NULL; u32 n = a->nall;
            for (u32 k = 0, i = cursor[w]; k < 256 && k < n; k++, i = (i + 1) % n) {
                Chunk *x = a->all[i];
                if (x && x->live > 0 && (double)x->live / x->nslots < g_thresh) { c = x; cursor[w] = (i + 1) % n; break; }
                cursor[w] = (i + 1) % n;
            }
            if (c) evacuate_chunk(a, c);
            AUNLOCK(a); for (u32 k = g_nsh; k-- > 0;) SUNLOCK(k);
            double hold = (double)(mono_ns() - t0) / 1000.0; if (c && hold > g_maxhold_us) g_maxhold_us = hold;
            g_passes++;
        }
    }
    struct timespec c1; clock_gettime(CLOCK_THREAD_CPUTIME_ID, &c1); g_mover_cpu_ms = c1.tv_sec * 1e3 + c1.tv_nsec / 1e6;
    return NULL;
}
#endif
static void do_flush(void) {                                      // evict 90% of the cache at random, one set at a time under its shard lock
    Cache *c = &g_cache; u64 s = 99991;
    for (u32 k = 0; k < c->sets; k++) {
        u32 sh = shard_of_set(k); SLOCK(sh);
        for (int w = 0; w < 4; w++) {
            CEnt *e = &c->ptr[k * 4 + w];
            if (e->ref && rng(&s) % 100 < 90) { Obj *o = e->ref; e->ref = NULL; e->key = 0; obj_release(o); }
        }
        SUNLOCK(sh);
    }
}
static int cmpu(const void *a, const void *b) { u32 x = *(u32 *)a, y = *(u32 *)b; return x < y ? -1 : x > y; }
static const char *opt(int argc, char **argv, const char *k, const char *d) {
    size_t n = strlen(k);
    for (int i = 6; i < argc; i++) if (!strncmp(argv[i], k, n) && argv[i][n] == '=') return argv[i] + n + 1;
    return d;
}

int main(int argc, char **argv) {
    if (argc < 6) { fprintf(stderr, "usage: %s hit%% db_us concurrency workers requests_per_worker [users= flush= mover= thresh= shards= clone= skew=]\n", argv[0]); return 2; }
    g_hit = (u32)atoi(argv[1]); g_db_us = (u32)atoi(argv[2]); g_conc = (u32)atoi(argv[3]); g_workers = (u32)atoi(argv[4]); g_reqs_per_worker = strtoull(argv[5], 0, 10);
    g_users = (u32)atol(opt(argc, argv, "users", "1000000")); g_flush = (u32)atoi(opt(argc, argv, "flush", "0")); g_mover = (u32)atoi(opt(argc, argv, "mover", "0"));
    g_thresh = atof(opt(argc, argv, "thresh", "0.5"));
    g_nsh = (u32)atoi(opt(argc, argv, "shards", "1")); g_clone = (u32)atoi(opt(argc, argv, "clone", "0")); g_skew = (u32)atoi(opt(argc, argv, "skew", "0"));
    if (g_nsh < 1 || g_nsh > 64) { fprintf(stderr, "shards 1..64\n"); return 2; }
    g_shift_at = g_flush ? g_reqs_per_worker * 4 / 10 : ~0ULL;
    objs_init(g_workers);
    u64 cap = (u64)g_users * g_hit / 100; g_cache.sets = (u32)(cap / 4 ? cap / 4 : 1);
    g_cache.ptr = calloc((size_t)g_cache.sets * 4, sizeof(CEnt));
    u64 pre = 1; for (u64 k = 1; k <= g_users; k++) { u8 v[56]; db_decode(k, v); cache_put((u32)(k % g_workers), k, v, &pre); }   // warm to the steady state
    g_keys.len = 4096; g_keys.ptr = calloc(4096, sizeof(u64));
    for (u32 t = 0; t < NTOK; t++) {
        char s[16]; int n = snprintf(s, sizeof s, "tok%u", t); u64 h = 1469598103934665603ull; for (int i = 0; i < n; i++) { h ^= (u8)s[i]; h *= 1099511628211ull; }
        u32 p = (u32)(h >> 20) & 4095; while (g_keys.ptr[p]) p = (p + 1) & 4095; g_keys.ptr[p] = h;
    }
    cpu_set_t cs; CPU_ZERO(&cs); CPU_SET(7, &cs); pthread_setaffinity_np(pthread_self(), sizeof cs, &cs);
    WCtx *cx = calloc(g_workers, sizeof *cx); pthread_t th[64];
#if SPLIT
    pthread_t mv;
#endif

    long rss_start = rss_kb(); double mapped_start = mapped_bytes(g_workers) / 1048576.0;
    double t0 = now_s();
    for (u32 i = 0; i < g_workers; i++) { cx[i].idx = (int)i; pthread_create(&th[i], NULL, worker_main, &cx[i]); }
#if SPLIT
    if (g_mover) pthread_create(&mv, NULL, mover_main, NULL);
#endif
    long rss_pre = rss_start, rss_post = rss_start;
    if (g_flush) {
        while (((volatile WCtx *)&cx[0])->started < g_shift_at) usleep(200);
        rss_pre = rss_kb(); do_flush(); rss_post = rss_kb();
    }
    for (u32 i = 0; i < g_workers; i++) pthread_join(th[i], NULL);
    double wall = now_s() - t0;
    g_stop = 1;
#if SPLIT
    if (g_mover) pthread_join(mv, NULL);
#endif
    long rss_end = rss_kb(); double mapped_end = mapped_bytes(g_workers) / 1048576.0;
    u64 total = 0, csum = 0, hits = 0, spins = 0; double busy = 0; u32 *all = malloc(g_workers * g_reqs_per_worker * sizeof(u32)); size_t m = 0;
    for (u32 i = 0; i < g_workers; i++) { total += cx[i].done; csum += cx[i].csum; hits += cx[i].hits; spins += cx[i].spins; busy += cx[i].busy_ns; memcpy(all + m, cx[i].lat, cx[i].done * sizeof(u32)); m += cx[i].done; }
    qsort(all, m, sizeof(u32), cmpu);
    printf("RUN layout=%s hit=%u db_us=%u conc=%u workers=%u reqs=%llu flush=%u mover=%u shards=%u clone=%u skew=%u spins_per_req=%.3f rps=%.0f busy_ns_per_req=%.1f p50_us=%.1f p99_us=%.1f p999_us=%.1f hit_actual=%.1f csum=%llu"
           " rss_start_mb=%.0f rss_preflush_mb=%.0f rss_postflush_mb=%.0f rss_end_mb=%.0f mapped_start_mb=%.0f mapped_end_mb=%.0f moved=%llu chunks_freed=%llu mover_cpu_ms=%.0f max_hold_us=%.0f pinned_skips=%llu\n",
           SPLIT ? "split" : "combined", g_hit, g_db_us, g_conc, g_workers, (unsigned long long)total, g_flush, g_mover, g_nsh, g_clone, g_skew, (double)spins / (double)total, total / wall, busy / (double)total,
           all[m / 2] / 1000.0, all[m * 99 / 100] / 1000.0, all[m * 999 / 1000] / 1000.0, 100.0 * hits / total, (unsigned long long)csum,
           rss_start / 1024.0, rss_pre / 1024.0, rss_post / 1024.0, rss_end / 1024.0, mapped_start, mapped_end,
           (unsigned long long)g_moved, (unsigned long long)g_cfreed, g_mover_cpu_ms, g_maxhold_us, (unsigned long long)g_pinned_skips);
    return 0;
}
