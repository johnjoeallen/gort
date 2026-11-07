#include <stdint.h>
#include <stdlib.h>
#include <string.h>
typedef struct { void *p; size_t n; } gort_xarr;
typedef struct { int64_t x, y; } Pt;
gort_xarr gort_x_same(void *p, size_t n) { return (gort_xarr){ p, n }; }
gort_xarr gort_x_firstHalf(void *p, size_t n) { return (gort_xarr){ p, n / 2 }; }
gort_xarr gort_x_dropFirst(void *p, size_t n) { return (gort_xarr){ (int64_t *)p + 1, n - 1 }; }
gort_xarr gort_x_doubled(void *p, size_t n) {
    int64_t *r = malloc(2 * n * sizeof(int64_t));
    memcpy(r, p, n * sizeof(int64_t));
    memcpy(r + n, p, n * sizeof(int64_t));
    return (gort_xarr){ r, 2 * n };
}
gort_xarr gort_x_points(size_t n) {
    Pt *r = malloc(n * sizeof(Pt));
    for (size_t i = 0; i < n; i++) r[i] = (Pt){ (int64_t)i, (int64_t)(10 * i) };
    return (gort_xarr){ r, n };
}
int64_t gort_x_sumBytes(void *p, size_t n) { int64_t s = 0; for (size_t i = 0; i < n; i++) s += ((uint8_t *)p)[i]; return s; }
