#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <gtk/gtk.h>
#include <webkit2/webkit2.h>

static WebKitWebView *view;
static const char *directory;

// A dead web process leaves the window alive, so only WebKit can say it happened and why.
static void terminated(WebKitWebView *web_view, WebKitWebProcessTerminationReason reason, gpointer unused) {
  char *path = g_build_filename(directory, "web-process.log", NULL);
  FILE *log = fopen(path, "a");
  if (log) { fprintf(log, "%d\n", (int)reason); fclose(log); }
  g_free(path);
}

static void publish(const char *text) {
  char *temporary = g_build_filename(directory, "response.tmp", NULL), *path = g_build_filename(directory, "response.json", NULL);
  if (g_file_set_contents(temporary, text, -1, NULL)) rename(temporary, path);
  g_free(temporary); g_free(path);
}
static void done(GObject *source, GAsyncResult *result, gpointer unused) {
  GError *error = NULL;
  JSCValue *value = webkit_web_view_call_async_javascript_function_finish(WEBKIT_WEB_VIEW(source), result, &error);
  if (!value) { char *text = g_strdup_printf("!%s", error->message); publish(text); g_free(text); g_error_free(error); return; }
  char *text = jsc_value_to_string(value);
  publish(text); g_free(text); g_object_unref(value);
}
static void native(void) {
  GtkWidget *web = GTK_WIDGET(view), *top = gtk_widget_get_toplevel(web);
  GdkDisplay *display = gtk_widget_get_display(web);
  GdkRGBA background;
  webkit_web_view_get_background_color(view, &background);
  char *text = g_strdup_printf("{\"backend\":\"%s\",\"decorated\":%s,\"top\":[%d,%d],\"web\":[%d,%d],\"background\":[%d,%d,%d]}", G_OBJECT_TYPE_NAME(display),
    gtk_window_get_decorated(GTK_WINDOW(top)) ? "true" : "false", gtk_widget_get_allocated_width(top), gtk_widget_get_allocated_height(top),
    gtk_widget_get_allocated_width(web), gtk_widget_get_allocated_height(web), (int)(background.red * 255 + .5), (int)(background.green * 255 + .5), (int)(background.blue * 255 + .5));
  publish(text); g_free(text);
}
static gboolean poll_request(gpointer unused) {
  char *path = g_build_filename(directory, "request.js", NULL), *body = NULL;
  if (g_file_get_contents(path, &body, NULL, NULL)) {
    unlink(path);
    if (g_str_equal(body, "@native")) native();
    else if (g_str_equal(body, "@focus")) {
      gtk_window_present(GTK_WINDOW(gtk_widget_get_toplevel(GTK_WIDGET(view))));
      gtk_widget_grab_focus(GTK_WIDGET(view));
      publish("yes");
    }
    else if (g_str_has_prefix(body, "@resize ")) {
      int width = 0, height = 0;
      if (sscanf(body, "@resize %d %d", &width, &height) == 2 && width >= 960 && height >= 540) {
        gtk_window_resize(GTK_WINDOW(gtk_widget_get_toplevel(GTK_WIDGET(view))), width, height);
        publish("yes");
      } else publish("!invalid resize");
    }
    else webkit_web_view_call_async_javascript_function(view, body, -1, NULL, NULL, NULL, NULL, done, NULL);
    g_free(body);
  }
  g_free(path);
  return G_SOURCE_CONTINUE;
}
static void adopt(WebKitWebView *web_view) {
  if (view || !(directory = getenv("CW_PROBE_DIR"))) return;
  view = g_object_ref(web_view);
  g_signal_connect(web_view, "web-process-terminated", G_CALLBACK(terminated), NULL);
  g_timeout_add(30, poll_request, NULL);
}
void webkit_web_view_load_uri(WebKitWebView *web_view, const gchar *uri) {
  void (*real)(WebKitWebView *, const gchar *) = dlsym(RTLD_NEXT, "webkit_web_view_load_uri");
  adopt(web_view); real(web_view, uri);
}
void webkit_web_view_load_request(WebKitWebView *web_view, WebKitURIRequest *request) {
  void (*real)(WebKitWebView *, WebKitURIRequest *) = dlsym(RTLD_NEXT, "webkit_web_view_load_request");
  adopt(web_view); real(web_view, request);
}
