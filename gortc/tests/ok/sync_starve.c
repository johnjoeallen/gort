#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
static pthread_t th[16]; static int nth;
struct a { void (*f)(void *); void *env; };
static void *run(void *p) { struct a x = *(struct a *)p; free(p); x.f(x.env); return 0; }
void gort_x_startThread(void *f, void *env) {
    struct a *x = malloc(sizeof *x); x->f = f; x->env = env;
    pthread_create(&th[nth++], 0, run, x);
}
void gort_x_joinAll(void) { for (int i = 0; i < nth; i++) pthread_join(th[i], 0); }
