/* feed/native.c - the C side of the feed server's `extern` declarations (db.gort): SQLite, HMAC-SHA256, base64url, a JSON syntax check, a clock.
 * SQLite is declared by hand and linked as libsqlite3.so.0, so no development package is needed. One connection, a table of prepared
 * statements; none of these calls parks, so on one worker a request runs its statements start to finish. */
#include <stdint.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct { void *p; size_t n; } gort_xarr;
typedef struct sqlite3 sqlite3;
typedef struct sqlite3_stmt sqlite3_stmt;
extern int sqlite3_open_v2(const char *, sqlite3 **, int, const char *);
extern int sqlite3_exec(sqlite3 *, const char *, void *, void *, char **);
extern int sqlite3_prepare_v2(sqlite3 *, const char *, int, sqlite3_stmt **, const char **);
extern int sqlite3_bind_int64(sqlite3_stmt *, int, int64_t);
extern int sqlite3_bind_text(sqlite3_stmt *, int, const char *, int, void (*)(void *));
extern int sqlite3_step(sqlite3_stmt *);
extern int sqlite3_reset(sqlite3_stmt *);
extern int sqlite3_clear_bindings(sqlite3_stmt *);
extern int64_t sqlite3_column_int64(sqlite3_stmt *, int);
extern const unsigned char *sqlite3_column_text(sqlite3_stmt *, int);
extern int sqlite3_column_bytes(sqlite3_stmt *, int);
extern int sqlite3_changes(sqlite3 *);
extern const char *sqlite3_errmsg(sqlite3 *);
extern int sqlite3_busy_timeout(sqlite3 *, int);
#define TRANSIENT ((void (*)(void *))-1)

static sqlite3 *db;
#define MAXSTMT 32
static sqlite3_stmt *stmts[MAXSTMT];
static int nstmts;
static time_t started;

static char *cstr(void *p, size_t n) { char *s = malloc(n + 1); memcpy(s, p, n); s[n] = 0; return s; }
static gort_xarr copy_out(const void *p, size_t n) {
    if (!p || n == 0) return (gort_xarr){ NULL, 0 };
    void *r = malloc(n); memcpy(r, p, n); return (gort_xarr){ r, n };
}

/* ---- SQLite ---- */
int32_t gort_x_dbOpen(void *path, size_t path_len) {
    char *s = cstr(path, path_len);
    int rc = sqlite3_open_v2(s, &db, 0x2 /* READWRITE */ | 0x8000 /* FULLMUTEX */, NULL);
    free(s);
    if (rc != 0) return rc;
    sqlite3_busy_timeout(db, 5000);
    started = time(NULL);
    /* WAL with synchronous=NORMAL: the challenge's rule 6 */
    return sqlite3_exec(db, "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA mmap_size=268435456; PRAGMA temp_store=MEMORY;", NULL, NULL, NULL);
}
int32_t gort_x_dbPrepare(void *sql, size_t n) {
    if (nstmts >= MAXSTMT) return -1;
    char *s = cstr(sql, n);
    int rc = sqlite3_prepare_v2(db, s, -1, &stmts[nstmts], NULL);
    free(s);
    return rc == 0 ? nstmts++ : -1;
}
int32_t gort_x_dbExec(void *sql, size_t n) { char *s = cstr(sql, n); int rc = sqlite3_exec(db, s, NULL, NULL, NULL); free(s); return rc; }
void gort_x_dbReset(int32_t id) { sqlite3_reset(stmts[id]); sqlite3_clear_bindings(stmts[id]); }
void gort_x_dbBindInt(int32_t id, int32_t i, int64_t v) { sqlite3_bind_int64(stmts[id], i, v); }
void gort_x_dbBindText(int32_t id, int32_t i, void *p, size_t n) { sqlite3_bind_text(stmts[id], i, p ? (const char *)p : "", (int)n, TRANSIENT); }
int32_t gort_x_dbStep(int32_t id) { return sqlite3_step(stmts[id]); }          /* 100 row, 101 done */
int64_t gort_x_dbInt(int32_t id, int32_t col) { return sqlite3_column_int64(stmts[id], col); }
gort_xarr gort_x_dbText(int32_t id, int32_t col) {
    const unsigned char *t = sqlite3_column_text(stmts[id], col);
    return copy_out(t, (size_t)sqlite3_column_bytes(stmts[id], col));
}
int64_t gort_x_dbChanges(void) { return sqlite3_changes(db); }
gort_xarr gort_x_dbError(void) { const char *m = sqlite3_errmsg(db); return copy_out(m, strlen(m)); }

/* ---- the web page (ui.html, turned into ui.inc by build.sh) ---- */
static const char UI[] =
#include "ui.inc"
;
gort_xarr gort_x_uiHtml(void) { return copy_out(UI, sizeof UI - 1); }

/* ---- clock ---- */
int64_t gort_x_nowSec(void) { return (int64_t)time(NULL); }
int64_t gort_x_uptimeSec(void) { return (int64_t)(time(NULL) - started); }

/* ---- SHA-256, HMAC, base64url ---- */
typedef struct { uint32_t h[8]; uint8_t buf[64]; uint64_t len; size_t n; } sha;
static const uint32_t K[64] = {
 0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
 0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
 0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
 0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2 };
#define ROR(x, n) (((x) >> (n)) | ((x) << (32 - (n))))
static void sha_block(sha *s, const uint8_t *p) {
    uint32_t w[64], a, b, c, d, e, f, g, h;
    for (int i = 0; i < 16; i++) w[i] = (uint32_t)p[4*i] << 24 | (uint32_t)p[4*i+1] << 16 | (uint32_t)p[4*i+2] << 8 | p[4*i+3];
    for (int i = 16; i < 64; i++) {
        uint32_t s0 = ROR(w[i-15], 7) ^ ROR(w[i-15], 18) ^ (w[i-15] >> 3), s1 = ROR(w[i-2], 17) ^ ROR(w[i-2], 19) ^ (w[i-2] >> 10);
        w[i] = w[i-16] + s0 + w[i-7] + s1;
    }
    a = s->h[0]; b = s->h[1]; c = s->h[2]; d = s->h[3]; e = s->h[4]; f = s->h[5]; g = s->h[6]; h = s->h[7];
    for (int i = 0; i < 64; i++) {
        uint32_t t1 = h + (ROR(e, 6) ^ ROR(e, 11) ^ ROR(e, 25)) + ((e & f) ^ (~e & g)) + K[i] + w[i];
        uint32_t t2 = (ROR(a, 2) ^ ROR(a, 13) ^ ROR(a, 22)) + ((a & b) ^ (a & c) ^ (b & c));
        h = g; g = f; f = e; e = d + t1; d = c; c = b; b = a; a = t1 + t2;
    }
    s->h[0] += a; s->h[1] += b; s->h[2] += c; s->h[3] += d; s->h[4] += e; s->h[5] += f; s->h[6] += g; s->h[7] += h;
}
static void sha_init(sha *s) {
    static const uint32_t i[8] = { 0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19 };
    memcpy(s->h, i, sizeof i); s->len = 0; s->n = 0;
}
static void sha_add(sha *s, const uint8_t *p, size_t n) {
    s->len += n;
    while (n) {
        size_t k = 64 - s->n < n ? 64 - s->n : n;
        memcpy(s->buf + s->n, p, k); s->n += k; p += k; n -= k;
        if (s->n == 64) { sha_block(s, s->buf); s->n = 0; }
    }
}
static void sha_end(sha *s, uint8_t out[32]) {
    uint64_t bits = s->len * 8;
    uint8_t pad = 0x80; sha_add(s, &pad, 1);
    pad = 0; while (s->n != 56) sha_add(s, &pad, 1);
    uint8_t l[8]; for (int i = 0; i < 8; i++) l[i] = (uint8_t)(bits >> (56 - 8 * i));
    sha_add(s, l, 8);
    for (int i = 0; i < 8; i++) { out[4*i] = s->h[i] >> 24; out[4*i+1] = s->h[i] >> 16; out[4*i+2] = s->h[i] >> 8; out[4*i+3] = s->h[i]; }
}
static void hmac(const uint8_t *key, size_t kn, const uint8_t *msg, size_t mn, uint8_t out[32]) {
    uint8_t k[64] = {0}, ipad[64], opad[64], inner[32]; sha s;
    if (kn > 64) { sha_init(&s); sha_add(&s, key, kn); sha_end(&s, k); } else memcpy(k, key, kn);
    for (int i = 0; i < 64; i++) { ipad[i] = k[i] ^ 0x36; opad[i] = k[i] ^ 0x5c; }
    sha_init(&s); sha_add(&s, ipad, 64); sha_add(&s, msg, mn); sha_end(&s, inner);
    sha_init(&s); sha_add(&s, opad, 64); sha_add(&s, inner, 32); sha_end(&s, out);
}
static const char B64[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
/* the HMAC-SHA256 of msg under key, as unpadded base64url */
gort_xarr gort_x_hmacB64(void *msg, size_t mn, void *key, size_t kn) {
    uint8_t d[32]; hmac(key, kn, msg, mn, d);
    char o[44]; size_t n = 0;
    for (int i = 0; i < 32; i += 3) {
        uint32_t v = (uint32_t)d[i] << 16 | (i + 1 < 32 ? (uint32_t)d[i+1] << 8 : 0) | (i + 2 < 32 ? d[i+2] : 0);
        o[n++] = B64[v >> 18]; o[n++] = B64[(v >> 12) & 63];
        if (i + 1 < 32) o[n++] = B64[(v >> 6) & 63];
        if (i + 2 < 32) o[n++] = B64[v & 63];
    }
    return copy_out(o, n);
}
/* unpadded base64url to bytes; empty if the input is not valid base64url */
gort_xarr gort_x_b64Decode(void *p, size_t n) {
    const uint8_t *s = p; uint8_t *o = malloc(n + 3); size_t m = 0; uint32_t acc = 0; int bits = 0;
    if (n % 4 == 1) { free(o); return (gort_xarr){ NULL, 0 }; }
    for (size_t i = 0; i < n; i++) {
        int v = s[i] >= 'A' && s[i] <= 'Z' ? s[i] - 'A' : s[i] >= 'a' && s[i] <= 'z' ? s[i] - 'a' + 26 : s[i] >= '0' && s[i] <= '9' ? s[i] - '0' + 52 : s[i] == '-' ? 62 : s[i] == '_' ? 63 : -1;
        if (v < 0) { free(o); return (gort_xarr){ NULL, 0 }; }
        acc = acc << 6 | (uint32_t)v; bits += 6;
        if (bits >= 8) { bits -= 8; o[m++] = (uint8_t)(acc >> bits); }
    }
    gort_xarr r = copy_out(o, m); free(o); return r;
}

/* ---- is this syntactically valid JSON? (a type mismatch is not a syntax error, and the spec tells them apart) ---- */
typedef struct { const uint8_t *p, *e; int depth; } jv;
static void jws(jv *j) { while (j->p < j->e && (*j->p == ' ' || *j->p == '\t' || *j->p == '\n' || *j->p == '\r')) j->p++; }
static int jval(jv *j);
static int jstr(jv *j) {
    if (j->p >= j->e || *j->p != '"') return 0;
    j->p++;
    while (j->p < j->e) {
        uint8_t c = *j->p++;
        if (c == '"') return 1;
        if (c < 0x20) return 0;
        if (c == '\\') {
            if (j->p >= j->e) return 0;
            uint8_t d = *j->p++;
            if (d == 'u') { for (int i = 0; i < 4; i++) { if (j->p >= j->e || !((*j->p >= '0' && *j->p <= '9') || (*j->p >= 'a' && *j->p <= 'f') || (*j->p >= 'A' && *j->p <= 'F'))) return 0; j->p++; } }
            else if (!strchr("\"\\/bfnrt", d)) return 0;
        }
    }
    return 0;
}
static int jnum(jv *j) {
    const uint8_t *s = j->p;
    if (j->p < j->e && *j->p == '-') j->p++;
    if (j->p >= j->e) return 0;
    if (*j->p == '0') j->p++; else if (*j->p >= '1' && *j->p <= '9') while (j->p < j->e && *j->p >= '0' && *j->p <= '9') j->p++; else return 0;
    if (j->p < j->e && *j->p == '.') { j->p++; const uint8_t *d = j->p; while (j->p < j->e && *j->p >= '0' && *j->p <= '9') j->p++; if (j->p == d) return 0; }
    if (j->p < j->e && (*j->p == 'e' || *j->p == 'E')) { j->p++; if (j->p < j->e && (*j->p == '+' || *j->p == '-')) j->p++; const uint8_t *d = j->p; while (j->p < j->e && *j->p >= '0' && *j->p <= '9') j->p++; if (j->p == d) return 0; }
    return j->p > s;
}
static int jlit(jv *j, const char *w) { size_t n = strlen(w); if ((size_t)(j->e - j->p) < n || memcmp(j->p, w, n)) return 0; j->p += n; return 1; }
static int jval(jv *j) {
    jws(j);
    if (j->p >= j->e || j->depth > 64) return 0;
    switch (*j->p) {
    case '"': return jstr(j);
    case 't': return jlit(j, "true");
    case 'f': return jlit(j, "false");
    case 'n': return jlit(j, "null");
    case '{': {
        j->p++; j->depth++; jws(j);
        if (j->p < j->e && *j->p == '}') { j->p++; j->depth--; return 1; }
        for (;;) {
            jws(j);
            if (!jstr(j)) return 0;
            jws(j);
            if (j->p >= j->e || *j->p++ != ':') return 0;
            if (!jval(j)) return 0;
            jws(j);
            if (j->p >= j->e) return 0;
            if (*j->p == ',') { j->p++; continue; }
            if (*j->p == '}') { j->p++; j->depth--; return 1; }
            return 0;
        }
    }
    case '[': {
        j->p++; j->depth++; jws(j);
        if (j->p < j->e && *j->p == ']') { j->p++; j->depth--; return 1; }
        for (;;) {
            if (!jval(j)) return 0;
            jws(j);
            if (j->p >= j->e) return 0;
            if (*j->p == ',') { j->p++; continue; }
            if (*j->p == ']') { j->p++; j->depth--; return 1; }
            return 0;
        }
    }
    default: return jnum(j);
    }
}
bool gort_x_jsonOk(void *p, size_t n) { jv j = { p, (const uint8_t *)p + n, 0 }; if (!jval(&j)) return 0; jws(&j); return j.p == j.e; }

/* ---- no bitwise operators in the language: the constant-time compare and the character count live here ---- */
bool gort_x_sameBytes(void *a, size_t an, void *b, size_t bn) {
    if (an != bn) return 0;
    uint8_t d = 0;
    for (size_t i = 0; i < an; i++) d |= ((uint8_t *)a)[i] ^ ((uint8_t *)b)[i];
    return d == 0;
}
int64_t gort_x_utf8Chars(void *p, size_t n) { int64_t c = 0; for (size_t i = 0; i < n; i++) if ((((uint8_t *)p)[i] & 0xC0) != 0x80) c++; return c; }
