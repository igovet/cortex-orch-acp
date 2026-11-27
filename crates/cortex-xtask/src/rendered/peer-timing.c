/* Test-only timing of one exact synthetic ACP stdout frame. Never logs other bytes. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static ssize_t (*next_write)(int, const void *, size_t);
static int trace = -1;

__attribute__((constructor)) static void init(void) {
    next_write = dlsym(RTLD_NEXT, "write");
    if (!next_write) _exit(125);
    const char *path = getenv("CORTEX_FIXTURE_PEER_TRACE");
    if (path) {
        trace = open(path, O_WRONLY | O_APPEND | O_NOFOLLOW | O_CLOEXEC);
        /* The fake peer execs an MCP relay; never preload the observer there. */
        unsetenv("CORTEX_FIXTURE_PEER_TRACE");
        unsetenv("LD_PRELOAD");
    }
}

static bool marker(const char *data, size_t len, char sid[17]) {
    static const char prefix[] =
        "{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"fixture-";
    static const char suffix[] =
        "\",\"update\":{\"content\":{\"text\":\"Post-question coordinator text\",\"type\":\"text\"},"
        "\"sessionUpdate\":\"agent_message_chunk\"}}}\n";
    size_t head = sizeof(prefix) - 1, tail = sizeof(suffix) - 1;
    if (len <= head + tail || len > head + tail + 8 ||
        memcmp(data, prefix, head) || memcmp(data + len - tail, suffix, tail)) return false;
    size_t digits = len - head - tail;
    for (size_t i = 0; i < digits; ++i)
        if (data[head + i] < '0' || data[head + i] > '9') return false;
    memcpy(sid, "fixture-", 8);
    memcpy(sid + 8, data + head, digits);
    sid[8 + digits] = '\0';
    return true;
}

static uint64_t mono(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts)) return 0;
    return (uint64_t)ts.tv_sec * 1000000000 + (uint64_t)ts.tv_nsec;
}

ssize_t write(int fd, const void *buf, size_t len) {
    char sid[17], record[128];
    if (fd != STDOUT_FILENO || trace < 0 || !marker(buf, len, sid))
        return next_write(fd, buf, len);
    uint64_t begin = mono();
    ssize_t written = next_write(fd, buf, len);
    int saved_errno = errno;
    uint64_t end = mono();
    if (written == (ssize_t)len && begin && end >= begin) {
        int size = snprintf(record, sizeof(record), "%ld %s %llu %llu\n", (long)getpid(), sid,
            (unsigned long long)begin, (unsigned long long)end);
        if (size > 0 && (size_t)size < sizeof(record))
            (void)next_write(trace, record, (size_t)size);
    }
    errno = saved_errno;
    return written;
}
