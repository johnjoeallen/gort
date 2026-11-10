// idle.c: open N TCP connections to 127.0.0.1:port and hold them (no traffic) until killed. Prints "ready" when all are up.
#include <arpa/inet.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <unistd.h>
int main(int ac, char **av) {
  int port = atoi(av[1]), n = atoi(av[2]);
  struct sockaddr_in a = { .sin_family = AF_INET, .sin_port = htons(port) }; inet_pton(AF_INET, "127.0.0.1", &a.sin_addr);
  int ok = 0;
  for (int i = 0; i < n; i++) { int s = socket(AF_INET, SOCK_STREAM, 0); if (connect(s, (void *)&a, sizeof a) == 0) ok++; else { perror("connect"); close(s); } }
  printf("ready %d\n", ok); fflush(stdout);
  pause();
  return 0;
}
