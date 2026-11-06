/* rt/posix.c - a small POSIX runtime for the `extern` declarations of gort/runtime/leaves.gort (what http-mini needs).
 *
 * Sockets are blocking; each worker is one thread and each `spawn`ed connection another:
 * `spawn` starts an OS thread per task (not yet vthreads). Reference counts are atomic.
 *
 * Link it with the program:  gortc run --link rt/posix.c gort/runtime and gort/http-mini
 */
#define _GNU_SOURCE
#include <errno.h>
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

/* ---- Conn ---- */
int gort_x_Conn_recv(G_Conn *self, gort_arr *buf, size_t *ret_out) {
    struct rt_conn *c = (struct rt_conn *)self;
    for (;;) {
        ssize_t n = recv(c->fd, gort_rt_arr_data(buf), gort_rt_arr_len(buf), 0);
        if (n < 0 && errno == EINTR) continue;
        gort_rt_arr_release(buf);                       /* the argument arrived owned */
        if (n >= 0) { *ret_out = (size_t)n; return 0; }
        return gort_rt_throw_io();
    }
}
int gort_x_Conn_send(G_Conn *self, gort_arr *bytes, size_t *ret_out) {
    struct rt_conn *c = (struct rt_conn *)self;
    for (;;) {
        ssize_t n = send(c->fd, gort_rt_arr_data(bytes), gort_rt_arr_len(bytes), MSG_NOSIGNAL);
        if (n < 0 && errno == EINTR) continue;
        gort_rt_arr_release(bytes);
        if (n >= 0) { *ret_out = (size_t)n; return 0; }
        return gort_rt_throw_io();
    }
}
int gort_x_Conn_waitReadable(G_Conn *self) {
    struct pollfd p = { ((struct rt_conn *)self)->fd, POLLIN, 0 };
    while (poll(&p, 1, -1) < 0) if (errno != EINTR) return gort_rt_throw_io();
    return 0;
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
 * `max` bytes are discarded, then close. Blocking here for now; the vthread runtime will hand this off. */
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
        struct pollfd p = { c->fd, POLLIN, 0 };
        if (poll(&p, 1, (int)(left / 1000000) + 1) <= 0) break;
        ssize_t n = recv(c->fd, buf, sizeof buf, 0);
        if (n <= 0) break;
        got += (uint64_t)n;
    }
    gort_x_Conn_cleanup(self);
}

/* ---- Listener ---- */
int gort_x_Listener_bindReusePort(uint16_t port, G_Listener **ret_out) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
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
        int fd = accept(lfd, NULL, NULL);
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
gort_arr *gort_x_leak(gort_arr *bytes) { return bytes; }   /* the argument arrives owned; the caller keeps it for the run */
void gort_x_log(G_IoError e) { (void)e; fprintf(stderr, "gort: connection failed (I/O error)\n"); }
void gort_x_exit(int32_t code) { exit(code); }

struct worker { size_t core; void (*body)(void *); void *env; pthread_t th; };
static struct worker workers[256];
static size_t nworkers;
static void *worker_main(void *p) {
    struct worker *w = p;
    cpu_set_t set; CPU_ZERO(&set); CPU_SET(w->core % (size_t)sysconf(_SC_NPROCESSORS_ONLN), &set);
    pthread_setaffinity_np(pthread_self(), sizeof set, &set);   /* best effort */
    w->body(w->env);
    return NULL;
}
void gort_x_spawnWorker(size_t core, void *f, void *env) {
    if (nworkers == sizeof workers / sizeof *workers) { fprintf(stderr, "gort: too many workers\n"); exit(2); }
    struct worker *w = &workers[nworkers++];
    w->core = core; w->body = (void (*)(void *))f; w->env = env;
    if (pthread_create(&w->th, NULL, worker_main, w) != 0) { perror("pthread_create"); exit(2); }
}
/* `spawn call` becomes spawnTask(fn() { call }): one detached thread per task; the exit hook waits for the running count to reach zero */
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
