#include <stdint.h>
void gort_x_runTwice(int64_t l, void* f, void* env) { ((void(*)(void*))f)(env); }
int64_t gort_x_runWith(void* f, void* env) { return ((int64_t(*)(void*, int64_t))f)(env, 10); }
