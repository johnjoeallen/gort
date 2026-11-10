// rc.h - the Gort memory model, hand-emulated. Shared by rcm.c (W1-W4) and httpd.c (W5).
#pragma once
#include <stdint.h>
#include <stdlib.h>
#include <stdio.h>
#ifndef MODE
#define MODE 2
#endif
#ifndef VARIANT
#define VARIANT "C2-flagged"
#endif
#ifndef IMPL
#define IMPL "c-O2"
#endif
#define NOINLINE __attribute__((noinline))
#define AINLINE static inline __attribute__((always_inline))
#define LIKELY(x) __builtin_expect(!!(x), 1)
#define UNLIKELY(x) __builtin_expect(!!(x), 0)

// ------------------------------------------------------------------ object header + counting
typedef struct Obj { uint32_t rc, weak; uint8_t shared, type; uint8_t pad[6]; } Obj;  // 16 bytes
enum { T_NODE = 1, T_ENTRY, T_BUF, T_ARR, T_REQ, T_RESP, T_CTX, T_SESS };

#ifdef COUNT
static __thread uint64_t c_inc, c_atomic, c_dec;
#define CNT(x) x
#else
#define CNT(x)
#endif
static __thread uint64_t t_allocs, t_frees;   // identical bookkeeping in every variant (leak check, allocs/request)

// Compile-time barrier on the NON-atomic paths: gcc would otherwise cancel adjacent inc/dec pairs (it cannot do so for atomics),
// which would hand C1/C2 an elision that C3 does not get. Emits no instructions.
#define CBARRIER() __asm__ volatile("" ::: "memory")
#if MODE != 0
AINLINE void rc_inc(Obj *o) {
  CNT(c_inc++);
#if MODE == 1
  o->rc++; CBARRIER();
#elif MODE == 2 || MODE == 4
  if (UNLIKELY(o->shared)) { CNT(c_atomic++); __atomic_fetch_add(&o->rc, 1, __ATOMIC_RELAXED); }
  else { o->rc++; CBARRIER(); }
#else
  CNT(c_atomic++); __atomic_fetch_add(&o->rc, 1, __ATOMIC_RELAXED);
#endif
}
AINLINE int rc_dec(Obj *o) {   // returns 1 when the count reached zero
  CNT(c_dec++);
#if MODE == 1
  CBARRIER(); return --o->rc == 0;
#elif MODE == 2 || MODE == 4
  if (UNLIKELY(o->shared)) {
    if (__atomic_fetch_sub(&o->rc, 1, __ATOMIC_RELEASE) == 1) { __atomic_thread_fence(__ATOMIC_ACQUIRE); return 1; }
    return 0;
  }
  CBARRIER(); return --o->rc == 0;
#else
  if (__atomic_fetch_sub(&o->rc, 1, __ATOMIC_RELEASE) == 1) { __atomic_thread_fence(__ATOMIC_ACQUIRE); return 1; }
  return 0;
#endif
}
#endif

static void obj_free(Obj *o);
#if MODE == 0
#define PASS(x) (x)
#define DROP(x) ((void)0)           // a non-owning reference: nothing to do
#define OWN_DROP(x) obj_free(&(x)->h)   // the owner frees explicitly
#define CLONE_IF_COUNTED(o) ((void)0)
#else
AINLINE void rc_release(Obj *o) { if (rc_dec(o)) obj_free(o); }
#define PASS(x) (rc_inc(&(x)->h), (x))
#define DROP(x) rc_release(&(x)->h)
#define OWN_DROP(x) rc_release(&(x)->h)
#define CLONE_IF_COUNTED(o) rc_inc(o)
#endif
// Elision sites (MODE 4 only). PASS_B/DROP_B = borrow into a callee / out of a field because the
// caller provably outlives the callee. PASS_L/DROP_L = move on last use (no inc/dec pair).
#if MODE == 4
#define PASS_B(x) (x)
#define DROP_B(x) ((void)0)
#define PASS_L(x) (x)
#define DROP_L(x) ((void)0)
#else
#define PASS_B(x) PASS(x)
#define DROP_B(x) DROP(x)
#define PASS_L(x) PASS(x)
#define DROP_L(x) DROP(x)
#endif

// weak refs (only Node uses them): weak starts at 1 (the strong refs' implicit weak), memory is freed when it hits 0
static int g_weak;
AINLINE void weak_inc(Obj *o) { if (o->shared) __atomic_fetch_add(&o->weak, 1, __ATOMIC_RELAXED); else o->weak++; }
AINLINE int weak_dec(Obj *o) {
  if (o->shared) return __atomic_fetch_sub(&o->weak, 1, __ATOMIC_ACQ_REL) == 1;
  return --o->weak == 0;
}

static void *oalloc(size_t sz, int type) {
  Obj *o = malloc(sz);
  if (!o) { perror("malloc"); exit(1); }
  o->rc = 1; o->weak = 1; o->shared = 0; o->type = (uint8_t)type;
  t_allocs++;
  return o;
}

