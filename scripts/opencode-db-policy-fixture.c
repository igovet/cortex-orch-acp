/* Standalone diagnostic peer; never point this at an actual account. */
#define _GNU_SOURCE
#include <grp.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <sqlite3.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define ACCOUNT "acct"
#define DB ACCOUNT "/opencode.db"
#define WAL DB "-wal"
#define SHM DB "-shm"
#define NEIGHBOR DB "-wal.unrelated"
#define NEW_PEER_FILE ACCOUNT "/extra"
#define LABEL "/proc/self/attr/apparmor/current"
#define PEER "/bin/db-peer"

enum mode { NATIVE, HIDDEN, VISIBLE, CONTROL };
struct reply {
    int rc, rows;
    dev_t dev[3];
    ino_t ino[3];
};
struct peer { pid_t pid; int in, out; };
static const char *const paths[] = {DB, WAL, SHM};
static const char *const profiles[] = {NULL, "cortex-db-hidden", "cortex-db-visible"};

#define check(expr) do { if (!(expr)) { \
    fprintf(stderr, "fixture:%d: %s; errno=%d\n", __LINE__, #expr, errno); \
    exit(1); \
} } while (0)
#define denied(expr) do { errno = 0; check((expr) == -1 && errno == EACCES); } while (0)

static void transfer(int fd, void *buf, size_t size, bool send)
{
    char *p = buf;
    while (size) {
        ssize_t n = send ? write(fd, p, size) : read(fd, p, size);
        if (n < 0 && errno == EINTR)
            continue;
        check(n > 0);
        size -= (size_t)n;
        p += n;
    }
}

static void sql(sqlite3 *db, const char *query)
{
    int rc = sqlite3_exec(db, query, NULL, NULL, NULL);
    if (rc != SQLITE_OK) {
        fprintf(stderr, "SQLite rc=%d: %s (%s)\n", rc, sqlite3_errmsg(db), query);
        exit(1);
    }
}

static void expect_text(sqlite3 *db, const char *query, const char *value)
{
    sqlite3_stmt *stmt = NULL;
    check(sqlite3_prepare_v2(db, query, -1, &stmt, NULL) == SQLITE_OK);
    check(sqlite3_step(stmt) == SQLITE_ROW);
    const unsigned char *out = sqlite3_column_text(stmt, 0);
    check(out && strcmp((const char *)out, value) == 0);
    check(sqlite3_step(stmt) == SQLITE_DONE);
    check(sqlite3_finalize(stmt) == SQLITE_OK);
}

static int rows(sqlite3 *db)
{
    sqlite3_stmt *stmt = NULL;
    check(sqlite3_prepare_v2(db, "SELECT count(*) FROM ledger", -1, &stmt, NULL) == SQLITE_OK);
    check(sqlite3_step(stmt) == SQLITE_ROW);
    int n = sqlite3_column_int(stmt, 0);
    check(sqlite3_finalize(stmt) == SQLITE_OK);
    return n;
}

static void integrity(sqlite3 *db)
{
    expect_text(db, "PRAGMA integrity_check", "ok");
    expect_text(db, "SELECT count(*) FROM pragma_foreign_key_check", "0");
    expect_text(db, "SELECT kind FROM example WHERE key=1", "synthetic-login");
    expect_text(db, "SELECT kind FROM example WHERE key=2", "synthetic-history");
}

static void label(enum mode mode)
{
    int fd = open(LABEL, O_RDONLY | O_CLOEXEC);
    check(fd >= 0);
    char actual[128] = {0}, expected[128];
    ssize_t bytes = read(fd, actual, sizeof(actual) - 1);
    check(bytes > 0 && bytes < (ssize_t)sizeof(actual) - 1);
    check(close(fd) == 0);
    if (mode == NATIVE)
        snprintf(expected, sizeof(expected), "unconfined");
    else
        snprintf(expected, sizeof(expected), "%s (enforce)", profiles[mode]);
    actual[strcspn(actual, "\n")] = '\0';
    check(strcmp(actual, expected) == 0);
}

static void neighbors(enum mode mode, bool has_late)
{
    int fd;
    struct stat st;
    if (mode == HIDDEN) {
        denied(open(ACCOUNT, O_RDONLY | O_DIRECTORY | O_CLOEXEC));
    } else {
        fd = open(ACCOUNT, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        check(fd >= 0 && fsync(fd) == 0);
        DIR *entries = fdopendir(fd);
        check(entries != NULL);
        bool found = false, late = false;
        struct dirent *ent;
        while ((ent = readdir(entries)) != NULL) {
            if (!strcmp(ent->d_name, "opencode.db-wal.unrelated"))
                found = true;
            if (!strcmp(ent->d_name, "late"))
                late = true;
        }
        check(found && late == has_late);
        check(closedir(entries) == 0);
    }
    denied(open(NEIGHBOR, O_RDONLY | O_CLOEXEC));
    denied(open(NEIGHBOR, O_RDWR | O_TRUNC | O_CLOEXEC));
    denied(open(NEW_PEER_FILE, O_CREAT | O_EXCL | O_RDWR | O_CLOEXEC, 0600));
    denied(chmod(NEIGHBOR, 0644));
    denied(unlink(NEIGHBOR));
    denied(rename(NEIGHBOR, DB));
    denied(rename(DB, NEIGHBOR));
    denied(link(NEIGHBOR, NEW_PEER_FILE));
    denied(open(ACCOUNT "/unrelated-dir/file", O_RDONLY | O_CLOEXEC));
    if (has_late) {
        denied(open(ACCOUNT "/late", O_RDONLY | O_CLOEXEC));
        denied(unlink(ACCOUNT "/late"));
    }
    /* Do not record any metadata values; getattr can be distinct from open. */
    if (stat(NEIGHBOR, &st) == 0)
        fprintf(stderr, "guest: %s known-name sibling stat permitted\n", profiles[mode]);
    else
        check(errno == EACCES);
}

static void run_peer(enum mode mode, int actor)
{
    if (mode != CONTROL)
        label(mode);
    sqlite3 *db = NULL;
    check(sqlite3_open_v2(DB, &db, SQLITE_OPEN_READWRITE, NULL) == SQLITE_OK);
    sql(db, "PRAGMA busy_timeout=100; PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL");
    expect_text(db, "PRAGMA journal_mode", "wal");
    integrity(db);
    if (mode == VISIBLE)
        for (size_t j = 0; j < sizeof(paths)/sizeof(paths[0]); j++)
            check(chmod(paths[j], 0600) == 0);
    struct reply r = {0};
    transfer(STDOUT_FILENO, &r, sizeof(r), true);
    for (;;) {
        char op;
        transfer(STDIN_FILENO, &op, 1, false);
        memset(&r, 0, sizeof(r));
        switch (op) {
        case 's':
            sql(db, "BEGIN");
            r.rows = rows(db);
            break;
        case 'c': r.rows = rows(db); break;
        case 't': sql(db, "COMMIT"); break;
        case 'w':
            r.rc = sqlite3_exec(db, "BEGIN IMMEDIATE", NULL, NULL, NULL);
            break;
        case 'i': {
            char statement[136];
            snprintf(statement, sizeof(statement), "INSERT INTO ledger VALUES(%d,100,1)", actor);
            sql(db, statement);
            break;
        }
        case 'u': {
            char statement[136];
            snprintf(statement, sizeof(statement), "INSERT INTO ledger VALUES(%d,101,1)", actor);
            sql(db, statement);
            break;
        }
        case 'n':
            sql(db, "PRAGMA busy_timeout=5000");
            for (int i = 0; i < 24; i++) {
                char statement[180];
                snprintf(statement, sizeof(statement),
                         "BEGIN IMMEDIATE; INSERT INTO ledger VALUES(%d,%d,2); COMMIT;", actor, i);
                sql(db, statement);
            }
            sql(db, "PRAGMA busy_timeout=100");
            break;
        case 'p': {
            int wal = -1, checkpoint = -1;
            check(sqlite3_wal_checkpoint_v2(db, NULL, SQLITE_CHECKPOINT_PASSIVE,
                                            &wal, &checkpoint) == SQLITE_OK);
            check(wal > checkpoint && checkpoint >= 0);
            break;
        }
        case 'f':
            for (size_t j = 0; j < sizeof(paths)/sizeof(paths[0]); j++) {
                struct stat st;
                check(stat(paths[j], &st) == 0 && S_ISREG(st.st_mode));
                r.dev[j] = st.st_dev;
                r.ino[j] = st.st_ino;
            }
            break;
        case 'a': {
            check(mode == VISIBLE);
            pid_t child = fork();
            check(child >= 0);
            if (!child) {
                execl(PEER, PEER, "descendant", (char *)NULL);
                _exit(126);
            }
            int status;
            check(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
            break;
        }
        case 'v':
            integrity(db);
            r.rows = rows(db);
            break;
        case 'r': {
            char query[180];
            int n = snprintf(query, sizeof(query),
                             "SELECT group_concat(seq) FROM ledger WHERE actor=%d", actor - 1);
            check(n > 0 && (size_t)n < sizeof(query));
            expect_text(db, query, "100");
            break;
        }
        case 'q':
            check(sqlite3_close(db) == SQLITE_OK);
            transfer(STDOUT_FILENO, &r, sizeof(r), true);
            return;
        default: check(false);
        }
        transfer(STDOUT_FILENO, &r, sizeof(r), true);
    }
}

static struct peer start(enum mode mode, int actor)
{
    int in[2], out[2];
    check(pipe2(in, O_CLOEXEC) == 0);
    check(pipe2(out, O_CLOEXEC) == 0);
    pid_t pid = fork();
    check(pid >= 0);
    if (!pid) {
        check(dup2(in[0], STDIN_FILENO) == STDIN_FILENO);
        check(dup2(out[1], STDOUT_FILENO) == STDOUT_FILENO);
        char mode_str[16], actor_str[16];
        snprintf(mode_str, sizeof(mode_str), "%d", mode);
        snprintf(actor_str, sizeof(actor_str), "%d", actor);
        if (mode == CONTROL)
            execl("/proc/self/exe", "db-peer", "peer", mode_str, actor_str, (char *)NULL);
        else if (mode == NATIVE)
            execl(PEER, PEER, "peer", mode_str, actor_str, (char *)NULL);
        else
            execl("/bin/aa-exec", "aa-exec", "-p", profiles[mode], "--", PEER,
                  "peer", mode_str, actor_str, (char *)NULL);
        _exit(126);
    }
    check(close(in[0]) == 0 && close(out[1]) == 0);
    struct peer p = {pid, in[1], out[0]};
    return p;
}

static struct reply receive(struct peer p)
{
    struct reply r;
    transfer(p.out, &r, sizeof(r), false);
    return r;
}

static struct reply cmd(struct peer p, char op)
{
    transfer(p.in, &op, 1, true);
    return receive(p);
}

static struct peer ready(enum mode mode, int actor)
{
    struct peer p = start(mode, actor);
    receive(p);
    return p;
}

static void stop(struct peer p, bool crash)
{
    int status;
    if (crash)
        check(kill(p.pid, SIGKILL) == 0);
    else
        cmd(p, 'q');
    check(waitpid(p.pid, &status, 0) == p.pid);
    check(crash ? WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL
                : WIFEXITED(status) && WEXITSTATUS(status) == 0);
    check(close(p.in) == 0 && close(p.out) == 0);
}

static void sidecars_absent(void)
{
    struct stat st;
    for (size_t j = 1; j < sizeof(paths)/sizeof(paths[0]); j++)
        check(lstat(paths[j], &st) == -1 && errno == ENOENT);
}

static void seed_file(const char *path)
{
    int fd = open(path, O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    const char marker[] = "disposable-only";
    check(fd >= 0 && write(fd, marker, sizeof(marker)) == (ssize_t)sizeof(marker));
    check(close(fd) == 0);
}

static void hidden_case(void)
{
    pid_t pid = fork();
    check(pid >= 0);
    if (!pid) {
        execl("/bin/aa-exec", "aa-exec", "-p", profiles[HIDDEN], "--", PEER,
              "hidden", (char *)NULL);
        _exit(126);
    }
    int status;
    check(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
    puts("guest: hiding directory names denied opening the parent for fsync");
}

static void crash_case(enum mode victim, enum mode survivor, int actor, int before)
{
    sidecars_absent();
    struct peer writer = ready(victim, actor);
    check(cmd(writer, 'w').rc == SQLITE_OK);
    cmd(writer, 'i');
    cmd(writer, 't');
    check(cmd(writer, 'w').rc == SQLITE_OK);
    cmd(writer, 'u');
    struct stat st;
    check(stat(WAL, &st) == 0 && st.st_size > 32);
    stop(writer, true);
    check(stat(WAL, &st) == 0 && st.st_size > 32);
    struct peer recovered = ready(survivor, actor + 1);
    check(cmd(recovered, 'v').rows == before + 1);
    cmd(recovered, 'r');
    stop(recovered, false);
    sidecars_absent();
}

int main(int argc, char **argv)
{
    if (argc == 4 && !strcmp(argv[1], "peer")) {
        run_peer((enum mode)atoi(argv[2]), atoi(argv[3]));
        return 0;
    }
    if (argc == 2 && !strcmp(argv[1], "descendant")) {
        label(VISIBLE);
        neighbors(VISIBLE, true);
        return 0;
    }
    if (argc == 2 && !strcmp(argv[1], "hidden")) {
        label(HIDDEN);
        neighbors(HIDDEN, false);
        return 0;
    }
    bool control = argc == 2 && !strcmp(argv[1], "native-control");
    if (control) {
        umask(077);
        check(mkdir(ACCOUNT, 0700) == 0);
    } else {
        check(argc == 1 && getuid() == 0 && getpid() != 1);
        check(chdir("/") == 0 && getppid() == 1);
        check(setgroups(0, NULL) == 0 && setgid(1000) == 0 && setuid(1000) == 0);
        label(NATIVE);
    }
    enum mode host_mode = control ? CONTROL : NATIVE;
    enum mode policy_mode = control ? CONTROL : VISIBLE;
    printf("%s: SQLite %s; uid %u\n", control ? "native control" : "guest",
           sqlite3_libversion(), (unsigned)getuid());
    sqlite3 *db = NULL;
    check(sqlite3_open_v2(DB, &db, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, NULL) == SQLITE_OK);
    expect_text(db, "PRAGMA journal_mode=WAL", "wal");
    sql(db, "CREATE TABLE example(key INTEGER PRIMARY KEY,kind TEXT);"
            "INSERT INTO example VALUES(1,'synthetic-login'),(2,'synthetic-history');"
            "CREATE TABLE ledger(actor INTEGER,seq INTEGER,parent INTEGER REFERENCES example(key),"
            "PRIMARY KEY(actor,seq));");
    check(sqlite3_close(db) == SQLITE_OK);
    sidecars_absent();
    seed_file(NEIGHBOR);
    check(mkdir(ACCOUNT "/unrelated-dir", 0700) == 0);
    seed_file(ACCOUNT "/unrelated-dir/file");
    if (!control)
        hidden_case();

    struct peer native = ready(host_mode, 1);
    struct peer task = ready(policy_mode, 2);
    struct peer helper = ready(policy_mode, 3);
    if (!control) {
        seed_file(ACCOUNT "/late");
        cmd(task, 'a');
        cmd(helper, 'a');
    }
    struct reply file[3] = {cmd(native, 'f'), cmd(task, 'f'), cmd(helper, 'f')};
    for (size_t j = 0; j < sizeof(paths)/sizeof(paths[0]); j++) {
        struct stat st;
        check(stat(paths[j], &st) == 0);
        for (size_t i = 0; i < 3; i++)
            check(file[i].ino[j] == st.st_ino && file[i].dev[j] == st.st_dev);
    }
    if (!control)
        puts("guest: both confined peers and descendants; sibling mutation/content denied; directory fsync and same file identities");

    check(cmd(helper, 's').rows == 0);
    check(cmd(native, 'w').rc == SQLITE_OK);
    check(cmd(task, 'w').rc == SQLITE_BUSY);
    cmd(native, 'i');
    cmd(native, 't');
    check(cmd(task, 'w').rc == SQLITE_OK);
    check(cmd(native, 'w').rc == SQLITE_BUSY);
    cmd(task, 'i');
    cmd(task, 't');
    cmd(task, 'p');
    check(cmd(helper, 'c').rows == 0);
    cmd(helper, 't');
    check(cmd(helper, 'c').rows == 2);
    struct peer all[] = {native, task, helper};
    char stress = 'n';
    for (size_t j = 0; j < 3; j++)
        transfer(all[j].in, &stress, 1, true);
    for (size_t j = 0; j < 3; j++)
        receive(all[j]);
    for (size_t j = 0; j < 3; j++)
        check(cmd(all[j], 'v').rows == 74);
    stop(helper, false);
    stop(native, false);
    stop(task, false);
    sidecars_absent();
    for (int i = 0; i < 4; i++) {
        task = ready(policy_mode, 10 + i * 2);
        native = ready(host_mode, 11 + i * 2);
        check(cmd(task, 'c').rows == 74 && cmd(native, 'c').rows == 74);
        stop(i % 2 ? task : native, false);
        stop(i % 2 ? native : task, false);
        sidecars_absent();
    }
    puts("bidirectional writer lock, snapshot/checkpoint, concurrent writes, last-close/reopen passed");
    crash_case(policy_mode, host_mode, 30, 74);
    crash_case(host_mode, policy_mode, 32, 75);
    puts("process-kill WAL recovery succeeded in both directions; no power-loss claim");

    struct stat st;
    check(stat(NEIGHBOR, &st) == 0 && (st.st_mode & 0777) == 0600);
    int fd = open(NEIGHBOR, O_RDONLY | O_CLOEXEC);
    char marker[sizeof("disposable-only")];
    check(fd >= 0 && read(fd, marker, sizeof(marker)) == (ssize_t)sizeof(marker));
    check(!memcmp(marker, "disposable-only", sizeof(marker)) && close(fd) == 0);
    check(lstat(NEW_PEER_FILE, &st) == -1 && errno == ENOENT);
    if (control)
        puts("CORTEX_DB_NATIVE_CONTROL_PASS (no account policy exercised)");
    return 0;
}
