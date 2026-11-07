/* gort_rt0.h - the slice-0 runtime: counted objects, strings and arrays for generated code.
 *
 * Single-threaded, non-atomic counts, no scheduler. This is NOT c/runtime/rt.h (tasks, pools, sockets); slice 0 has no
 * parking, so it needs none of that.
 *
 * Where a counted object's control block lives is still open (D18 item 11, benchmark B18). Generated code reaches it
 * only through GORT_CTL, gort_new, gort_retain, gort_release_last and gort_free, so the layout can change in one place.
 * Slice 0 keeps the control block in a 16-byte header just before the body; a Ref<T> is the address of the body. */
#ifndef GORT_RT0_H
#define GORT_RT0_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdio.h>
#include <sched.h>
#include <pthread.h>
#include <time.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>

typedef struct { size_t strong; uint32_t weak; uint32_t lock; } gort_ctl;   /* `lock`: the sync/mutate word, 0 free, bit 31 writer, else the reader count */
#define GORT_CTL(p) (((gort_ctl *)(p)) - 1)

/* live heap objects (counted objects, heap strings, arrays); main() checks that it is zero at exit */
static size_t gort_live = 0;

static void gort_trap(const char *msg) __attribute__((noreturn));
static void gort_trap(const char *msg) {
    fflush(stdout);
    fprintf(stderr, "gort: runtime error: %s\n", msg);
    exit(101);
}
#define GORT_NZ(b) ((b) ? 0 : (gort_trap("division by zero"), 0))

static inline size_t gort_chk(size_t i, size_t len) {
    if (i >= len) gort_trap("index out of bounds");
    return i;
}

/* ---- counted objects ---- */
/* ---- sync blocks: a reader-writer lock in the object header, re-entrant within one call tree, FIFO, and parking.
 *
 * Lock word: bit 31 a writer holds it, bit 30 somebody is queued, bits 0-29 the number of readers.
 *
 * A block never parks while it holds the lock (the compiler checks), so the OS thread that holds a lock is the only code running on it until
 * the block ends: the thread is the identity of "this call tree". Every block pushes an entry on a thread-local stack saying what it did, and
 * the lock is released only when the entry that actually took it is popped (the outermost block):
 *   not held yet                    -> take it (READ or WRITE), released on pop
 *   a block, already held (any mode)-> nothing to do, nothing to release (NOOP)
 *   writes, held for reads only     -> UPGRADE in place (waits until this thread is the only reader); pop drops back to a plain read
 *
 * Waiting. A busy lock is spun on briefly, then the acquirer queues FIFO (per lock, in a hashed table of buckets). A release with waiters queued
 * WAKES the head of the queue (a writer alone, or the run of readers at the head); the woken waiter takes the lock itself when it runs, keeping its
 * place in the queue if a newcomer got there first. The lock is never owned by a waiter that is not running. A vthread that holds no lock
 * PARKS: the worker runs other vthreads until it is woken. A wait made while the thread already holds a lock (a nested block, the
 * second object of a group) blocks the OS thread instead, since yielding would run other vthreads while this one still holds its locks; so does a
 * wait made off a worker. Blocked waits give up after a while with a trap: that is two threads taking two objects in opposite orders, or two
 * threads upgrading the same object. ---- */
#define GORT_WRBIT 0x80000000u
#define GORT_WAITBIT 0x40000000u
#define GORT_PRIOBIT 0x20000000u   /* the oldest waiter has waited too long: newcomers must queue behind it */
#define GORT_CNTMASK 0x0fffffffu
#ifndef GORT_STARVE_NS
#define GORT_STARVE_NS 1000000     /* 1 ms */
#endif
enum { GORT_H_READ, GORT_H_WRITE, GORT_H_NOOP, GORT_H_UP };
typedef struct { void *obj; unsigned char kind; } gort_held;
#define GORT_HELD_MAX 256
static __thread gort_held gort_held_stk[GORT_HELD_MAX];
static __thread unsigned gort_held_n;

/* the runtime's side of parking (rt/posix.c); absent in a plain program, whose waits then block the thread */
extern void *gort_rt_vt_self(void) __attribute__((weak));                  /* the vthread running now, or NULL off a worker */
extern void gort_rt_vt_park(void *vt) __attribute__((weak));               /* return when gort_rt_vt_unpark(vt) has been called */
extern void gort_rt_vt_unpark(void *vt) __attribute__((weak));             /* callable from any thread; a wake before the park is not lost */

typedef struct gort_waiter { void *obj; int write; int signaled; uint64_t since; void *vt; struct gort_waiter *next; pthread_cond_t cv; } gort_waiter;
typedef struct { pthread_mutex_t m; gort_waiter *head, *tail; } gort_bucket;
static gort_bucket gort_buckets[64];       /* all zero is a valid unlocked mutex on glibc */
static size_t gort_stat_parked, gort_stat_blocked, gort_stat_prio;
static inline gort_bucket *gort_bucket_of(void *p) { return &gort_buckets[((uintptr_t)p >> 4) % 64]; }

/* what this call tree holds on p: 0 nothing, 1 read, 2 write */
static inline int gort_held_mode(void *p) {
    int m = 0;
    for (unsigned i = 0; i < gort_held_n; i++)
        if (gort_held_stk[i].obj == p) { if (gort_held_stk[i].kind == GORT_H_WRITE || gort_held_stk[i].kind == GORT_H_UP) return 2; m = 1; }
    return m;
}
static inline void gort_held_push(void *p, unsigned char kind) {
    if (gort_held_n == GORT_HELD_MAX) gort_trap("sync blocks nested too deeply");
    gort_held_stk[gort_held_n].obj = p; gort_held_stk[gort_held_n].kind = kind; gort_held_n++;
}
static inline unsigned char gort_held_pop(void *p) {
    if (gort_held_n == 0 || gort_held_stk[gort_held_n - 1].obj != p) gort_trap("internal error: sync block released out of order");
    return gort_held_stk[--gort_held_n].kind;
}

static inline uint64_t gort_lk_now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec * 1000000000u + (uint64_t)t.tv_nsec; }
static bool gort_queued(gort_bucket *b, void *p) {
    for (gort_waiter *x = b->head; x; x = x->next) if (x->obj == p) return true;
    return false;
}
/* ---- the queue (bucket mutex held in all of these) ---- */
/* Is x first in line for its lock? A writer: no other waiter for the object is ahead of it. A reader: no writer is ahead of it. */
static bool gort_at_head(gort_bucket *b, gort_waiter *x) {
    for (gort_waiter *w = b->head; w; w = w->next) {
        if (w->obj != x->obj) continue;
        if (w == x) return true;
        if (w->write || x->write) return false;
    }
    return false;
}
static void gort_unqueue(gort_bucket *b, gort_waiter *x) {
    gort_waiter **pp = &b->head, *prev = NULL;
    while (*pp && *pp != x) { prev = *pp; pp = &(*pp)->next; }
    if (*pp) { *pp = x->next; if (b->tail == x) b->tail = prev; }
}
/* A woken waiter takes the lock itself, if it is still free and it is first in line. It does NOT own the lock while it waits to be scheduled:
 * a descheduled owner would stall every other worker. A newcomer may take a free lock first; the waiter then goes back to sleep and is woken
 * again by the next release, still at the head of the queue. */
static bool gort_try_take(gort_bucket *b, gort_waiter *x) {
    void *p = x->obj;
    uint32_t *l = &GORT_CTL(p)->lock;
    if (!gort_at_head(b, x)) return false;
    for (;;) {
        uint32_t v = __atomic_load_n(l, __ATOMIC_RELAXED);
        if (x->write ? (v & (GORT_WRBIT | GORT_CNTMASK)) != 0 : (v & GORT_WRBIT) != 0) return false;
        if (__atomic_compare_exchange_n(l, &v, x->write ? (v | GORT_WRBIT) : v + 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) break;
    }
    gort_unqueue(b, x);
    bool more = false;
    for (gort_waiter *w = b->head; w; w = w->next) if (w->obj == p) { more = true; break; }
    __atomic_fetch_and(l, more ? ~GORT_PRIOBIT : ~(GORT_PRIOBIT | GORT_WAITBIT), __ATOMIC_ACQ_REL);   /* the starved waiter has its turn */
    return true;
}
static void gort_signal(gort_waiter *x) {
    if (x->signaled) return;
    x->signaled = 1;
    if (x->vt) gort_rt_vt_unpark(x->vt); else pthread_cond_signal(&x->cv);
}
/* the lock was released with waiters queued: wake the head (a writer alone, or the run of readers at the head) */
static void gort_wake_head(gort_bucket *b, void *p) {
    gort_waiter *w = b->head;
    while (w && w->obj != p) w = w->next;
    if (!w) { __atomic_fetch_and(&GORT_CTL(p)->lock, ~(GORT_WAITBIT | GORT_PRIOBIT), __ATOMIC_ACQ_REL); return; }
    if (w->write) { gort_signal(w); return; }
    for (; w; w = w->next) { if (w->obj != p) continue; if (w->write) break; gort_signal(w); }
}

/* Spin before queueing: hold times are short (a body cannot park), so a lock that is busy now is usually free within a few hundred cycles, and a
 * park-and-wake costs system calls. */
#ifndef GORT_SPIN
#define GORT_SPIN 2000
#endif
static inline void gort_cpu_relax(void) {
#if defined(__x86_64__) || defined(__i386__)
    __asm__ __volatile__("pause");
#elif defined(__aarch64__)
    __asm__ __volatile__("yield");
#endif
}
/* Bounded overtaking: if the oldest waiter for p has waited over GORT_STARVE_NS, set the priority bit. Newcomers (fast path and spin alike) then
 * queue behind it, the holders drain, and it takes the lock next. Only newcomers that find waiters queued come here, so the common path is untouched. */
static void gort_check_starved(gort_bucket *b, void *p) {
    for (gort_waiter *w = b->head; w; w = w->next)
        if (w->obj == p) {
            if (gort_lk_now() - w->since > GORT_STARVE_NS &&
                !(__atomic_fetch_or(&GORT_CTL(p)->lock, GORT_PRIOBIT, __ATOMIC_ACQ_REL) & GORT_PRIOBIT)) __atomic_fetch_add(&gort_stat_prio, 1, __ATOMIC_RELAXED);
            return;
        }
}
static void gort_lock_slow(void *p, int w) {
    gort_bucket *b = gort_bucket_of(p);
    uint32_t *l = &GORT_CTL(p)->lock;
    if (__atomic_load_n(l, __ATOMIC_RELAXED) & GORT_WAITBIT) {
        pthread_mutex_lock(&b->m);
        gort_check_starved(b, p);
        pthread_mutex_unlock(&b->m);
    }
    for (int i = 0; i < GORT_SPIN; i++) {
        uint32_t v = __atomic_load_n(l, __ATOMIC_RELAXED);
        if (v & GORT_PRIOBIT) break;
        if ((w ? (v & (GORT_WRBIT | GORT_CNTMASK)) == 0 : (v & GORT_WRBIT) == 0) &&
            __atomic_compare_exchange_n(l, &v, w ? (v | GORT_WRBIT) : v + 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) return;
        gort_cpu_relax();
    }
    gort_waiter x; x.obj = p; x.write = w; x.signaled = 0; x.next = NULL; x.since = gort_lk_now();
    void *vt = (gort_held_n == 0 && gort_rt_vt_self) ? gort_rt_vt_self() : NULL;   /* park only while holding nothing */
    x.vt = vt;
    pthread_cond_init(&x.cv, NULL);
    pthread_mutex_lock(&b->m);
    if (b->tail) b->tail->next = &x; else b->head = &x;
    b->tail = &x;
    __atomic_fetch_or(l, GORT_WAITBIT, __ATOMIC_ACQ_REL);
    bool counted = false;
    struct timespec dl; clock_gettime(CLOCK_REALTIME, &dl); dl.tv_sec += 5;
    while (!gort_try_take(b, &x)) {                        /* (also covers: it was released before the bit was set) */
        x.signaled = 0;
        if (vt) {
            if (!counted) { __atomic_fetch_add(&gort_stat_parked, 1, __ATOMIC_RELAXED); counted = true; }
            pthread_mutex_unlock(&b->m);
            while (!__atomic_load_n(&x.signaled, __ATOMIC_ACQUIRE)) gort_rt_vt_park(vt);
            pthread_mutex_lock(&b->m);
        } else {
            if (!counted) { __atomic_fetch_add(&gort_stat_blocked, 1, __ATOMIC_RELAXED); counted = true; }
            if (pthread_cond_timedwait(&x.cv, &b->m, &dl) != 0 && !x.signaled) {
                gort_unqueue(b, &x);                       /* give up: leave the queue, then say why */
                pthread_mutex_unlock(&b->m);
                gort_trap("lock wait too long (another thread holds this object, two threads upgrade the same lock, or two objects are locked in opposite orders)");
            }
        }
    }
    pthread_mutex_unlock(&b->m);
    pthread_cond_destroy(&x.cv);
}
static void gort_release_slow(void *p) {
    gort_bucket *b = gort_bucket_of(p);
    pthread_mutex_lock(&b->m);
    uint32_t v = __atomic_load_n(&GORT_CTL(p)->lock, __ATOMIC_ACQUIRE);
    if (!(v & (GORT_WRBIT | GORT_CNTMASK))) gort_wake_head(b, p);   /* still free (a newcomer may have taken it already) */
    pthread_mutex_unlock(&b->m);
}

static inline void gort_rd_lock(void *p) {
    if (gort_held_mode(p)) { gort_held_push(p, GORT_H_NOOP); return; }
    uint32_t *l = &GORT_CTL(p)->lock;
    uint32_t v = __atomic_load_n(l, __ATOMIC_RELAXED);
    if (!(v & (GORT_WRBIT | GORT_PRIOBIT | GORT_WAITBIT)) && __atomic_compare_exchange_n(l, &v, v + 1, 1, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) { gort_held_push(p, GORT_H_READ); return; }
    gort_lock_slow(p, 0);                       /* returns holding it */
    gort_held_push(p, GORT_H_READ);
}
static inline void gort_rd_unlock(void *p) {
    if (gort_held_pop(p) != GORT_H_READ) return;
    uint32_t v = __atomic_fetch_sub(&GORT_CTL(p)->lock, 1, __ATOMIC_RELEASE);
    if (((v - 1) & (GORT_CNTMASK | GORT_WAITBIT)) == GORT_WAITBIT) gort_release_slow(p);
}
static inline void gort_wr_lock(void *p) {
    int m = gort_held_mode(p);
    if (m == 2) { gort_held_push(p, GORT_H_NOOP); return; }
    uint32_t *l = &GORT_CTL(p)->lock;
    if (m == 1) {                               /* held for reading by this thread: upgrade when it is the only reader */
        unsigned spins = 0;
        for (;;) {
            uint32_t v = __atomic_load_n(l, __ATOMIC_RELAXED);
            if (!(v & GORT_WRBIT) && (v & GORT_CNTMASK) == 1 && __atomic_compare_exchange_n(l, &v, (v & (GORT_WAITBIT | GORT_PRIOBIT)) | GORT_WRBIT, 1, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) break;
            if (++spins > 100000000u) gort_trap("lock wait too long (two threads upgrade the same lock)");
            if ((spins & 1023) == 0) sched_yield();
        }
        gort_held_push(p, GORT_H_UP);
        return;
    }
    uint32_t v = __atomic_load_n(l, __ATOMIC_RELAXED);
    if (!(v & (GORT_WRBIT | GORT_CNTMASK | GORT_PRIOBIT | GORT_WAITBIT)) && __atomic_compare_exchange_n(l, &v, v | GORT_WRBIT, 1, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) { gort_held_push(p, GORT_H_WRITE); return; }
    gort_lock_slow(p, 1);
    gort_held_push(p, GORT_H_WRITE);
}
static inline void gort_wr_unlock(void *p) {
    unsigned char k = gort_held_pop(p);
    uint32_t *l = &GORT_CTL(p)->lock;
    if (k == GORT_H_WRITE) {
        uint32_t v = __atomic_fetch_and(l, ~GORT_WRBIT, __ATOMIC_RELEASE);
        if (v & GORT_WAITBIT) gort_release_slow(p);
    } else if (k == GORT_H_UP) {                /* back to the plain read it was (one reader: this thread) */
        for (;;) {
            uint32_t v = __atomic_load_n(l, __ATOMIC_RELAXED);
            if (__atomic_compare_exchange_n(l, &v, (v & (GORT_WAITBIT | GORT_PRIOBIT)) | 1, 1, __ATOMIC_RELEASE, __ATOMIC_RELAXED)) break;
        }
    }
}

/* several objects in one block, each with its own mode: lock them in address order, so two threads naming the same objects in different
 * orders cannot deadlock; unlock in the reverse order. `w[i]` is 1 for an object that is written. */
static inline void gort_lock_group(void **a, unsigned char *w, size_t n) {
    for (size_t i = 1; i < n; i++) {
        void *k = a[i]; unsigned char km = w[i]; size_t j = i;
        while (j > 0 && (uintptr_t)a[j - 1] > (uintptr_t)k) { a[j] = a[j - 1]; w[j] = w[j - 1]; j--; }
        a[j] = k; w[j] = km;
    }
    for (size_t i = 0; i < n; i++) { if (w[i]) gort_wr_lock(a[i]); else gort_rd_lock(a[i]); }
}
static inline void gort_unlock_group(void **a, unsigned char *w, size_t n) {
    for (size_t i = n; i-- > 0;) { if (w[i]) gort_wr_unlock(a[i]); else gort_rd_unlock(a[i]); }
}

static inline void *gort_new(size_t size) {
    gort_ctl *c = (gort_ctl *)calloc(1, sizeof(gort_ctl) + size);
    if (!c) gort_trap("out of memory");
    c->strong = 1;
    __atomic_fetch_add(&gort_live, 1, __ATOMIC_RELAXED);
    return c + 1;
}
static inline void *gort_retain(void *p) { if (p) __atomic_fetch_add(&GORT_CTL(p)->strong, 1, __ATOMIC_RELAXED); return p; }
static inline bool gort_release_last(void *p) { return __atomic_sub_fetch(&GORT_CTL(p)->strong, 1, __ATOMIC_ACQ_REL) == 0; }
static inline void gort_free(void *p) { free(GORT_CTL(p)); __atomic_fetch_sub(&gort_live, 1, __ATOMIC_RELAXED); }

/* ---- String: immutable bytes, counted. A literal is permanent: its count is never touched. ---- */
typedef struct gort_str { size_t strong; size_t len; const uint8_t *data; bool perm; } gort_str;

static inline gort_str *gort_str_retain(gort_str *s) { if (!s->perm) __atomic_fetch_add(&s->strong, 1, __ATOMIC_RELAXED); return s; }
static inline void gort_str_release(gort_str *s) {
    if (s->perm) return;
    if (__atomic_sub_fetch(&s->strong, 1, __ATOMIC_ACQ_REL) == 0) { free(s); __atomic_fetch_sub(&gort_live, 1, __ATOMIC_RELAXED); }
}
static gort_str *gort_str_make(const uint8_t *data, size_t len) {
    gort_str *s = (gort_str *)malloc(sizeof(gort_str) + len + 1);
    if (!s) gort_trap("out of memory");
    uint8_t *d = (uint8_t *)(s + 1);
    if (len) memcpy(d, data, len);
    d[len] = 0;
    s->strong = 1; s->len = len; s->data = d; s->perm = false;
    __atomic_fetch_add(&gort_live, 1, __ATOMIC_RELAXED);
    return s;
}
static gort_str *gort_str_concat(const gort_str *a, const gort_str *b) {
    gort_str *s = (gort_str *)malloc(sizeof(gort_str) + a->len + b->len + 1);
    if (!s) gort_trap("out of memory");
    uint8_t *d = (uint8_t *)(s + 1);
    if (a->len) memcpy(d, a->data, a->len);
    if (b->len) memcpy(d + a->len, b->data, b->len);
    d[a->len + b->len] = 0;
    s->strong = 1; s->len = a->len + b->len; s->data = d; s->perm = false;
    __atomic_fetch_add(&gort_live, 1, __ATOMIC_RELAXED);
    return s;
}
static inline bool gort_str_eq(const gort_str *a, const gort_str *b) {
    return a->len == b->len && (a->len == 0 || memcmp(a->data, b->data, a->len) == 0);
}
static gort_str *gort_str_from_i(long long v) { char b[32]; int n = snprintf(b, sizeof b, "%lld", v); return gort_str_make((const uint8_t *)b, (size_t)n); }
static gort_str *gort_str_from_u(unsigned long long v) { char b[32]; int n = snprintf(b, sizeof b, "%llu", v); return gort_str_make((const uint8_t *)b, (size_t)n); }
static gort_str *gort_str_from_f(double v) { char b[40]; int n = snprintf(b, sizeof b, "%g", v); return gort_str_make((const uint8_t *)b, (size_t)n); }
static gort_str *gort_str_from_b(bool v) { return gort_str_make((const uint8_t *)(v ? "true" : "false"), v ? 4 : 5); }

/* ---- Array<T> (slice 0: scalar elements, fixed length): the control block is separate from the element run ---- */
/* a view (slice) has `parent`: the array whose run it shares; it counts the parent, never the run */
typedef struct gort_arr { size_t strong; size_t len; size_t cap; void *data; struct gort_arr *parent; size_t esize; void (*edrop)(void *); } gort_arr;

/* An array header sits after a gort_ctl, so the object lock (`sync (reads xs)`) works on it like on a struct. A view locks its root. */
static gort_arr *gort_arr_alloc(void) {
    gort_ctl *c = (gort_ctl *)calloc(1, sizeof(gort_ctl) + sizeof(gort_arr));
    if (!c) gort_trap("out of memory");
    return (gort_arr *)(c + 1);
}
static inline void *gort_arr_lockobj(gort_arr *a) { return a->parent ? (void *)a->parent : (void *)a; }
static gort_arr *gort_arr_new(size_t esize, size_t len) {
    gort_arr *a = gort_arr_alloc();
    if (len) {
        a->data = calloc(len, esize);
        if (!a->data) gort_trap("out of memory");
    }
    a->strong = 1; a->len = len; a->cap = len; a->esize = esize;
    __atomic_fetch_add(&gort_live, 1, __ATOMIC_RELAXED);
    return a;
}
/* elements that hold counts (an array of optional handles): `edrop` releases one element */
static gort_arr *gort_arr_new_d(size_t esize, size_t len, void (*edrop)(void *)) {
    gort_arr *a = gort_arr_new(esize, len);
    a->edrop = edrop;
    return a;
}
static inline gort_arr *gort_arr_retain(gort_arr *a) { __atomic_fetch_add(&a->strong, 1, __ATOMIC_RELAXED); return a; }
static inline void gort_arr_release(gort_arr *a) {
    if (__atomic_sub_fetch(&a->strong, 1, __ATOMIC_ACQ_REL) == 0) {
        if (a->parent) gort_arr_release(a->parent);
        else {
            if (a->edrop) for (size_t i = 0; i < a->cap; i++) a->edrop((char *)a->data + i * a->esize);
            free(a->data);
        }
        free(GORT_CTL(a)); __atomic_fetch_sub(&gort_live, 1, __ATOMIC_RELAXED);
    }
}
static gort_arr *gort_arr_slice(gort_arr *a, size_t esize, size_t lo, size_t hi) {
    if (lo > hi || hi > a->cap) gort_trap("slice out of range");
    gort_arr *v = gort_arr_alloc();
    gort_arr *root = a->parent ? a->parent : a;
    v->strong = 1; v->len = v->cap = hi - lo; v->data = (char *)a->data + lo * esize; v->parent = gort_arr_retain(root);
    __atomic_fetch_add(&gort_live, 1, __ATOMIC_RELAXED);
    return v;
}
/* growable arrays: the run is replaced by one twice as long (the first growth is to 256); nothing may be viewing it */
static void gort_arr_grow(gort_arr *a) {
    if (a->parent || a->strong > 1) gort_trap("grow while the array is shared or viewed");
    size_t n = a->cap ? a->cap * 2 : 256;
    void *d = realloc(a->data, n * a->esize);
    if (!d) gort_trap("out of memory");
    memset((char *)d + a->cap * a->esize, 0, (n - a->cap) * a->esize);
    a->data = d; a->cap = n;
}
/* appends n scalar elements after the used length, growing the run as needed (not while shared or viewed) */
static void gort_arr_append(gort_arr *a, const void *src, size_t n) {
    if (a->parent || a->strong > 1) gort_trap("append while the array is shared or viewed");
    while (a->len + n > a->cap) gort_arr_grow(a);
    if (n) memcpy((char *)a->data + a->len * a->esize, src, n * a->esize);
    a->len += n;
}
static void gort_arr_move(gort_arr *a, size_t lo, size_t hi, size_t to) {
    if (lo > hi || hi > a->cap || to > a->cap - (hi - lo)) gort_trap("moveWithin out of range");
    memmove((char *)a->data + to * a->esize, (char *)a->data + lo * a->esize, (hi - lo) * a->esize);
}

/* An extern function sees an array as a raw pointer and a length, and gives one back as a `gort_xarr`. What it returns is either (part of) an
 * array it was given, which is then that same array (or a view of it), or memory it allocated with malloc, which is copied into a new array and
 * freed. `args` are the handles that were passed. */
typedef struct { void *p; size_t n; } gort_xarr;
static gort_arr *gort_arr_from_c(gort_xarr r, size_t esize, gort_arr **args, size_t nargs) {
    if (!r.p) {
        if (r.n) gort_trap("extern returned a null array with a length");
        return gort_arr_new(esize, 0);
    }
    for (size_t i = 0; i < nargs; i++) {
        gort_arr *a = args[i];
        char *d = (char *)a->data;
        if (!d || (char *)r.p < d || (char *)r.p > d + a->cap * esize) continue;
        size_t off = (size_t)((char *)r.p - d);
        if (off % esize) gort_trap("extern returned a pointer into an argument array that is not an element boundary");
        size_t lo = off / esize;
        if (lo + r.n > a->cap) gort_trap("extern returned an array that runs past its argument");
        if (lo == 0 && r.n == a->len) return gort_arr_retain(a);
        return gort_arr_slice(a, esize, lo, lo + r.n);
    }
    gort_arr *v = gort_arr_new(esize, r.n);
    if (r.n) memcpy(v->data, r.p, r.n * esize);
    free(r.p);
    return v;
}

/* ---- json.parse<T>: a small strict parser; the compiler writes one decoder per struct on top of these ---- */
typedef struct { const uint8_t *s, *p, *end; long depth, maxdepth, maxstr; char err[96]; } gort_jp;
static int gort_jp_fail(gort_jp *j, const char *m) {
    if (!j->err[0]) snprintf(j->err, sizeof j->err, "%.60s at byte %ld", m, (long)(j->p - j->s));
    return 1;
}
static void gort_jp_init(gort_jp *j, const gort_arr *a, long maxdepth, long maxstr) {
    j->s = j->p = (const uint8_t *)a->data; j->end = j->p + a->len; j->depth = 0; j->maxdepth = maxdepth; j->maxstr = maxstr; j->err[0] = 0;
}
static void gort_jp_ws(gort_jp *j) { while (j->p < j->end && (*j->p == ' ' || *j->p == '\t' || *j->p == '\n' || *j->p == '\r')) j->p++; }
static int gort_jp_peek(gort_jp *j) { gort_jp_ws(j); return j->p < j->end ? *j->p : -1; }
static int gort_jp_expect(gort_jp *j, char c) {
    gort_jp_ws(j);
    if (j->p < j->end && *j->p == (uint8_t)c) { j->p++; return 0; }
    char m[24]; snprintf(m, sizeof m, "expected '%c'", c); return gort_jp_fail(j, m);
}
static int gort_jp_end(gort_jp *j) { gort_jp_ws(j); return j->p == j->end ? 0 : gort_jp_fail(j, "unexpected text after the value"); }
static int gort_jp_isnull(gort_jp *j) {
    gort_jp_ws(j);
    if (j->end - j->p >= 4 && !memcmp(j->p, "null", 4)) { j->p += 4; return 1; }
    return 0;
}
static int gort_jp_num_tok(gort_jp *j, const uint8_t **a, size_t *n) {
    gort_jp_ws(j);
    const uint8_t *q = j->p;
    while (q < j->end && ((*q >= '0' && *q <= '9') || *q == '-' || *q == '+' || *q == '.' || *q == 'e' || *q == 'E')) q++;
    if (q == j->p) return gort_jp_fail(j, "expected a number");
    *a = j->p; *n = (size_t)(q - j->p); j->p = q; return 0;
}
static int gort_jp_int(gort_jp *j, int64_t *out, int64_t lo, int64_t hi) {
    const uint8_t *a; size_t n; if (gort_jp_num_tok(j, &a, &n)) return 1;
    size_t i = 0; int neg = 0; if (a[0] == '-') { neg = 1; i = 1; }
    if (i == n) return gort_jp_fail(j, "bad number");
    int64_t v = 0;
    for (; i < n; i++) {
        if (a[i] < '0' || a[i] > '9') return gort_jp_fail(j, "expected an integer");
        if (__builtin_mul_overflow(v, 10, &v) || __builtin_sub_overflow(v, a[i] - '0', &v)) return gort_jp_fail(j, "integer out of range");
    }
    if (!neg) { if (v == INT64_MIN) return gort_jp_fail(j, "integer out of range"); v = -v; }
    if (v < lo || v > hi) return gort_jp_fail(j, "integer out of range");
    *out = v; return 0;
}
static int gort_jp_uint(gort_jp *j, uint64_t *out, uint64_t hi) {
    const uint8_t *a; size_t n; if (gort_jp_num_tok(j, &a, &n)) return 1;
    uint64_t v = 0;
    for (size_t i = 0; i < n; i++) {
        if (a[i] < '0' || a[i] > '9') return gort_jp_fail(j, "expected an unsigned integer");
        if (__builtin_mul_overflow(v, (uint64_t)10, &v) || __builtin_add_overflow(v, (uint64_t)(a[i] - '0'), &v)) return gort_jp_fail(j, "integer out of range");
    }
    if (v > hi) return gort_jp_fail(j, "integer out of range");
    *out = v; return 0;
}
static int gort_jp_f64(gort_jp *j, double *out) {
    const uint8_t *a; size_t n; if (gort_jp_num_tok(j, &a, &n)) return 1;
    char b[64]; if (n >= sizeof b) return gort_jp_fail(j, "number too long");
    memcpy(b, a, n); b[n] = 0; char *e; *out = strtod(b, &e);
    return *e ? gort_jp_fail(j, "bad number") : 0;
}
static int gort_jp_bool(gort_jp *j, bool *out) {
    gort_jp_ws(j);
    if (j->end - j->p >= 4 && !memcmp(j->p, "true", 4)) { j->p += 4; *out = true; return 0; }
    if (j->end - j->p >= 5 && !memcmp(j->p, "false", 5)) { j->p += 5; *out = false; return 0; }
    return gort_jp_fail(j, "expected true or false");
}
static int gort_jp_hex4(gort_jp *j, unsigned *u) {
    if (j->end - j->p < 4) return gort_jp_fail(j, "bad \\u escape");
    unsigned v = 0;
    for (int i = 0; i < 4; i++) { int c = j->p[i]; v <<= 4;
        if (c >= '0' && c <= '9') v |= c - '0'; else if (c >= 'a' && c <= 'f') v |= c - 'a' + 10; else if (c >= 'A' && c <= 'F') v |= c - 'A' + 10; else return gort_jp_fail(j, "bad \\u escape"); }
    j->p += 4; *u = v; return 0;
}
static int gort_jp_str(gort_jp *j, gort_str **out) {
    if (gort_jp_expect(j, '"')) return 1;
    size_t cap = 16, n = 0; uint8_t *b = (uint8_t *)malloc(cap);
    if (!b) gort_trap("out of memory");
    for (;;) {
        if (j->p >= j->end) { free(b); return gort_jp_fail(j, "unterminated string"); }
        uint8_t c = *j->p++;
        if (c == '"') break;
        if (c < 0x20) { free(b); return gort_jp_fail(j, "control character in string"); }
        uint8_t tmp[4]; size_t tn = 0;
        if (c == '\\') {
            if (j->p >= j->end) { free(b); return gort_jp_fail(j, "unterminated string"); }
            uint8_t e = *j->p++;
            switch (e) {
                case '"': case '\\': case '/': tmp[tn++] = e; break;
                case 'b': tmp[tn++] = 8; break; case 'f': tmp[tn++] = 12; break; case 'n': tmp[tn++] = 10; break;
                case 'r': tmp[tn++] = 13; break; case 't': tmp[tn++] = 9; break;
                case 'u': {
                    unsigned u; if (gort_jp_hex4(j, &u)) { free(b); return 1; }
                    if (u >= 0xD800 && u < 0xDC00 && j->end - j->p >= 6 && j->p[0] == '\\' && j->p[1] == 'u') {
                        unsigned lo; j->p += 2; if (gort_jp_hex4(j, &lo)) { free(b); return 1; }
                        u = 0x10000 + ((u - 0xD800) << 10) + (lo - 0xDC00);
                    }
                    if (u < 0x80) tmp[tn++] = (uint8_t)u;
                    else if (u < 0x800) { tmp[tn++] = 0xC0 | (u >> 6); tmp[tn++] = 0x80 | (u & 0x3F); }
                    else if (u < 0x10000) { tmp[tn++] = 0xE0 | (u >> 12); tmp[tn++] = 0x80 | ((u >> 6) & 0x3F); tmp[tn++] = 0x80 | (u & 0x3F); }
                    else { tmp[tn++] = 0xF0 | (u >> 18); tmp[tn++] = 0x80 | ((u >> 12) & 0x3F); tmp[tn++] = 0x80 | ((u >> 6) & 0x3F); tmp[tn++] = 0x80 | (u & 0x3F); }
                    break; }
                default: free(b); return gort_jp_fail(j, "bad escape");
            }
        } else tmp[tn++] = c;
        if (n + tn > (size_t)j->maxstr) { free(b); return gort_jp_fail(j, "string too long"); }
        if (n + tn > cap) { cap *= 2; b = (uint8_t *)realloc(b, cap); if (!b) gort_trap("out of memory"); }
        memcpy(b + n, tmp, tn); n += tn;
    }
    *out = gort_str_make(b, n); free(b); return 0;
}
/* an object key, left in place: no escapes are interpreted, a key with a backslash never matches a field */
static int gort_jp_key(gort_jp *j, const uint8_t **k, size_t *n) {
    if (gort_jp_expect(j, '"')) return 1;
    const uint8_t *q = j->p;
    while (q < j->end && *q != '"') { if (*q == '\\') q++; q++; }
    if (q >= j->end) return gort_jp_fail(j, "unterminated string");
    *k = j->p; *n = (size_t)(q - j->p); j->p = q + 1; return 0;
}
static int gort_jp_skip(gort_jp *j) {
    int c = gort_jp_peek(j);
    if (c == '"') { const uint8_t *k; size_t n; return gort_jp_key(j, &k, &n); }
    if (c == '{' || c == '[') {
        char close = c == '{' ? '}' : ']'; j->p++;
        if (++j->depth > j->maxdepth) return gort_jp_fail(j, "nesting too deep");
        if (gort_jp_peek(j) == close) { j->p++; j->depth--; return 0; }
        for (;;) {
            if (c == '{') { const uint8_t *k; size_t n; if (gort_jp_key(j, &k, &n) || gort_jp_expect(j, ':')) return 1; }
            if (gort_jp_skip(j)) return 1;
            int d = gort_jp_peek(j);
            if (d == ',') { j->p++; continue; }
            if (d == close) { j->p++; j->depth--; return 0; }
            return gort_jp_fail(j, "expected ',' or a closing bracket");
        }
    }
    if (c == 't' || c == 'f') { bool b; return gort_jp_bool(j, &b); }
    if (c == 'n') return gort_jp_isnull(j) ? 0 : gort_jp_fail(j, "bad value");
    const uint8_t *a; size_t n; return gort_jp_num_tok(j, &a, &n);
}

/* ---- hash(x) for Map keys ---- */
static uint64_t gort_hash_u(uint64_t x) { x += 0x9e3779b97f4a7c15ULL; x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ULL; x = (x ^ (x >> 27)) * 0x94d049bb133111ebULL; return x ^ (x >> 31); }
static uint64_t gort_hash_str(const gort_str *s) { uint64_t h = 1469598103934665603ULL; for (size_t i = 0; i < s->len; i++) { h ^= s->data[i]; h *= 1099511628211ULL; } return gort_hash_u(h); }

static long gort_mem_find(const uint8_t *h, size_t hn, const uint8_t *n, size_t nn) {
    if (nn == 0) return 0;
    if (nn > hn) return -1;
    for (size_t i = 0; i + nn <= hn; i++) if (h[i] == n[0] && memcmp(h + i, n, nn) == 0) return (long)i;
    return -1;
}

/* ---- output ---- */
static void gort_print_str(const gort_str *s) { fwrite(s->data, 1, s->len, stdout); }
static void gort_print_i(long long v) { printf("%lld", v); }
static void gort_print_u(unsigned long long v) { printf("%llu", v); }
static void gort_print_f(double v) { printf("%g", v); }
static void gort_print_b(bool v) { fputs(v ? "true" : "false", stdout); }
static void gort_nl(void) { putchar('\n'); }
/* ---- time: Duration and Instant are signed nanoseconds ---- */
#include <time.h>
#include <errno.h>
static int64_t gort_now_ns(void) { struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts); return (int64_t)ts.tv_sec * 1000000000LL + ts.tv_nsec; }
#endif

/* the exception in flight: set by `throw`, taken by a matching `catch` */
typedef struct { void* obj; int id; const char* name; void (*rel)(void*); } gort_exc_t;
static gort_exc_t gort_exc;
static void gort_exc_release(gort_exc_t* e) {
    if (e->obj) { e->rel(e->obj); e->obj = NULL; }
}

/* optionals: a null handle, or a flag test that traps */
static void* gort_nn(void* p) { if (!p) gort_trap("null value used"); return p; }
static void gort_has(bool h) { if (!h) gort_trap("null value used"); }

/* ---- command line: `main(cfg: Config)` fills the fields of cfg from --name=value or --name value ---- */
static void gort_arg_fail(const char *a, size_t n, const char *msg, const char *usage) {
    fprintf(stderr, "gort: --%.*s: %s\n%s", (int)n, a, msg, usage);
    exit(2);
}
static unsigned long long gort_arg_int(const char *a, size_t n, const char *v, int is_signed, int bits, const char *usage) {
    if (!v || !*v) gort_arg_fail(a, n, "needs a value", usage);
    char *end; errno = 0;
    if (is_signed) {
        long long x = strtoll(v, &end, 10);
        if (*end || errno || (bits < 64 && (x < -(1LL << (bits - 1)) || x >= (1LL << (bits - 1))))) gort_arg_fail(a, n, "not a number in range", usage);
        return (unsigned long long)x;
    }
    if (*v == '-') gort_arg_fail(a, n, "not a number in range", usage);
    unsigned long long x = strtoull(v, &end, 10);
    if (*end || errno || (bits < 64 && x >= (1ULL << bits))) gort_arg_fail(a, n, "not a number in range", usage);
    return x;
}
static double gort_arg_f64(const char *a, size_t n, const char *v, const char *usage) {
    if (!v || !*v) gort_arg_fail(a, n, "needs a value", usage);
    char *end; double x = strtod(v, &end);
    if (*end) gort_arg_fail(a, n, "not a number", usage);
    return x;
}
static bool gort_arg_bool(const char *a, size_t n, const char *v, const char *usage) {
    if (!v || !strcmp(v, "true")) return true;
    if (!strcmp(v, "false")) return false;
    gort_arg_fail(a, n, "expected true or false", usage);
    return false;
}

/* ---- the runtime's view (see rt/posix.c): non-static entry points for a separately compiled C runtime ---- */
void *gort_rt_new(size_t n) { return gort_new(n); }                       /* a counted object of n bytes, for an extern struct */
void *gort_rt_arr_data(gort_arr *a) { return a->data; }
size_t gort_rt_arr_len(gort_arr *a) { return a->len; }
size_t gort_rt_arr_cap(gort_arr *a) { return a->cap; }
void gort_rt_arr_release(gort_arr *a) { gort_arr_release(a); }              /* an array argument arrives owned: the extern function releases it */
void gort_rt_exit_hook(void) __attribute__((weak));                        /* run after main returns: the runtime joins its threads here */
