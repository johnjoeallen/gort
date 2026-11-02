// D15, second benchmark: realistic loop bodies. See bench/results/handle_real.md for the design and the results.
//
// Container: Handle { ptr, len, cap, spin lock, mutex }.  Every variant takes the same lock the same way, even the raw one.
//   lock  none   : unshared object, no lock (rule R6)
//         spin   : atomic exchange / release store, uncontended (a sync block on a cheap lock)
//         mutex  : pthread mutex, uncontended
//   B     batch  : elements processed per lock (B=1 is a sync per access, as db.get does; B=256 is one sync over a loop)
// Element body (all reads go through the container pointer):
//   x = a[i];  g = a[(x*K1 >> 7) & mask]  (data-dependent gather);  r = x ^ g;  W rounds of r ^= r>>17; r *= K2  (dependent mixing);
//   out[i] = r;  sum += r
//   W is the work axis: 0, 1, 2, 4, 8, 16, 32, 64 rounds.
// Modes (what the compiler would generate for a window of that length):
//   raw        pointer loaded once per lock window, kept in a register (what C does)
//   raw2       an identical second copy of raw: the difference raw2 - raw is the noise floor of this machine and build
//   rl16       the base pointer is reloaded once per 16 elements (a may-grow call every 16th element)
//   rlelem     reloaded once per element (a may-grow call every element; both reads use that one load)
//   rlacc      reloaded at every access, two per element (the worst case)
// Output lines:  R <size> <lock> <B> <W> <mode> <ns/element>   (best of reps; one pass is M = 2^20 element bodies)
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <stdatomic.h>
#include <pthread.h>
#include <time.h>

typedef uint64_t u64;
typedef struct { u64 *ptr; size_t len, cap; atomic_int spin; pthread_mutex_t mu; } Handle;
enum { L_NONE, L_SPIN, L_MUTEX };
enum { M_RAW, M_RAW2, M_RL16, M_RLELEM, M_RLACC };
#define BARRIER() __asm__ volatile("" ::: "memory")
#define K1 0x9E3779B97F4A7C15ull
#define K2 0xBF58476D1CE4E5B9ull
#define M (1u << 20)

static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec * 1e9 + t.tv_nsec; }

static inline __attribute__((always_inline)) void lock(Handle *h, int lt) {
    if (lt == L_SPIN) { while (atomic_exchange_explicit(&h->spin, 1, memory_order_acquire)) __builtin_ia32_pause(); }
    else if (lt == L_MUTEX) pthread_mutex_lock(&h->mu);
}
static inline __attribute__((always_inline)) void unlock(Handle *h, int lt) {
    if (lt == L_SPIN) atomic_store_explicit(&h->spin, 0, memory_order_release);
    else if (lt == L_MUTEX) pthread_mutex_unlock(&h->mu);
}

#define MIX(r, W) do { for (int w_ = 0; w_ < (W); w_++) { (r) ^= (r) >> 17; (r) *= K2; } } while (0)

static inline __attribute__((always_inline)) u64 run(Handle *h, u64 *out, size_t B, int W, int lt, const int mode) {
    const size_t n = h->len, mask = n - 1;
    u64 sum = 0; size_t pos = 0;
    for (size_t done = 0; done < M; done += B) {
        lock(h, lt);
        u64 *p = h->ptr;                                  // the window's base pointer, loaded after the lock is taken
        for (size_t k = 0; k < B; k++) {
            size_t i = (pos + k) & mask;
            u64 x, g;
            if (mode == M_RLACC) {
                BARRIER(); x = h->ptr[i];
                BARRIER(); g = h->ptr[((x * K1) >> 7) & mask];
            } else {
                if (mode == M_RLELEM) { BARRIER(); p = h->ptr; }
                else if (mode == M_RL16 && (k & 15) == 0) { BARRIER(); p = h->ptr; }
                x = p[i]; g = p[((x * K1) >> 7) & mask];
            }
            u64 r = x ^ g;
            MIX(r, W);
            out[i] = r; sum += r;
        }
        unlock(h, lt);
        pos = (pos + B) & mask;
    }
    return sum;
}
#define DEF(NAME, MODE) __attribute__((noinline)) static u64 NAME(Handle *h, u64 *o, size_t B, int W, int lt) { return run(h, o, B, W, lt, MODE); }
DEF(f_raw, M_RAW) DEF(f_raw2, M_RAW2) DEF(f_rl16, M_RL16) DEF(f_rlelem, M_RLELEM) DEF(f_rlacc, M_RLACC)
typedef u64 (*Fn)(Handle *, u64 *, size_t, int, int);

static volatile u64 sink;

int main(int argc, char **argv) {
    int reps = argc > 1 ? atoi(argv[1]) : 7;
    size_t sizes[] = { 512, 32768, 8388608 }; const char *sn[] = { "4KB", "256KB", "64MB" };
    const char *ln[] = { "none", "spin", "mutex" };
    int Ws[] = { 0, 1, 2, 4, 8, 16, 32, 64 };
    const char *mn[] = { "raw", "raw2", "rl16", "rlelem", "rlacc" };
    Fn fns[] = { f_raw, f_raw2, f_rl16, f_rlelem, f_rlacc };
    size_t Bs[] = { 1, 256 };
    for (int s = 0; s < 3; s++) {
        size_t n = sizes[s];
        Handle h; h.ptr = malloc(n * sizeof(u64)); h.len = n; h.cap = n; atomic_init(&h.spin, 0); pthread_mutex_init(&h.mu, NULL);
        u64 *out = malloc(n * sizeof(u64));
        u64 st = 88172645463325252ull; for (size_t i = 0; i < n; i++) { st ^= st << 13; st ^= st >> 7; st ^= st << 17; h.ptr[i] = st; out[i] = 0; }
        for (int l = 0; l < 3; l++) for (int b = 0; b < 2; b++) for (int wi = 0; wi < 8; wi++) {
            u64 ref = 0;
            for (int m = 0; m < 5; m++) {
                double best = 1e30; u64 v = fns[m](&h, out, Bs[b], Ws[wi], l);   // warm-up
                for (int r = 0; r < reps; r++) { double t0 = now(); v = fns[m](&h, out, Bs[b], Ws[wi], l); double t1 = now(); if (t1 - t0 < best) best = t1 - t0; }
                if (m == 0) ref = v; else if (v != ref) { fprintf(stderr, "CHECKSUM MISMATCH %s %s B=%zu W=%d %s\n", sn[s], ln[l], Bs[b], Ws[wi], mn[m]); return 1; }
                sink = v;
                printf("R %s %s %zu %d %s %.4f\n", sn[s], ln[l], Bs[b], Ws[wi], mn[m], best / (double)M);
            }
        }
        free(h.ptr); free(out);
    }
    return 0;
}
