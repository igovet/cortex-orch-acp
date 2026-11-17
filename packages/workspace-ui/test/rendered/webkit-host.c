// The WebKitGTK window the rendered drivers drive: one GTK window holding one WebKitWebView with an ephemeral context.
// Commands arrive one per line on stdin as `<id> <command> [<base64 argument>...]`; each is answered on stdout with one line
// `<id> ok <base64 value>` or `<id> err <base64 message>`. The drivers (webkit.mjs) own every decision; this file only
// exposes the same calls the former PyGObject drivers made: load, evaluate_javascript, resize, window capture and the
// window's show/hide/move/decoration. It needs DISPLAY and GDK_BACKEND set by its caller (a private Xvfb).
//   cc -O1 -o webkit-host webkit-host.c $(pkg-config --cflags --libs gtk+-3.0 webkit2gtk-4.1)
#include <gtk/gtk.h>
#include <webkit2/webkit2.h>
#include <glib-unix.h>
#include <stdio.h>
#include <string.h>

static GtkWidget *win;
static WebKitWebView *view;
static GString *line;

static void reply(const char *id, gboolean ok, const char *value) {
  char *encoded = g_base64_encode((const guchar *)value, strlen(value));
  printf("%s %s %s\n", id, ok ? "ok" : "err", encoded);
  fflush(stdout);
  g_free(encoded);
}

typedef struct { char *id; } Pending;

static void evaluated(GObject *source, GAsyncResult *result, gpointer data) {
  Pending *pending = data;
  GError *error = NULL;
  JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(source), result, &error);
  if (!value) {
    reply(pending->id, FALSE, error ? error->message : "evaluate_javascript failed");
    if (error) g_error_free(error);
  } else {
    char *text = jsc_value_to_string(value);
    reply(pending->id, TRUE, text ? text : "");
    g_free(text);
    g_object_unref(value);
  }
  g_free(pending->id);
  g_free(pending);
}

static char *decode(const char *text, gsize *size) {
  gsize n = 0;
  guchar *raw = g_base64_decode(text ? text : "", &n);
  char *out = g_malloc(n + 1);
  memcpy(out, raw, n);
  out[n] = 0;
  g_free(raw);
  if (size) *size = n;
  return out;
}

static gboolean capture(const char *id, const char *png, gboolean root, const char *raw) {
  GdkWindow *window = root ? gdk_get_default_root_window() : gtk_widget_get_window(win);
  int width, height;
  gtk_window_get_size(GTK_WINDOW(win), &width, &height);
  if (root) { width = MIN(width, gdk_window_get_width(window)); height = MIN(height, gdk_window_get_height(window)); }
  GdkPixbuf *pixels = gdk_pixbuf_get_from_window(window, 0, 0, width, height);
  if (!pixels) { reply(id, FALSE, "native capture unavailable"); return FALSE; }
  GError *error = NULL;
  if (!gdk_pixbuf_save(pixels, png, "png", &error, NULL)) { reply(id, FALSE, error->message); g_error_free(error); g_object_unref(pixels); return FALSE; }
  char *text;
  if (raw && *raw) {
    int stride = gdk_pixbuf_get_rowstride(pixels), channels = gdk_pixbuf_get_n_channels(pixels), w = gdk_pixbuf_get_width(pixels), h = gdk_pixbuf_get_height(pixels);
    FILE *file = fopen(raw, "wb");
    if (!file) { reply(id, FALSE, "cannot write raw pixels"); g_object_unref(pixels); return FALSE; }
    fwrite(gdk_pixbuf_read_pixels(pixels), 1, (gsize)stride * h, file);
    fclose(file);
    text = g_strdup_printf("%d %d %d %d", w, h, stride, channels);
  } else text = g_strdup_printf("%d %d", gdk_pixbuf_get_width(pixels), gdk_pixbuf_get_height(pixels));
  reply(id, TRUE, text);
  g_free(text);
  g_object_unref(pixels);
  return TRUE;
}

static void create(int width, int height, gboolean decorated, const char *script) {
  win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
  gtk_window_set_default_size(GTK_WINDOW(win), width, height);
  if (!decorated) gtk_window_set_decorated(GTK_WINDOW(win), FALSE);
  WebKitUserContentManager *content = webkit_user_content_manager_new();
  if (script && *script) {
    WebKitUserScript *user = webkit_user_script_new(script, WEBKIT_USER_CONTENT_INJECT_TOP_FRAME, WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, NULL, NULL);
    webkit_user_content_manager_add_script(content, user);
    webkit_user_script_unref(user);
  }
  view = WEBKIT_WEB_VIEW(g_object_new(WEBKIT_TYPE_WEB_VIEW, "web-context", webkit_web_context_new_ephemeral(), "user-content-manager", content, NULL));
  gtk_container_add(GTK_CONTAINER(win), GTK_WIDGET(view));
  gtk_widget_show_all(win);
}

static void command(char *text) {
  char **part = g_strsplit(text, " ", -1);
  guint n = g_strv_length(part);
  if (n < 2) { g_strfreev(part); return; }
  const char *id = part[0], *name = part[1];
  char *a = n > 2 ? decode(part[2], NULL) : NULL, *b = n > 3 ? decode(part[3], NULL) : NULL, *c = n > 4 ? decode(part[4], NULL) : NULL;
  if (!strcmp(name, "new")) {
    // new <width height decorated> <user script>
    int width = 1280, height = 800, decorated = 1;
    if (a) sscanf(a, "%d %d %d", &width, &height, &decorated);
    create(width, height, decorated, b);
    reply(id, TRUE, "");
  } else if (!win) reply(id, FALSE, "no window: send `new` first");
  else if (!strcmp(name, "load")) { webkit_web_view_load_uri(view, a); reply(id, TRUE, ""); }
  else if (!strcmp(name, "reload")) { webkit_web_view_reload(view); reply(id, TRUE, ""); }
  else if (!strcmp(name, "eval")) {
    Pending *pending = g_new0(Pending, 1);
    pending->id = g_strdup(id);
    webkit_web_view_evaluate_javascript(view, a, -1, NULL, NULL, NULL, evaluated, pending);
  }
  else if (!strcmp(name, "resize")) { int w = 0, h = 0; sscanf(a, "%d %d", &w, &h); gtk_window_resize(GTK_WINDOW(win), w, h); reply(id, TRUE, ""); }
  else if (!strcmp(name, "move")) { int x = 0, y = 0; sscanf(a, "%d %d", &x, &y); gtk_window_move(GTK_WINDOW(win), x, y); reply(id, TRUE, ""); }
  else if (!strcmp(name, "size")) { int w, h; gtk_window_get_size(GTK_WINDOW(win), &w, &h); char *t = g_strdup_printf("%d %d", w, h); reply(id, TRUE, t); g_free(t); }
  else if (!strcmp(name, "origin")) {
    int x = 0, y = 0;
    gdk_window_get_origin(gtk_widget_get_window(win), &x, &y);
    char *t = g_strdup_printf("%d %d", x, y);
    reply(id, TRUE, t); g_free(t);
  }
  else if (!strcmp(name, "hide")) { gtk_widget_hide(win); reply(id, TRUE, ""); }
  else if (!strcmp(name, "show")) { gtk_widget_show_all(win); reply(id, TRUE, ""); }
  else if (!strcmp(name, "capture")) capture(id, a, b && !strcmp(b, "root"), c);
  else if (!strcmp(name, "clipboard")) {
    // The CLIPBOARD selection as text, read through GTK the way a pasting application would.
    char *text = gtk_clipboard_wait_for_text(gtk_clipboard_get(GDK_SELECTION_CLIPBOARD));
    reply(id, TRUE, text ? text : "\x01none");
    g_free(text);
  }
  else if (!strcmp(name, "version")) {
    char *t = g_strdup_printf("%u.%u.%u", webkit_get_major_version(), webkit_get_minor_version(), webkit_get_micro_version());
    reply(id, TRUE, t); g_free(t);
  }
  else if (!strcmp(name, "quit")) { reply(id, TRUE, ""); gtk_main_quit(); }
  else reply(id, FALSE, "unknown command");
  g_free(a); g_free(b); g_free(c);
  g_strfreev(part);
}

static gboolean readable(gint fd, GIOCondition condition, gpointer data) {
  char buffer[65536];
  gssize got = read(fd, buffer, sizeof buffer);
  if (got <= 0) { gtk_main_quit(); return G_SOURCE_REMOVE; }
  for (gssize i = 0; i < got; i++) {
    if (buffer[i] == '\n') { command(line->str); g_string_truncate(line, 0); }
    else g_string_append_c(line, buffer[i]);
  }
  return G_SOURCE_CONTINUE;
}

int main(int argc, char **argv) {
  gtk_init(&argc, &argv);
  line = g_string_new("");
  g_unix_fd_add(0, G_IO_IN | G_IO_HUP, readable, NULL);
  gtk_main();
  return 0;
}
