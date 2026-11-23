/* Guest-only Codex file-backend syscall fixture. All records are synthetic. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define ACCOUNT "/codex-account"
#define AUTH ACCOUNT "/auth.json"
#define LOCK ACCOUNT "/auth.lock"
#define NEIGHBOR ACCOUNT "/history.json"
#define EXTRA ACCOUNT "/extra"
#define LABEL "/proc/self/attr/apparmor/current"
#define PEER "/bin/auth-peer"
#define PROFILE "cortex-codex-file"

#define check(expr) do { if (!(expr)) { \
    fprintf(stderr, "auth fixture:%d: %s; errno=%d\n", __LINE__, #expr, errno); \
    exit(1); \
} } while (0)
#define denied(expr) do { errno = 0; check((expr) == -1 && errno == EACCES); } while (0)

struct writer { pid_t pid; int in, out; };

static void send_byte(int fd, char value)
{
    check(write(fd, &value, 1) == 1);
}

static void expect_byte(int fd, char value)
{
    char actual = '\0';
    check(read(fd, &actual, 1) == 1 && actual == value);
}

static void expect_contents(const char *path, const char *expected)
{
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    check(fd >= 0);
    char buf[96] = {0};
    size_t len = strlen(expected);
    check(len < sizeof(buf) && read(fd, buf, sizeof(buf)) == (ssize_t)len);
    check(memcmp(buf, expected, len) == 0 && close(fd) == 0);
}

static void label(bool confined)
{
    expect_contents(LABEL, confined ? PROFILE " (enforce)\n" : "unconfined\n");
}

static void check_neighbors(void)
{
    denied(open(ACCOUNT, O_RDONLY | O_DIRECTORY | O_CLOEXEC));
    denied(open(NEIGHBOR, O_RDONLY | O_CLOEXEC));
    denied(open(NEIGHBOR, O_RDWR | O_TRUNC | O_CLOEXEC));
    denied(open(EXTRA, O_CREAT | O_EXCL | O_RDWR | O_CLOEXEC, 0600));
    denied(rename(NEIGHBOR, AUTH));
    denied(unlink(NEIGHBOR));
    struct stat metadata;
    if (stat(NEIGHBOR, &metadata) == 0)
        fputs("guest: known-name sibling stat permitted; directory enumeration denied\n", stderr);
    else
        check(errno == EACCES);
}

static void write_file(const char *path, const char *text, int flags)
{
    int fd = open(path, O_WRONLY | O_CREAT | O_CLOEXEC | flags, 0600);
    check(fd >= 0);
    size_t length = strlen(text);
    check(write(fd, text, length) == (ssize_t)length && close(fd) == 0);
}

static void writer(bool confined, const char *before, const char *after)
{
    label(confined);
    if (confined)
        check_neighbors();
    int fd = open(LOCK, O_RDWR | O_CREAT | O_CLOEXEC, 0600);
    check(fd >= 0);
    if (flock(fd, LOCK_EX | LOCK_NB) != 0) {
        check(errno == EWOULDBLOCK);
        send_byte(STDOUT_FILENO, 'B');
        check(flock(fd, LOCK_EX) == 0);
    }
    send_byte(STDOUT_FILENO, 'L');
    expect_byte(STDIN_FILENO, 'G');
    expect_contents(AUTH, before);
    write_file(AUTH, after, O_TRUNC);
    check(close(fd) == 0);
}

static void reap(pid_t child)
{
    int status;
    check(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

static struct writer start_writer(bool confined, const char *before, const char *after)
{
    int in[2], out[2];
    check(pipe2(in, O_CLOEXEC) == 0 && pipe2(out, O_CLOEXEC) == 0);
    pid_t child = fork();
    check(child >= 0);
    if (!child) {
        check(dup2(in[0], STDIN_FILENO) == STDIN_FILENO);
        check(dup2(out[1], STDOUT_FILENO) == STDOUT_FILENO);
        if (confined)
            execl("/bin/aa-exec", "aa-exec", "-p", PROFILE, "--",
                  PEER, "writer", "confined", before, after, (char *)NULL);
        else
            execl(PEER, PEER, "writer", "native", before, after, (char *)NULL);
        _exit(126);
    }
    check(close(in[0]) == 0 && close(out[1]) == 0);
    return (struct writer){child, in[1], out[0]};
}

static void finish_writer(struct writer writer)
{
    send_byte(writer.in, 'G');
    reap(writer.pid);
    check(close(writer.in) == 0 && close(writer.out) == 0);
}

static void check_helper(const char *role, const char *expected)
{
    pid_t child = fork();
    check(child >= 0);
    if (!child) {
        execl("/bin/aa-exec", "aa-exec", "-p", PROFILE, "--",
              PEER, role, expected, (char *)NULL);
        _exit(126);
    }
    reap(child);
}

int main(int argc, char **argv)
{
    if (argc == 5 && !strcmp(argv[1], "writer")) {
        check(!strcmp(argv[2], "confined") || !strcmp(argv[2], "native"));
        writer(!strcmp(argv[2], "confined"), argv[3], argv[4]);
        return 0;
    }
    if (argc == 3 && (!strcmp(argv[1], "catalogue") || !strcmp(argv[1], "usage") ||
                      !strcmp(argv[1], "descendant"))) {
        label(true);
        expect_contents(AUTH, argv[2]);
        check_neighbors();
        if (!strcmp(argv[1], "catalogue")) {
            pid_t child = fork();
            check(child >= 0);
            if (!child) {
                execl(PEER, PEER, "descendant", argv[2], (char *)NULL);
                _exit(126);
            }
            reap(child);
        }
        return 0;
    }

    check(argc == 1 && getuid() == 0 && getpid() != 1 && getppid() == 1);
    check(setgroups(0, NULL) == 0 && setgid(1000) == 0 && setuid(1000) == 0);
    label(false);
    const char *const data[] = {
        "{\"generation\":0}", "{\"generation\":1}", "{\"generation\":2}",
        "{\"generation\":3}", "{\"generation\":4}",
    };
    const char neighbor[] = "disposable-non-auth";
    write_file(AUTH, data[0], O_EXCL);
    write_file(NEIGHBOR, neighbor, O_EXCL);
    struct stat initial_auth, auth, initial_lock, lock;
    check(lstat(AUTH, &initial_auth) == 0);
    check(lstat(LOCK, &lock) == -1 && errno == ENOENT);

    struct writer confined = start_writer(true, data[0], data[1]);
    expect_byte(confined.out, 'L');
    check(lstat(LOCK, &initial_lock) == 0 && S_ISREG(initial_lock.st_mode));
    struct writer native = start_writer(false, data[1], data[2]);
    expect_byte(native.out, 'B');
    finish_writer(confined);
    expect_byte(native.out, 'L');
    finish_writer(native);

    native = start_writer(false, data[2], data[3]);
    expect_byte(native.out, 'L');
    confined = start_writer(true, data[3], data[4]);
    expect_byte(confined.out, 'B');
    finish_writer(native);
    expect_byte(confined.out, 'L');
    finish_writer(confined);

    check(lstat(AUTH, &auth) == 0 && auth.st_dev == initial_auth.st_dev &&
          auth.st_ino == initial_auth.st_ino);
    check(lstat(LOCK, &lock) == 0 && lock.st_dev == initial_lock.st_dev &&
          lock.st_ino == initial_lock.st_ino);
    expect_contents(AUTH, data[4]);
    check_helper("catalogue", data[4]);
    check_helper("usage", data[4]);
    expect_contents(NEIGHBOR, neighbor);
    check(lstat(EXTRA, &lock) == -1 && errno == ENOENT);
    puts("guest: absent lock created under profile; bidirectional lock contention and in-place synthetic refresh passed");
    puts("guest: independent simulated catalogue/usage and inherited descendant read approved contents only");
    return 0;
}
