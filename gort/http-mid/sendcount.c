/* sendcount.c - LD_PRELOAD shim for test.sh: prints one line per send() so a test can count the writes a server makes. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <sys/socket.h>
ssize_t send(int fd, const void *b, size_t n, int fl) {
    static ssize_t (*real)(int, const void *, size_t, int);
    if (!real) real = dlsym(RTLD_NEXT, "send");
    fprintf(stderr, "SEND %zu\n", n);
    return real(fd, b, n, fl);
}
