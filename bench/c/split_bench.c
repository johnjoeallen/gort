// D17 benchmark: combined slot (counts + body in one arena slot) against a split control block (counts + body pointer in a
// never-moving control-block arena, body in a movable type arena). Hand-written C, no Strata compiler.
//
//   gcc -O2 -DSPLIT=0 -o split_0 split_bench.c      combined: Ref = pointer to a slot { strong, weak, meta, body[] }
//   gcc -O2 -DSPLIT=1 -o split_1 split_bench.c      split:    Ref = pointer to a control block { strong, weak, body*, meta }; the body is
//                                                             in a second arena, in a slot { back pointer, body[] }
//
// usage: split_N work=alloc|deref|retain|weak n=<objects> body=<bytes> [keep=<basis points>] [pattern=scatter|cluster] [thresh=<0..1>]
// Both modes use the same arena code: 64 KB chunks aligned to 64 KB, the chunk found from a pointer by masking, a per-chunk free-index
// stack and used-bitmap kept in the chunk header (not in the free slots, so a free page can be released), an empty chunk is unmapped.
// Output: one RESULT line (key=value) per measurement.
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <time.h>
#include <sys/mman.h>

#ifndef SPLIT
#define SPLIT 0
#endif
typedef uint64_t u64; typedef uint32_t u32; typedef uint16_t u16; typedef uint8_t u8;
#define CHUNK (64UL << 10)
#define PAGE 4096UL

static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec + t.tv_nsec * 1e-9; }
static long rss_kb(void) {
    FILE *f = fopen("/proc/self/status", "r"); char l[256]; long v = -1;
    while (f && fgets(l, sizeof l, f)) if (!strncmp(l, "VmRSS:", 6)) { v = atol(l + 6); break; }
    if (f) fclose(f);
    return v;
}
static u64 rs = 88172645463325252ULL;
static inline u64 rnd(void) { u64 x = rs; x ^= x << 13; x ^= x >> 7; x ^= x << 17; return rs = x; }

typedef struct Chunk {
    struct Chunk *prev, *next;
    u32 idx, nslots, live, bump, nfree; u8 in_list;
    u16 *freeidx; u64 *bits; char *slots;
} Chunk;
typedef struct { u32 slot, nslots; size_t o_free, o_bits, o_slots; Chunk *head; Chunk **all; u32 nall, capall; u64 nmapped; } Arena;

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
    Chunk *c = (Chunk *)b;
    c->nslots = a->nslots; c->freeidx = (u16 *)(b + a->o_free); c->bits = (u64 *)(b + a->o_bits); c->slots = (char *)(b + a->o_slots);
    if (a->nall == a->capall) { a->capall = a->capall ? a->capall * 2 : 1024; a->all = realloc(a->all, a->capall * sizeof *a->all); }
    c->idx = a->nall; a->all[a->nall++] = c; a->nmapped++;
    list_push(a, c);
    return c;
}
static void chunk_unmap(Arena *a, Chunk *c) { a->all[c->idx] = NULL; a->nmapped--; munmap(c, CHUNK); }
static inline void *a_alloc(Arena *a) {
    Chunk *c = a->head ? a->head : chunk_new(a);
    u32 i = c->nfree ? c->freeidx[--c->nfree] : c->bump++;
    c->live++; c->bits[i >> 6] |= 1ULL << (i & 63);
    if (c->live == c->nslots) list_remove(a, c);
    return c->slots + (size_t)i * a->slot;
}
static inline void a_free(Arena *a, void *p) {
    Chunk *c = (Chunk *)((uintptr_t)p & ~(CHUNK - 1));
    u32 i = (u32)(((char *)p - c->slots) / a->slot);
    c->bits[i >> 6] &= ~(1ULL << (i & 63)); c->freeidx[c->nfree++] = (u16)i;
    int was_full = c->live == c->nslots;
    c->live--;
    if (c->live == 0) { if (!was_full) list_remove(a, c); chunk_unmap(a, c); return; }
    if (was_full) list_push(a, c);
}
static inline int slot_used(Chunk *c, u32 i) { return (c->bits[i >> 6] >> (i & 63)) & 1; }
// Option 1 of D17: release every page whose slots are all free. Returns pages released.
static u64 release_pages(Arena *a) {
    u64 n = 0;
    for (u32 k = 0; k < a->nall; k++) {
        Chunk *c = a->all[k]; if (!c) continue;
        for (size_t off = al(a->o_slots, PAGE); off + PAGE <= CHUNK; off += PAGE) {
            long first = (long)((off - a->o_slots) / a->slot), last = (long)((off + PAGE - 1 - a->o_slots) / a->slot);
            if (first >= (long)c->bump) continue;                    // never touched
            if (last >= (long)c->nslots) last = c->nslots - 1;
            int any = 0; for (long s = first; s <= last && !any; s++) any = slot_used(c, (u32)s);
            if (!any) { madvise((char *)c + off, PAGE, MADV_DONTNEED); n++; }
        }
    }
    return n;
}

static u32 BODY;
#if SPLIT
typedef struct Obj { u32 strong, weak; char *body; u64 meta; } Obj;      // 24 B control block
static Arena A_cb, A_body;
static void objs_init(u32 body) { BODY = body; arena_init(&A_cb, sizeof(Obj)); arena_init(&A_body, 8 + body); }
static inline Obj *obj_new(void) {
    Obj *o = a_alloc(&A_cb); char *s = a_alloc(&A_body);
    *(Obj **)s = o; o->body = s + 8; o->strong = 1; o->weak = 0; o->meta = 0; return o;
}
static inline char *body_of(Obj *o) { return o->body; }
static inline void drop_strong(Obj *o) {
    if (--o->strong == 0) { a_free(&A_body, o->body - 8); o->body = NULL; if (o->weak == 0) a_free(&A_cb, o); }
}
static inline void drop_weak(Obj *o) { if (--o->weak == 0 && o->strong == 0) a_free(&A_cb, o); }
static u64 mapped_bytes(void) { return (A_cb.nmapped + A_body.nmapped) * CHUNK; }
static u64 release_all(void) { return release_pages(&A_cb) + release_pages(&A_body); }
static u64 evacuate(double thresh, u64 *chunks_freed) {          // move bodies out of sparse chunks; control blocks do not move
    u64 moved = 0; *chunks_freed = 0;
    for (u32 k = 0; k < A_body.nall; k++) {
        Chunk *c = A_body.all[k];
        if (!c || (double)c->live / c->nslots >= thresh) continue;
        if (c->in_list) list_remove(&A_body, c);
        for (u32 i = 0; i < c->bump; i++) {
            if (!slot_used(c, i)) continue;
            char *p = c->slots + (size_t)i * A_body.slot, *q = a_alloc(&A_body);
            memcpy(q, p, A_body.slot); (*(Obj **)q)->body = q + 8; moved++;
        }
        chunk_unmap(&A_body, c); (*chunks_freed)++;
    }
    return moved;
}
#else
typedef struct Obj { u32 strong, weak; u64 meta; char body[]; } Obj;     // 16 B header, body inline
static Arena A_obj;
static void objs_init(u32 body) { BODY = body; arena_init(&A_obj, sizeof(Obj) + body); }
static inline Obj *obj_new(void) { Obj *o = a_alloc(&A_obj); o->strong = 1; o->weak = 0; o->meta = 0; return o; }
static inline char *body_of(Obj *o) { return o->body; }
static inline void drop_strong(Obj *o) { if (--o->strong == 0 && o->weak == 0) a_free(&A_obj, o); }
static inline void drop_weak(Obj *o) { if (--o->weak == 0 && o->strong == 0) a_free(&A_obj, o); }
static u64 mapped_bytes(void) { return A_obj.nmapped * CHUNK; }
static u64 release_all(void) { return release_pages(&A_obj); }
static u64 evacuate(double thresh, u64 *chunks_freed) { (void)thresh; *chunks_freed = 0; return 0; }   // refs are direct: nothing can move
#endif

static const char *arg(int argc, char **argv, const char *k, const char *d) {
    size_t n = strlen(k);
    for (int i = 1; i < argc; i++) if (!strncmp(argv[i], k, n) && argv[i][n] == '=') return argv[i] + n + 1;
    return d;
}
static void shuffle(u32 *p, u32 n) { for (u32 i = n - 1; i > 0; i--) { u32 j = rnd() % (i + 1), t = p[i]; p[i] = p[j]; p[j] = t; } }
static volatile u64 sink;
#define MODE_NAME (SPLIT ? "split" : "combined")

int main(int argc, char **argv) {
    const char *work = arg(argc, argv, "work", "alloc");
    u32 n = (u32)atol(arg(argc, argv, "n", "1500000")), body = (u32)atol(arg(argc, argv, "body", "64"));
    u32 keep = (u32)atol(arg(argc, argv, "keep", "100"));
    int cluster = !strcmp(arg(argc, argv, "pattern", "scatter"), "cluster");
    double thresh = atof(arg(argc, argv, "thresh", "0.5"));
    objs_init(body);
    Obj **refs = malloc((size_t)n * sizeof *refs); u32 *perm = malloc((size_t)n * sizeof *perm); u8 *surv = malloc(n);
    for (u32 i = 0; i < n; i++) { perm[i] = i; refs[i] = NULL; surv[i] = 0; }
    shuffle(perm, n);
    long base = rss_kb();
    printf("RESULT work=%s mode=%s n=%u body=%u", work, MODE_NAME, n, body);

    if (!strcmp(work, "alloc")) {
        double t = now();
        for (u32 i = 0; i < n; i++) { refs[i] = obj_new(); *(u64 *)body_of(refs[i]) = i; }
        double fill = (now() - t) / n * 1e9;
        long rss_full = rss_kb() - base;
        t = now();
        for (u32 k = 0; k < n; k++) drop_strong(refs[perm[k]]);
        double fr = (now() - t) / n * 1e9;
        long rss_freed = rss_kb() - base;
        for (u32 i = 0; i < n; i++) refs[i] = obj_new();
        u32 ops = n < 2000000 ? n : 2000000;
        t = now();
        for (u32 k = 0; k < ops; k++) { u32 j = rnd() % n; drop_strong(refs[j]); refs[j] = obj_new(); *(u64 *)body_of(refs[j]) = k; }
        double churn = (now() - t) / ops * 1e9;
        printf(" fill_ns=%.2f free_random_ns=%.2f churn_pair_ns=%.2f rss_full_kb=%ld rss_after_free_kb=%ld\n", fill, fr, churn, rss_full, rss_freed);
    } else if (!strcmp(work, "deref")) {
        for (u32 i = 0; i < n; i++) { refs[i] = obj_new(); *(u64 *)body_of(refs[i]) = i; }
        for (u32 k = 0; k < n; k++) *(Obj **)(body_of(refs[perm[k]]) + 8) = refs[perm[(k + 1) % n]];   // one random cycle through every object
        u64 total = 30000000ULL; u32 passes = (u32)(total / n); if (!passes) passes = 1;
        u64 s = 0; double t = now();
        for (u32 p = 0; p < passes; p++) for (u32 i = 0; i < n; i++) s += *(u64 *)body_of(refs[i]);
        double seq = (now() - t) / ((double)passes * n) * 1e9;
        t = now();
        for (u32 p = 0; p < passes; p++) for (u32 k = 0; k < n; k++) s += *(u64 *)body_of(refs[perm[k]]);
        double rd = (now() - t) / ((double)passes * n) * 1e9;
        u64 steps = 10000000ULL; Obj *o = refs[perm[0]]; t = now();
        for (u64 k = 0; k < steps; k++) o = *(Obj **)(body_of(o) + 8);
        double chase = (now() - t) / steps * 1e9;
        s += (u64)(uintptr_t)o; sink = s;
        printf(" seq_ns=%.3f random_ns=%.3f chase_ns=%.3f mapped_mb=%.1f\n", seq, rd, chase, mapped_bytes() / 1048576.0);
    } else if (!strcmp(work, "retain")) {
        for (u32 i = 0; i < n; i++) { refs[i] = obj_new(); *(u64 *)body_of(refs[i]) = i; surv[i] = cluster ? (u64)i * 10000 < (u64)n * keep : (rnd() % 10000) < keep; }
        u64 nsurv = 0; for (u32 i = 0; i < n; i++) nsurv += surv[i];
        long rss_full = rss_kb() - base;
        double t = now();
        for (u32 k = 0; k < n; k++) if (!surv[perm[k]]) { drop_strong(refs[perm[k]]); refs[perm[k]] = NULL; }
        double tfree = now() - t; long rss_free = rss_kb() - base; double mb_free = mapped_bytes() / 1048576.0;
        t = now(); u64 pages = release_all(); double trel = now() - t; long rss_rel = rss_kb() - base;
        u64 cf = 0; t = now(); u64 moved = evacuate(thresh, &cf); double tev = now() - t;
        t = now(); pages += release_all(); trel += now() - t;
        long rss_ev = rss_kb() - base; double mb_ev = mapped_bytes() / 1048576.0;
        u64 s = 0; for (u32 i = 0; i < n; i++) if (refs[i]) s += *(u64 *)body_of(refs[i]) == i;   // survivors still read correctly after the move
        printf(" keep_bp=%u pattern=%s survivors=%lu ok=%d rss_full_kb=%ld rss_after_free_kb=%ld mapped_after_free_mb=%.1f rss_after_pagerelease_kb=%ld pages_released=%lu"
               " rss_after_evac_kb=%ld mapped_after_evac_mb=%.1f moved=%lu chunks_freed=%lu free_ms=%.0f release_ms=%.0f evac_ms=%.0f ns_per_move=%.1f\n",
               keep, cluster ? "cluster" : "scatter", nsurv, s == nsurv, rss_full, rss_free, mb_free, rss_rel, pages, rss_ev, mb_ev, moved, cf,
               tfree * 1e3, trel * 1e3, tev * 1e3, moved ? tev / moved * 1e9 : 0.0);
    } else if (!strcmp(work, "weak")) {
        for (u32 i = 0; i < n; i++) { refs[i] = obj_new(); *(u64 *)body_of(refs[i]) = i; refs[i]->weak++; }
        long rss_full = rss_kb() - base;
        for (u32 k = 0; k < n; k++) drop_strong(refs[perm[k]]);                  // every object dies while a WeakRef to it lingers
        long rss_dead = rss_kb() - base; double mb_dead = mapped_bytes() / 1048576.0;
        for (u32 k = 0; k < n; k++) drop_weak(refs[perm[k]]);
        long rss_gone = rss_kb() - base;
        printf(" rss_full_kb=%ld rss_dead_with_weak_kb=%ld mapped_dead_mb=%.1f rss_after_weak_dropped_kb=%ld\n", rss_full, rss_dead, mb_dead, rss_gone);
    } else { fprintf(stderr, "unknown work\n"); return 2; }
    return 0;
}
