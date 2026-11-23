#include <stdint.h>
#include <stdlib.h>
// A runtime object comes from gort_rt_new: the count header, then the body.
typedef struct { int64_t n; } body;
extern void *gort_rt_new(size_t);
static void *mk(void) { return gort_rt_new(sizeof(body)); }
void *gort_x_Counter_make(void) { return mk(); }
void *gort_x_Counter_makeLocal(void) { return mk(); }
int64_t gort_x_Counter_get(void *self) { return ((body *)self)->n; }
void gort_x_Counter_inc(void *self) { ((body *)self)->n++; }
void *gort_x_Beacon_make(void) { return mk(); }
int64_t gort_x_Beacon_ping(void *self) { return ++((body *)self)->n; }
