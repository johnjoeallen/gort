// B18 benchmark: where a counted object's control block lives (D18 item 11). No mover, so nothing here ever moves a body.
// Hand-written C, no Gort compiler.
//
//   gcc -O2 -DLAYOUT=0 -DBODY=64 [-DATOMIC=1] -o ctl_0_64 ctl_bench.c
//
//   LAYOUT 0  H  header in the slot: slot = { strong, weak, meta, body[] } (16 B), Ref = the slot (the slice-0 runtime's layout)
//   LAYOUT 1  D  control section in the chunk: [chunk header][control: N x 8 B][bodies: N x BODY]; Ref = the body address;
//                the entry is found by arithmetic: index = (addr - chunk - O_BODY) / BODY (BODY is a compile-time constant, so
//                a shift or a reciprocal multiply), entry = chunk + O_CTL + 8 * index
//   LAYOUT 2  C  the same, but the counts live in a separate allocation per chunk, reached through a pointer in the chunk header
//   LAYOUT 3  B  external slab: Ref = a 24 B control block { strong, weak, body*, meta } in a control arena, body in a body arena
//
// All four use the same arena code: 64 KB chunks aligned to 64 KB, the chunk found by masking, free-index stack and used bitmap kept
// in the chunk header section (so a free page can be released), an empty chunk unmapped, a slot not reused while its weak count is
// above zero (no per-slot generation, as decided in D18).
//
// usage: ctl_L_B work=alloc|access|mem|weak|retain n=<objects> [keep=<basis points>]
// Output: one RESULT line (key=value) per measurement.
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <time.h>
#include <assert.h>
#include <sys/mman.h>

#ifndef LAYOUT
#define LAYOUT 0
#endif
#ifndef BODY
#define BODY 64
#endif
#ifndef ATOMIC
#define ATOMIC 0
#endif
typedef uint64_t u64; typedef uint32_t u32; typedef uint16_t u16; typedef uint8_t u8;
#define CHUNK (64UL << 10)
#define PAGE 4096UL
#define AL(v, a) ((((size_t)(v)) + (a) - 1) / (a) * (a))
#define HDR 128
#define NSLOTS(slot, ctlb) ((u32)((CHUNK - 512) / ((slot) + (ctlb) + 3)))
#define O_BITS_(n) AL(HDR + 2 * (size_t)(n), 8)
#define O_CTL_(n) AL(O_BITS_(n) + 8 * (((size_t)(n) + 63) / 64), 64)
#define O_BODY_(n, ctlb) AL(O_CTL_(n) + (size_t)(n) * (ctlb), 64)

#if ATOMIC
#define CINC(x) __atomic_fetch_add(&(x), 1, __ATOMIC_RELAXED)
#define CDEC(x) __atomic_sub_fetch(&(x), 1, __ATOMIC_ACQ_REL)
#else
#define CINC(x) ((x)++)
#define CDEC(x) (--(x))
#endif

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
    u16 *freeidx; u64 *bits; char *slots; char *ctl;       // ctl: the control section (D) or the separate allocation (C)
} Chunk;
typedef struct { u32 slot, ctlb, nslots; size_t o_free, o_bits, o_ctl, o_slots; u32 recip; Chunk *head; Chunk **all; u32 nall, capall; u64 nmapped; } Arena;

static void arena_init(Arena *a, u32 slot, u32 ctlb) {
    memset(a, 0, sizeof *a); a->slot = slot; a->ctlb = ctlb;
    u32 n = NSLOTS(slot, ctlb);
    a->nslots = n; a->o_free = HDR; a->o_bits = O_BITS_(n); a->o_ctl = O_CTL_(n); a->o_slots = O_BODY_(n, ctlb);
    assert(a->o_slots + (size_t)n * slot <= CHUNK);
    a->recip = (u32)(((1ULL << 32) + slot - 1) / slot);       // exact floor(off / slot) for off < 65536
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
    c->ctl = (char *)(b + a->o_ctl);
#if LAYOUT == 2
    if (a->ctlb) c->ctl = aligned_alloc(64, AL((size_t)a->nslots * a->ctlb, 64));          // separate allocation, not in the chunk
    if (a->ctlb) memset(c->ctl, 0, AL((size_t)a->nslots * a->ctlb, 64));
#endif
    if (a->nall == a->capall) { a->capall = a->capall ? a->capall * 2 : 1024; a->all = realloc(a->all, a->capall * sizeof *a->all); }
    c->idx = a->nall; a->all[a->nall++] = c; a->nmapped++;
    list_push(a, c);
    return c;
}
static void chunk_unmap(Arena *a, Chunk *c) {
#if LAYOUT == 2
    if (a->ctlb) free(c->ctl);
#endif
    a->all[c->idx] = NULL; a->nmapped--; munmap(c, CHUNK);
}
static inline u32 slot_index(Arena *a, Chunk *c, const void *p) { return (u32)((((u64)((const char *)p - c->slots)) * a->recip) >> 32); }
static inline void *a_alloc(Arena *a, u32 *out_i, Chunk **out_c) {
    Chunk *c = a->head ? a->head : chunk_new(a);
    u32 i = c->nfree ? c->freeidx[--c->nfree] : c->bump++;
    c->live++; c->bits[i >> 6] |= 1ULL << (i & 63);
    if (c->live == c->nslots) list_remove(a, c);
    if (out_i) *out_i = i;
    if (out_c) *out_c = c;
    return c->slots + (size_t)i * a->slot;
}
static inline void a_free(Arena *a, void *p) {
    Chunk *c = (Chunk *)((uintptr_t)p & ~(CHUNK - 1));
    u32 i = slot_index(a, c, p);
    c->bits[i >> 6] &= ~(1ULL << (i & 63)); c->freeidx[c->nfree++] = (u16)i;
    int was_full = c->live == c->nslots;
    c->live--;
    if (c->live == 0) { if (!was_full) list_remove(a, c); chunk_unmap(a, c); return; }
    if (was_full) list_push(a, c);
}
static inline int slot_used(Chunk *c, u32 i) { return (c->bits[i >> 6] >> (i & 63)) & 1; }
// release every page of a section (elements of `esize` bytes starting at `base_off`) whose elements are all free
static u64 release_section(Arena *a, size_t base_off, u32 esize, size_t limit) {
    u64 n = 0;
    for (u32 k = 0; k < a->nall; k++) {
        Chunk *c = a->all[k]; if (!c) continue;
        for (size_t off = AL(base_off, PAGE); off + PAGE <= limit; off += PAGE) {   // only pages wholly inside the section
            long first = (long)((off - base_off) / esize), last = (long)((off + PAGE - 1 - base_off) / esize);
            if (first >= (long)c->bump) continue;
            if (last >= (long)c->nslots) last = c->nslots - 1;
            int any = 0; for (long s = first; s <= last && !any; s++) any = slot_used(c, (u32)s);
            if (!any) { madvise((char *)c + off, PAGE, MADV_DONTNEED); n++; }
        }
    }
    return n;
}

typedef struct { u32 strong, weak; } Ctl;
typedef void *Ref;

#if LAYOUT == 0
#define NAME "H"
typedef struct { u32 strong, weak; u64 meta; } Hdr;                      // 16 B header, body follows
static Arena A_obj;
static void objs_init(void) { arena_init(&A_obj, 16 + BODY, 0); }
static inline Ref ref_new(void) { Hdr *h = a_alloc(&A_obj, NULL, NULL); h->strong = 1; h->weak = 0; h->meta = 0; return h; }
static inline char *body_of(Ref r) { return (char *)r + 16; }
static inline Ctl *ctl_of(Ref r) { return (Ctl *)r; }
static inline void drop_strong(Ref r) { Ctl *c = ctl_of(r); if (CDEC(c->strong) == 0 && c->weak == 0) a_free(&A_obj, r); }
static inline void drop_weak(Ref r) { Ctl *c = ctl_of(r); if (CDEC(c->weak) == 0 && c->strong == 0) a_free(&A_obj, r); }
static u64 mapped_bytes(void) { return A_obj.nmapped * CHUNK; }
static u64 release_all(void) { return release_section(&A_obj, A_obj.o_slots, 16 + BODY, CHUNK); }
#elif LAYOUT == 1 || LAYOUT == 2
#if LAYOUT == 1
#define NAME "D"
#else
#define NAME "C"
#endif
static Arena A_obj;
#define N_ NSLOTS(BODY, 8)
#define OB_ O_BODY_(N_, 8)
#define OC_ O_CTL_(N_)
static void objs_init(void) { arena_init(&A_obj, BODY, 8); assert(A_obj.o_slots == OB_ && A_obj.o_ctl == OC_); }
static inline Ctl *ctl_of(Ref r) {
    uintptr_t c = (uintptr_t)r & ~(CHUNK - 1);
    size_t idx = ((uintptr_t)r - c - OB_) / BODY;                           // BODY is a constant: a shift, or a reciprocal multiply
#if LAYOUT == 1
    return (Ctl *)(c + OC_ + idx * 8);
#else
    return (Ctl *)(((Chunk *)c)->ctl + idx * 8);
#endif
}
static inline Ref ref_new(void) {
    u32 i; Chunk *c; char *b = a_alloc(&A_obj, &i, &c);
    Ctl *k = (Ctl *)(c->ctl + (size_t)i * 8); k->strong = 1; k->weak = 0;
    return b;
}
static inline char *body_of(Ref r) { return r; }
static inline void drop_strong(Ref r) { Ctl *c = ctl_of(r); if (CDEC(c->strong) == 0 && c->weak == 0) a_free(&A_obj, r); }
static inline void drop_weak(Ref r) { Ctl *c = ctl_of(r); if (CDEC(c->weak) == 0 && c->strong == 0) a_free(&A_obj, r); }
static u64 mapped_bytes(void) { return A_obj.nmapped * CHUNK; }
static u64 release_all(void) {
    u64 n = release_section(&A_obj, A_obj.o_slots, BODY, CHUNK);
#if LAYOUT == 1
    n += release_section(&A_obj, A_obj.o_ctl, 8, A_obj.o_ctl + (size_t)A_obj.nslots * 8);
#endif
    return n;
}
#else
#define NAME "B"
typedef struct { u32 strong, weak; char *body; u64 meta; } Obj;           // 24 B control block
static Arena A_cb, A_body;
static void objs_init(void) { arena_init(&A_cb, sizeof(Obj), 0); arena_init(&A_body, BODY, 0); }
static inline Ref ref_new(void) {
    Obj *o = a_alloc(&A_cb, NULL, NULL); o->body = a_alloc(&A_body, NULL, NULL); o->strong = 1; o->weak = 0; o->meta = 0; return o;
}
static inline char *body_of(Ref r) { return ((Obj *)r)->body; }
static inline Ctl *ctl_of(Ref r) { return (Ctl *)r; }
static inline void drop_strong(Ref r) {
    Obj *o = r;
    if (CDEC(o->strong) == 0) { a_free(&A_body, o->body); o->body = NULL; if (o->weak == 0) a_free(&A_cb, o); }
}
static inline void drop_weak(Ref r) { Obj *o = r; if (CDEC(o->weak) == 0 && o->strong == 0) a_free(&A_cb, o); }
static u64 mapped_bytes(void) { return (A_cb.nmapped + A_body.nmapped) * CHUNK; }
static u64 release_all(void) { return release_section(&A_cb, A_cb.o_slots, sizeof(Obj), CHUNK) + release_section(&A_body, A_body.o_slots, BODY, CHUNK); }
#endif

static const char *arg(int argc, char **argv, const char *k, const char *d) {
    size_t n = strlen(k);
    for (int i = 1; i < argc; i++) if (!strncmp(argv[i], k, n) && argv[i][n] == '=') return argv[i] + n + 1;
    return d;
}
static void shuffle(u32 *p, u32 n) { for (u32 i = n - 1; i > 0; i--) { u32 j = rnd() % (i + 1), t = p[i]; p[i] = p[j]; p[j] = t; } }
static volatile u64 sink;
static inline u64 ld(Ref r) { return *(u64 *)body_of(r); }
static inline Ref next_of(Ref r) { return *(Ref *)(body_of(r) + 8); }
// use of a Ref the way generated code uses one: take a count, read, drop the count
static inline u64 use(Ref r) { Ctl *c = ctl_of(r); CINC(c->strong); u64 v = ld(r); CDEC(c->strong); return v; }

int main(int argc, char **argv) {
    const char *work = arg(argc, argv, "work", "alloc");
    u32 n = (u32)atol(arg(argc, argv, "n", "1500000"));
    u32 keep = (u32)atol(arg(argc, argv, "keep", "100"));
    objs_init();
    Ref *refs = malloc((size_t)n * sizeof *refs); u32 *perm = malloc((size_t)n * sizeof *perm); u8 *surv = malloc(n);
    for (u32 i = 0; i < n; i++) { perm[i] = i; refs[i] = NULL; surv[i] = 0; }
    shuffle(perm, n);
    long base = rss_kb();
    printf("RESULT work=%s layout=%s body=%d atomic=%d n=%u", work, NAME, BODY, ATOMIC, n);

    if (!strcmp(work, "alloc")) {
        double t = now();
        for (u32 i = 0; i < n; i++) { refs[i] = ref_new(); *(u64 *)body_of(refs[i]) = i; }
        double fill = (now() - t) / n * 1e9;
        long rss_full = rss_kb() - base;
        t = now();
        for (u32 k = 0; k < n; k++) drop_strong(refs[perm[k]]);
        double fr = (now() - t) / n * 1e9;
        for (u32 i = 0; i < n; i++) refs[i] = ref_new();
        u32 ops = n < 2000000 ? n : 2000000;
        t = now();
        for (u32 k = 0; k < ops; k++) { u32 j = rnd() % n; drop_strong(refs[j]); refs[j] = ref_new(); *(u64 *)body_of(refs[j]) = k; }
        double churn = (now() - t) / ops * 1e9;
        printf(" fill_ns=%.2f free_random_ns=%.2f churn_pair_ns=%.2f rss_full_kb=%ld\n", fill, fr, churn, rss_full);
    } else if (!strcmp(work, "access")) {
        for (u32 i = 0; i < n; i++) { refs[i] = ref_new(); *(u64 *)body_of(refs[i]) = i; }
        for (u32 k = 0; k < n; k++) *(Ref *)(body_of(refs[perm[k]]) + 8) = refs[perm[(k + 1) % n]];   // one random cycle through every object
        u64 total = 30000000ULL; u32 passes = (u32)(total / n); if (!passes) passes = 1;
        u64 s = 0; double t;
        t = now(); for (u32 p = 0; p < passes; p++) for (u32 i = 0; i < n; i++) s += ld(refs[i]);
        double seq = (now() - t) / ((double)passes * n) * 1e9;
        t = now(); for (u32 p = 0; p < passes; p++) for (u32 k = 0; k < n; k++) s += ld(refs[perm[k]]);
        double rd = (now() - t) / ((double)passes * n) * 1e9;
        u64 steps = 10000000ULL; Ref o = refs[perm[0]];
        t = now(); for (u64 k = 0; k < steps; k++) o = next_of(o);
        double chase = (now() - t) / steps * 1e9;
        s += (u64)(uintptr_t)o;
        // the same three with a count taken and dropped around each read
        t = now(); for (u32 p = 0; p < passes; p++) for (u32 i = 0; i < n; i++) s += use(refs[i]);
        double cseq = (now() - t) / ((double)passes * n) * 1e9;
        t = now(); for (u32 p = 0; p < passes; p++) for (u32 k = 0; k < n; k++) s += use(refs[perm[k]]);
        double crd = (now() - t) / ((double)passes * n) * 1e9;
        o = refs[perm[0]];
        t = now();
        for (u64 k = 0; k < steps; k++) { Ctl *c = ctl_of(o); CINC(c->strong); Ref nx = next_of(o); CDEC(c->strong); o = nx; }
        double cchase = (now() - t) / steps * 1e9;
        // clone (the count stays up) then drop later: a count change on an object whose body is not read
        t = now(); for (u32 p = 0; p < passes; p++) for (u32 k = 0; k < n; k++) { Ctl *c = ctl_of(refs[perm[k]]); CINC(c->strong); }
        for (u32 p = 0; p < passes; p++) for (u32 k = 0; k < n; k++) { Ctl *c = ctl_of(refs[perm[k]]); CDEC(c->strong); }
        double ccount = (now() - t) / ((double)passes * n * 2) * 1e9;
        s += (u64)(uintptr_t)o; sink = s;
        printf(" seq_ns=%.3f random_ns=%.3f chase_ns=%.3f cseq_ns=%.3f crandom_ns=%.3f cchase_ns=%.3f count_only_ns=%.3f mapped_mb=%.1f\n",
               seq, rd, chase, cseq, crd, cchase, ccount, mapped_bytes() / 1048576.0);
    } else if (!strcmp(work, "mem")) {
        for (u32 i = 0; i < n; i++) { refs[i] = ref_new(); *(u64 *)body_of(refs[i]) = i; }
        printf(" rss_full_kb=%ld bytes_per_object=%.1f mapped_mb=%.1f\n", rss_kb() - base, (rss_kb() - base) * 1024.0 / n, mapped_bytes() / 1048576.0);
    } else if (!strcmp(work, "retain")) {
        for (u32 i = 0; i < n; i++) { refs[i] = ref_new(); *(u64 *)body_of(refs[i]) = i; surv[i] = (rnd() % 10000) < keep; }
        u64 nsurv = 0; for (u32 i = 0; i < n; i++) nsurv += surv[i];
        long rss_full = rss_kb() - base;
        for (u32 k = 0; k < n; k++) if (!surv[perm[k]]) { drop_strong(refs[perm[k]]); refs[perm[k]] = NULL; }
        long rss_free = rss_kb() - base;
        u64 pages = release_all(); long rss_rel = rss_kb() - base;
        u64 ok = 0; for (u32 i = 0; i < n; i++) if (refs[i]) ok += ld(refs[i]) == i;
        printf(" keep_bp=%u survivors=%lu ok=%d rss_full_kb=%ld rss_after_free_kb=%ld rss_after_pagerelease_kb=%ld pages_released=%lu\n",
               keep, nsurv, ok == nsurv, rss_full, rss_free, rss_rel, pages);
    } else if (!strcmp(work, "weak")) {
        for (u32 i = 0; i < n; i++) { refs[i] = ref_new(); *(u64 *)body_of(refs[i]) = i; ctl_of(refs[i])->weak++; }
        long rss_full = rss_kb() - base;
        for (u32 k = 0; k < n; k++) drop_strong(refs[perm[k]]);                  // every object dies while a WeakRef to it lingers
        long rss_dead = rss_kb() - base;
        for (u32 k = 0; k < n; k++) drop_weak(refs[perm[k]]);
        long rss_gone = rss_kb() - base;
        printf(" rss_full_kb=%ld rss_dead_with_weak_kb=%ld rss_after_weak_dropped_kb=%ld\n", rss_full, rss_dead, rss_gone);
    } else { fprintf(stderr, "unknown work\n"); return 2; }
    return 0;
}
