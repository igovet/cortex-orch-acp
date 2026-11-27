/* Test-only: sample visible root pixels inside the owned desktop window. */
#define _POSIX_C_SOURCE 200809L
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

static uint64_t ns(struct timespec t) {
    return UINT64_C(1000000000) * (uint64_t)t.tv_sec + (uint64_t)t.tv_nsec;
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    Display *d = XOpenDisplay(NULL);
    if (!d) return 2;
    Window window = strtoul(argv[1], NULL, 10), root = DefaultRootWindow(d), child;
    XWindowAttributes a, screen;
    int x, y;
    if (!XGetWindowAttributes(d, root, &screen) || !XGetWindowAttributes(d, window, &a)
        || a.map_state != IsViewable
        || !XTranslateCoordinates(d, window, root, 0, 0, &x, &y, &child)
        || x < 0 || y < 0 || a.width < 1 || a.height < 1
        || a.width > 1920 || a.height > 1200
        || x + a.width > screen.width || y + a.height > screen.height) return 2;
    size_t length = (size_t)a.width * a.height;
    unsigned char *pixels = malloc(length);
    if (!pixels) return 2;
    printf("%d %d\n", a.width, a.height);
    for (;;) {
        struct timespec begin, end;
        if (clock_gettime(CLOCK_MONOTONIC, &begin)) return 2;
        XImage *image = XGetImage(d, root, x, y, a.width, a.height, AllPlanes, ZPixmap);
        if (clock_gettime(CLOCK_MONOTONIC, &end) || !image) return 2;
        if (image->red_mask != 0xff0000 || image->green_mask != 0xff00 || image->blue_mask != 0xff)
            return 2;
        for (int j = 0; j < a.height; ++j) {
            for (int i = 0; i < a.width; ++i) {
                unsigned long c = XGetPixel(image, i, j);
                pixels[(size_t)j * a.width + i] =
                    (77 * ((c >> 16) & 255) + 150 * ((c >> 8) & 255) + 29 * (c & 255)) >> 8;
            }
        }
        XDestroyImage(image);
        uint64_t stamp[] = {ns(begin), ns(end)};
        if (fwrite(stamp, sizeof(stamp), 1, stdout) != 1
            || fwrite(pixels, length, 1, stdout) != 1 || fflush(stdout)) return 2;
        struct timespec next = begin;
        next.tv_nsec += 40000000L;
        if (next.tv_nsec >= 1000000000L) { ++next.tv_sec; next.tv_nsec -= 1000000000L; }
        if (clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &next, NULL)) return 2;
    }
}
