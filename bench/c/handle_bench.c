// D15: cost of reaching array elements through a non-moving handle {ptr,len,cap} instead of a raw pointer.
//
//   raw        base pointer in a register for the whole loop (what C does; only valid if nothing can move the store)
//   raw+bar    same, with a compiler barrier each iteration (isolates the cost of the barrier itself)
//   hoisted    handle, ptr loaded once per deref window (the compiler's choice when no may-grow call is in the loop)
//   reload     handle, ptr reloaded every access (barrier forces it): the worst case, e.g. a may-grow call every iteration
//   every K    handle, ptr reloaded once per K accesses, a noinline call between (models a may-grow call that rarely grows)
//   cursor     (handle, offset) iterator: each step loads ptr, adds offset
//
// workloads: seq (sum a[i]), rnd (sum a[idx[i]], idx precomputed random permutation of the same range)
// sizes: 4 KB (L1), 256 KB (L2), 64 MB (memory)
//
// Also a correctness check: take (handle, offset) references, grow the array until the store moves many times, and confirm
// every reference still reads the right value; a raw pointer taken before the first grow is shown to have moved.
//
// usage: handle_bench [reps]      (pin with taskset for stable numbers)
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <time.h>

typedef uint64_t u64;
typedef struct { u64 *ptr; size_t len, cap; } Handle;

#define BARRIER() __asm__ volatile("" ::: "memory")

static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec * 1e9 + t.tv_nsec; }

__attribute__((noinline)) static void maybe_grow(Handle *h, size_t i) { // may move the store, almost never does
    if (__builtin_expect(i == (size_t)-1, 0)) { h->ptr = realloc(h->ptr, h->cap * 16); h->cap *= 2; }
    BARRIER();
}

__attribute__((noinline)) static u64 seq_raw(Handle *h)      { u64 s = 0; u64 *p = h->ptr; size_t n = h->len; for (size_t i = 0; i < n; i++) s += p[i]; return s; }
__attribute__((noinline)) static u64 seq_rawbar(Handle *h)   { u64 s = 0; u64 *p = h->ptr; size_t n = h->len; for (size_t i = 0; i < n; i++) { s += p[i]; BARRIER(); } return s; }
__attribute__((noinline)) static u64 seq_hoisted(Handle *h)  { u64 s = 0; u64 *p = h->ptr; size_t n = h->len; for (size_t i = 0; i < n; i++) s += p[i]; return s; }
__attribute__((noinline)) static u64 seq_reload(Handle *h)   { u64 s = 0; size_t n = h->len; for (size_t i = 0; i < n; i++) { s += h->ptr[i]; BARRIER(); } return s; }
__attribute__((noinline)) static u64 seq_every(Handle *h, size_t K) {
    u64 s = 0; size_t n = h->len;
    for (size_t i = 0; i < n; i += K) { maybe_grow(h, i); u64 *p = h->ptr; size_t e = i + K < n ? i + K : n; for (size_t j = i; j < e; j++) s += p[j]; }
    return s;
}
// same call pattern but the base pointer is NOT reloaded after it (unsound if the call grew; measures the call alone)
__attribute__((noinline)) static u64 seq_every_nr(Handle *h, size_t K) {
    u64 s = 0; size_t n = h->len; u64 *p = h->ptr;
    for (size_t i = 0; i < n; i += K) { maybe_grow(h, i); size_t e = i + K < n ? i + K : n; for (size_t j = i; j < e; j++) s += p[j]; }
    return s;
}
__attribute__((noinline)) static u64 seq_cursor(Handle *h)   { u64 s = 0; size_t n = h->len; for (size_t off = 0; off < n; off++) { s += *(h->ptr + off); BARRIER(); } return s; }

__attribute__((noinline)) static u64 rnd_raw(Handle *h, const uint32_t *ix)    { u64 s = 0; u64 *p = h->ptr; size_t n = h->len; for (size_t i = 0; i < n; i++) s += p[ix[i]]; return s; }
__attribute__((noinline)) static u64 rnd_reload(Handle *h, const uint32_t *ix) { u64 s = 0; size_t n = h->len; for (size_t i = 0; i < n; i++) { s += h->ptr[ix[i]]; BARRIER(); } return s; }
__attribute__((noinline)) static u64 rnd_rawbar(Handle *h, const uint32_t *ix) { u64 s = 0; u64 *p = h->ptr; size_t n = h->len; for (size_t i = 0; i < n; i++) { s += p[ix[i]]; BARRIER(); } return s; }

static volatile u64 sink;
#define TIME(NAME, CALL) do { double best = 1e30; for (int r = 0; r < reps; r++) { double t0 = now(); u64 v = (CALL); double t1 = now(); sink = v; if (t1 - t0 < best) best = t1 - t0; } \
    printf("  %-16s %7.3f ns/elem\n", NAME, best / (double)h.len); } while (0)

static void correctness(void) {
    Handle h = { malloc(8 * sizeof(u64)), 0, 8 };
    for (size_t i = 0; i < 8; i++) h.ptr[h.len++] = i * 7;
    u64 *raw = &h.ptr[3]; size_t off = 3;           // a raw interior pointer and a (handle, offset) reference
    u64 *first = h.ptr; int moves = 0, grows = 0;
    for (size_t i = 8; i < (1u << 20); i++) {
        if (h.len == h.cap) { u64 *old = h.ptr; h.ptr = realloc(h.ptr, h.cap * 2 * sizeof(u64)); h.cap *= 2; grows++; if (h.ptr != old) moves++; }
        h.ptr[h.len++] = i * 7;
    }
    int ok = (h.ptr[off] == 21) && (h.ptr[h.len - 1] == (h.len - 1) * 7);
    printf("correctness: %d grows, store moved %d times; (handle,offset) read %s; raw pointer %s\n", grows, moves, ok ? "OK" : "WRONG",
           (h.ptr == first) ? "still valid (never moved)" : "now DANGLING (its address is not the live element)");
    (void)raw;
}

int main(int argc, char **argv) {
    int reps = argc > 1 ? atoi(argv[1]) : 7;
    size_t sizes[] = { 4096, 262144, 67108864 }; const char *names[] = { "4 KB (L1)", "256 KB (L2)", "64 MB (memory)" };
    correctness();
    for (int s = 0; s < 3; s++) {
        size_t n = sizes[s] / sizeof(u64);
        Handle h = { malloc(n * sizeof(u64) * 2), n, n * 2 };
        for (size_t i = 0; i < n; i++) h.ptr[i] = i;
        uint32_t *ix = malloc(n * sizeof(uint32_t)); for (size_t i = 0; i < n; i++) ix[i] = i;
        srand(1); for (size_t i = n - 1; i > 0; i--) { size_t j = (size_t)rand() % (i + 1); uint32_t t = ix[i]; ix[i] = ix[j]; ix[j] = t; }
        printf("\n== %s, %zu elements\n sequential sum\n", names[s], n);
        TIME("raw", seq_raw(&h));            TIME("raw+bar", seq_rawbar(&h));      TIME("hoisted", seq_hoisted(&h));
        TIME("reload", seq_reload(&h));      TIME("cursor", seq_cursor(&h));
        TIME("call only 1", seq_every_nr(&h, 1)); TIME("every 1", seq_every(&h, 1));
        TIME("call only 16", seq_every_nr(&h, 16));   TIME("every 16", seq_every(&h, 16));  TIME("call only 256", seq_every_nr(&h, 256)); TIME("every 256", seq_every(&h, 256));
        printf(" random gather\n");
        TIME("raw", rnd_raw(&h, ix));        TIME("raw+bar", rnd_rawbar(&h, ix));  TIME("reload", rnd_reload(&h, ix));
        free(h.ptr); free(ix);
    }
    return 0;
}
