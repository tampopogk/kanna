#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/vsock.h>
#include <unistd.h>

struct pair { int from, to; };

static void *pump(void *arg) {
    struct pair *p = arg;
    char buf[65536];
    ssize_t n;
    while ((n = read(p->from, buf, sizeof buf)) > 0) {
        ssize_t off = 0;
        while (off < n) {
            ssize_t w = write(p->to, buf + off, n - off);
            if (w <= 0) goto done;
            off += w;
        }
    }
done:
    shutdown(p->to, SHUT_WR);
    return NULL;
}

static void *splice_fds(void *arg) {
    int *fds = arg;
    struct pair a = {fds[0], fds[1]}, b = {fds[1], fds[0]};
    pthread_t t1, t2;
    pthread_create(&t1, NULL, pump, &a);
    pthread_create(&t2, NULL, pump, &b);
    pthread_join(t1, NULL);
    pthread_join(t2, NULL);
    close(fds[0]);
    close(fds[1]);
    free(fds);
    return NULL;
}

static void spawn_splice(int a, int b) {
    int *fds = malloc(2 * sizeof(int));
    fds[0] = a;
    fds[1] = b;
    pthread_t t;
    pthread_create(&t, NULL, splice_fds, fds);
    pthread_detach(t);
}

static int vsock_connect(unsigned cid, unsigned port) {
    int fd = socket(AF_VSOCK, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    struct sockaddr_vm a;
    memset(&a, 0, sizeof a);
    a.svm_len = sizeof a;
    a.svm_family = AF_VSOCK;
    a.svm_cid = cid;
    a.svm_port = port;
    if (connect(fd, (struct sockaddr *)&a, sizeof a) != 0) {
        int e = errno;
        close(fd);
        errno = e;
        return -1;
    }
    return fd;
}

static int unix_connect(const char *path) {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un a;
    memset(&a, 0, sizeof a);
    a.sun_family = AF_UNIX;
    strncpy(a.sun_path, path, sizeof a.sun_path - 1);
    if (connect(fd, (struct sockaddr *)&a, sizeof a) != 0) {
        close(fd);
        return -1;
    }
    return fd;
}

static int serve(unsigned port, const char *path) {
    int s = socket(AF_VSOCK, SOCK_STREAM, 0);
    struct sockaddr_vm a;
    memset(&a, 0, sizeof a);
    a.svm_len = sizeof a;
    a.svm_family = AF_VSOCK;
    a.svm_cid = VMADDR_CID_ANY;
    a.svm_port = port;
    if (s < 0 || bind(s, (struct sockaddr *)&a, sizeof a) != 0 || listen(s, 16) != 0) {
        perror("vsock listen");
        return 1;
    }
    for (;;) {
        int c = accept(s, NULL, NULL);
        if (c < 0) continue;
        int u = unix_connect(path);
        if (u < 0) {
            close(c);
            continue;
        }
        spawn_splice(c, u);
    }
}

static int forward(unsigned tcp_port, unsigned vport) {
    int s = socket(AF_INET, SOCK_STREAM, 0);
    int one = 1;
    setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    struct sockaddr_in a;
    memset(&a, 0, sizeof a);
    a.sin_family = AF_INET;
    a.sin_port = htons(tcp_port);
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (bind(s, (struct sockaddr *)&a, sizeof a) != 0 || listen(s, 64) != 0) {
        perror("tcp listen");
        return 1;
    }
    for (;;) {
        int c = accept(s, NULL, NULL);
        if (c < 0) continue;
        int v = vsock_connect(VMADDR_CID_HOST, vport);
        if (v < 0) {
            close(c);
            continue;
        }
        spawn_splice(c, v);
    }
}

static int probe(unsigned cid, unsigned port) {
    int fd = vsock_connect(cid, port);
    if (fd < 0) {
        printf("{\"cid\":%u,\"port\":%u,\"connected\":false,\"errno\":%d,\"error\":\"%s\"}\n", cid, port, errno, strerror(errno));
        return 1;
    }
    printf("{\"cid\":%u,\"port\":%u,\"connected\":true}\n", cid, port);
    close(fd);
    return 0;
}

static int local_cid(void) {
    int fd = socket(AF_VSOCK, SOCK_STREAM, 0);
    unsigned cid = 0;
    if (fd >= 0 && ioctl(fd, IOCTL_VM_SOCKETS_GET_LOCAL_CID, &cid) == 0)
        printf("%u\n", cid);
    else
        printf("unknown errno=%d\n", errno);
    return 0;
}

int main(int argc, char **argv) {
    signal(SIGPIPE, SIG_IGN);
    if (argc == 4 && !strcmp(argv[1], "serve")) return serve(atoi(argv[2]), argv[3]);
    if (argc == 4 && !strcmp(argv[1], "forward")) return forward(atoi(argv[2]), atoi(argv[3]));
    if (argc == 4 && !strcmp(argv[1], "probe")) return probe(strtoul(argv[2], NULL, 10), strtoul(argv[3], NULL, 10));
    if (argc == 2 && !strcmp(argv[1], "cid")) return local_cid();
    fprintf(stderr, "usage: vsockrelay serve PORT UNIX_PATH | forward TCP_PORT VSOCK_PORT | probe CID PORT | cid\n");
    return 64;
}
