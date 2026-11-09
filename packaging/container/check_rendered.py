#!/usr/bin/env python3
"""No-image WebKitGTK owner journey and visual audit of the shipped desktop binary.

The journey runs on a private X display with a real window manager and session bus.
The visual audit reads computed styles and rectangles from the real binary, unmodified:
an LD_PRELOAD probe compiled here adds a request file to its web view, so the product
carries no inspector or debug route. No screenshot is captured and no image is read.
"""
import argparse
import hashlib
import itertools
import json
import os
from pathlib import Path
import re
import resource
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import traceback

ROOT = Path(__file__).resolve().parents[2]
BUILDER = "cortex-workspace-builder:ubuntu26.04-rust1.99.1-node24.14.1"
IMAGE = "cortex-workspace-rendered:ubuntu26.04"
RECIPE = f"""FROM {BUILDER}
USER root
RUN apt-get update && apt-get install -y --no-install-recommends \\
    xvfb xdotool x11-utils openbox dbus at-spi2-core python3-gi python3-pyatspi gir1.2-webkit2-4.1 gir1.2-gtk-3.0 \\
    bubblewrap xdg-dbus-proxy mutter git pulseaudio pulseaudio-utils bzip2 && rm -rf /var/lib/apt/lists/*
ENTRYPOINT []
"""
SCREEN = (1920, 1200)
MONITOR = (1920, 1200)
# The accepted stylesheet's values, read again from the app itself on every audit.
TITLEBAR_H = 40
CHEVRON_MAX = 34
SHADOW_BLUR_MAX = 8
FAILURES = []
PROGRESS = {"step": None, "screen": None, "passed": 0}
# What a dying process leaves on stderr, counted by name. Message bodies are never retained.
STDERR_MARKERS = ("panic", "Segmentation", "Aborted", "core dumped", "CRITICAL", "WARNING", "EGL", "MESA", "Error reading events from display", "Broken pipe",
                  "Lost connection", "Protocol error", "bwrap", "Operation not permitted", "out of memory", "WebProcess")
WEB_TERMINATION = {0: "crashed", 1: "exceeded_memory_limit", 2: "terminated_by_api"}
# How libatspi refuses a node the page has since removed: GError code 0 once it has disposed the object, code 1 when the app answers for it.
STALE_NODE = {0: "The application no longer exists", 1: "Object does not exist at path"}

PROBE = r"""
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
"""

# Everything a screen is judged on, as numbers and short names. Runs in the page; returns one JSON string.
AUDIT = r"""
const vw = innerWidth, vh = innerHeight, theme = document.documentElement.dataset.theme;
const css = el => getComputedStyle(el);
const box = el => { const r = el.getBoundingClientRect(); return { x: +r.x.toFixed(1), y: +r.y.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1) }; };
const name = el => `${el.tagName.toLowerCase()}${el.id ? "#" + el.id : ""}${typeof el.className === "string" && el.className ? "." + el.className.trim().split(/\s+/).slice(0, 3).join(".") : ""}`;
const token = (t, property = "background-color") => { const p = document.createElement("i"); p.style.setProperty(property, `var(${t})`); document.body.append(p); const v = css(p).getPropertyValue(property); p.remove(); return v; };
const parse = v => {
  let m = /rgba?\(([\d.]+)[, ]+([\d.]+)[, ]+([\d.]+)(?:[,/ ]+([\d.]+))?/.exec(v);
  if (m) return [+m[1], +m[2], +m[3], m[4] === undefined ? 1 : +m[4]];
  m = /color\(srgb ([\d.]+) ([\d.]+) ([\d.]+)(?: \/ ([\d.]+))?/.exec(v);
  return m ? [m[1] * 255, m[2] * 255, m[3] * 255, m[4] === undefined ? 1 : +m[4]] : [0, 0, 0, 0];
};
const lum = ([r, g, b]) => (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
const seen = el => { const s = css(el), r = el.getBoundingClientRect(); return el.getClientRects().length > 0 && s.visibility !== "hidden" && s.display !== "none" && r.width > 0 && r.height > 0 && r.right > 0 && r.bottom > 0 && r.left < vw && r.top < vh; };
const surface = token("--bg-canvas");
const root = document.getElementById("root"), bar = document.querySelector(".titlebar");
const page = {
  theme, surface, viewport: [vw, vh], html: css(document.documentElement).backgroundColor, body: css(document.body).backgroundColor, root: css(root).backgroundColor,
  rootBox: box(root), htmlBox: box(document.documentElement), bodyBox: box(document.body), bodyMargin: css(document.body).margin,
  scroll: [document.documentElement.scrollWidth, document.documentElement.scrollHeight],
};
const barBox = bar ? box(bar) : null, inBar = bar ? [...bar.querySelectorAll("a, button")].filter(seen) : [];
const outside = inBar.filter(el => { const r = el.getBoundingClientRect(), b = bar.getBoundingClientRect(); return r.left < b.left - .5 || r.right > b.right + .5 || r.top < b.top - .5 || r.bottom > b.bottom + .5; }).map(name);
const overlaps = [];
inBar.forEach((a, i) => inBar.slice(i + 1).forEach(b => { if (a.contains(b) || b.contains(a)) return; const p = a.getBoundingClientRect(), q = b.getBoundingClientRect(); if (Math.min(p.right, q.right) - Math.max(p.left, q.left) > 1 && Math.min(p.bottom, q.bottom) - Math.max(p.top, q.top) > 1) overlaps.push(name(a) + "+" + name(b)); }));
const split = document.querySelector(".titlebar .split");
const win = [...document.querySelectorAll(".win")];
const chrome = {
  barBox, outside, overlaps, controls: inBar.length,
  winGroups: win.length, winButtons: win.flatMap(w => [...w.querySelectorAll("button")]).length, winHidden: win.some(w => w.getAttribute("aria-hidden") === "true"),
  split: split ? { box: box(split), display: css(split).display, parts: [...split.children].map(box) } : null,
};
const texts = new Map(), faces = {};
const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
const dashes = [];
for (let n; (n = walker.nextNode());) {
  const t = n.textContent.trim(), p = n.parentElement;
  if (!t || !p || !seen(p) || ["SCRIPT", "STYLE"].includes(p.tagName)) continue;
  const first = css(p).fontFamily.split(",")[0].replace(/["']/g, "").trim();
  faces[first] = (faces[first] ?? 0) + 1;
  if (/^[-‒–—―]{1,3}$/.test(t)) dashes.push(name(p));
}
const light = [], native = [], shadows = [], themed = [], clipped = [];
for (const el of document.querySelectorAll("body, body *")) {
  if (!seen(el)) continue;
  const s = css(el), c = parse(s.backgroundColor), r = el.getBoundingClientRect();
  if (theme === "dark" && c[3] > .5 && lum(c) > .55 && !el.classList.contains("i") && !el.classList.contains("tick") && !el.closest(".btn-primary") && !el.closest(".bar")) light.push(`${name(el)} ${s.backgroundColor} ${Math.round(r.width)}x${Math.round(r.height)}`);
  if (el.matches("input, textarea, select, button") && !(el.type === "checkbox" || el.type === "radio")) {
    if (["outset", "inset"].includes(s.borderTopStyle) || (el.matches("select") && s.appearance !== "none") || s.appearance === "button" || s.appearance === "push-button") native.push(`${name(el)} ${s.appearance} ${s.borderTopStyle}`);
  }
  for (const m of s.boxShadow.matchAll(/(?:rgba?\([^)]*\)|color\([^)]*\))\s+(-?[\d.]+)px\s+(-?[\d.]+)px\s+([\d.]+)px/g)) { const blur = +m[3]; if (blur > 0) shadows.push(blur); }
  if (el.matches("a, button, [role=tab], [role=menuitem]") && /theme/i.test(`${el.title} ${el.getAttribute("aria-label") ?? ""} ${el.textContent}`)) themed.push(name(el));
  if (el.matches(".btn, .nav, .chip, .snav") && el.scrollWidth > el.clientWidth + 1 && !el.querySelector(".trunc") && !el.classList.contains("trunc")) clipped.push(name(el));
}
const loaded = [...document.fonts].filter(f => f.status === "loaded").map(f => `${f.family.replace(/["']/g, "")} ${f.weight}`);
const ruler = (family, weight = 400) => { const s = document.createElement("span"); s.textContent = "Cortex Workspace Rg 0123456789"; s.style.cssText = `position:absolute;visibility:hidden;font:${weight} 14px ${family}`; document.body.append(s); const w = s.getBoundingClientRect().width; s.remove(); return +w.toFixed(2); };
const fonts = {
  stack: css(document.body).fontFamily.split(",")[0].replace(/["']/g, ""), faces, loaded: [...new Set(loaded)], check: document.fonts.check('14px "Roboto Variable"'),
  widths: { roboto: ruler('"Roboto Variable"'), sans: ruler("sans-serif"), medium: ruler('"Roboto Variable"', 500) },
};
const icons = [...document.querySelectorAll(".i")].filter(seen);
const unmasked = icons.filter(el => { const s = css(el); return (s.maskImage ?? s.webkitMaskImage ?? "none") === "none" && (s.webkitMaskImage ?? "none") === "none"; });
const plainIcons = unmasked.length;
// Which icon, what the engine computed for it, and whether it paints a solid block: --m is empty when no .i-<name> rule exists.
const plainIconDetail = unmasked.slice(0, 8).map(el => { const s = css(el), b = box(el); return { icon: el.className, mask: s.maskImage ?? null, webkitMask: s.webkitMaskImage ?? null, m: s.getPropertyValue("--m").slice(0, 40), background: s.backgroundColor, box: [b.w, b.h], row: (el.parentElement?.textContent ?? "").trim().slice(0, 40) }; });
const known = new Set();
const rules = list => { for (const rule of list) { if (rule.selectorText) for (const m of rule.selectorText.matchAll(/\.([\w-]+)/g)) known.add(m[1]); if (rule.cssRules) rules(rule.cssRules); } };
for (const sheet of document.styleSheets) rules(sheet.cssRules);
const unknown = new Set();
for (const el of document.querySelectorAll("body *")) if (seen(el) && typeof el.className === "string") for (const c of el.classList) if (!known.has(c)) unknown.add(c);
const ui = {
  dialogs: [...document.querySelectorAll("dialog[open]")].map(name),
  navs: [...document.querySelectorAll(".sidebar .nav")].filter(seen).map(el => { const b = box(el); return [el.textContent.trim().slice(0, 14), b.x, b.y, b.w, b.h, css(el).fontWeight]; }),
  crumb: document.querySelector(".crumbs")?.textContent ?? "",
  main: document.querySelector(".main") ? box(document.querySelector(".main")) : null,
  mainText: (document.querySelector(".main")?.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 90),
  strips: [...document.querySelectorAll(".main .strip")].filter(seen).map(el => [name(el), box(el).y, box(el).h]),
  icons: icons.length, plainIcons, plainIconDetail, userSelect: css(document.body).webkitUserSelect,
};
return JSON.stringify({ page, chrome, fonts, ui, unknown: [...unknown].slice(0, 30), light: light.slice(0, 12), native: native.slice(0, 12), maxShadow: Math.max(0, ...shadows), themed: themed.slice(0, 8), dashes: dashes.slice(0, 12), clipped: clipped.slice(0, 12) });
"""

POPOVERS = r"""
const pause = () => new Promise(r => setTimeout(r, 60));
const css = el => getComputedStyle(el), out = [];
const triggers = [...document.querySelectorAll("button[popovertarget]")].filter(el => el.getClientRects().length);
const seen = new Set();
for (const trigger of triggers) {
  const id = trigger.getAttribute("popovertarget"), pop = document.getElementById(id);
  if (seen.has(id) || !pop) continue;
  seen.add(id);
  trigger.click();
  await pause();
  const r = pop.getBoundingClientRect(), s = css(pop);
  out.push({ id, supported: typeof pop.showPopover === "function", open: pop.matches(":popover-open"), display: s.display, background: s.backgroundColor,
    rect: [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)], inside: r.left >= 0 && r.top >= 0 && r.right <= innerWidth + .5 && r.bottom <= innerHeight + .5, items: pop.querySelectorAll(".mi, button, a, dt").length });
  pop.hidePopover?.();
  await pause();
}
return JSON.stringify(out);
"""


# Where WebKitGTK could differ from the Chromium the mockups were reviewed in.
ENGINE = r"""
const supports = c => CSS.supports(c), probe = document.createElement("div");
probe.style.cssText = "position:absolute;visibility:hidden;width:60px;height:60px;overflow:scroll";
document.body.append(probe);
const scrollbar = probe.offsetWidth - probe.clientWidth;
probe.remove();
await document.fonts.load('14px "Roboto Variable"', "Привет");
await document.fonts.load('12.5px "Cousine"', "Привет");
const measure = family => { const s = document.createElement("span"); s.textContent = "Привет, проект"; s.style.cssText = `position:absolute;visibility:hidden;font:14px ${family}`; document.body.append(s); const w = s.getBoundingClientRect().width; s.remove(); return +w.toFixed(2); };
return JSON.stringify({
  popover: typeof HTMLElement.prototype.showPopover === "function", dialog: typeof HTMLDialogElement.prototype.showModal === "function",
  colorMix: supports("color: color-mix(in srgb, red 50%, transparent)"), has: supports("selector(:has(a))"), focusVisible: supports("selector(:focus-visible)"), container: supports("container-type: inline-size"),
  scrollbarGutter: supports("scrollbar-gutter: stable"), mask: supports("mask: none") || supports("-webkit-mask: none"), positionArea: supports("position-area: bottom"),
  scrollbar, cyrillicRoboto: measure('"Roboto Variable"'), cyrillicSans: measure("sans-serif"),
  pageMenu: !document.body.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true })),
  linkDrag: getComputedStyle(document.querySelector(".sidebar a")).webkitUserDrag,
  faces: [...document.fonts].filter(f => f.status === "loaded").length,
});
"""


def seed_workers(project):
    """Rows the runtime writes for two workers, in the disposable project's real store: the fake engine delegates to none.

    Everything above the store is the shipped code, so the helper, the port and the screens read them as they would on any machine.
    """
    import sqlite3
    with sqlite3.connect(project / ".cortex-dev/cortex.sqlite3", timeout=10) as db:
        db.execute("PRAGMA foreign_keys=OFF")
        (runtime,) = db.execute("SELECT runtime_task_id FROM workspace_executions ORDER BY rowid DESC LIMIT 1").fetchone()
        now = "2026-11-05T10:00:00+00:00"
        for number, (node, status, role, title) in enumerate((("build", "running", "worker-frontend-dev", "Connect the board"), ("review", "completed", "worker-code-reviewer", "Review the board")), 1):
            assignment, attempt = f"a-{node}", f"x-{node}"
            db.execute("INSERT INTO work_assignments (assignment_id, task_id, node_key, requirement_bindings_json, status, requirement_state, idempotency_key, created_at) VALUES (?,?,?,'[]',?,'current',?,?)", (assignment, runtime, node, status, f"k-{node}", now))
            db.execute("INSERT INTO work_assignment_attempts (attempt_id, task_id, assignment_id, attempt_no, requirement_bindings_json, status, idempotency_key, created_at) VALUES (?,?,?,1,'[]',?,?,?)", (attempt, runtime, assignment, status, f"ka-{node}", now))
            events = [("delegation_requested", {"role": role, "title": title}), ("progress", {"kind": "worker_activity", "title": "Reading the board"}),
                      ("progress", {"kind": "worker_activity", "title": "pytest -q", "tool_kind": "execute", "status": "completed"})]
            if status == "completed":
                events.append(("worker_result", {"summary": "No findings in the board.", "checks": [{"name": "unit tests", "outcome": "passed"}]}))
            for index, (kind, payload) in enumerate(events):
                db.execute("INSERT INTO task_events (task_id, event_type, assignment_ref, dedup_key, payload_json, origin, created_at) VALUES (?,?,?,?,?,'runtime',?)", (runtime, kind, assignment, f"seed-{number}-{index}", json.dumps(payload), now))


def outcome(step, status="passed", **details):
    if status == "passed":
        PROGRESS.update(step=step, screen=details.get("screen", PROGRESS["screen"]), passed=PROGRESS["passed"] + 1)
    print(json.dumps({"step": step, "status": status, **details}, ensure_ascii=False), flush=True)


def fault(step, **details):
    FAILURES.append(step)
    outcome(step, "failed", **details)


def stop(process):
    if process is None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=8)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)


def exit_status(code):
    """A Popen return code as evidence: still running, a normal exit, or the signal that ended it."""
    if code is None:
        return {"state": "running"}
    if code >= 0:
        return {"state": "exited", "exit_code": code}
    try:
        return {"state": "signalled", "signal": signal.Signals(-code).name}
    except ValueError:
        return {"state": "signalled", "signal": str(-code)}


def stderr_summary(path):
    """Size and marker counts of a process's stderr, never its lines."""
    try:
        data = Path(path).read_bytes().decode("utf-8", "replace")
    except OSError:
        return {"bytes": 0, "lines": 0, "markers": {}}
    return {"bytes": len(data.encode()), "lines": data.count("\n"), "markers": {word: data.count(word) for word in STDERR_MARKERS if word in data}}


def core_files(directory):
    """Each core a dead process left, by name and size. kernel.core_pattern is the plain `core`, so it lands in the process's directory."""
    return [{"name": path.name, "bytes": path.stat().st_size} for path in sorted(Path(directory).glob("core*"))]


def web_terminations(directory):
    """Every web-process-terminated WebKit reported to the probe, in order."""
    try:
        lines = (Path(directory) / "web-process.log").read_text().split()
    except OSError:
        return []
    return [WEB_TERMINATION.get(int(line), line) for line in lines]


def owned_processes(group):
    """Name and state of this process group's members and of any WebKit process, from /proc. Never a command line."""
    found = []
    for stat in Path("/proc").glob("[0-9]*/stat"):
        try:
            text = stat.read_text()
        except OSError:
            continue
        name, fields = text[text.index("(") + 1:text.rindex(")")], text[text.rindex(")") + 2:].split()
        if int(fields[2]) == group or name.startswith("WebKit"):
            found.append(f"{name}:{fields[0]}")
    return sorted(found)


def failure_class(app_code, terminations, compositor_code, page_answers, error):
    """Which of the separate things went wrong, strongest evidence first. A compositor death takes a Wayland client with it, so it is named before the app."""
    if compositor_code is not None:
        return "compositor_exited"
    if app_code is not None:
        return "app_exited"
    if terminations:
        return "web_process_terminated"
    if any(text in error for text in STALE_NODE.values()):
        return "accessibility_node_stale" if page_answers else "page_unresponsive"
    return "journey_step_failed" if error else None


def stale_node(error, app_code, terminations, compositor_code, page_answers):
    """The one tolerated case: the page replaced a node the harness held, and every process is alive and answering.

    The same AT-SPI text also follows a real death, so the text alone never decides. A dead app, web process or compositor stays a failure.
    """
    return (getattr(error, "domain", None) == "atspi_error" and STALE_NODE.get(getattr(error, "code", None), "\0") in str(error)
            and failure_class(app_code, terminations, compositor_code, page_answers, str(error)) == "accessibility_node_stale")


def wait_for(check, step, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.05)
    raise RuntimeError(step)


def judge(report):
    """Names every regression in one audit; an empty list means the screen passed."""
    wrong = []
    page, chrome, fonts = report["page"], report["chrome"], report["fonts"]
    vw, vh = page["viewport"]
    for part in ("html", "body", "root"):
        if page[part] != page["surface"]:
            wrong.append(f"{part}_background_is_not_the_theme_surface")
    for part in ("rootBox", "htmlBox", "bodyBox"):
        box = page[part]
        if (box["x"], box["y"], box["w"], box["h"]) != (0, 0, vw, vh):
            wrong.append(f"{part}_does_not_fill_the_viewport")
    if page["bodyMargin"] != "0px":
        wrong.append("body_has_a_margin")
    if page["scroll"][0] > vw or page["scroll"][1] > vh:
        wrong.append("page_scrolls")
    bar = chrome["barBox"]
    if not bar or bar["y"] != 0 or bar["h"] != TITLEBAR_H or bar["w"] != vw:
        wrong.append("title_band_geometry")
    if chrome["outside"]:
        wrong.append("control_outside_title_band")
    if chrome["overlaps"]:
        wrong.append("title_band_controls_overlap")
    if chrome["winGroups"] != 1 or chrome["winButtons"] != 3 or chrome["winHidden"]:
        wrong.append("window_controls_are_not_one_real_group")
    split = chrome["split"]
    if split and (len(split["parts"]) != 2 or split["parts"][1]["w"] > CHEVRON_MAX or split["display"] == "grid"):
        wrong.append("split_chevron_stretched")
    if report["light"]:
        wrong.append("light_background_in_dark_theme")
    if report["native"]:
        wrong.append("unstyled_native_control")
    if report["maxShadow"] > SHADOW_BLUR_MAX:
        wrong.append("strong_shadow")
    if report["dashes"]:
        wrong.append("dash_stands_for_missing_data")
    if report["clipped"]:
        wrong.append("control_text_clipped")
    if report["ui"]["plainIcons"]:
        wrong.append("icon_lost_its_mask")
    if report["ui"]["userSelect"] != "text":
        wrong.append("page_text_is_not_selectable")
    if "Cousine" in fonts["faces"] and not any(face.startswith("Cousine") for face in fonts["loaded"]):
        wrong.append("code_font_not_loaded")
    if fonts["stack"] != "Roboto Variable" or not fonts["check"]:
        wrong.append("design_font_not_first")
    if not any(face.startswith("Roboto Variable") for face in fonts["loaded"]):
        wrong.append("design_font_not_loaded")
    if fonts["widths"]["roboto"] == fonts["widths"]["sans"]:
        wrong.append("design_font_not_rendering")
    if any(face not in ("Roboto Variable", "Cousine") for face in fonts["faces"]):
        wrong.append("text_in_a_fallback_face")
    return wrong


def session(args):
    import pyatspi
    import gi
    gi.require_version("WebKit2", "4.1")
    from gi.repository import WebKit2
    outcome("runtime_versions", webkitgtk=".".join(str(f()) for f in (WebKit2.get_major_version, WebKit2.get_minor_version, WebKit2.get_micro_version)),
            application="1.0.6", tauri="2.10.1", display="private native Wayland with Mutter; software compositing" if args.journey_wayland else "private Xvfb/X11 with Openbox; software compositing", images="none",
            roboto_installed_on_system=subprocess.run(["fc-list", "Roboto"], capture_output=True, text=True).stdout.strip() != "")
    scratch = Path(args.scratch)
    artifacts = Path(args.artifacts)
    # Every child inherits this directory and limit, so a crash leaves a core here.
    cores, logs = scratch / "cores", scratch / "logs"
    cores.mkdir()
    logs.mkdir()
    os.chdir(cores)
    resource.setrlimit(resource.RLIMIT_CORE, (resource.getrlimit(resource.RLIMIT_CORE)[1],) * 2)
    channel = json.loads((ROOT / "crates/workspace-protocol/channels.json").read_text())["dev"]
    project = scratch / "projects/journey"
    second = scratch / "projects/second"
    for repository in (project, second):
        repository.mkdir(parents=True)
        (repository / "README.md").write_text("# Journey\n\nA disposable project.\n")
        (repository / "a.txt").write_text("Disposable project notes.\n")
        for command in (["init", "-q", "-b", "main"], ["add", "README.md"], ["-c", "user.name=Audit", "-c", "user.email=audit@example.invalid", "commit", "-q", "-m", "Journey history"]):
            subprocess.run(["git", *command], cwd=repository, check=True, capture_output=True)
    probe_dir = scratch / "probe"
    probe_dir.mkdir()
    shim = scratch / "probe.so"
    source = scratch / "probe.c"
    source.write_text(PROBE)
    xvfb = service = app = wm = pulse = compositor = None
    wayland_env = None
    wayland_key = None
    sequence = itertools.count()
    launches = itertools.count(1)

    def spawn_app(env):
        """The desktop binary with stderr in a file: a pipe nobody reads would stall it once full, and a file outlives it."""
        log = logs / f"app-{next(launches)}.stderr"
        with log.open("wb") as sink:
            process = subprocess.Popen([str(artifacts / channel["desktop_binary"])], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=sink, start_new_session=True)
        process.stderr_log = log
        return process

    def page_answers_now():
        try:
            return js("return 'yes';", 3) == "yes"
        except Exception:
            return False

    def node_went_stale(error):
        """Says so only with fresh process evidence, and records each case. Anything else is the caller's failure."""
        if getattr(error, "domain", None) != "atspi_error":
            return False
        status = app.poll()
        answers = status is None and page_answers_now()
        stale = stale_node(error, status, web_terminations(probe_dir), compositor.poll() if compositor is not None else None, answers)
        if stale:
            outcome("accessibility_node_stale", "diagnostic", error_code=error.code, after_step=PROGRESS["step"], app=exit_status(status), page_answers=answers)
        return stale

    def retry_stale(action, seconds=15):
        """Repeats an accessibility action whose node the page replaced. libatspi refuses a stale node before anything is sent, so nothing runs twice."""
        deadline = time.monotonic() + seconds
        while True:
            try:
                return action()
            except Exception as error:
                if time.monotonic() >= deadline or not node_went_stale(error):
                    raise
                time.sleep(0.05)

    def native_evidence(error=""):
        """What each process really did, read before cleanup ends them."""
        status = app.poll() if app is not None else None
        page_answers = page_answers_now() if app is not None and status is None else None
        terminations = web_terminations(probe_dir)
        code = compositor.poll() if compositor is not None else None
        launched = sorted(logs.glob("app-*.stderr"))
        return {"failure_class": failure_class(status, terminations, code, page_answers, error), "app": exit_status(status) if app is not None else None,
                "app_processes": owned_processes(app.pid) if app is not None else [], "page_answers": page_answers, "web_process_terminations": terminations,
                "compositor": exit_status(code) if compositor is not None else None, "service": exit_status(service.poll()) if service is not None else None,
                "app_stderr": stderr_summary(app.stderr_log) if app is not None else None, "compositor_stderr": stderr_summary(logs / "compositor.stderr"),
                "app_launches": len(launched), "largest_app_stderr_bytes": max((path.stat().st_size for path in launched), default=0),
                "cores": core_files(cores), "core_limit": resource.getrlimit(resource.RLIMIT_CORE)[0]}

    def settings(theme):
        # GTK reads this with no settings daemon, and WebKit derives prefers-color-scheme from it: the real System path.
        path = Path(os.environ["XDG_CONFIG_HOME"]) / "gtk-3.0"
        path.mkdir(exist_ok=True)
        (path / "settings.ini").write_text(f"[Settings]\ngtk-theme-name={'Adwaita-dark' if theme == 'dark' else 'Adwaita'}\n")
        if args.journey_wayland:
            keyfile = Path(os.environ["XDG_CONFIG_HOME"]) / "glib-2.0/settings/keyfile"
            keyfile.parent.mkdir(parents=True, exist_ok=True)
            keyfile.write_text(f"[org/gnome/desktop/interface]\ngtk-theme='{'Adwaita-dark' if theme == 'dark' else 'Adwaita'}'\ncolor-scheme='{'prefer-dark' if theme == 'dark' else 'prefer-light'}'\n")

    def start_service():
        return subprocess.Popen([str(artifacts / channel["service_binary"]), str(scratch / "projects"),
                                 args.python, str(ROOT), "--fixture-launcher", str(ROOT / "tests/test_workspace_journey.py")],
                                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                stderr=(scratch / "service.log").open("ab"), start_new_session=True)

    def js(body, seconds=10):
        (probe_dir / "response.json").unlink(missing_ok=True)
        temporary = probe_dir / f"request-{next(sequence)}.tmp"
        temporary.write_text(body)
        temporary.rename(probe_dir / "request.js")
        answer = probe_dir / "response.json"
        wait_for(lambda: answer.exists() or (app is not None and app.poll() is not None), "probe_no_answer", seconds)
        if not answer.exists():
            raise RuntimeError("app_exited_during_request:" + json.dumps(exit_status(app.poll())))
        text = answer.read_text()
        if text.startswith("!"):
            raise RuntimeError("probe_script_failed:" + text[1:160])
        return text

    def query(body, seconds=10):
        return json.loads(js(body, seconds))

    def launch(theme="dark", ready=True):
        settings(theme)
        (probe_dir / "request.js").unlink(missing_ok=True)
        env = {**(wayland_env or os.environ), "LD_PRELOAD": str(shim), "CW_PROBE_DIR": str(probe_dir)}
        return spawn_app(env)

    def loaded():
        wait_for(lambda: js("return document.readyState === 'complete' && !!document.querySelector('.titlebar') ? 'yes' : ''", 5) == "yes", "page_not_ready", 30)
        js("await document.fonts.ready; return 'yes';", 10)
        if args.journey_wayland:
            js("window.__cwClick = 0; document.addEventListener('mousedown', () => window.__cwClick++, {once:true}); return 'yes';")
            remote_call(remote, interface, "NotifyPointerMotionRelative", GLib.Variant("(dd)", (-float(MONITOR[0] * 2), -float(MONITOR[1] * 2))))
            time.sleep(.15)
            remote_call(remote, interface, "NotifyPointerMotionRelative", GLib.Variant("(dd)", (float(MONITOR[0] // 2), float(MONITOR[1] // 2))))
            time.sleep(.15)
            remote_call(remote, interface, "NotifyPointerButton", GLib.Variant("(ib)", (272, True)))
            time.sleep(.06)
            remote_call(remote, interface, "NotifyPointerButton", GLib.Variant("(ib)", (272, False)))
            time.sleep(.15)
            js("@focus")
            outcome("wayland_focus_observation", "diagnostic", facts=query("return JSON.stringify({focused:document.hasFocus(),clicks:window.__cwClick});"))
            wait_for(lambda: js("return document.hasFocus() ? 'yes' : '';") == "yes", "wayland_native_keyboard_focus")

    def audit(label, theme):
        time.sleep(0.15)
        report = query(AUDIT)
        wrong = judge(report)
        if report["page"]["theme"] != theme:
            wrong.append("theme_is_not_the_system_theme")
        if "settings" not in label and report["themed"]:
            wrong.append("theme_control_outside_settings")
        summary = {"screen": label, "theme": report["page"]["theme"], "surface": report["page"]["surface"], "viewport": report["page"]["viewport"], "root": report["page"]["rootBox"],
                   "bar_h": report["chrome"]["barBox"] and report["chrome"]["barBox"]["h"], "bar_controls": report["chrome"]["controls"], "win_buttons": report["chrome"]["winButtons"],
                   "split": report["chrome"]["split"] and [part["w"] for part in report["chrome"]["split"]["parts"]], "faces": report["fonts"]["faces"], "loaded": report["fonts"]["loaded"],
                   "shadow_blur_max": report["maxShadow"], "strips": report["ui"]["strips"], "crumb": report["ui"]["crumb"][:60], "main": report["ui"]["mainText"], "classes_without_a_rule": report["unknown"]}
        if wrong:
            fault("visual_audit", screen=label, wrong=wrong, light=report["light"], native=report["native"], dashes=report["dashes"], clipped=report["clipped"], themed=report["themed"], plain_icons=report["ui"]["plainIconDetail"],
                  outside=report["chrome"]["outside"], overlaps=report["chrome"]["overlaps"], page=report["page"], chrome=report["chrome"], fonts=report["fonts"])
        else:
            outcome("visual_audit", **summary)
        return report

    def popovers(label):
        results = query(POPOVERS, 20)
        bad = [p for p in results if not (p["supported"] and p["open"] and p["display"] != "none" and p["rect"][2] > 0 and p["rect"][3] > 0 and p["inside"] and p["items"] > 0)]
        if bad or not results:
            fault("popovers", screen=label, results=results, bad=[p["id"] for p in bad])
        else:
            outcome("popovers", screen=label, opened=[{k: p[k] for k in ("id", "rect", "items")} for p in results])

    def nav(label):
        wanted = json.dumps(label)
        found = js(f"const el = [...document.querySelectorAll('.sidebar .nav')].find(e => e.textContent.trim().startsWith({wanted})); if (!el) return 'no'; el.click(); return 'yes';")
        if found != "yes":
            raise RuntimeError("nav_missing:" + label)
        time.sleep(0.2)

    def follow(selector):
        """Each link the selector finds, opened and audited in turn."""
        return query(f"return JSON.stringify([...document.querySelectorAll({json.dumps(selector)})].map(a => a.getAttribute('href')).filter(Boolean));")

    def open_link(href):
        js(f"document.querySelector('a[href={json.dumps(href)}]').click(); return 'yes';")

    def press(text):
        wanted = json.dumps(text)
        if js(f"const scope = document.querySelector('dialog[open]') ?? document; const buttons = [...scope.querySelectorAll('button')].filter(e => !e.disabled && e.getAttribute('aria-disabled') !== 'true'); const b = buttons.find(e => e.textContent.trim() === {wanted}) ?? buttons.find(e => e.textContent.trim().startsWith({wanted})); if (!b) return 'no'; b.click(); return 'yes';") != "yes":
            raise RuntimeError("button_missing:" + text)
        time.sleep(0.2)

    def inspector_open(label):
        """The inspector sits beside the page, never over it."""
        shown = query("const i = document.querySelector('.inspector'), m = document.querySelector('.main'), a = i.getBoundingClientRect(), b = m.getBoundingClientRect(); return JSON.stringify({ visible: getComputedStyle(i).display !== 'none' && a.width > 0, overlap: a.width > 0 && a.left < b.right - .5, width: a.width, rows: i.querySelectorAll('.ins-sec').length, empty: [...i.querySelectorAll('dd')].filter(d => !d.textContent.trim()).length });")
        (outcome if shown["visible"] and not shown["overlap"] and shown["rows"] and not shown["empty"] else fault)("inspector", screen=label, **shown)

    def settings_screens(label, theme):
        js("document.querySelector('a[aria-label=\"Settings\"]').click(); return 'yes';")
        time.sleep(0.3)
        links = query("return JSON.stringify([...document.querySelectorAll('.sidebar a.snav')].filter(a => a.getAttribute('href')).map(a => a.getAttribute('href')));")
        for href in links:
            js(f"document.querySelector('.sidebar a.snav[href={json.dumps(href)}]').click(); return 'yes';")
            audit(f"settings {href} {label}", theme)
        popovers(f"settings {label}")
        js("[...document.querySelectorAll('.sidebar .nav')].find(e => e.textContent.includes('Back to workspace')).click(); return 'yes';")
        time.sleep(0.2)

    def escape():
        if args.journey_wayland:
            wayland_key(0xff1b, True); wayland_key(0xff1b, False)
        else:
            subprocess.run(["xdotool", "key", "--clearmodifiers", "Escape"], check=True)

    def resize(width, height):
        if args.journey_wayland:
            js(f"@resize {width} {height}")
            wait_for(lambda: query("@native")["top"] == [width, height], "wayland_journey_resize")
        else:
            subprocess.run(["xdotool", "windowsize", window_id(), str(width), str(height)], check=True)

    def common_screens(label, theme):
        baseline = None
        entry = query("const a = [...document.querySelectorAll('.sidebar .nav')].find(e => e.textContent.trim().startsWith('Archive')); return JSON.stringify({ disabled: a.getAttribute('aria-disabled'), href: a.getAttribute('href') });")
        if entry["disabled"] == "true" or not entry["href"]:
            fault("archive_entry_is_off", archive=entry)
        for screen in ("Home", "Inbox", "Archive", "New task"):
            nav(screen)
            report = audit(f"{screen.lower()} {label}", theme)
            if baseline is None:
                baseline = [row[1:] for row in report["ui"]["navs"]]
            elif [row[1:] for row in report["ui"]["navs"]] != baseline:
                fault("navigation_moved", screen=screen, before=baseline, after=report["ui"]["navs"])
        nav("Home")
        js("[...document.querySelectorAll('.sidebar .nav')].find(e => e.textContent.includes('Search')).click(); return 'yes';")
        wait_for(lambda: js("return document.querySelector('dialog[open]') ? 'yes' : ''") == "yes", "search_palette")
        audit(f"search {label}", theme)
        escape()
        wait_for(lambda: js("return document.querySelector('dialog[open]') ? '' : 'yes'") == "yes", "search_palette_closed")
        js("[...document.querySelectorAll('.sb-foot button')].find(b => b.textContent.includes('Add project')).click(); return 'yes';")
        time.sleep(0.2)
        audit(f"add-project dialog {label}", theme)
        escape()
        time.sleep(0.2)
        settings_screens(label, theme)

    def window_id():
        ids = subprocess.check_output(["xdotool", "search", "--all", "--onlyvisible", "--pid", str(app.pid),
                                       "--name", "^" + channel["title"] + "$"], text=True).splitlines()
        if len(ids) != 1:
            raise RuntimeError("owned_window_identity_ambiguous")
        return ids[0]

    def geometry(window):
        text = subprocess.check_output(["xdotool", "getwindowgeometry", "--shell", window], text=True)
        values = dict(line.split("=", 1) for line in text.splitlines() if "=" in line)
        return int(values["X"]), int(values["Y"]), int(values["WIDTH"]), int(values["HEIGHT"])

    def xprop(window, name):
        return subprocess.run(["xprop", "-id", window, name], capture_output=True, text=True).stdout.strip()

    def rect_of(selector):
        return query(f"const r = document.querySelector({json.dumps(selector)}).getBoundingClientRect(); return JSON.stringify([r.x + r.width / 2, r.y + r.height / 2]);")

    def pointer(window, x, y, *actions):
        ox, oy, _, _ = geometry(window)
        subprocess.run(["xdotool", "mousemove", str(ox + round(x)), str(oy + round(y)), *actions], check=True)
        time.sleep(0.08)

    def drag(window, start, by):
        pointer(window, *start)
        subprocess.run(["xdotool", "mousedown", "1"], check=True)
        time.sleep(0.1)
        ox, oy, _, _ = geometry(window)
        ax, ay = ox + round(start[0]), oy + round(start[1])
        for step in range(1, 9):
            subprocess.run(["xdotool", "mousemove", str(ax + by[0] * step // 8), str(ay + by[1] * step // 8)], check=True)
            time.sleep(0.03)
        subprocess.run(["xdotool", "mouseup", "1"], check=True)
        time.sleep(0.3)

    def maximized(window):
        state = xprop(window, "_NET_WM_STATE")
        return "_NET_WM_STATE_MAXIMIZED_VERT" in state and "_NET_WM_STATE_MAXIMIZED_HORZ" in state

    def window_behaviour():
        window = window_id()
        hints = xprop(window, "_MOTIF_WM_HINTS")
        extents = xprop(window, "_NET_FRAME_EXTENTS")
        native = json.loads(js("@native"))
        if native["background"] != [24, 27, 26]:
            fault("window_background_is_not_the_surface", native=native)
        match = re.search(r"= (0x[0-9a-f]+), (0x[0-9a-f]+), (0x[0-9a-f]+)", hints)
        no_frame = bool(match) and (int(match.group(1), 16) & 2) != 0 and int(match.group(3), 16) == 0
        if not no_frame or native["decorated"] or native["top"] != native["web"]:
            fault("os_decoration_present", hints=hints, extents=extents, native=native)
        else:
            outcome("no_os_decoration", hints=hints, extents=extents, native=native)
        start = geometry(window)
        view = query("return JSON.stringify([innerWidth, innerHeight]);")
        if [start[2], start[3]] != view:
            fault("webview_does_not_fill_window", window=start, viewport=view)

        def click(selector):
            pointer(window, *rect_of(selector), "click", "1")
            time.sleep(0.35)

        # The real buttons, with the real pointer, through a real window manager.
        click('.win button[aria-label="Maximize"]')
        wait_for(lambda: maximized(window), "maximize_button")
        full = geometry(window)
        label = wait_for(lambda: js("return document.querySelector('.win button:nth-child(2)').getAttribute('aria-label') === 'Restore' ? 'Restore' : ''") == "Restore", "restore_label")
        filled = query("const r = document.getElementById('root').getBoundingClientRect(); return JSON.stringify([r.width, r.height, innerWidth, innerHeight]);")
        outcome("maximize", window=full, root_and_viewport=filled, label=label)
        if full[2:] != SCREEN or filled[:2] != list(SCREEN):
            fault("maximize_does_not_fill", window=full, root=filled)
        click('.win button[aria-label="Restore"]')
        wait_for(lambda: not maximized(window), "restore_button")
        outcome("restore", window=geometry(window))
        click('.win button[aria-label="Minimize"]')
        wait_for(lambda: "_NET_WM_STATE_HIDDEN" in xprop(window, "_NET_WM_STATE"), "minimize_button")
        outcome("minimize", state=xprop(window, "_NET_WM_STATE"))
        subprocess.run(["xdotool", "windowactivate", "--sync", window], check=False)
        wait_for(lambda: "_NET_WM_STATE_HIDDEN" not in xprop(window, "_NET_WM_STATE"), "unminimize")

        subprocess.run(["xdotool", "windowmove", window, "100", "60"], check=True)
        subprocess.run(["xdotool", "windowsize", window, "1300", "860"], check=True)
        time.sleep(0.4)
        before = geometry(window)
        blank = query("const b = document.querySelector('.tb-main').getBoundingClientRect(), g = document.querySelector('.tb-main > .grow').getBoundingClientRect(); return JSON.stringify([g.x + g.width / 2, b.y + b.height / 2]);")
        drag(window, blank, (140, 90))
        after = geometry(window)
        outcome("band_drag", before=before, after=after, blank=blank)
        # The window follows from where the pointer is when the slop is crossed, so a quick drag lands a step short.
        if abs(after[0] - before[0] - 140) > 30 or abs(after[1] - before[1] - 90) > 30 or after[2:] != before[2:]:
            fault("band_drag_failed", before=before, after=after)
        # A press after the drag must reach the page at once.
        pressed = js("return document.querySelector('.tb-toggle').getAttribute('aria-pressed');")
        click(".tb-toggle")
        if js("return document.querySelector('.tb-toggle').getAttribute('aria-pressed');") == pressed:
            fault("first_click_after_drag_lost")
        ring = query("const a = document.activeElement, s = getComputedStyle(a); return JSON.stringify({ active: a.tagName, focusVisible: a.matches(':focus-visible'), outline: s.outlineStyle });")
        if ring["focusVisible"] or ring["outline"] != "none":
            fault("focus_ring_stays_after_a_mouse_click", ring=ring)
        else:
            outcome("no_focus_ring_after_mouse_click", **ring)
        click(".tb-toggle")
        held = geometry(window)
        drag(window, rect_of(".tb-toggle"), (120, 60))
        if geometry(window)[:2] != held[:2]:
            fault("control_in_band_dragged_the_window", before=held, after=geometry(window))
        else:
            outcome("controls_do_not_drag", at=held)
        if js("return document.querySelector('.tb-toggle').getAttribute('aria-pressed');") != pressed:
            click(".tb-toggle")
        pointer(window, *blank, "click", "--repeat", "2", "--delay", "90", "1")
        wait_for(lambda: maximized(window), "double_click_maximize")
        outcome("double_click_maximizes", window=geometry(window))
        blank_full = query("const b = document.querySelector('.tb-main').getBoundingClientRect(), g = document.querySelector('.tb-main > .grow').getBoundingClientRect(); return JSON.stringify([g.x + g.width / 2, b.y + b.height / 2]);")
        pointer(window, *blank_full, "click", "--repeat", "2", "--delay", "90", "1")
        wait_for(lambda: not maximized(window), "double_click_restore")
        time.sleep(0.3)
        before = geometry(window)
        for edge, start_at, by, expect in (
                ("right", (before[2] - 2, before[3] / 2), (90, 0), (90, 0)),
                ("bottom", (before[2] / 2, before[3] - 2), (0, 70), (0, 70))):
            now = geometry(window)
            at = start_at if edge == "right" else (now[2] / 2, now[3] - 2)
            drag(window, at, by)
            grown = geometry(window)
            outcome("edge_resize", edge=edge, before=now, after=grown)
            if abs(grown[2] - now[2] - expect[0]) > 6 or abs(grown[3] - now[3] - expect[1]) > 6:
                fault("edge_resize_failed", edge=edge, before=now, after=grown)
        now = geometry(window)
        drag(window, (2, 2), (-30, -20))
        grown = geometry(window)
        outcome("edge_resize", edge="top_left", before=now, after=grown)
        if grown[2] - now[2] < 20 or grown[3] - now[3] < 12 or grown[0] >= now[0]:
            fault("edge_resize_failed", edge="top_left", before=now, after=grown)
        subprocess.run(["xdotool", "windowsize", window, "960", "540"], check=True)
        time.sleep(0.5)
        audit("home at minimum size", "dark")
        subprocess.run(["xdotool", "windowsize", window, "1440", "900"], check=True)
        time.sleep(0.4)

    def wayland_stage():
        """The owner's session type: a real headless GNOME compositor, its window rules and its pointer."""
        nonlocal app
        from gi.repository import Gio, GLib
        name = "cw-wayland"
        env = {k: v for k, v in os.environ.items() if k not in ("DISPLAY", "GDK_BACKEND", "GTK_A11Y", "NO_AT_BRIDGE")}
        # On Wayland GTK takes the theme from GSettings, as on the owner's desktop.
        keyfile = Path(os.environ["XDG_CONFIG_HOME"]) / "glib-2.0/settings/keyfile"
        keyfile.parent.mkdir(parents=True, exist_ok=True)
        keyfile.write_text("[org/gnome/desktop/interface]\ngtk-theme='Adwaita-dark'\ncolor-scheme='prefer-dark'\n")
        env["GSETTINGS_BACKEND"] = "keyfile"
        with (logs / "stage-compositor.stderr").open("wb") as sink:
            compositor = subprocess.Popen(["mutter", "--headless", "--wayland", "--no-x11", "--virtual-monitor", f"{MONITOR[0]}x{MONITOR[1]}", "--wayland-display", name],
                                          env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=sink, start_new_session=True)
        try:
            socket = Path(os.environ["XDG_RUNTIME_DIR"]) / name
            wait_for(lambda: socket.exists() or compositor.poll() is not None, "wayland_compositor_start", 20)
            if compositor.poll() is not None:
                raise RuntimeError("wayland_compositor_failed:" + json.dumps({**exit_status(compositor.returncode), "stderr": stderr_summary(logs / "stage-compositor.stderr")}))
            (probe_dir / "request.js").unlink(missing_ok=True)
            app = spawn_app({**env, "GDK_BACKEND": "wayland", "WAYLAND_DISPLAY": name, "LD_PRELOAD": str(shim), "CW_PROBE_DIR": str(probe_dir)})
            loaded()
            native = lambda: json.loads(js("@native"))
            state = native()
            size = lambda: tuple(native()["top"])
            if state["backend"] != "GdkWaylandDisplay" or state["decorated"] or state["top"] != state["web"]:
                fault("wayland_frame_present", native=state)
            else:
                outcome("wayland_no_frame", native=state)
            audit("home on wayland", "dark")
            press = lambda label: js(f"document.querySelector('.win button[aria-label={json.dumps(label)}]').click(); return 'yes';")
            press("Maximize")
            wait_for(lambda: size() == MONITOR, "wayland_maximize")
            wait_for(lambda: js("return document.querySelector('.win button[aria-label=\"Restore\"]') ? 'yes' : ''") == "yes", "wayland_restore_label")
            outcome("wayland_maximize", size=size())
            audit("home on wayland maximized", "dark")
            press("Restore")
            wait_for(lambda: size() != MONITOR, "wayland_restore")
            outcome("wayland_restore", size=size())

            bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
            call = lambda path, iface, method, value=None: bus.call_sync("org.gnome.Mutter.RemoteDesktop", path, iface, method, value, None, Gio.DBusCallFlags.NONE, 5000, None)
            remote = call("/org/gnome/Mutter/RemoteDesktop", "org.gnome.Mutter.RemoteDesktop", "CreateSession").unpack()[0]
            iface = "org.gnome.Mutter.RemoteDesktop.Session"
            call(remote, iface, "Start")
            js("window.__cwPointer = null; document.addEventListener('mousemove', e => { window.__cwPointer = [e.clientX, e.clientY]; }, true); return 'yes';")
            at = lambda: json.loads(js("return JSON.stringify(window.__cwPointer);"))
            nudge = lambda dx, dy: (call(remote, iface, "NotifyPointerMotionRelative", GLib.Variant("(dd)", (float(dx), float(dy)))), time.sleep(0.03))
            button = lambda down: (call(remote, iface, "NotifyPointerButton", GLib.Variant("(ib)", (272, down))), time.sleep(0.06))

            def go_to(x, y):
                for _ in range(40):
                    now = at()
                    if now is None:
                        nudge(40, 25)
                        continue
                    if abs(now[0] - x) <= 1 and abs(now[1] - y) <= 1:
                        return now
                    nudge(x - now[0], y - now[1])
                    time.sleep(0.05)
                raise RuntimeError("wayland_pointer_unreachable")

            def glide(dx, dy, steps=8):
                for _ in range(steps):
                    nudge(dx / steps, dy / steps)

            blank_at = "const b = document.querySelector('.tb-main').getBoundingClientRect(), g = document.querySelector('.tb-main > .grow').getBoundingClientRect(); return JSON.stringify([Math.round(g.x + g.width / 2), Math.round(b.y + b.height / 2)]);"
            blank = query(blank_at)
            go_to(*blank)
            button(True); glide(150, 60); button(False)
            time.sleep(0.4)
            nudge(1, 0); time.sleep(0.2)
            after = at()
            moved = abs(after[0] - blank[0]) < 60 and abs(after[1] - blank[1]) < 40
            (outcome if moved else fault)("wayland_band_drag", pointer_in_page_before=blank, after=after, pointer_moved=[150, 60])
            pressed = js("return document.querySelector('.tb-toggle').getAttribute('aria-pressed');")
            go_to(*query("const r = document.querySelector('.tb-toggle').getBoundingClientRect(); return JSON.stringify([Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)]);"))
            button(True); button(False)
            time.sleep(0.2)
            if js("return document.querySelector('.tb-toggle').getAttribute('aria-pressed');") == pressed:
                fault("wayland_first_click_after_drag_lost")
            else:
                outcome("wayland_click_after_drag")
            button(True); button(False)
            time.sleep(0.2)
            blank = query(blank_at)
            go_to(*blank)
            button(True); button(False); time.sleep(0.05); button(True); button(False)
            wait_for(lambda: size() == MONITOR, "wayland_double_click_maximize")
            outcome("wayland_double_click_maximizes", size=size())
            blank = query(blank_at)
            go_to(*blank)
            button(True); button(False); time.sleep(0.05); button(True); button(False)
            wait_for(lambda: size() != MONITOR, "wayland_double_click_restore")
            time.sleep(0.3)
            before = size()
            go_to(before[0] - 2, before[1] // 2)
            button(True); glide(-70, 0); button(False)
            time.sleep(0.4)
            right = size()
            (outcome if abs(before[0] - right[0] - 70) <= 8 and right[1] == before[1] else fault)("wayland_edge_resize", edge="right", before=before, after=right)
            go_to(right[0] // 2, right[1] - 2)
            button(True); glide(0, -50); button(False)
            time.sleep(0.4)
            bottom = size()
            (outcome if abs(right[1] - bottom[1] - 50) <= 8 and bottom[0] == right[0] else fault)("wayland_edge_resize", edge="bottom", before=right, after=bottom)
            audit("home on wayland resized", "dark")
            # Wayland has no way to read minimized state back, so this proves the call is accepted and the app lives on.
            press("Minimize")
            time.sleep(0.4)
            outcome("wayland_minimize_requested", alive=app.poll() is None, observed="state not readable on Wayland")
            press("Close window")
            if app.wait(timeout=8) != 0:
                raise RuntimeError("wayland_close_failed")
            stop(app)
            app = None
            outcome("wayland_close_button", exit_code=0)
        finally:
            if app is not None:
                stop(app)
                app = None
            stop(compositor)

    try:
        flags = subprocess.check_output(["pkg-config", "--cflags", "--libs", "webkit2gtk-4.1", "gtk+-3.0"], text=True).split()
        build = subprocess.run(["gcc", "-shared", "-fPIC", "-O1", "-w", "-o", str(shim), str(source), *flags], capture_output=True, text=True)
        if build.returncode:
            raise RuntimeError("probe_build_failed:" + build.stderr[-400:])
        xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", f"{SCREEN[0]}x{SCREEN[1]}x24", "-nolisten", "tcp"],
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, start_new_session=True)
        display = xvfb.stdout.readline().strip()
        if not display.isdigit():
            raise RuntimeError("private_display_unavailable")
        os.environ["DISPLAY"] = ":" + display
        os.environ["GDK_BACKEND"] = "x11"
        # Xvfb has no hardware compositor; keep the WebKit sandbox enabled.
        os.environ["WEBKIT_DISABLE_COMPOSITING_MODE"] = "1"
        os.environ["NO_AT_BRIDGE"] = "0"
        os.environ["GTK_A11Y"] = "always"
        wm = subprocess.Popen(["openbox", "--sm-disable"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                              stderr=subprocess.DEVNULL, start_new_session=True)
        wait_for(lambda: subprocess.run(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"], capture_output=True, text=True).stdout.count("window id"), "window_manager_start")
        runtime = scratch / "runtime" / channel["runtime_namespace"]
        service = start_service()
        wait_for(lambda: (runtime / "service.sock").exists(), "service_start")
        if args.only == "wayland":
            wayland_stage()
            return 1 if FAILURES else 0
        if args.journey_wayland:
            from gi.repository import Gio, GLib
            import xml.etree.ElementTree as ET
            wayland_env = {k: v for k, v in os.environ.items() if k not in ("DISPLAY", "GDK_BACKEND")}
            wayland_env.update(GSETTINGS_BACKEND="keyfile", GDK_BACKEND="wayland", WAYLAND_DISPLAY="cw-full-wayland")
            with (logs / "compositor.stderr").open("wb") as sink:
                compositor = subprocess.Popen(["mutter", "--headless", "--wayland", "--no-x11", "--virtual-monitor", f"{MONITOR[0]}x{MONITOR[1]}", "--wayland-display", "cw-full-wayland"],
                                              env=wayland_env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=sink, start_new_session=True)
            wait_for(lambda: (Path(os.environ["XDG_RUNTIME_DIR"]) / "cw-full-wayland").is_socket(), "full_wayland_compositor")
            bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
            def remote_call(path, interface, method, value=None):
                return bus.call_sync("org.gnome.Mutter.RemoteDesktop", path, interface, method, value, None, Gio.DBusCallFlags.NONE, 5000, None)
            remote = remote_call("/org/gnome/Mutter/RemoteDesktop", "org.gnome.Mutter.RemoteDesktop", "CreateSession").unpack()[0]
            interface = "org.gnome.Mutter.RemoteDesktop.Session"
            xml = remote_call(remote, "org.freedesktop.DBus.Introspectable", "Introspect").unpack()[0]
            methods = {m.attrib["name"] for i in ET.fromstring(xml).findall("interface") if i.attrib["name"] == interface for m in i.findall("method")}
            assert {"Start", "NotifyKeyboardKeysym", "NotifyPointerMotionRelative", "NotifyPointerButton"} <= methods, "installed_mutter_keyboard_api_unavailable"
            remote_call(remote, interface, "Start")
            def wayland_key(key, down):
                remote_call(remote, interface, "NotifyKeyboardKeysym", GLib.Variant("(ub)", (key, down)))
            # Materialize the virtual keyboard before GTK enumerates the seat.
            wayland_key(0xffe1, True); wayland_key(0xffe1, False)
            time.sleep(.3)
            outcome("full_wayland_keyboard_capability", observed_by="installed session introspection")
        app = launch("dark")
        desktop = pyatspi.Registry.getDesktop(0)

        def close_window():
            if args.journey_wayland:
                js('document.querySelector(".win button:last-child").click(); return "yes";')
                assert app.wait(timeout=8) == 0, "wayland_journey_close_failed"
                stop(app)
                outcome("native_window_close", exit_code=0, backend="wayland")
                return
            # Send the standard WM_DELETE_WINDOW client message to our exact X window.
            # No process signal, screenshot or browser inspector is used.
            import ctypes as c
            x = c.CDLL("libX11.so.6")
            class Data(c.Union):
                _fields_ = [("l", c.c_long * 5)]
            class Client(c.Structure):
                _fields_ = [("type", c.c_int), ("serial", c.c_ulong), ("send_event", c.c_int),
                            ("display", c.c_void_p), ("window", c.c_ulong), ("message_type", c.c_ulong),
                            ("format", c.c_int), ("data", Data)]
            class Event(c.Union):
                _fields_ = [("client", Client), ("pad", c.c_long * 24)]
            x.XOpenDisplay.argtypes = [c.c_char_p]
            x.XOpenDisplay.restype = c.c_void_p
            x.XInternAtom.argtypes = [c.c_void_p, c.c_char_p, c.c_int]
            x.XInternAtom.restype = c.c_ulong
            x.XSendEvent.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_long, c.POINTER(Event)]
            x.XFlush.argtypes = [c.c_void_p]
            x.XCloseDisplay.argtypes = [c.c_void_p]
            windows = subprocess.check_output(["xdotool", "search", "--all", "--onlyvisible", "--pid", str(app.pid),
                                               "--name", "^" + channel["title"] + "$"], text=True).splitlines()
            if len(windows) != 1:
                raise RuntimeError("owned_window_identity_ambiguous")
            display_handle = x.XOpenDisplay(None)
            if not display_handle:
                raise RuntimeError("owned_display_unavailable")
            try:
                event = Event()
                event.client.type = 33
                event.client.send_event = 1
                event.client.display = display_handle
                event.client.window = int(windows[0])
                event.client.message_type = x.XInternAtom(display_handle, b"WM_PROTOCOLS", 0)
                event.client.format = 32
                event.client.data.l[0] = x.XInternAtom(display_handle, b"WM_DELETE_WINDOW", 0)
                if not x.XSendEvent(display_handle, event.client.window, 0, 0, c.byref(event)):
                    raise RuntimeError("window_close_message_failed")
                x.XFlush(display_handle)
            finally:
                x.XCloseDisplay(display_handle)
            if app.wait(timeout=8) != 0:
                raise RuntimeError("native_window_close_failed")
            stop(app)  # terminate any remaining owned renderer-process group members
            outcome("native_window_close", exit_code=0)

        def close_button():
            if args.journey_wayland:
                close_window()
                return
            window = window_id()
            pointer(window, *rect_of('.win button[aria-label="Close window"]'), "click", "1")
            if app.wait(timeout=8) != 0:
                raise RuntimeError("close_button_failed")
            stop(app)
            outcome("close_button", exit_code=0)

        def tree(node=None, depth=0):
            if node is None:
                roots = []
                try:
                    children = list(desktop)
                except Exception:
                    # The desktop child list itself can expire during window relaunch.
                    # Return no match; the caller's bounded wait must find the new app.
                    return []
                for child in children:
                    try:
                        if child and os.getpgid(child.get_process_id()) == app.pid:
                            roots.append(child)
                    except Exception:
                        # Even testing a defunct accessibility object's truth value can fail.
                        # Skip it without widening the exact owned process-group filter.
                        pass
                return [item for root in roots for item in tree(root)]
            if depth > 35:
                return []
            result = [node]
            try:
                for child in node:
                    if child:
                        result.extend(tree(child, depth + 1))
            except Exception:
                pass
            return result

        def find(name, role=None):
            for node in tree():
                try:
                    if node.name == name and (role is None or node.getRoleName() == role):
                        return node
                except Exception:
                    pass

        def text_seen(text):
            for node in tree():
                try:
                    if text in node.name:
                        return True
                    if text in node.queryText().getText(0, -1):
                        return True
                except Exception:
                    pass
            return False

        def click(name):
            def once():
                node = wait_for(lambda: find(name), "control_missing:" + name)
                state = node.getState()
                if not state.contains(pyatspi.STATE_ENABLED):
                    raise RuntimeError("control_disabled:" + name)
                if not node.queryAction().doAction(0):
                    raise RuntimeError("control_action_failed:" + name)
            retry_stale(once)
            time.sleep(0.15)

        def add_project(path):
            # Paseo's flow: a way in, then a field that searches the machine. Enter adds the first row, the typed path.
            click("Add project")
            options = "[...document.querySelectorAll('dialog[open] [role=option]')]"
            wait_for(lambda: js(f"return {options}.length ? 'yes' : ''") == "yes", "add_project_ways_in")
            js(f"{options}.find(o => o.textContent.startsWith('Search for folder')).click(); return 'yes';")
            wait_for(lambda: js("return document.querySelector('dialog[open] input') ? 'yes' : ''") == "yes", "add_project_field")
            js(f"const e = document.querySelector('dialog[open] input'); e.value = {json.dumps(str(path))}; e.dispatchEvent(new InputEvent('input', {{bubbles: true}})); return 'yes';")
            wait_for(lambda: js(f"return {options}.some(o => o.getAttribute('aria-selected') === 'true') ? 'yes' : ''") == "yes", "add_project_row")
            js("document.querySelector('dialog[open] input').dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true, cancelable: true})); return 'yes';")

        def focus_refused(name, node):
            """What the page and the accessibility tree say about a field that refused focus. It explains the failure, it never clears it."""
            def states(item):
                try:
                    return sorted(str(state) for state in item.getState().getStates())
                except Exception as error:
                    return ["unreadable:" + type(error).__name__]
            page = query(f"""const label = {json.dumps(name)}, fields = [...document.querySelectorAll('textarea, input')].filter(e => e.getAttribute('aria-label') === label);
return JSON.stringify({{fields: fields.map(e => {{ const r = e.getBoundingClientRect(), s = getComputedStyle(e); return {{disabled: e.disabled, inert: !!e.closest('[inert]'), inDialog: !!e.closest('dialog'), hint: e.placeholder.slice(0, 80), box: [r.x, r.y, r.width, r.height].map(Math.round), display: s.display, visibility: s.visibility}}; }}),
  open: [...document.querySelectorAll('dialog[open]')].map(d => d.getAttribute('aria-label')), active: `${{document.activeElement?.tagName}}:${{document.activeElement?.getAttribute('aria-label') ?? ''}}`, hasFocus: document.hasFocus(), route: location.pathname, viewport: [innerWidth, innerHeight]}});""")
            def named(item):
                try:
                    return item.name == name
                except Exception:
                    return False
            same = [item for item in tree() if named(item)]
            retried = []
            for item in same[:4]:
                try:
                    retried.append(bool(item.queryComponent().grabFocus()))
                except Exception as error:
                    retried.append(type(error).__name__)
            outcome("field_focus_refused", "diagnostic", field=name, page=page, refused_node=states(node), nodes=[states(item) for item in same[:4]], node_count=len(same), retried=retried)

        def fill(name, value):
            def focused():
                field = wait_for(lambda: find(name), "field_missing:" + name)
                if not field.queryComponent().grabFocus():
                    focus_refused(name, field)
                    raise RuntimeError("field_focus_failed:" + name)
                return field
            node = retry_stale(focused)
            time.sleep(0.2)
            if args.journey_wayland:
                wayland_key(0xffe3, True)
                wayland_key(ord("a"), True); wayland_key(ord("a"), False)
                wayland_key(0xffe3, False)
                for char in value:
                    if char == "\n":
                        wayland_key(0xffe1, True)
                        wayland_key(0xff0d, True); wayland_key(0xff0d, False)
                        wayland_key(0xffe1, False)
                    else:
                        key = ord(char) if ord(char) <= 255 else 0x01000000 | ord(char)
                        wayland_key(key, True); wayland_key(key, False)
                    time.sleep(.008)
            else:
                subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+a"], check=True)
                for index, line in enumerate(value.split("\n")):
                    if index:
                        subprocess.run(["xdotool", "key", "--clearmodifiers", "shift+Return"], check=True)
                    subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "8", "--", line], check=True)
            # WebKit can include the new value in an implicit label. Verify the exact
            # entry that acquired native focus, rather than looking up its old name.
            def entered():
                try:
                    return node.queryText().getText(0, -1) == value
                except Exception as error:
                    if not node_went_stale(error):
                        raise
                    return js("return document.activeElement?.value ?? '';") == value
            wait_for(entered, "field_value_mismatch:" + name, seconds=3)

        rect = retry_stale(lambda: wait_for(lambda: find("Add project"), "bundled_window_loaded").queryComponent().getExtents(pyatspi.DESKTOP_COORDS))
        if rect.width <= 0 or rect.height <= 0:
            raise RuntimeError("rendered_control_has_no_geometry")
        outcome("bundled_native_window", binary_sha256=hashlib.sha256((artifacts / channel["desktop_binary"]).read_bytes()).hexdigest(),
                evidence="WebKitGTK accessible rendered control has nonzero screen geometry")
        loaded()
        if args.only == "voice":
            # Inspect the actual installed webview, not a presumed browser speech capability.
            facts = query("""const speech = typeof window.SpeechRecognition === 'function' || typeof window.webkitSpeechRecognition === 'function';
const result = await Promise.race([(async()=>{try {const stream=await navigator.mediaDevices.getUserMedia({audio:true}); stream.getTracks().forEach(t=>t.stop()); return 'granted';}catch(e){return e.name;}})(),new Promise(r=>setTimeout(()=>r('unobservable_timeout'),1500))]);
return JSON.stringify({speech, capture:result, mediaDevices:typeof navigator.mediaDevices?.getUserMedia});""")
            outcome("webview_voice_capabilities", "diagnostic", **facts)
            assert facts["speech"] is False, "speech_api_now_available_reassess_native_route"
            # Fixture PCM feeds a real isolated PulseAudio monitor. No host device or audio socket is mounted.
            audio = Path(os.environ["XDG_RUNTIME_DIR"]) / "pulse"
            audio.mkdir(mode=0o700, exist_ok=True)
            pulse = subprocess.Popen(["pulseaudio", "-n", "--daemonize=no", "--exit-idle-time=-1",
                                      "--load=module-native-protocol-unix socket=" + str(audio / "native") + " auth-anonymous=1",
                                      "--load=module-null-sink sink_name=cw_voice"],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            wait_for(lambda: (audio / "native").exists(), "voice_audio_server")
            subprocess.run(["pactl", "set-default-source", "cw_voice.monitor"], check=True)
            state_voice = Path(os.environ["XDG_STATE_HOME"]) / "cortex-workspace-dev/voice"
            state_voice.mkdir(parents=True)
            model_name = json.loads((ROOT / "packaging/voice/model.json").read_text())["name"]
            shutil.copytree(artifacts / "voice-model", state_voice / model_name)
            add_project(project)
            wait_for(lambda: find("Activate"), "voice_registered")
            subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+n"], check=True)
            wait_for(lambda: find("Describe the task"), "voice_composer")
            fill("Describe the task", "Typed draft")
            subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+d"], check=True)
            def voice_status():
                return js("return document.querySelector('.micbar [role=status]')?.textContent ?? '';")
            def voice_text():
                return js("return document.querySelector('.mic-text')?.textContent ?? '';")
            wait_for(lambda: "Listening" in voice_status(), "voice_native_recording", 30)
            playback = subprocess.Popen(["paplay", "--device=cw_voice", str(artifacts / "voice-fixture.wav")],
                                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            try:
                wait_for(lambda: "родион" in voice_text(), "voice_live_russian_text", 20)
                assert playback.wait(timeout=15) == 0
            finally:
                stop(playback)
            subprocess.run(["xdotool", "windowsize", window_id(), "1100", "780"], check=True)
            time.sleep(.3)
            layout = query("""const b=document.querySelector('.micbar'), t=b.querySelector('.mic-text'), c=b.closest('.composer'), br=b.getBoundingClientRect(), cr=c.getBoundingClientRect();
return JSON.stringify({textWidth:t.getBoundingClientRect().width,textHeight:t.getBoundingClientRect().height,wrap:getComputedStyle(t).whiteSpace,
contained:br.left>=cr.left-1 && br.right<=cr.right+1,controls:[...b.querySelectorAll('button')].every(x=>{const r=x.getBoundingClientRect();return r.width>0&&r.left>=br.left-1&&r.right<=br.right+1&&r.bottom<=br.bottom+1;})});""")
            assert layout["textWidth"] > 0 and layout["textHeight"] > 0 and layout["wrap"] == "pre-wrap" and layout["contained"] and layout["controls"], "voice_composer_clipped"
            outcome("voice_composer_geometry", **layout)
            subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+d"], check=True)
            wait_for(lambda: "Ready to insert" in voice_status(), "voice_stopped_without_send", 20)
            assert js("return document.querySelector('[aria-label=\"Describe the task\"]').value;") == "Typed draft"
            assert not (project / ".cortex").exists() and not (project / ".cortex-dev").exists(), "voice_must_not_start_engine_or_save_task"
            press("Insert")
            wait_for(lambda: not js("return document.querySelector('.micbar') ? 'yes' : '';"), "voice_insert_closed")
            inserted = js("return document.querySelector('[aria-label=\"Describe the task\"]').value;")
            assert inserted.startswith("Typed draft ") and "родион" in inserted, "voice_insert_lost_draft"
            # Mic toggles use the same capture path; cancelling preserves the earlier editable draft.
            js("document.querySelector('[aria-label=\"Voice input\"]').click(); return 'yes';")
            wait_for(lambda: "Listening" in voice_status(), "voice_mic_button_restart", 30)
            js("document.querySelector('.micbar button').click(); return 'yes';")
            wait_for(lambda: not js("return document.querySelector('.micbar') ? 'yes' : '';"), "voice_cancel")
            assert js("return document.querySelector('[aria-label=\"Describe the task\"]').value;") == inserted
            wait_for(lambda: subprocess.check_output(["pactl","list","short","source-outputs"],text=True).strip() == "", "voice_capture_reaped")
            assert not (project / ".cortex").exists() and not (project / ".cortex-dev").exists()
            # Closed native controls exercise the same real socket manager, with no test-only product route.
            bridge = query("return JSON.stringify({invoke:typeof window.__TAURI_INTERNALS__?.invoke});")
            assert bridge["invoke"] == "function", "native_invoke_not_observed"
            first = query("const p=await window.__TAURI_INTERNALS__.invoke('workspace_request',{operation:'voice.start',message:{requestId:'voice-lease-check'}}); return JSON.stringify(p);")
            duplicate = query("return JSON.stringify(await window.__TAURI_INTERNALS__.invoke('workspace_request',{operation:'voice.start',message:{requestId:'voice-lease-check'}}));")
            assert first["sessionId"] == duplicate["sessionId"], "voice_duplicate_started_again"
            refusal = js("try {await window.__TAURI_INTERNALS__.invoke('workspace_request',{operation:'voice.start',message:{requestId:'voice-busy-check'}});return 'accepted';}catch(e){return String(e);}")
            assert "voice_busy" in refusal, "voice_busy_not_bound"
            refusal = js("try {await window.__TAURI_INTERNALS__.invoke('workspace_request',{operation:'voice.read',message:{sessionId:'wrong-session'}});return 'accepted';}catch(e){return String(e);}")
            assert "voice_session_unknown" in refusal, "voice_session_not_bound"
            wait_for(lambda: subprocess.check_output(["pactl","list","short","source-outputs"],text=True).strip() != "", "voice_lease_capture")
            close_window()
            app = None
            wait_for(lambda: subprocess.check_output(["pactl","list","short","source-outputs"],text=True).strip() == "", "voice_window_lease_expiry", 15)
            outcome("voice_socket_lease", duplicate_start_same_identity=True,busy_refused=True,wrong_session_refused=True,window_close_capture_reaped=True)
            outcome("native_voice_round_trip", capture="real PulseAudio virtual source", transcription="real sherpa-onnx Russian fixture",
                    ctrl_d_start_stop=True,live_text=True,insert_preserves_draft=True,cancel=True,never_auto_sent=True,engine_started=False)
            return 0
        if args.only == "terminal":
            add_project(project)
            wait_for(lambda: find("Activate"), "terminal_registered")
            # PTYs are owner sessions; opening one must not activate an ACP engine.
            subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+j"], check=True)
            wait_for(lambda: js("return document.querySelector('.terminal-status')?.textContent === 'Running' ? 'yes' : '';") == "yes", "terminal_running")
            assert not (project / ".cortex").exists() and not (project / ".cortex-dev").exists()
            def terminal_text():
                return js("return document.querySelector('.terminal-screen')?.textContent ?? '';")
            def shell_input(text):
                js("document.querySelector('.xterm-helper-textarea').focus(); return 'yes';")
                subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "3", "--", text], check=True)
                subprocess.run(["xdotool", "key", "--clearmodifiers", "Return"], check=True)
            shell_input("printf 'CW_%s\\n' TERMINAL; pwd > terminal-cwd; id -u > terminal-uid; stty size > terminal-size; echo $$ > terminal-pid")
            wait_for(lambda: (project / "terminal-pid").is_file() and "CW_TERMINAL" in terminal_text(), "terminal_round_trip")
            assert (project / "terminal-cwd").read_text().strip() == str(project)
            assert (project / "terminal-uid").read_text().strip() == str(os.getuid())
            pid = int((project / "terminal-pid").read_text().strip())
            before = (project / "terminal-size").read_text().strip()
            subprocess.run(["xdotool", "windowsize", window_id(), "1100", "780"], check=True)
            time.sleep(.6)
            shell_input("stty size > terminal-resized")
            wait_for(lambda: (project / "terminal-resized").is_file(), "terminal_resize")
            assert (project / "terminal-resized").read_text().strip() != before
            # Real preference controls in this disposable app, never the owner's settings.
            project_path = js("return location.pathname;")
            for theme in ("Light", "Dark"):
                js("document.querySelector('a[aria-label=Settings]').click(); return 'yes';")
                click(theme)
                js(f"history.pushState(null,'',{json.dumps(project_path)}); dispatchEvent(new PopStateEvent('popstate')); return 'yes';")
                wait_for(lambda: "CW_TERMINAL" in terminal_text(), "terminal_theme_reattach")
                facts = query("const t=document.querySelector('.terminal'), row=document.querySelector('.xterm-rows'), viewport=document.querySelector('.xterm-scrollable-element'); return JSON.stringify({theme:document.documentElement.dataset.theme,font:getComputedStyle(row).fontFamily,bg:getComputedStyle(t).backgroundColor,renderer:getComputedStyle(viewport).backgroundColor,width:row.clientWidth,height:row.clientHeight});")
                outcome("terminal_theme_facts", "diagnostic", **facts)
                assert facts["theme"] == theme.lower(), "terminal_theme_mismatch"
                assert "Cousine" in facts["font"] and facts["width"] > 0 and facts["height"] > 0, "terminal_font_or_geometry"
                assert facts["bg"] == facts["renderer"], "terminal_renderer_background"
            js("document.querySelector('[aria-label=\"Hide terminal\"]').click(); return 'yes';")
            assert os.path.exists(f"/proc/{pid}")
            subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+j"], check=True)
            wait_for(lambda: "CW_TERMINAL" in terminal_text(), "terminal_hide_reattach")
            js("document.querySelector('a[aria-label=Settings]').click(); return 'yes';")
            click("System")
            js(f"history.pushState(null,'',{json.dumps(project_path)}); dispatchEvent(new PopStateEvent('popstate')); return 'yes';")
            close_window()
            app = launch("light")
            wait_for(lambda: find("Add project"), "terminal_reopen_window")
            loaded()
            js("[...document.querySelectorAll('.proj')].find(p=>p.textContent.includes('journey')).click(); return 'yes';")
            subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+j"], check=True)
            wait_for(lambda: "CW_TERMINAL" in terminal_text(), "terminal_retained_after_window_close")
            shell_input("echo $$ > terminal-reattached-pid")
            wait_for(lambda: (project / "terminal-reattached-pid").is_file(), "terminal_reattached_pid")
            assert int((project / "terminal-reattached-pid").read_text().strip()) == pid
            js("document.querySelector('[aria-label=\"Close shell\"]').click(); return 'yes';")
            wait_for(lambda: js("return document.querySelector('.terminal-status')?.textContent === 'Closed' ? 'yes' : '';") == "yes", "terminal_explicit_close")
            assert not os.path.exists(f"/proc/{pid}")
            outcome("native_terminal_round_trip", keyboard="Ctrl+J and real PTY input", retained_after_window_close=True, resized=True, shell_reaped=True, engine_started=False)
            return 0
        if args.only == "attachments":
            add_project(project)
            wait_for(lambda: find("Activate"), "attachment_project")
            click("Activate")
            wait_for(lambda: not find("Activate"), "attachment_activation")
            click("New task")
            fill("Describe the task", "Explain this disposable project.")
            click("Save as draft")
            wait_for(lambda: js("return document.querySelector('.card-link') ? 'yes' : ''") == "yes", "attachment_draft")
            js("document.querySelector('.card-link').click(); return 'yes';")
            click("Start")
            wait_for(lambda: text_seen("Which direction"), "attachment_question")
            (project / "expect-attachments").touch()
            click("… Other answer")
            fill("Your answer", "first line\nsecond line")

            # Drive the actual GTK file chooser, not a synthetic FileList.
            picked = scratch / "notes.txt"
            picked.write_text("answer fixture")
            pointer(window_id(), *rect_of('.ws-dc-other button[aria-label="Attach files"]'), "click", "1")
            time.sleep(.5)
            subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+l"], check=True)
            subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "3", "--", str(picked)], check=True)
            subprocess.run(["xdotool", "key", "--clearmodifiers", "Return"], check=True)
            time.sleep(.5)
            subprocess.run(["xdotool", "key", "--clearmodifiers", "Return"], check=True)
            wait_for(lambda: js("return document.querySelector('.ws-dc-other .ws-att')?.textContent.includes('notes.txt') ? 'yes' : '';") == "yes", "native_file_picker")
            outcome("webkitgtk_native_file_picker", surface="Other answer")
            click("Send answer")
            wait_for(lambda: (project / "answer-resource-received").exists() or text_seen("changed on the machine"), "attachment_answer")
            if not (project / "answer-resource-received").exists():
                click("Send answer")
                wait_for(lambda: (project / "answer-resource-received").exists(), "attachment_answer_retry")
            outcome("attachment_answer_resource_received", engine="fake", multiline=True)

            clipboard_script = scratch / "clipboard_fixture.py"
            clipboard_script.write_text("""import sys, gi
from pathlib import Path
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk, GdkPixbuf
pixel=GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB,True,8,1,1)
pixel.fill(0x506070ff)
clipboard=Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
clipboard.set_image(pixel)
Path(sys.argv[1]).touch()
Gtk.main()
""")
            ready = scratch / "clipboard-ready"
            clipboard = subprocess.Popen([sys.executable, "-B", str(clipboard_script), str(ready)],
                                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            try:
                wait_for(ready.exists, "image_clipboard_ready")
                js("document.addEventListener('paste', e => { window.__pasteFacts = {types:[...e.clipboardData.types], files:e.clipboardData.files.length, items:[...e.clipboardData.items].map(i => [i.kind,i.type,!!i.getAsFile()])}; }, {once:true}); return 'yes';")
                pointer(window_id(), *rect_of('[aria-label="Message"]'), "click", "1")
                subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+v"], check=True)
                time.sleep(.8)
                outcome("clipboard_event_facts", "diagnostic", facts=query("return JSON.stringify(window.__pasteFacts ?? null);"))
                wait_for(lambda: js("return document.querySelector('.dock .composer .ws-att') ? 'yes' : '';") == "yes", "webkitgtk_native_image_paste")
                js("document.querySelector('.dock .composer .ws-att-x').click(); return 'yes';")
            finally:
                stop(clipboard)
            outcome("webkitgtk_native_image_paste", fixture="generated one-pixel clipboard", inspected_image=False)

            # Actual text/uri-list drag from an external GTK window. Tauri must not intercept it.
            fixture = scratch / "drop.txt"
            fixture.write_text("queue fixture")
            source = scratch / "drag_source.py"
            source.write_text("""import sys, gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk
w=Gtk.Window(title="CW attachment fixture")
w.set_default_size(180,100)
label=Gtk.EventBox()
label.add(Gtk.Label(label="Drag fixture"))
w.add(label)
label.drag_source_set(Gdk.ModifierType.BUTTON1_MASK, [Gtk.TargetEntry.new("text/uri-list",0,0)], Gdk.DragAction.COPY)
label.connect("drag-data-get", lambda widget,context,data,info,timestamp: data.set_uris([sys.argv[1]]))
w.connect("destroy", Gtk.main_quit)
w.show_all()
Gtk.main()
""")
            dragger = subprocess.Popen([sys.executable, "-B", str(source), fixture.as_uri()],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            try:
                def source_id():
                    found = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(dragger.pid), "--name", "^CW attachment fixture$"], capture_output=True, text=True).stdout.split()
                    return found[0] if len(found) == 1 else None
                sid = wait_for(source_id, "attachment_drag_source")
                subprocess.run(["xdotool", "windowmove", sid, "1650", "100"], check=True)
                sx, sy, sw, sh = geometry(sid)
                ox, oy, _, _ = geometry(window_id())
                tx, ty = rect_of('[aria-label="Message"]')
                js("window.__dropFacts = []; for (const type of ['dragenter','dragover','drop']) document.addEventListener(type, e => { if (window.__dropFacts.length < 8 || type === 'drop') window.__dropFacts.push({type,types:[...e.dataTransfer.types],files:e.dataTransfer.files.length}); }, true); return 'yes';")
                ax, ay, bx, by = sx + sw // 2, sy + sh // 2, ox + round(tx), oy + round(ty)
                subprocess.run(["xdotool", "mousemove", str(ax), str(ay), "mousedown", "1"], check=True)
                time.sleep(.15)
                for step in range(1, 21):
                    subprocess.run(["xdotool", "mousemove", str(ax + (bx-ax)*step//20), str(ay + (by-ay)*step//20)], check=True)
                    time.sleep(.04)
                time.sleep(.2)
                subprocess.run(["xdotool", "mouseup", "1"], check=True)
                time.sleep(.6)
                outcome("drop_event_facts", "diagnostic", facts=query("return JSON.stringify(window.__dropFacts);"))
                wait_for(lambda: js("return document.querySelector('.dock .composer .ws-att')?.textContent.includes('drop.txt') ? 'yes' : '';") == "yes", "webkitgtk_native_file_drop")
            finally:
                stop(dragger)
            outcome("webkitgtk_native_file_drop", source="external GTK text/uri-list", tauri_handler="disabled")
            fill("Message", "Queued from the audit")
            press("Queue")
            wait_for(lambda: js("return document.querySelector('.queue')?.textContent.includes('drop.txt') ? 'yes' : ''") == "yes", "attachment_queued")
            press("Edit")
            wait_for(lambda: js("return !document.querySelector('.queue') && document.querySelector('[aria-label=\"Message\"]').value === 'Queued from the audit' && document.querySelector('.dock .composer').textContent.includes('drop.txt') ? 'yes' : ''") == "yes", "attachment_edit")
            fill("Message", "Focus on durable conversation")
            press("Steer")
            wait_for(lambda: js("return document.querySelector('.timeline')?.textContent.includes('Focus on durable conversation') ? 'yes' : ''") == "yes", "attachment_steer")
            (project / "release-stream").touch()
            wait_for(lambda: (project / "resources-received").exists(), "attachment_runtime_resource")
            outcome("attachments_native_journey", queue_edit=True, runtime_received=True, screenshots=0)
            return 1 if FAILURES else 0
        audit("home at launch", "dark")
        engine = query(ENGINE, 15)
        needed = ("popover", "dialog", "colorMix", "has", "focusVisible", "container", "scrollbarGutter", "mask")
        if not all(engine[name] for name in needed) or engine["scrollbar"] != 11 or engine["cyrillicRoboto"] == engine["cyrillicSans"] or not engine["pageMenu"] or engine["linkDrag"] != "none":
            fault("webkitgtk_feature_gap", engine=engine)
        else:
            outcome("webkitgtk_features", **engine)
        if args.journey_wayland:
            native = query("@native")
            assert native["backend"] == "GdkWaylandDisplay" and not native["decorated"] and native["top"] == native["web"]
            outcome("full_journey_wayland_backend", native=native)
        else:
            window_behaviour()
        common_screens("no project", "dark")
        popovers("home no project")
        close_button()
        app = launch("light")
        wait_for(lambda: find("Add project"), "light_window")
        loaded()
        surface = json.loads(js("@native"))
        if surface["background"] != [255, 255, 255]:
            fault("window_background_is_not_the_surface", native=surface)
        common_screens("no project", "light")
        popovers("home no project light")
        close_window()
        app = launch("dark")
        wait_for(lambda: find("Add project"), "journey_window")
        loaded()
        add_project(project)
        wait_for(lambda: find("Activate"), "registered_project_visible")
        assert not (project / ".cortex").exists() and not (project / ".cortex-dev").exists()
        assert (project / ".cortex-dev-owner.json").is_file()
        outcome("registration_is_inert", channel="dev")
        audit("project before activation", "dark")
        click("Activate")
        wait_for(lambda: not find("Activate"), "activation_finished")
        outcome("activation")
        audit("project board empty", "dark")
        popovers("project board")
        click("New task")
        audit("new task in project", "dark")
        assert js("const radio=[...document.querySelectorAll(\'label.rc\')].find(l=>l.textContent.includes(\'Separate worktree\'))?.querySelector(\'input\'); return radio && !radio.disabled ? \'enabled\' : \'disabled\';") == "enabled"
        assert ".cortex-dev-owner.json" not in subprocess.check_output(["git","status","--porcelain"],cwd=project,text=True)
        outcome("task_worktree_choice_and_helper_exclusion")
        click("Sprint")
        assert js("return document.querySelector('.c-route') && ![...document.querySelectorAll('.c-route button')].some(b => b.title.includes('sprint route changes are unavailable')) ? 'yes' : 'no';") == "yes"
        audit("new sprint configured route", "dark")
        outcome("sprint_route_control_not_placeholder")
        click("Task")
        fill("Describe the task", "Explain this disposable project.")
        click("Save as draft")
        wait_for(lambda: text_seen("Explain this disposable project"), "draft_visible")
        assert not (project / "turns").exists()
        outcome("description_only_draft_does_not_start")
        audit("project board with draft", "dark")
        inspector_open("project board with draft")
        add_project(second)
        wait_for(lambda: js("return document.querySelectorAll('.proj').length === 2 ? 'yes' : ''") == "yes", "second_project_visible")
        outcome("two_projects")
        common_screens("two projects", "dark")
        # Card links expose the task title. Select the matching rendered link.
        def task_link():
            for node in tree():
                try:
                    if node.getRoleName() == "link" and "Explain this disposable project" in node.name:
                        return node
                except Exception as error:
                    if not node_went_stale(error):
                        raise

        def open_task(step):
            def once():
                assert wait_for(task_link, step).queryAction().doAction(0)
            retry_stale(once)
        journey = "[...document.querySelectorAll('.proj')].find(p => p.textContent.includes('journey')).click(); return 'yes';"
        js(journey)
        open_task("task_link")
        time.sleep(0.3)
        audit("task session draft", "dark")
        inspector_open("task session draft")
        click("Start")
        wait_for(lambda: text_seen("Which direction"), "runtime_question")
        outcome("start_and_question")
        audit("task session question", "dark")
        popovers("task session question")
        resize(960, 540)
        time.sleep(0.5)
        audit("task session at minimum size", "dark")
        resize(1440, 900)
        time.sleep(0.4)
        close_window()
        app = launch("light")
        wait_for(lambda: find("Add project"), "reopen_window")
        loaded()
        js(journey)
        time.sleep(0.3)
        audit("project board light", "light")
        open_task("reopen_question_link")
        wait_for(lambda: text_seen("Which direction"), "reopened_question")
        assert service.poll() is None
        outcome("question_survives_window_close")
        audit("task session question light", "light")
        click("… Other answer")
        fill("Your answer", "first line\nsecond line")
        click("Send answer")
        wait_for(lambda: (project / "streaming").exists() or text_seen("changed on the machine"), "answer_or_explicit_stale_refusal")
        if not (project / "streaming").exists():
            # Deliberate second owner action; never an automatic replay.
            click("Send answer")
            wait_for(lambda: (project / "streaming").exists(), "explicit_answer_retry")
        outcome("multiline_answer_and_streaming")
        close_window()
        app = launch("dark")
        wait_for(lambda: find("Add project"), "reopen_streaming_window")
        loaded()
        open_task("streaming_task_link")
        wait_for(lambda: text_seen("Working") or text_seen("Planning") or text_seen("Executing"), "observed_streaming")
        outcome("streaming_survives_window_close")
        audit("task session streaming", "dark")
        inspector_open("task session streaming")
        # Saved on the machine but not delivered; Edit restores the composer without starting a turn.
        fill("Message", "Queued from the audit")
        press("Queue")
        wait_for(lambda: js("return document.querySelector('.queue') ? 'yes' : ''") == "yes", "queued_message_visible")
        audit("task session with a queued message", "dark")
        close_window()
        app = launch("dark")
        wait_for(lambda: find("Add project"), "reopen_durable_queue")
        loaded()
        js(journey)
        open_task("durable_queue_task_link")
        wait_for(lambda: text_seen("Queued from the audit"), "durable_queue_restored")
        assert not text_seen("may be lost if the app closes")
        outcome("queue_survives_window_close")
        press("Edit")
        wait_for(lambda: js("return !document.querySelector('.queue') && document.querySelector('[aria-label=\"Message\"]').value === 'Queued from the audit' ? 'yes' : ''") == "yes", "edit_returns_to_the_field")
        outcome("queue_and_edit")
        fill("Message", "Focus on durable conversation")
        press("Steer")
        wait_for(lambda: text_seen("Focus on durable conversation") and js("return document.querySelector('.timeline')?.textContent.includes('Streaming from the fake engine.') ? 'yes' : '';") == "yes", "steer_recorded_during_turn")
        outcome("steer_durable_capture_during_turn")
        seed_workers(project)
        wait_for(lambda: js("return document.querySelectorAll('.wrow').length === 2 ? 'yes' : ''") == "yes", "workers_in_the_conversation", 15)
        outcome("workers_from_the_real_store", through="service, helper, port and screen")
        audit("task session with workers", "dark")
        inspector_open("task session with workers")
        tabs = follow(".tabs a.tab")
        for href in tabs:
            open_link(href)
            time.sleep(0.2)
            audit(f"task tab {href.rsplit('/', 1)[-1]}", "dark")
        workers_tab = next(href for href in tabs if href.endswith("/workers"))
        open_link(workers_tab)
        time.sleep(0.2)
        for href in follow(".wlist a.wcard"):
            open_link(href)
            time.sleep(0.2)
            audit(f"worker {href.rsplit('/', 1)[-1]}", "dark")
            popovers(f"worker {href.rsplit('/', 1)[-1]}")
        open_link(next(href for href in follow(".wlist a.wcard") if href.endswith("/a-build")))
        time.sleep(0.2)
        open_link(follow(".d-title a.btn")[0])
        time.sleep(0.3)
        audit("worker live view", "dark")
        open_link(tabs[0])
        time.sleep(0.2)
        (project / "release-stream").touch()
        wait_for(lambda: text_seen("Runtime-owned journey result."), "runtime_result")
        assert not text_seen("accepted by you")
        assert (project / "turns").read_text().splitlines() == ["0", "1", "2"]
        assert (project / "steer-applied").is_file()
        assert js("return document.querySelector(\'.rcard-h\').textContent;").startswith("Result v1")
        outcome("runtime_owned_result", fake_engine_turns=3)
        audit("task session result", "dark")
        inspector_open("task session result")
        task_at = js("return location.pathname;")
        for href in follow(".tabs a.tab"):
            open_link(href)
            time.sleep(0.2)
            audit(f"result tab {href.rsplit('/', 1)[-1]}", "dark")
        # Result feedback must return to Cortex and produce a new plain result ordinal.
        open_link(task_at)
        js("window.__cwReviewTrigger = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Request changes…'); window.__cwReviewTrigger.focus(); return 'yes';")
        press("Request changes…")
        js("const t = document.querySelector('dialog[open] textarea'); t.value = 'Owner review feedback'; t.dispatchEvent(new Event('input',{bubbles:true})); return 'yes';")
        review_submit_at = time.monotonic()
        press("Send and reopen")
        # A runtime result can arrive before the submit receipt/refresh finishes.
        # Never navigate behind a still-modal dialog or bypass native worker focus.
        wait_for(lambda: js("return document.querySelector('dialog[open]') ? '' : 'yes';") == "yes", "review_dialog_closed")
        focus = query("""const trigger = window.__cwReviewTrigger; return JSON.stringify({triggerConnected:trigger.isConnected,focusReturned:document.activeElement === (trigger.isConnected ? trigger : document.querySelector('main')),explicitInert:document.querySelectorAll('[inert]').length});""")
        assert focus["focusReturned"] and focus["explicitInert"] == 0, focus
        outcome("review_dialog_closed", elapsed_ms=round((time.monotonic() - review_submit_at) * 1000), **focus)
        wait_for(lambda: (project / "review-returned").exists() and text_seen("Review feedback returned to Cortex."), "review_returns_to_cortex")
        assert js("return document.querySelector('.rcard-h').textContent;").startswith("Result v2")
        outcome("review_feedback_and_second_result")
        # A second fake-engine run exercises actual runtime dispatch, not seeded visual rows.
        (project / "start-worker-scenario").touch()
        nav("New task")
        fill("Describe the task", "Exercise worker controls")
        press("Save as draft")
        wait_for(lambda: js("return [...document.querySelectorAll('.card-link')].some(a => a.textContent.includes('Exercise worker controls')) ? 'yes' : ''") == "yes", "worker_task_saved")
        js("[...document.querySelectorAll('.card-link')].find(a => a.textContent.includes('Exercise worker controls')).click(); return 'yes';")
        press("Start")
        wait_for(lambda: (project / "worker-one").exists() and (project / "worker-two").exists(), "real_workers_dispatched")
        worker_base = js("return location.pathname;")
        js("[...document.querySelectorAll('.tabs a')].find(a => a.textContent.includes('Workers')).click(); return 'yes';")
        wait_for(lambda: js("const cards = [...document.querySelectorAll('.wcard')]; return ['Receive owner guidance', 'Wait for command control'].every(label => cards.some(a => a.textContent.includes(label))) ? 'yes' : ''") == "yes", "real_workers_loaded", 20)
        one_href = js("return [...document.querySelectorAll('.wcard')].find(a => a.textContent.includes('Receive owner guidance')).getAttribute('href');")
        two_href = js("return [...document.querySelectorAll('.wcard')].find(a => a.textContent.includes('Wait for command control')).getAttribute('href');")
        open_link(one_href)
        # An attempt the snapshot still shows as planned reads as Not confirmed, and its field is rightly disabled until the next
        # reread, so this waits for the worker it picked to take messages. A field that never opens still fails here, and one that
        # opens must then take real native focus below. Both workers share a role, so the label alone cannot say whose field this is.
        composer_at = time.monotonic()
        wait_for(lambda: js("const f = document.querySelector('[aria-label=\"Message backend-dev\"]'); return f && !f.disabled && document.querySelector('.d-goal')?.textContent === 'Receive owner guidance' ? 'yes' : ''") == "yes", "live_worker_composer", 20)
        outcome("live_worker_composer", waited_ms=round((time.monotonic() - composer_at) * 1000))
        fill("Message backend-dev", "Keep the worker message verbatim\nsecond line")
        js("document.querySelector('[aria-label=\"Send to backend-dev\"]').click(); return 'yes';")
        wait_for(lambda: (project / "worker-guidance-delivered").exists(), "worker_guidance_safe_point")
        wait_for(lambda: text_seen("Worker guidance completed."), "finished_worker_result", 20)
        audit("actual finished worker", "dark")
        js("[...document.querySelectorAll('a')].find(a => a.textContent.trim() === 'Show full history').click(); return 'yes';")
        wait_for(lambda: text_seen("Worker received owner guidance and inspected the project."), "finished_worker_history")
        outcome("worker_message_and_finished_history", receipt="Cortex safe point, not proof of model action")
        js(f"history.pushState(null,'',{json.dumps(two_href)}); dispatchEvent(new PopStateEvent('popstate')); return 'yes';")
        wait_for(lambda: js("return [...document.querySelectorAll('button')].some(b => b.textContent.trim() === 'Stop command…') ? 'yes' : ''") == "yes", "running_gateway_command", 20)
        press("Stop command…")
        js("const t = document.querySelector('dialog[open] textarea'); t.value = 'Owner requested a focused stop'; t.dispatchEvent(new Event('input',{bubbles:true})); return 'yes';")
        js("[...document.querySelectorAll('dialog[open] button')].find(b => b.textContent.trim() === 'Stop command').click(); return 'yes';")
        wait_for(lambda: (project / "worker-command-stopped").exists(), "gateway_command_stopped")
        wait_for(lambda: js("return document.querySelector('dialog[open]') ? '' : 'yes';") == "yes", "command_stop_dialog_closed")
        press("Stop…")
        press("Stop worker")
        wait_for(lambda: text_seen("Cancelled"), "worker_cancelled", 20)
        wait_for(lambda: js("return document.querySelector('dialog[open]') ? '' : 'yes';") == "yes", "worker_stop_dialog_closed")
        outcome("command_and_worker_stop", through="real gateway process and runtime cancellation")
        js(f"history.pushState(null,'',{json.dumps(task_at)}); dispatchEvent(new PopStateEvent('popstate')); return 'yes';")
        wait_for(lambda: text_seen("Runtime-owned journey result."), "return_to_result")
        js(journey)
        time.sleep(0.3)
        audit("project board with a result", "dark")
        popovers("project board with a result")
        # A real local Git refusal must stay readable inside its still-modal form.
        open_link(next(href for href in follow(".ptab") if href.endswith("/git")))
        wait_for(lambda: js("return [...document.querySelectorAll('button')].some(b => b.textContent.trim() === 'Create branch here') ? 'yes' : '';") == "yes", "branch_dialog_trigger")
        js("window.__cwBranchTrigger = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Create branch here'); window.__cwBranchTrigger.focus(); return 'yes';")
        press("Create branch here")
        fill("Branch name", "main")
        press("Create branch")
        wait_for(lambda: js("return document.querySelector('dialog[open] [role=alert]')?.textContent.trim() ? 'yes' : '';") == "yes", "branch_refusal_inside_modal")
        assert js("return document.querySelector('dialog[open] input').value;") == "main"
        audit("branch refusal stays in dialog", "dark")
        press("Cancel")
        wait_for(lambda: js("return document.querySelector('dialog[open]') ? '' : 'yes';") == "yes", "branch_cancel_closed")
        assert js("return document.activeElement === window.__cwBranchTrigger ? 'yes' : '';") == "yes"
        outcome("dialog_refusal_and_cancel_focus", form_retained=True, focus_returned=True)
        js(journey)
        # A view may carry its own query on these links, so each is found again by its place.
        for index, href in enumerate(follow(".ptab")):
            js(f"document.querySelectorAll('.ptab')[{index}].click(); return 'yes';")
            time.sleep(0.4)
            audit(f"project view {href.rsplit('/', 1)[-1].split('?')[0]}", "dark")
        js(journey)
        time.sleep(0.2)
        js(f"history.pushState(null,'',{json.dumps(task_at)}); dispatchEvent(new PopStateEvent('popstate')); return 'yes';")
        wait_for(lambda: js("return location.pathname;") == task_at, "back_to_the_task")
        stop(service)
        service = None
        wait_for(lambda: text_seen("Offline"), "disconnect_visible")
        wait_for(lambda: text_seen("Last confirmed") and text_seen("old"), "last_confirmed_age_visible")
        outcome("disconnected_state")
        audit("task session offline", "dark")
        nav("Home")
        audit("home offline with project", "dark")
        close_window()
        app = launch("light")
        wait_for(lambda: find("Add project"), "offline_start")
        loaded()
        wait_for(lambda: js("return document.querySelector('.ws-conn') ? 'yes' : ''") == "yes", "offline_banner")
        audit("home offline at launch", "light")
        popovers("offline light")
        close_window()
        app = None
        service = start_service()
        wait_for(lambda: (runtime / "service.sock").exists(), "service_restart")
        if not args.journey_wayland:
            wayland_stage()
        final = native_evidence()
        outcome("run_evidence", "diagnostic", **final)
        if final["web_process_terminations"]:
            fault("web_process_terminated", reasons=final["web_process_terminations"])
        if final["cores"]:
            fault("process_left_a_core", cores=final["cores"])
        if FAILURES:
            outcome("visual_audit_summary", "failed", failures=len(FAILURES), names=sorted(set(FAILURES)))
            return 1
        outcome("visual_audit_summary", failures=0)
        return 0
    except Exception as error:
        reason = (str(error) or type(error).__name__) if isinstance(error, (RuntimeError, AssertionError)) else repr(error)[:300]
        outcome("journey_incomplete", "failed", reason=reason, last_step=PROGRESS["step"], last_screen=PROGRESS["screen"], steps_passed=PROGRESS["passed"],
                at=[f"{frame.name}:{frame.lineno}" for frame in traceback.extract_tb(error.__traceback__) if Path(frame.filename).name == "check_rendered.py"][-6:])
        outcome("native_process_evidence", "diagnostic", **native_evidence(reason))
        # Only names/roles from our exact disposable app are retained, never raw logs.
        if app is not None:
            try:
                if args.only == "voice":
                    outcome("voice_at_failure", "diagnostic", status=js("return document.querySelector('.micbar [role=status]')?.textContent ?? '';", 3), error=js("return document.querySelector('.micbar[role=alert]')?.textContent ?? '';", 3)[:200])
                outcome("terminal_at_failure", "diagnostic", status=js("return document.querySelector('.terminal-status')?.textContent ?? '';", 3), error=js("return document.querySelector('.terminal-error')?.textContent ?? '';", 3)[:200])
                outcome("dialog_at_failure", "diagnostic", state=query("""return JSON.stringify({open:[...document.querySelectorAll('dialog[open]')].map(d=>d.getAttribute('aria-label')),receipt:document.querySelector('.ws-receipt')?.textContent??null,focus:document.activeElement?.tagName});"""))
                outcome("page_at_failure", "diagnostic", route=js("return location.pathname;", 3),
                        project_file_count=len(list(project.iterdir())), service_log_bytes=(scratch / "service.log").stat().st_size)
            except Exception:
                pass
            # Capture accessibility names before terminating the exact owned app; a node can go stale during this walk too.
            last_nodes = []
            for node in (tree() if "tree" in locals() else []):
                try:
                    if node.name:
                        last_nodes.append({"role": node.getRoleName(), "name": node.name[:180]})
                except Exception:
                    pass
            stop(app)
        if "tree" in locals():
            names = locals().get("last_nodes", [])
            outcome("owned_accessibility_diagnostic", "diagnostic", nodes=names[:90])
        return 1
    finally:
        for process in (app, service, pulse, compositor, wm, xvfb):
            stop(process)
        outcome("owned_process_cleanup")


def in_container(args):
    """Host side: a disposable no-network container, read-only sources, the exact built artifacts."""
    subprocess.run(["docker", "build", "--quiet", "--tag", IMAGE, "-"], input=RECIPE, text=True, check=True, stdout=subprocess.DEVNULL)
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from runtime_payload import payload_files
    with tempfile.TemporaryDirectory(prefix="cw-rendered-", dir=Path(args.artifacts).resolve().parent if args.only == "voice" else ROOT / ".workspace-build") as directory:
        # The runtime as packaged, plus only what the fake engine and this check need. Never the checkout, Git or local state.
        checkout = Path(directory)
        for relative, path in payload_files(ROOT):
            target = checkout / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
        shutil.copytree(ROOT / "tests", checkout / "tests", ignore=shutil.ignore_patterns("__pycache__"))
        shutil.copytree(ROOT / "packaging/container", checkout / "packaging/container", dirs_exist_ok=True)
        if args.only == "voice":
            shutil.copytree(Path(args.artifacts) / "voice", checkout / "packaging/voice", dirs_exist_ok=True)
        target = checkout / "crates/workspace-protocol/channels.json"
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / "crates/workspace-protocol/channels.json", target)
        command = ["docker", "run", "--rm", "--network=none", "--user", f"{os.getuid()}:{os.getgid()}", "--shm-size=512m",
                   "--cap-drop=ALL", "--security-opt=seccomp=unconfined", "--security-opt=apparmor=unconfined",
                   "--mount", f"type=bind,source={Path(args.artifacts).resolve()},target=/artifacts,readonly",
                   "--mount", f"type=bind,source={checkout},target=/checkout,readonly",
                   "--env", "HOME=/tmp", IMAGE, "python3", "-B", "/checkout/packaging/container/check_rendered.py",
                   "--artifacts", "/artifacts", "--python", "/usr/bin/python3", *(["--only", args.only] if args.only else []), *(["--journey-wayland"] if args.journey_wayland else [])]
        return subprocess.run(command).returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", required=True)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--scratch")
    parser.add_argument("--journey-wayland", action="store_true", help="run the complete common journey on native Wayland, using the installed Mutter keyboard API")
    parser.add_argument("--container", action="store_true", help="run inside the disposable rendered-check container")
    parser.add_argument("--only", choices=("wayland", "attachments", "terminal", "voice"), help="run one stage, to iterate on it")
    args = parser.parse_args()
    if args.journey_wayland and args.only:
        parser.error("--journey-wayland is the full journey, not a single stage")
    if args.container:
        return in_container(args)
    args.artifacts = str(Path(args.artifacts).resolve())
    args.python = str(Path(args.python).absolute())
    if args.scratch:
        return session(args)
    with tempfile.TemporaryDirectory(prefix="cw-native-") as directory:
        scratch = Path(directory)
        for name in ("home", "config", "state", "cache", "runtime"):
            (scratch / name).mkdir(mode=0o700)
        env = {"PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": str(scratch / "home"),
               "XDG_CONFIG_HOME": str(scratch / "config"), "XDG_STATE_HOME": str(scratch / "state"),
               "XDG_CACHE_HOME": str(scratch / "cache"), "XDG_RUNTIME_DIR": str(scratch / "runtime"),
               "PYTHONDONTWRITEBYTECODE": "1"}
        wrapper = subprocess.Popen(["dbus-run-session", "--", sys.executable, "-B", str(Path(__file__).resolve()),
                                    "--artifacts", args.artifacts, "--python", args.python, "--scratch", str(scratch), *(["--only", args.only] if args.only else []), *(["--journey-wayland"] if args.journey_wayland else [])],
                                   env=env, stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            return wrapper.wait(timeout=900)
        finally:
            stop(wrapper)


if __name__ == "__main__":
    raise SystemExit(main())
