#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>
/* a stand-in for the runtime: each spawn is a thread; leaving a scope waits until none is left */
static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t c = PTHREAD_COND_INITIALIZER;
static int live;
struct a { void (*f)(void *); void *env; };
static void *run(void *p) {
    struct a x = *(struct a *)p; free(p);
    x.f(x.env);
    pthread_mutex_lock(&m); live--; pthread_cond_broadcast(&c); pthread_mutex_unlock(&m);
    return 0;
}
void gort_x_spawnTask(void *f, void *env) {
    struct a *x = malloc(sizeof *x); x->f = f; x->env = env;
    pthread_mutex_lock(&m); live++; pthread_mutex_unlock(&m);
    pthread_t t; pthread_attr_t at; pthread_attr_init(&at); pthread_attr_setdetachstate(&at, PTHREAD_CREATE_DETACHED);
    pthread_create(&t, &at, run, x);
}
void gort_rt_scope_join(void) {
    pthread_mutex_lock(&m);
    while (live) pthread_cond_wait(&c, &m);
    pthread_mutex_unlock(&m);
}
void gort_x_pause(int64_t ms) { usleep((useconds_t)ms * 1000); }
