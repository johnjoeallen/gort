// httpd.c - W5: minimal HTTP/1.1 keep-alive server, epoll, N threads (SO_REUSEPORT), written in the Gort model.
// Build variants via -DMODE (0 raw, 2 flagged, 3 atomic, 4 elided). Same rc.h macros as W1-W4.
// Every connection, request, header array, response and buffer is a counted object; every pass inc/dec's.
// Connections never cross threads, so in MODE 2/4 they stay on the non-atomic path (the shared flag is never set).
// Memory policy: the 4 KB read buffer is allocated when bytes arrive and freed when the connection has no partial request
// (an idle connection costs one Conn object). This is what a counted-buffer design naturally does; goroutine/virtual-thread
// servers keep per-connection stacks/buffers instead.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <pthread.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/socket.h>
#include <unistd.h>
#include "rc.h"

typedef struct { Obj h; uint32_t len; uint8_t d[4096]; } Buf4k;
typedef struct { Obj h; uint32_t len; uint8_t d[512]; } OutBuf;
typedef struct { Obj h; int n; struct { uint16_t no, nl, vo, vl; } hs[16]; } HArr;
typedef struct { Obj h; HArr *hdrs; Buf4k *buf; uint16_t path_off, path_len; uint32_t total; } Req;
typedef struct { Obj h; uint32_t status; OutBuf *out; } Resp;
typedef struct { Obj h; int fd, closing; Buf4k *rbuf; int ep; } Conn;

static void obj_free(Obj *o) {
  switch (o->type) {
    case T_CTX: { Conn *c = (Conn *)o; close(c->fd); if (c->rbuf) OWN_DROP(c->rbuf); break; }
    case T_REQ: { Req *r = (Req *)o; OWN_DROP(r->hdrs); DROP(r->buf); break; }   // buf is shared with the Conn: not owned by the request
    case T_RESP: { Resp *r = (Resp *)o; OWN_DROP(r->out); break; }
    default: break;
  }
  t_frees++; free(o);
}

static const char BODY[] = "{\"status\":\"ok\",\"service\":\"rcm\",\"n\":42}";
static volatile uint64_t g_sink2;

static NOINLINE uint64_t mw_log(Conn *c, Req *rq) {   // reads the Host header
  HArr *h = PASS_B(rq->hdrs); uint64_t v = 0;
  for (int i = 0; i < h->n; i++) if (h->hs[i].nl == 4 && !strncasecmp((char *)rq->buf->d + h->hs[i].no, "host", 4)) v += h->hs[i].vl;
  DROP_B(h); DROP_B(c); DROP_B(rq);
  return v;
}
static NOINLINE Resp *handler(Conn *c, Req *rq) {
  Resp *rp = oalloc(sizeof(Resp), T_RESP); rp->status = 200;
  OutBuf *o = oalloc(sizeof(OutBuf), T_BUF);
  o->len = (uint32_t)snprintf((char *)o->d, sizeof o->d, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %zu\r\n\r\n%s", sizeof BODY - 1, BODY);
  rp->out = PASS_L(o); DROP_L(o);
  DROP_B(c); DROP_B(rq);
  return rp;
}
static NOINLINE Resp *route(Conn *c, Req *rq) {
  g_sink2 += mw_log(PASS_B(c), PASS_B(rq));
  Resp *rp = handler(PASS_B(c), PASS_B(rq));
  DROP_B(c); DROP_B(rq);
  return rp;
}
static NOINLINE void write_resp(Conn *c, Resp *rp) {
  OutBuf *o = PASS_B(rp->out);
  ssize_t w = write(c->fd, o->d, o->len); (void)w;
  DROP_B(o); DROP_B(c); DROP_B(rp);
}

// parse one complete request at the start of c->rbuf; returns NULL if incomplete
static Req *parse(Conn *c) {
  Buf4k *b = c->rbuf; uint8_t *end = memmem(b->d, b->len, "\r\n\r\n", 4);
  if (!end) return NULL;
  Req *rq = oalloc(sizeof(Req), T_REQ); HArr *h = oalloc(sizeof(HArr), T_ARR); h->n = 0;
  uint8_t *p = b->d, *eol = memchr(p, '\n', b->len);
  uint8_t *sp1 = memchr(p, ' ', eol - p); rq->path_off = (uint16_t)(sp1 + 1 - b->d);
  uint8_t *sp2 = memchr(sp1 + 1, ' ', eol - sp1); rq->path_len = (uint16_t)(sp2 - sp1 - 1);
  p = eol + 1;
  while (p < end + 2 && h->n < 16) {
    uint8_t *e = memchr(p, '\r', end + 2 - p); if (!e || e == p) break;
    uint8_t *col = memchr(p, ':', e - p); if (!col) break;
    h->hs[h->n].no = (uint16_t)(p - b->d); h->hs[h->n].nl = (uint16_t)(col - p);
    uint8_t *v = col + 1; while (v < e && *v == ' ') v++;
    h->hs[h->n].vo = (uint16_t)(v - b->d); h->hs[h->n].vl = (uint16_t)(e - v); h->n++;
    p = e + 2;
  }
  rq->hdrs = PASS_L(h); DROP_L(h);
  rq->buf = PASS(b);               // the request keeps the read buffer alive (a clone of the Conn's reference)
  rq->total = (uint32_t)(end + 4 - b->d);
  return rq;
}

// callee owns one reference to c
static NOINLINE void on_readable(Conn *c) {
  if (!c->rbuf) { Buf4k *b = oalloc(sizeof(Buf4k), T_BUF); b->len = 0; c->rbuf = PASS_L(b); DROP_L(b); }
  Buf4k *b = c->rbuf;
  for (;;) {
    ssize_t n = read(c->fd, b->d + b->len, sizeof b->d - b->len);
    if (n > 0) { b->len += (uint32_t)n; if (b->len < sizeof b->d) break; }
    else if (n == 0) { c->closing = 1; break; }
    else { if (errno != EAGAIN && errno != EINTR) c->closing = 1; break; }
  }
  Req *rq;
  while (!c->closing && (rq = parse(c))) {
    uint32_t total = rq->total;
    Resp *rp = route(PASS_B(c), PASS_B(rq));
    write_resp(PASS_B(c), PASS_B(rp)); OWN_DROP(rp);
    OWN_DROP(rq);                      // frees the request and its header array; drops its clone of the buffer
    memmove(b->d, b->d + total, b->len - total); b->len -= total;
  }
  if (b->len == 0 || c->closing) { OWN_DROP(c->rbuf); c->rbuf = NULL; }
  DROP(c);
  if (c->closing) { epoll_ctl(c->ep, EPOLL_CTL_DEL, c->fd, NULL); OWN_DROP(c); }   // drop the epoll registration's reference
}

typedef struct { int port; int id; } WA;
static void *worker(void *p) {
  WA *wa = p;
  int ls = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0), one = 1;
  setsockopt(ls, SOL_SOCKET, SO_REUSEPORT, &one, sizeof one); setsockopt(ls, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
  struct sockaddr_in a = { .sin_family = AF_INET, .sin_port = htons(wa->port), .sin_addr.s_addr = htonl(INADDR_LOOPBACK) };
  if (bind(ls, (void *)&a, sizeof a) || listen(ls, 8192)) { perror("bind/listen"); exit(1); }
  int ep = epoll_create1(0);
  struct epoll_event ev = { .events = EPOLLIN, .data.ptr = NULL }; epoll_ctl(ep, EPOLL_CTL_ADD, ls, &ev);
  struct epoll_event evs[256];
  for (;;) {
    int n = epoll_wait(ep, evs, 256, -1);
    for (int i = 0; i < n; i++) {
      if (!evs[i].data.ptr) {
        for (;;) {
          int fd = accept4(ls, NULL, NULL, SOCK_NONBLOCK);
          if (fd < 0) break;
          setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
          Conn *c = oalloc(sizeof(Conn), T_CTX); c->fd = fd; c->closing = 0; c->rbuf = NULL; c->ep = ep;
          struct epoll_event e2 = { .events = EPOLLIN | EPOLLRDHUP, .data.ptr = c }; epoll_ctl(ep, EPOLL_CTL_ADD, fd, &e2);
        }
      } else { Conn *c = evs[i].data.ptr; on_readable(PASS_B(c)); }
    }
  }
  return NULL;
}
int main(int ac, char **av) {
  int port = ac > 1 ? atoi(av[1]) : 8080, T = ac > 2 ? atoi(av[2]) : 8;
  pthread_t th[64]; WA wa[64];
  for (int i = 0; i < T; i++) { wa[i].port = port; wa[i].id = i; pthread_create(&th[i], NULL, worker, &wa[i]); }
  for (int i = 0; i < T; i++) pthread_join(th[i], NULL);
  return 0;
}
