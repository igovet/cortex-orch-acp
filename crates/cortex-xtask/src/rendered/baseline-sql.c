/* Measurement-only companion link. Never linked into a distributed binary.
 * Rust's rusqlite opens every connection via sqlite3_open_v2. Include the
 * sqlite3.h shipped with its pinned libsqlite3-sys, then link with GNU
 * --wrap=sqlite3_open_v2. Do not retain SQL, arguments, paths or row data. */
#define _GNU_SOURCE
#include <sqlite3.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

static int output = -1;
static _Atomic unsigned records;
enum event { START = 1, OPEN, STMT, READ_STMT, ROW, INTERNAL, CLOSE };

static void record(enum event type) {
    struct timespec clock;
    if (atomic_fetch_add(&records, 1) >= 4000000 ||
        clock_gettime(CLOCK_MONOTONIC, &clock) != 0) _exit(125);
    uint64_t data[2] = { (uint64_t)clock.tv_sec * 1000000000 + clock.tv_nsec, type };
    if (write(output, data, sizeof(data)) != sizeof(data)) _exit(125);
}

__attribute__((constructor)) static void init(void) {
    const char *path = getenv("CORTEX_FIXTURE_SQL_TRACE");
    if (!path) return;
    output = open(path, O_WRONLY | O_APPEND | O_NOFOLLOW | O_CLOEXEC);
    struct stat st;
    if (output < 0 || fstat(output, &st) || !S_ISREG(st.st_mode) || st.st_uid != geteuid()
        || (st.st_mode & 077) || st.st_nlink != 1 || st.st_size != 0) _exit(125);
    /* This executable can launch itself in other modes. Count only its main
     * service process, on all threads; descendants do not inherit the opt-in. */
    if (unsetenv("CORTEX_FIXTURE_SQL_TRACE")) _exit(125);
    record(START);
}

static int trace(unsigned mask, void *ctx, void *stmt, void *detail) {
    (void)ctx;
    if (mask == SQLITE_TRACE_ROW) record(ROW);
    else if (mask == SQLITE_TRACE_CLOSE) record(CLOSE);
    else if (mask == SQLITE_TRACE_STMT) {
        const char *sql = detail;
        const char *prepared = sqlite3_sql(stmt);
        if (sql && sql[0] == '-' && sql[1] == '-' && (!prepared || strcmp(sql, prepared))) record(INTERNAL);
        else record(sqlite3_stmt_readonly(stmt) && sqlite3_column_count(stmt) > 0 ? READ_STMT : STMT);
    }
    return 0;
}

int __real_sqlite3_open_v2(const char *, sqlite3 **, int, const char *);
int __wrap_sqlite3_open_v2(const char *name, sqlite3 **db, int flags, const char *vfs) {
    int rc = __real_sqlite3_open_v2(name, db, flags, vfs);
    if (rc == SQLITE_OK && output >= 0) {
        if (sqlite3_trace_v2(*db, SQLITE_TRACE_STMT | SQLITE_TRACE_ROW | SQLITE_TRACE_CLOSE,
                             trace, NULL) != SQLITE_OK) _exit(125);
        record(OPEN);
    }
    return rc;
}
