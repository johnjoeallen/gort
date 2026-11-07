/* rt/posix.c - a small POSIX runtime for the `extern` declarations of gort/runtime/leaves.gort (what http-mini needs).
 *
 * Each worker is an OS thread running a scheduler: `spawn` makes a vthread (a coroutine on its own stack, kept on that worker),
 * and `accept`, `recv`, `send` and `waitReadable` PARK the vthread on an epoll set while the socket is not ready, so one worker
 * serves many connections. Sockets are non-blocking. Reference counts are atomic. Outside a worker (the main thread) the same
 * calls fall back to blocking in poll(), and `spawn` falls back to an OS thread.
 *
 * Link it with the program:  gortc run --link rt/posix.c gort/runtime and gort/http-mini
 */
#define _GNU_SOURCE
#include <errno.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/mman.h>
#include <sys/timerfd.h>
#include <ucontext.h>
#include <poll.h>
#include <pthread.h>
#include <sched.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <time.h>
#include <unistd.h>

/* the generated program's side (gort_rt0.h and codegen) */
typedef struct gort_arr gort_arr;
typedef struct { void *p; size_t n; } gort_xarr;      /* an array given back by an extern function: pointer and length */
typedef struct G_Conn G_Conn;
typedef struct G_Listener G_Listener;
typedef struct { char _empty; } G_IoError;
void *gort_rt_new(size_t n);
void *gort_rt_arr_data(gort_arr *a);
size_t gort_rt_arr_len(gort_arr *a);
void gort_rt_arr_release(gort_arr *a);
int gort_rt_throw_io(void);
void gort_rt_exit_hook(void);

/* the objects behind the handles: the compiler's `char _empty` is the first byte of these */
struct rt_conn { int fd; };
struct rt_listener { int fd; };

static G_Conn *conn_new(int fd) { struct rt_conn *c = gort_rt_new(sizeof *c); c->fd = fd; return (G_Conn *)c; }

/* ---- the scheduler: one per worker thread ---- */
struct vt {
    ucontext_t ctx; void *stack; void (*body)(void *); void *env;
    struct vt *next; bool done, woken;
    struct sched *home;          /* the worker this vthread lives on */
    int pstate;                  /* lock parking: 0 running, 1 parked, 2 a wake arrived before the park */
    struct vt *inext;            /* link in the home worker's inbox */
};
struct sched {
    int ep; struct vt *head, *tail, *cur; size_t nlive, nwait; ucontext_t home;
    /* vthreads woken from another thread (a lock handed over): they wait in the inbox until this worker drains it; the eventfd wakes epoll_wait */
    pthread_mutex_t inbox_m; struct vt *inbox; int evfd; size_t nparked;
};
static __thread struct sched *S;
#define STACK_SIZE (256 * 1024)

static void ready(struct vt *v) { v->next = NULL; if (S->tail) S->tail->next = v; else S->head = v; S->tail = v; }
static void trampoline(void) { struct vt *v = S->cur; v->body(v->env); v->done = true; }   /* returning resumes S->home (uc_link) */
static void vt_new(void (*body)(void *), void *env) {
    struct vt *v = calloc(1, sizeof *v);
    v->stack = mmap(NULL, STACK_SIZE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK, -1, 0);
    if (v->stack == MAP_FAILED) { perror("mmap"); exit(2); }
    v->body = body; v->env = env; v->home = S;
    getcontext(&v->ctx);
    v->ctx.uc_stack.ss_sp = v->stack; v->ctx.uc_stack.ss_size = STACK_SIZE; v->ctx.uc_link = &S->home;
    makecontext(&v->ctx, trampoline, 0);
    S->nlive++; ready(v);
}
static void drain_inbox(void) {
    pthread_mutex_lock(&S->inbox_m);
    struct vt *v = S->inbox; S->inbox = NULL;
    pthread_mutex_unlock(&S->inbox_m);
    while (v) { struct vt *n = v->inext; S->nparked--; ready(v); v = n; }   /* newest first: order among simultaneous wakes does not matter */
}
static void sched_run(void) {
    struct epoll_event evs[64];
    for (;;) {
        struct vt *v;
        drain_inbox();
        while ((v = S->head)) {
            S->head = v->next; if (!S->head) S->tail = NULL;
            S->cur = v; swapcontext(&S->home, &v->ctx); S->cur = NULL;
            if (v->done) { munmap(v->stack, STACK_SIZE); free(v); S->nlive--; }
            drain_inbox();
        }
        if (!S->nlive) break;
        if (S->head) continue;
        if (!S->nwait && !S->nparked) break;
        int n = epoll_wait(S->ep, evs, 64, -1);
        for (int i = 0; i < n; i++) {
            struct vt *w = evs[i].data.ptr;
            if (!w) { uint64_t c; (void)!read(S->evfd, &c, sizeof c); continue; }       /* the inbox doorbell */
            if (!w->woken) { w->woken = true; S->nwait--; ready(w); }
        }
    }
}

/* ---- parking on a lock (called by the generated program's sync blocks, gort_rt0.h) ---- */
void *gort_rt_vt_self(void) { return S ? S->cur : NULL; }
void gort_rt_vt_park(void *tok) {
    struct vt *v = tok;
    int exp = 0;
    if (__atomic_compare_exchange_n(&v->pstate, &exp, 1, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
        S->nparked++;
        swapcontext(&v->ctx, &S->home);                    /* the scheduler runs others until an unpark moves us from the inbox to the ready queue */
    }
    __atomic_store_n(&v->pstate, 0, __ATOMIC_RELEASE);     /* (state 2: the wake came first, so the park returns at once) */
}
void gort_rt_vt_unpark(void *tok) {
    struct vt *v = tok;
    for (;;) {
        int s = __atomic_load_n(&v->pstate, __ATOMIC_ACQUIRE);
        if (s == 2) return;
        if (s == 0) { if (__atomic_compare_exchange_n(&v->pstate, &s, 2, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) return; continue; }
        if (!__atomic_compare_exchange_n(&v->pstate, &s, 0, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) continue;
        struct sched *h = v->home;
        pthread_mutex_lock(&h->inbox_m);
        v->inext = h->inbox; h->inbox = v;
        pthread_mutex_unlock(&h->inbox_m);
        uint64_t one = 1; (void)!write(h->evfd, &one, sizeof one);
        return;
    }
}

/* Wait until `fd` is ready for `events` (EPOLLIN/EPOLLOUT) or `timeout_ns` passes (< 0: no limit). Parks the vthread on a worker, blocks in poll otherwise. */
static int wait_fd(int fd, uint32_t events, int64_t timeout_ns) {
    if (!S || !S->cur) {
        struct pollfd p = { fd, (short)((events & EPOLLIN ? POLLIN : 0) | (events & EPOLLOUT ? POLLOUT : 0)), 0 };
        while (poll(&p, 1, timeout_ns < 0 ? -1 : (int)(timeout_ns / 1000000) + 1) < 0) if (errno != EINTR) return -1;
        return 0;
    }
    struct vt *v = S->cur;
    struct epoll_event e = { .events = events | EPOLLONESHOT, .data.ptr = v };
    int tfd = -1;
    if (epoll_ctl(S->ep, EPOLL_CTL_ADD, fd, &e) < 0) return -1;
    if (timeout_ns >= 0) {
        tfd = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK);
        struct itimerspec its = { {0, 0}, { timeout_ns / 1000000000LL, timeout_ns % 1000000000LL } };
        if (!its.it_value.tv_sec && !its.it_value.tv_nsec) its.it_value.tv_nsec = 1;
        timerfd_settime(tfd, 0, &its, NULL);
        struct epoll_event te = { .events = EPOLLIN | EPOLLONESHOT, .data.ptr = v };
        epoll_ctl(S->ep, EPOLL_CTL_ADD, tfd, &te);
    }
    v->woken = false; S->nwait++;
    swapcontext(&v->ctx, &S->home);                      /* parked: the scheduler runs others until epoll wakes us */
    epoll_ctl(S->ep, EPOLL_CTL_DEL, fd, NULL);
    if (tfd >= 0) { epoll_ctl(S->ep, EPOLL_CTL_DEL, tfd, NULL); close(tfd); }
    return 0;
}

/* ---- Conn ---- */
int gort_x_Conn_recv(G_Conn *self, void *buf, size_t buf_len, size_t *ret_out) {
    struct rt_conn *c = (struct rt_conn *)self;
    for (;;) {
        ssize_t n = recv(c->fd, buf, buf_len, 0);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) { if (wait_fd(c->fd, EPOLLIN, -1) == 0) continue; }
        if (n >= 0) { *ret_out = (size_t)n; return 0; }
        return gort_rt_throw_io();
    }
}
int gort_x_Conn_send(G_Conn *self, const void *bytes, size_t bytes_len, size_t *ret_out) {
    struct rt_conn *c = (struct rt_conn *)self;
    for (;;) {
        ssize_t n = send(c->fd, bytes, bytes_len, MSG_NOSIGNAL);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) { if (wait_fd(c->fd, EPOLLOUT, -1) == 0) continue; }
        if (n >= 0) { *ret_out = (size_t)n; return 0; }
        return gort_rt_throw_io();
    }
}
int gort_x_Conn_waitReadable(G_Conn *self) {
    return wait_fd(((struct rt_conn *)self)->fd, EPOLLIN, -1) == 0 ? 0 : gort_rt_throw_io();
}
bool gort_x_Conn_hasUnread(G_Conn *self) {
    int n = 0;
    return ioctl(((struct rt_conn *)self)->fd, FIONREAD, &n) == 0 && n > 0;
}
void gort_x_Conn_cleanup(G_Conn *self) {
    struct rt_conn *c = (struct rt_conn *)self;
    if (c->fd >= 0) { close(c->fd); c->fd = -1; }
}
/* half-close the write side (the client sees the response and a clean end), read and discard until the client closes, `ns` passes or
 * `max` bytes are discarded, then close. Parks (with a timeout) while waiting. */
void gort_x_Conn_lingerClose(G_Conn *self, int64_t ns, uint64_t max) {
    struct rt_conn *c = (struct rt_conn *)self;
    if (c->fd < 0) return;
    shutdown(c->fd, SHUT_WR);
    struct timespec t0; clock_gettime(CLOCK_MONOTONIC, &t0);
    uint64_t got = 0; char buf[4096];
    for (;;) {
        struct timespec t1; clock_gettime(CLOCK_MONOTONIC, &t1);
        int64_t left = ns - ((int64_t)(t1.tv_sec - t0.tv_sec) * 1000000000LL + (t1.tv_nsec - t0.tv_nsec));
        if (left <= 0 || got >= max) break;
        ssize_t n = recv(c->fd, buf, sizeof buf, 0);
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) { if (wait_fd(c->fd, EPOLLIN, left) < 0) break; continue; }
        if (n <= 0) break;
        got += (uint64_t)n;
    }
    gort_x_Conn_cleanup(self);
}

/* ---- Listener ---- */
int gort_x_Listener_bindReusePort(uint16_t port, G_Listener **ret_out) {
    int fd = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0);
    if (fd < 0) return gort_rt_throw_io();
    int one = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    setsockopt(fd, SOL_SOCKET, SO_REUSEPORT, &one, sizeof one);
    struct sockaddr_in a; memset(&a, 0, sizeof a);
    a.sin_family = AF_INET; a.sin_addr.s_addr = htonl(INADDR_ANY); a.sin_port = htons(port);
    if (bind(fd, (struct sockaddr *)&a, sizeof a) < 0 || listen(fd, 1024) < 0) { close(fd); return gort_rt_throw_io(); }
    struct rt_listener *l = gort_rt_new(sizeof *l); l->fd = fd;
    *ret_out = (G_Listener *)l;
    return 0;
}
int gort_x_Listener_accept(G_Listener *self, G_Conn **ret_out) {
    int lfd = ((struct rt_listener *)self)->fd;
    for (;;) {
        int fd = accept4(lfd, NULL, NULL, SOCK_NONBLOCK);
        if (fd < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) { if (wait_fd(lfd, EPOLLIN, -1) == 0) continue; }
        if (fd >= 0) { int one = 1; setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one); *ret_out = conn_new(fd); return 0; }
        if (errno == EINTR || errno == ECONNABORTED) continue;
        return gort_rt_throw_io();
    }
}
void gort_x_Listener_cleanup(G_Listener *self) {
    struct rt_listener *l = (struct rt_listener *)self;
    if (l->fd >= 0) { close(l->fd); l->fd = -1; }
}

/* ---- the host ---- */
gort_xarr gort_x_leak(void *bytes, size_t n) { return (gort_xarr){ bytes, n }; }   /* the same array comes back: the caller gets its own handle */
void gort_x_log(G_IoError e) { (void)e; fprintf(stderr, "gort: connection failed (I/O error)\n"); }
void gort_x_exit(int32_t code) { exit(code); }

struct worker { size_t core; void (*body)(void *); void *env; pthread_t th; };
static struct worker workers[256];
static size_t nworkers;
static void *worker_main(void *p) {
    struct worker *w = p;
    cpu_set_t set; CPU_ZERO(&set); CPU_SET(w->core % (size_t)sysconf(_SC_NPROCESSORS_ONLN), &set);
    pthread_setaffinity_np(pthread_self(), sizeof set, &set);   /* best effort */
    struct sched sc; memset(&sc, 0, sizeof sc); sc.ep = epoll_create1(0);
    pthread_mutex_init(&sc.inbox_m, NULL);
    sc.evfd = eventfd(0, EFD_NONBLOCK);
    struct epoll_event bell = { .events = EPOLLIN, .data.ptr = NULL };
    epoll_ctl(sc.ep, EPOLL_CTL_ADD, sc.evfd, &bell);
    S = &sc;
    vt_new(w->body, w->env);                                    /* the worker's root vthread */
    sched_run();
    close(sc.ep); close(sc.evfd); S = NULL;
    return NULL;
}
void gort_x_spawnWorker(size_t core, void *f, void *env) {
    if (nworkers == sizeof workers / sizeof *workers) { fprintf(stderr, "gort: too many workers\n"); exit(2); }
    struct worker *w = &workers[nworkers++];
    w->core = core; w->body = (void (*)(void *))f; w->env = env;
    if (pthread_create(&w->th, NULL, worker_main, w) != 0) { perror("pthread_create"); exit(2); }
}

/* `spawn call` is spawnTask(fn() { call }): a vthread on this worker; from outside a worker, a detached OS thread (joined at exit by count) */
static pthread_mutex_t task_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t task_done = PTHREAD_COND_INITIALIZER;
static size_t running;
struct task { void (*body)(void *); void *env; };
static void *task_main(void *p) {
    struct task t = *(struct task *)p; free(p);
    t.body(t.env);
    pthread_mutex_lock(&task_lock);
    if (--running == 0) pthread_cond_broadcast(&task_done);
    pthread_mutex_unlock(&task_lock);
    return NULL;
}
void gort_x_spawnTask(void *f, void *env) {
    if (S) { vt_new((void (*)(void *))f, env); return; }
    struct task *t = malloc(sizeof *t);
    t->body = (void (*)(void *))f; t->env = env;
    pthread_mutex_lock(&task_lock); running++; pthread_mutex_unlock(&task_lock);
    pthread_t th; pthread_attr_t at;
    pthread_attr_init(&at); pthread_attr_setdetachstate(&at, PTHREAD_CREATE_DETACHED);
    int rc = pthread_create(&th, &at, task_main, t);
    pthread_attr_destroy(&at);
    if (rc != 0) { perror("pthread_create"); exit(2); }
}
void gort_rt_exit_hook(void) {
    for (size_t i = 0; i < nworkers; i++) pthread_join(workers[i].th, NULL);
    pthread_mutex_lock(&task_lock);
    while (running) pthread_cond_wait(&task_done, &task_lock);
    pthread_mutex_unlock(&task_lock);
}
