// B12 (mixed request) for D15: a request that parses, authenticates, looks up a shared cache under a lock, parks on a simulated database wait
// on a miss, renders JSON into growable buffers and "sends" it, run as vthread-like tasks on the Strata C runtime (c/runtime/rt.h: workers, run queue,
// timer heap, park/wake). Closed loop: C requests are always in flight per worker. busy_ns_per_req is the time a worker spent running tasks and
// timers per request (waiting for a database reply is not counted). See bench/results/mixed.md.
//
// Three builds differ ONLY in how a loop reads through a container (-DMODE=n):
//   0 raw      the base pointer is copied into a local once per window (after the lock) and used for every read: the C baseline
//   1 handle   every read is h->ptr[i]; the C compiler decides when to reload (this is what a Strata-to-C emitter would write)
//   2 barrier  every read is h->ptr[i] after a compiler barrier, so the pointer is reloaded at every access: the pessimistic bound
// Growing code (the output buffers' append) is identical in all three: a growable buffer must be reached through its header there.
//
// usage: mixed_N hit% db_us concurrency workers requests_per_worker
#define _GNU_SOURCE
#include "../../c/runtime/rt.h"
#ifndef MODE
#define MODE 0
#endif
#define BARRIER() __asm__ volatile("" ::: "memory")
#if MODE == 0
#define WIN(b) __typeof__((b)->ptr) wp_##b = (b)->ptr
#define RDP(b, i) (&wp_##b[(i)])
#elif MODE == 1
#define WIN(b)
#define RDP(b, i) (&(b)->ptr[(i)])
#else
#define WIN(b)
#define RDP(b, i) ({ BARRIER(); &(b)->ptr[(i)]; })
#endif
#define RDB(b, i) (*RDP(b, i))

typedef struct { u8 *ptr; u32 len, cap; } Buf;                 // growable byte array: non-moving header, movable store
typedef struct { u64 key; u8 val[56]; } Entry;                 // 64 bytes
typedef struct { Entry *ptr; u32 sets; u8 lk; } Cache;         // 4-way set-associative, random replacement
typedef struct { u64 *ptr; u32 len; } Keys;                    // token hashes; write-once, read without a lock
#define KEYSPACE 65536
#define NTOK 1024

static Cache g_cache;
static Keys g_keys;
static u32 g_db_us, g_conc, g_workers, g_hit;
static u64 g_reqs_per_worker;

static inline u64 mix64(u64 x) { x += 0x9E3779B97F4A7C15ull; x = (x ^ (x >> 30)) * 0xBF58476D1CE4E5B9ull; x = (x ^ (x >> 27)) * 0x94D049BB133111EBull; return x ^ (x >> 31); }
static inline u64 rng(u64 *s) { *s ^= *s << 13; *s ^= *s >> 7; *s ^= *s << 17; return *s; }

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
} Req;

typedef struct { Worker w; u64 done, started, rng, rng2; u64 csum, hits; double cpu_ns, busy_ns; u32 *lat; int idx; } WCtx;
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
static int cache_get(Req *r, u64 key) {                              // sync(cache): read-only window over the table
    Cache *c = &g_cache; int hit = 0;
    while (__atomic_exchange_n(&c->lk, 1, __ATOMIC_ACQUIRE)) __builtin_ia32_pause();
    { WIN(c); u32 base = set_of(key) * 4;
      for (int w = 0; w < 4; w++) { if (RDB(c, base + w).key == key) { memcpy(r->val, RDP(c, base + w)->val, 56); hit = 1; break; } } }
    __atomic_store_n(&c->lk, 0, __ATOMIC_RELEASE);
    return hit;
}
static void cache_put(Req *r, u64 key, u64 *s) {                    // mutate(cache)
    Cache *c = &g_cache;
    while (__atomic_exchange_n(&c->lk, 1, __ATOMIC_ACQUIRE)) __builtin_ia32_pause();
    { WIN(c); u32 base = set_of(key) * 4; u32 w = (u32)(rng(s) & 3);
      for (int i = 0; i < 4; i++) if (RDB(c, base + i).key == 0 || RDB(c, base + i).key == key) { w = i; break; }
      Entry *e = (Entry *)RDP(c, base + w); e->key = key; memcpy(e->val, r->val, 56); }
    __atomic_store_n(&c->lk, 0, __ATOMIC_RELEASE);
}
static void db_decode(u64 key, u8 *val) {                            // the "row": 56 bytes derived from the key (decoding cost stands in for a row parse)
    u64 x = key;
    for (int i = 0; i < 56; i += 8) { x = mix64(x); memcpy(val + i, &x, 8); }
    for (int i = 0; i < 56; i++) val[i] = (u8)('a' + val[i] % 26);
}

static u64 render_and_send(Req *r, u64 key) {
    Buf *o = &r->out, *h = &r->hdr; o->len = 0; h->len = 0;
    APPS(o, "{\"id\":"); appu(o, key);
    APPS(o, ",\"name\":\""); app(o, r->val, 24);
    APPS(o, "\",\"email\":\""); app(o, r->val + 24, 16); APPS(o, "@example.com");
    APPS(o, "\",\"tags\":[\"");
    for (int i = 0; i < 4; i++) { if (i) APPS(o, "\",\""); app(o, r->val + 40 + i * 4, 4); }
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
        r->rid = (u64)cx->idx * g_reqs_per_worker + cx->started++; r->t0 = mono_ns();
        u64 h = mix64(r->rid); r->key = h % KEYSPACE;                       // key and token depend only on the request id, so every build serves the same requests
        make_request(r, r->key, r->rid, (u32)((h >> 40) % NTOK));
        u64 id = parse_request(r);
        if (id != r->key || !authenticate(r)) { fprintf(stderr, "request rejected\n"); exit(1); }
        if (cache_get(r, r->key)) { cx->hits++; break; }
        u64 lat = (u64)g_db_us * 1000 * (80 + rng(&cx->rng2) % 41) / 100;    // the database wait: parks, the worker runs other requests
        timer_add(w, t, mono_ns() + lat); r->st = 1; return park(t, W_TIMER);
    }
    case 1:
        db_decode(r->key, r->val); cache_put(r, r->key, &cx->rng);
        break;
    }
    cx->csum += render_and_send(r, r->key);
    cx->lat[cx->done++] = (u32)(mono_ns() - r->t0);
    r->st = 0; return yield_now(t);                                       // the next request on this connection slot (or the slot retires at state 0)
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
        if (!t) {                                                      // nothing runnable: wait for the next database reply (not counted as busy time)
            if (w->tn) { u64 now = mono_ns(), d = w->th[0].when > now ? w->th[0].when - now : 0; if (d > 30000) { struct timespec ts = { 0, (long)(d - 20000) }; nanosleep(&ts, NULL); } }
            continue;
        }
        while (t) { Task *nx = t->next; t->queued = 0; t->parked = 0; t->resume(t); t = nx; }
        cx->busy_ns += (double)(mono_ns() - b0);
    }
    struct timespec c1; clock_gettime(CLOCK_THREAD_CPUTIME_ID, &c1);
    cx->cpu_ns = (c1.tv_sec - c0.tv_sec) * 1e9 + (c1.tv_nsec - c0.tv_nsec);
    return NULL;
}
static int cmpu(const void *a, const void *b) { u32 x = *(u32 *)a, y = *(u32 *)b; return x < y ? -1 : x > y; }

int main(int argc, char **argv) {
    if (argc < 7) { fprintf(stderr, "usage: %s hit%% db_us concurrency workers requests_per_worker\n", argv[0]); return 2; }
    g_hit = (u32)atoi(argv[1]); g_db_us = (u32)atoi(argv[2]); g_conc = (u32)atoi(argv[3]); g_workers = (u32)atoi(argv[4]); g_reqs_per_worker = strtoull(argv[5], 0, 10);
    (void)argv[6];
    u64 cap = (u64)KEYSPACE * g_hit / 100; g_cache.sets = (u32)(cap / 4 ? cap / 4 : 1);
    g_cache.ptr = calloc((size_t)g_cache.sets * 4, sizeof(Entry));
    u64 pre = 1; for (u64 k = 0; k < KEYSPACE; k++) {              // warm the cache to its steady state: insert every key once (random replacement settles)
        Req r; db_decode(k, r.val); cache_put(&r, k, &pre);
    }
    g_keys.len = 4096; g_keys.ptr = calloc(4096, sizeof(u64));
    for (u32 t = 0; t < NTOK; t++) {                               // tokens "tok<N>"
        char s[16]; int n = snprintf(s, sizeof s, "tok%u", t); u64 h = 1469598103934665603ull; for (int i = 0; i < n; i++) { h ^= (u8)s[i]; h *= 1099511628211ull; }
        u32 p = (u32)(h >> 20) & 4095; while (g_keys.ptr[p]) p = (p + 1) & 4095; g_keys.ptr[p] = h;
    }
    WCtx *cx = calloc(g_workers, sizeof *cx); pthread_t th[64];
    double t0 = now_s();
    for (u32 i = 0; i < g_workers; i++) { cx[i].idx = (int)i; pthread_create(&th[i], NULL, worker_main, &cx[i]); }
    for (u32 i = 0; i < g_workers; i++) pthread_join(th[i], NULL);
    double wall = now_s() - t0;
    u64 total = 0, csum = 0, hits = 0; double cpu = 0, busy = 0; u32 *all = malloc(g_workers * g_reqs_per_worker * sizeof(u32)); size_t m = 0;
    for (u32 i = 0; i < g_workers; i++) { total += cx[i].done; csum += cx[i].csum; hits += cx[i].hits; cpu += cx[i].cpu_ns; busy += cx[i].busy_ns; memcpy(all + m, cx[i].lat, cx[i].done * sizeof(u32)); m += cx[i].done; }
    qsort(all, m, sizeof(u32), cmpu);
    printf("RUN mode=%d hit=%u db_us=%u conc=%u workers=%u reqs=%llu rps=%.0f busy_ns_per_req=%.1f p50_us=%.1f p99_us=%.1f hit_actual=%.1f csum=%llu\n",
           MODE, g_hit, g_db_us, g_conc, g_workers, (unsigned long long)total, total / wall, busy / (double)total, all[m / 2] / 1000.0, all[m * 99 / 100] / 1000.0, 100.0 * hits / total, (unsigned long long)csum);
    return 0;
}
