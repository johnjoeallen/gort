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
#include <stdlib.h>
#include <string.h>

typedef struct { size_t strong; size_t weak; } gort_ctl;
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

static gort_arr *gort_arr_new(size_t esize, size_t len) {
    gort_arr *a = (gort_arr *)calloc(1, sizeof(gort_arr));
    if (!a) gort_trap("out of memory");
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
        free(a); __atomic_fetch_sub(&gort_live, 1, __ATOMIC_RELAXED);
    }
}
static gort_arr *gort_arr_slice(gort_arr *a, size_t esize, size_t lo, size_t hi) {
    if (lo > hi || hi > a->cap) gort_trap("slice out of range");
    gort_arr *v = (gort_arr *)calloc(1, sizeof(gort_arr));
    if (!v) gort_trap("out of memory");
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
