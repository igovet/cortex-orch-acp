"""The rendered layout gate in WebKitGTK, on fake data and a private virtual screen. A problem on any screen fails the run.

To add a screen held to the full rules, add one entry to SCREENS below: a name, its route, what must be on screen when it is ready, and its sizes.
Screens not moved to the layout primitives yet stay in KEPT and the flows in run(), under the row and reason rules only."""
import json
from urllib.parse import quote
import os
from pathlib import Path
import subprocess
import sys
import threading
import time

xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1920x1200x24", "-nolisten", "tcp"], stdout=subprocess.PIPE, text=True)
os.environ.update(DISPLAY=":" + xvfb.stdout.readline().strip(), GDK_BACKEND="x11", NO_AT_BRIDGE="1", WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1", WEBKIT_DISABLE_COMPOSITING_MODE="1")
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import Gtk, GLib, WebKit2, Gdk

label = sys.argv[2]
evidence = Path("/evidence")
win = Gtk.Window()
win.set_default_size(1280, 800)
view = WebKit2.WebView(web_context=WebKit2.WebContext.new_ephemeral())
win.add(view)
win.show_all()
exit_code = 0
loads = 0

# Screens and sizes the owner works at. Every row class here must stay one line, and every pane must keep its width.
SIZES = ((1280, 800), (1920, 1080))
PROBE = r"""
const skip = 'dialog, [popover], .sidebar, .titlebar, .cm-editor, .ws-live';
const rowSel = '.tn, .gf, .gg, .res-file, .etab, .hrow, .git-sum, .ecrumb, .side-h, .side-opts, .chg-tools, .gr, .sp-plan-h, .pt-row';
const clipOf = (el, self = false) => { let c = {l: 0, t: 0, r: innerWidth, b: innerHeight};
  for (let a = self ? el : el.parentElement; a && a !== document.documentElement; a = a.parentElement) { const s = getComputedStyle(a);
    if (s.overflowX !== 'visible' || s.overflowY !== 'visible') { const r = a.getBoundingClientRect(); c = {l: Math.max(c.l, r.left), t: Math.max(c.t, r.top), r: Math.min(c.r, r.right), b: Math.min(c.b, r.bottom)}; } }
  return c; };
const seen = el => { if (el.closest(skip)) return false; const s = getComputedStyle(el); return s.visibility !== 'hidden' && s.display !== 'none' && +s.opacity !== 0; };
const cut = (r, c) => ({l: Math.max(r.left, c.l), t: Math.max(r.top, c.t), r: Math.min(r.right, c.r), b: Math.min(r.bottom, c.b)});
const items = [];
const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
for (let n = walk.nextNode(); n; n = walk.nextNode()) { const p = n.parentElement; if (!n.nodeValue.trim() || !p || !seen(p)) continue;
  const g = document.createRange(); g.selectNodeContents(n); const c = clipOf(p, true);
  for (const r of g.getClientRects()) { const x = cut(r, c); if (x.r - x.l > 1 && x.b - x.t > 1) items.push({node: p, kind: 'text', label: n.nodeValue.trim().slice(0, 32), ...x}); } }
for (const el of document.querySelectorAll('button, a, input, textarea')) { if (!seen(el)) continue; const x = cut(el.getBoundingClientRect(), clipOf(el)); if (x.r - x.l > 1 && x.b - x.t > 1) items.push({node: el, kind: 'control', label: (el.getAttribute('aria-label') || el.textContent || '').trim().slice(0, 32), ...x}); }
const problems = [];
for (let i = 0; i < items.length && problems.length < 40; i++) for (let j = i + 1; j < items.length && problems.length < 40; j++) { const a = items[i], b = items[j];
  if (a.node === b.node || a.node.contains(b.node) || b.node.contains(a.node)) continue;
  const w = Math.min(a.r, b.r) - Math.max(a.l, b.l), h = Math.min(a.b, b.b) - Math.max(a.t, b.t);
  if (w > 1 && h > 1) problems.push(`overlap ${a.kind} "${a.label}" with ${b.kind} "${b.label}" (${Math.round(w)}x${Math.round(h)})`); }
for (const row of document.querySelectorAll(rowSel)) { if (!seen(row) || row.offsetWidth === 0) continue;
  if (row.scrollHeight > row.clientHeight + 1) problems.push(`row taller than its box: ${row.className} "${row.textContent.trim().slice(0, 32)}" (${row.scrollHeight}>${row.clientHeight})`);
  const tw = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
  for (let n = tw.nextNode(); n; n = tw.nextNode()) { if (!n.nodeValue.trim() || !n.parentElement || !seen(n.parentElement)) continue; const g = document.createRange(); g.selectNodeContents(n); const tops = new Set([...g.getClientRects()].filter(r => r.width > 1).map(r => Math.round(r.top))); if (tops.size > 1) problems.push(`wrapped text in ${row.className}: "${n.nodeValue.trim().slice(0, 32)}"`); } }
for (const el of document.querySelectorAll('[aria-disabled="true"][data-why]')) { if (!el.getClientRects().length) continue; const c = getComputedStyle(el, '::after').content, b = getComputedStyle(el, '::before').content; if ((c && c !== 'none' && c !== 'normal') || (b && b !== 'none' && b !== 'normal') || (!el.matches('.mi') && el.textContent.includes(el.dataset.why))) problems.push(`reason drawn inside control: "${(el.getAttribute('aria-label') || el.textContent || '').trim().slice(0, 40)}"`);
  if (el.matches('.btn-primary, .btn-danger')) { const s = getComputedStyle(el); if ((s.backgroundColor === 'rgba(0, 0, 0, 0)' || s.backgroundColor === 'transparent') && s.boxShadow === 'none') problems.push(`disabled primary lost its shape: "${el.textContent.trim().slice(0, 40)}"`); } }
for (const pane of document.querySelectorAll('.side-pane, .git-right, .ed')) { if (!seen(pane)) continue; const p = pane.getBoundingClientRect();
  for (const it of items) if (pane.contains(it.node) && (it.l < p.left - 1 || it.r > p.right + 1 || it.t < p.top - 1 || it.b > p.bottom + 1)) problems.push(`left pane ${pane.className}: "${it.label}"`);
  if (pane.matches('.side-pane') && p.width < 240) problems.push(`pane too narrow: ${pane.className} ${p.width}`);
  if (pane.matches('.git-right, .ed') && p.width < 480) problems.push(`pane too narrow: ${pane.className} ${p.width}`); }
for (const el of document.querySelectorAll('.ecrumb button, .etab-tools button, .git-sum button, .chg-tools button')) { const r = el.getBoundingClientRect(), c = clipOf(el); if (r.left < c.l - 1 || r.right > c.r + 1 || r.top < c.t - 1 || r.bottom > c.b + 1) problems.push(`control cut off: ${(el.textContent || el.getAttribute('aria-label')).trim().slice(0, 32)}`); }
return {problems: [...new Set(problems)], rows: document.querySelectorAll(rowSel).length, controls: items.filter(i => i.kind === 'control').length};
"""


# The full rules, for screens built from the layout primitives. A string in `problems` is a failure.
STRICT = r"""
const V = {w: innerWidth, h: innerHeight};
const problems = [];
const scope = document.querySelector('dialog[open]') || document.body;
const css = (el, pseudo) => getComputedStyle(el, pseudo);
// A screen reader's polite region is clipped to nothing on purpose.
const SKIP = '.ws-live';
// What a closed disclosure holds is not on screen, whatever rectangle the engine reports for it.
const folded = el => { const d = el.parentElement && el.parentElement.closest('details:not([open])'); return !!d && !(el.closest('summary') && d.contains(el.closest('summary'))); };
const seen = el => { if ((SKIP && el.closest(SKIP)) || folded(el)) return false; const s = css(el); if (s.display === 'none' || s.visibility === 'hidden' || +s.opacity === 0) return false; const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
const name = el => { const c = typeof el.className === 'string' ? el.className.trim().split(/\s+/).slice(0, 2).join('.') : ''; return el.tagName.toLowerCase() + (c ? '.' + c : ''); };
const label = el => (el.getAttribute('aria-label') || el.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 40);
const scrollsX = s => s.overflowX === 'auto' || s.overflowX === 'scroll';
const scrollsY = s => s.overflowY === 'auto' || s.overflowY === 'scroll';
const clipOf = (el, self) => { let c = {l: 0, t: 0, r: V.w, b: V.h}; for (let a = self ? el : el.parentElement; a && a !== document.documentElement; a = a.parentElement) { const s = css(a); if (s.overflowX !== 'visible' || s.overflowY !== 'visible') { const r = a.getBoundingClientRect(); c = {l: Math.max(c.l, r.left), t: Math.max(c.t, r.top), r: Math.min(c.r, r.right), b: Math.min(c.b, r.bottom)}; } } return c; };
const inScroller = (el, test) => { for (let a = el.parentElement; a && a !== document.documentElement; a = a.parentElement) if (test(css(a))) return true; return false; };
const shortens = s => s.textOverflow === 'ellipsis' || (s.webkitLineClamp && s.webkitLineClamp !== 'none');
// A box cut on purpose until a control beside it, which says so with aria-controls and aria-expanded, opens all of it.
const expander = el => !!el.id && !!document.querySelector(`[aria-controls="${el.id}"][aria-expanded="false"]`);
const inExpander = el => { for (let a = el.parentElement; a && a !== document.documentElement; a = a.parentElement) if (expander(a)) return true; return false; };
// Text that is cut leads to the whole of it: a link, a button, an expandable row or a box a press reveals.
const reachable = el => !!el.closest('a, button, [role="button"], [data-whole], summary, label, [aria-controls]') || expander(el);
const elements = [...scope.querySelectorAll('*')].filter(el => !(el.closest('svg') && el.tagName.toLowerCase() !== 'svg')).filter(el => seen(el));
const R = n => Math.round(n * 100) / 100;
const spread = xs => xs.length ? R(Math.max(...xs) - Math.min(...xs)) : 0;

// The page itself never scrolls.
const doc = document.documentElement;
if (doc.scrollWidth > doc.clientWidth + 1) problems.push(`page scrolls sideways: ${doc.scrollWidth}>${doc.clientWidth}`);
if (doc.scrollHeight > doc.clientHeight + 1) problems.push(`page scrolls vertically: ${doc.scrollHeight}>${doc.clientHeight}`);

// Nothing outside the viewport or its parent; hidden overflow only where text deliberately shortens.
let parents = 0, shortened = 0;
for (const el of elements) {
  const r = el.getBoundingClientRect(), s = css(el);
  const yFree = inScroller(el, scrollsY), xFree = inScroller(el, scrollsX);
  // A bold word inside a line that ends in an ellipsis runs past it on purpose: that overflow is the shortening.
  if (inScroller(el, shortens) || inExpander(el)) continue;
  if (!xFree && (r.left < -1 || r.right > V.w + 1)) problems.push(`outside viewport X: ${name(el)} "${label(el)}" ${R(r.left)}..${R(r.right)}`);
  if (!yFree && (r.top < -1 || r.bottom > V.h + 1)) problems.push(`outside viewport Y: ${name(el)} "${label(el)}" ${R(r.top)}..${R(r.bottom)}`);
  let p = el.parentElement; while (p && css(p).display === 'contents') p = p.parentElement;
  if (p && el !== scope && s.position !== 'absolute' && s.position !== 'fixed' && !el.matches('dialog')) {
    const q = p.getBoundingClientRect(), ps = css(p); parents++;
    if (!scrollsX(ps) && (r.left < q.left - 1 || r.right > q.right + 1)) problems.push(`outside parent X: ${name(el)} "${label(el)}" in ${name(p)} (${R(r.left)}..${R(r.right)} vs ${R(q.left)}..${R(q.right)})`);
    if (!scrollsY(ps) && (r.top < q.top - 1 || r.bottom > q.bottom + 1)) problems.push(`outside parent Y: ${name(el)} "${label(el)}" in ${name(p)} (${R(r.top)}..${R(r.bottom)} vs ${R(q.top)}..${R(q.bottom)})`);
  }
  const hx = el.scrollWidth > el.clientWidth + 1, hy = el.scrollHeight > el.clientHeight + 1;
  const ell = s.textOverflow === 'ellipsis', clamp = s.webkitLineClamp && s.webkitLineClamp !== 'none';
  if ((hx && ell) || (hy && clamp) || (hy && expander(el))) { shortened++; if (!reachable(el)) problems.push(`cut with no way to read it: ${name(el)} "${label(el)}"`); continue; }
  // The sanctioned sideways scrollers: the board's columns, a strip of open files, which a narrow pane scrolls rather than squeezing,
  // and a step's recorded output, whose lines are read as written (output_gate holds it to its column and to the keyboard).
  if (hx && scrollsX(s) && !el.matches('.board-wrap, .etab-list, .ws-out')) problems.push(`scrolls sideways: ${name(el)} ${el.scrollWidth}>${el.clientWidth}`);
  if (hx && !scrollsX(s) && s.overflowX !== 'visible') problems.push(`clipped sideways: ${name(el)} "${label(el)}" ${el.scrollWidth}>${el.clientWidth}`);
  if (hy && !scrollsY(s) && s.overflowY !== 'visible') problems.push(`clipped vertically: ${name(el)} "${label(el)}" ${el.scrollHeight}>${el.clientHeight}`);
}

// A label that shrank to nothing has no area left for the rules above to measure.
for (const el of scope.querySelectorAll('.trunc, .l-row-t, .l-sect-t, .l-z-mid > *, .l-branch, .l-fact b')) { if (!el.getClientRects().length || !el.textContent.trim()) continue; const w = el.getBoundingClientRect().width; if (w < 24 && el.scrollWidth > el.clientWidth + 1) problems.push(`text squeezed out: ${name(el)} "${label(el)}" ${R(w)}px`); }

// No text or control overlaps another that is not its ancestor or descendant. The sprint graph's canvas is a zoomable drawing
// whose labels follow its own detail levels and pins, like the SVG elements skipped above; the panels around it are held here.
const cut = (r, c) => ({l: Math.max(r.left, c.l), t: Math.max(r.top, c.t), r: Math.min(r.right, c.r), b: Math.min(r.bottom, c.b)});
const items = [];
const walk = document.createTreeWalker(scope, NodeFilter.SHOW_TEXT);
for (let n = walk.nextNode(); n; n = walk.nextNode()) { const p = n.parentElement; if (!n.nodeValue.trim() || !p || !seen(p) || p.closest('script,style,.gsv')) continue; const g = document.createRange(); g.selectNodeContents(n); const c = clipOf(p, true); for (const r of g.getClientRects()) { const x = cut(r, c); if (x.r - x.l > 1 && x.b - x.t > 1) items.push({node: p, kind: 'text', label: n.nodeValue.trim().slice(0, 32), ...x}); } }
for (const el of scope.querySelectorAll('button, a, input, textarea')) { if (!seen(el)) continue; const x = cut(el.getBoundingClientRect(), clipOf(el)); if (x.r - x.l > 1 && x.b - x.t > 1) items.push({node: el, kind: 'control', label: label(el), ...x}); }
let overlaps = 0;
for (let i = 0; i < items.length; i++) for (let j = i + 1; j < items.length; j++) { const a = items[i], b = items[j]; if (a.node === b.node || a.node.contains(b.node) || b.node.contains(a.node)) continue; const w = Math.min(a.r, b.r) - Math.max(a.l, b.l), h = Math.min(a.b, b.b) - Math.max(a.t, b.t); if (w > 1 && h > 1) { overlaps++; if (problems.length < 60) problems.push(`overlap ${a.kind} "${a.label}" with ${b.kind} "${b.label}" (${R(w)}x${R(h)})`); } }

// Every control is whole; a declared bar holds its height; controls in a bar share height, centre and baseline.
const ctx = document.createElement('canvas').getContext('2d');
const firstText = el => { const t = document.createTreeWalker(el, NodeFilter.SHOW_TEXT); for (let n = t.nextNode(); n; n = t.nextNode()) if (n.nodeValue.trim() && seen(n.parentElement)) return n; return null; };
const CTL = '.btn, .ib, .sel, .seg, .scope-btn, .ptab, .tab, .chip, .pick';
for (const el of scope.querySelectorAll(CTL)) { if (!seen(el)) continue; const r = el.getBoundingClientRect(), c = clipOf(el); if (r.left < c.l - 1 || r.right > c.r + 1 || (!inScroller(el, scrollsY) && (r.top < c.t - 1 || r.bottom > c.b + 1))) problems.push(`control cut off: "${label(el)}" ${R(r.left)}..${R(r.right)} clip ${R(c.l)}..${R(c.r)}`); }
const WANT = {toolbar: 44, 'toolbar-sm': 36, action: 48, dialog: 52, card: 34};
const bars = [];
const barEls = [...scope.querySelectorAll('[data-bar]')].filter(el => seen(el));
for (const b of barEls) {
  const r = b.getBoundingClientRect(), want = WANT[b.dataset.bar];
  const direct = [...b.querySelectorAll(CTL)].filter(el => seen(el)).filter(c => !c.parentElement.closest('.seg, .split') || c.matches('.seg'));
  const clip = clipOf(b, true);
  const visible = direct.filter(c => { const q = c.getBoundingClientRect(); return q.bottom > clip.t && q.top < clip.b; });
  const heights = visible.map(c => R(c.getBoundingClientRect().height)), centers = visible.map(c => { const q = c.getBoundingClientRect(); return q.top + q.height / 2; });
  const baselines = [], fonts = new Set();
  // A mono label is drawn lower so that its capitals are centred (mockup-styles.css); its baseline is not the interface face's,
  // so it is held to label-fit's centring rule below instead of the shared baseline.
  for (const c of visible.filter(c => c.matches('.btn, .sel, .scope-btn, .ptab, .tab'))) { const n = firstText(c); if (!n) continue; const g = document.createRange(); g.selectNodeContents(n); const rects = [...g.getClientRects()].filter(x => x.width > 1); if (new Set(rects.map(x => Math.round(x.top))).size > 1) problems.push(`wrapped label: "${label(c)}"`); const s = css(n.parentElement); if (s.fontFamily.startsWith('"Cousine"') || s.fontFamily.startsWith('Cousine')) continue; ctx.font = `${s.fontWeight} ${s.fontSize} ${s.fontFamily}`; fonts.add(s.fontSize); baselines.push(rects[0].top + ctx.measureText('x').fontBoundingBoxAscent); }
  const baseSpread = spread(baselines), centerSpread = spread(centers), heightSpread = spread(heights);
  bars.push({bar: b.dataset.bar, h: R(r.height), want, controls: visible.length, heights: [...new Set(heights)], heightSpread, centerSpread, baselineSpread: baseSpread});
  if (Math.abs(r.height - want) > .5) problems.push(`bar height ${b.dataset.bar}: ${R(r.height)} != ${want}`);
  if (b.scrollWidth > b.clientWidth + 1) problems.push(`bar overflow ${b.dataset.bar}: ${b.scrollWidth}>${b.clientWidth}`);
  if (heightSpread > .5) problems.push(`bar controls differ in height ${b.dataset.bar}: ${heights.join(',')}`);
  if (centerSpread > .5) problems.push(`bar controls not centred ${b.dataset.bar}: ${centerSpread}`);
  if (baseSpread > (fonts.size > 1 ? 1 : .5)) problems.push(`bar labels off baseline ${b.dataset.bar}: ${baseSpread}`);
  if ([...b.querySelectorAll(':scope > .l-bar-actions > :is(.btn, .split)')].filter(el => seen(el)).length > 3) problems.push(`too many actions in a bar: ${b.dataset.bar}`);
}

// Strips, section heads, inner scrollers, disabled controls.
const strips = [...scope.querySelectorAll('.l-strip')].filter(el => seen(el)).map(s => R(s.getBoundingClientRect().height));
for (const h of strips) if (h < 35.5 || h > 56.5) problems.push(`strip height ${h}`);
for (const el of scope.querySelectorAll('.l-region')) if (seen(el) && inScroller(el, scrollsY)) problems.push(`region inside another scroller: ${name(el)}`);
const reasons = [];
for (const el of scope.querySelectorAll('[aria-disabled="true"]')) { if (!seen(el)) continue;
  const after = css(el, '::after').content, before = css(el, '::before').content, id = el.getAttribute('aria-describedby'), t = id && document.getElementById(id);
  const s = css(el), shaped = !el.matches('.btn-primary, .btn-danger') || (s.backgroundColor !== 'rgba(0, 0, 0, 0)' && s.backgroundColor !== 'transparent') || s.boxShadow !== 'none';
  reasons.push({control: label(el), reason: t ? t.textContent.trim().slice(0, 60) : null, background: s.backgroundColor, h: R(el.getBoundingClientRect().height)});
  // A menu row carries its reason as its own second line, so it needs nothing beside it.
  const ownLine = el.matches('.mi') && !!el.dataset.why && el.textContent.includes(el.dataset.why);
  // A closed composer says its reason once, as the placeholder of its empty writing area, which its disabled tools name.
  const said = t && (t.textContent.trim() || (t.matches('textarea, input') && !t.value ? (t.placeholder || '').trim() : ''));
  if (!ownLine && (!t || !seen(t) || !said)) problems.push(`disabled control has no visible reason: "${label(el)}"`);
  if ((after && after !== 'none' && after !== 'normal') || (before && before !== 'none' && before !== 'normal') || (!el.matches('.mi') && el.textContent.includes(el.dataset.why || ' never '))) problems.push(`reason drawn inside control: "${label(el)}"`);
  if (!shaped) problems.push(`disabled primary lost its shape: "${label(el)}"`);
}
const heads = [];
for (const s of scope.querySelectorAll('.l-sect')) { if (!seen(s)) continue; const t = s.querySelector('.l-sect-t'), prev = s.previousElementSibling, next = s.nextElementSibling; if (!t) continue; const g = document.createRange(); g.selectNodeContents(t); const text = g.getBoundingClientRect();
  // A heading that opens its section is measured against what comes before the section.
  const outer = !prev && s.parentElement && s.parentElement.firstElementChild === s ? s.parentElement.previousElementSibling : null;
  const sideways = outer && (outer.getBoundingClientRect().right <= text.left || outer.getBoundingClientRect().left >= text.right);
  const above = prev || (sideways ? null : outer);
  if (above && seen(above)) { const gap = text.top - above.getBoundingClientRect().bottom; heads.push({head: label(s), gapAbove: R(gap)}); if (gap < 4) problems.push(`heading crowds what is above it: "${label(s)}" gap ${R(gap)}`); }
  if (next && next.matches('.l-row')) { const d = t.getBoundingClientRect().left - next.firstElementChild.getBoundingClientRect().left; if (Math.abs(d) > .5) problems.push(`section head not on the row edge: "${label(s)}" ${R(d)}`); } }
const stable = {};
for (const b of barEls) for (const c of b.querySelectorAll('.btn, .ib, .chip, .sel, .seg, .tab')) if (seen(c)) { const r = c.getBoundingClientRect(); stable[b.dataset.bar + ':' + label(c)] = [R(r.left), R(r.top), R(r.width), R(r.height)]; }
const slots = [...scope.querySelectorAll('.l-reason')].filter(el => seen(el)).map(s => s.textContent.trim().slice(0, 60));
// Controls keep the desktop's compact size at every width: nothing in a row is enlarged for touch (owner, 2026-11-10).
const MAX_CTL = 32.5;
// An answer choice may wrap a long answer; one that holds a single line is held to the same size as a button.
const lines = el => { const g = document.createRange(); g.selectNodeContents(el); return new Set([...g.getClientRects()].filter(x => x.width > 1 && x.height > 1).map(x => Math.round(x.top))).size; };
for (const el of scope.querySelectorAll(CTL + ', .ws-hint, .opt')) { if (!seen(el) || el.closest('.gsv')) continue; const r = el.getBoundingClientRect();
  const tall = el.matches('.opt') ? r.height > MAX_CTL + 2 && lines(el) <= 1 : r.height > MAX_CTL;
  if (tall || (el.matches('.ib') && r.width > MAX_CTL)) problems.push(`control larger than the desktop size: ${name(el)} "${label(el)}" ${R(r.width)}x${R(r.height)}`); }
// A help button is a control like its neighbours: a bare "?" glyph with no fill and no edge reads as a stray character.
for (const el of scope.querySelectorAll('button')) { if (!seen(el) || el.textContent.trim() !== '?') continue; const s = css(el);
  const fill = s.backgroundColor !== 'rgba(0, 0, 0, 0)' && s.backgroundColor !== 'transparent', edge = s.boxShadow !== 'none' || (parseFloat(s.borderTopWidth) > 0 && s.borderTopStyle !== 'none' && s.borderTopColor !== 'rgba(0, 0, 0, 0)');
  if (!fill && !edge) problems.push(`help button has no background: "${label(el)}"`); }
// An open menu or popover stays whole inside the window, so none of its rows is cut off at an edge.
for (const el of document.querySelectorAll('[popover]')) { if (!el.matches(':popover-open') || !seen(el)) continue; const r = el.getBoundingClientRect();
  if (r.left < -.5 || r.top < -.5 || r.right > V.w + .5 || r.bottom > V.h + .5) problems.push(`popover outside the window: ${name(el)} "${label(el)}" ${R(r.left)},${R(r.top)}..${R(r.right)},${R(r.bottom)} in ${V.w}x${V.h}`); }
// Every boxed label (control, tab, chip, badge, status word, table head) stays on one line and has its capitals in the middle
// of its box within 1 px: label-fit.js, shared with the page captures, which also check the drawn pixels.
const fit = (LABEL_FIT)(scope);
for (const p of fit.problems) problems.push(p);
return {viewport: V, labels: fit.labels, problems: [...new Set(problems)], counts: {elements: elements.length, parents, shortened, textAndControls: items.length, overlaps}, bars, strips, reasons, heads, stable, slots, support: {layers: typeof CSSLayerBlockRule !== 'undefined', container: CSS.supports('container-type: inline-size'), lineClamp: CSS.supports('-webkit-line-clamp: 2')}};
"""

STRICT = STRICT.replace("LABEL_FIT", os.environ["CW_LABEL_FIT"])

# One line per screen held to the full rules above: name, query, ready expression, sizes.
HARNESS = "document.querySelector('.l-page .l-bar') && window.flip"
DIALOG = "document.querySelector('dialog[open] .l-bar') && window.flip"
FULL = ((960, 540), (1024, 768), (1280, 800), (1872, 1100))
OWNER = ((1024, 768), (1280, 800), (1872, 1100))
board_at = lambda state: "route=" + quote(f"/m/<M>/p/p/s/{state}/board", safe="")
QUEUE = "document.querySelector('[aria-label=\"Sprint queue\"]') && document.querySelector('.card')"
LOGO = "document.querySelector('.l-startup-logo img')?.complete && document.querySelector('.l-startup-logo img').naturalWidth > 0"
# The review screens: a result in the conversation, Changes with and without Git, Ready to merge. 33 checks, long Russian text.
review_at = lambda path: "world=review&route=" + quote(f"/m/<M>/t/{path}", safe="")
RESULT = "document.querySelector('.rcard [data-bar=card]') && document.querySelector('.rcard .rv-checks')"
CHANGES = "document.querySelector('.rv-bar [data-bar=action]') && document.querySelector('.evid .l-row')"
READY = "document.querySelector('.ready .rcard [data-bar=card]')"
SCREENS = (
    ("result-conversation", review_at("rv"), RESULT, OWNER),
    ("changes-no-git", review_at("rv/changes"), CHANGES, OWNER),
    ("changes-git", review_at("rg/changes"), CHANGES + " && document.querySelector('.fhead')", OWNER),
    ("ready-review-and-merge", review_at("rm/changes"), READY, OWNER),
    ("ready-no-git", review_at("rn/changes"), READY, OWNER),
    ("board-paused", board_at("b-pause"), QUEUE, ((960, 540), ) + OWNER),
    ("board-running", board_at("b-run"), QUEUE, ((960, 540), ) + OWNER),
    ("board-blocked", board_at("b-need"), QUEUE, OWNER),
    ("board-idle", board_at("b-idle"), QUEUE, OWNER),
    ("board-manual", board_at("b-manual"), QUEUE, OWNER),
    ("board-no-next", board_at("b-none"), QUEUE, OWNER),
    ("board-complete", board_at("b-done"), QUEUE, OWNER),
    ("board-clamped", board_at("b-clamp"), QUEUE, OWNER),
    ("board-paused-dark", board_at("b-pause") + "&dark=1", QUEUE, ((1280, 800), )),
    ("startup", "startup=pending", LOGO, OWNER),
    ("startup-dark", "startup=pending&dark=1", LOGO, ((1280, 800), )),
    ("startup-failed", "startup=failed", LOGO + " && document.querySelector('.l-startup-slot .btn')", OWNER),
    ("primitives", "view=primitives", HARNESS, FULL),
    ("primitives-long", "view=primitives&stress=1", HARNESS, FULL),
    ("primitives-compact", "view=primitives&pane=780", HARNESS, ((1024, 768), )),
    ("primitives-tight", "view=primitives&pane=620", HARNESS, ((1024, 768), )),
    ("primitives-tight-long", "view=primitives&pane=620&stress=1", HARNESS, ((1024, 768), )),
    ("primitives-dialog", "view=primitives&dialog=1", DIALOG, ((960, 540), (1280, 800))),
    ("primitives-dialog-long", "view=primitives&dialog=1&stress=1", DIALOG, ((960, 540), (1280, 800))),
)

# Every other screen, on the same long names, at the owner's widths. Each is ready once its own main part is on screen.
at = lambda route: "route=" + quote(route, safe="")
FRAME = "document.querySelector('.titlebar') && !document.querySelector('.l-startup')"
on = lambda selector: FRAME + " && document.querySelector(" + json.dumps(selector) + ")"
MS = "/settings/machine/<M>/"
PAGES = (
    ("home", at("/m/<M>"), on(".home")),
    ("inbox", at("/m/<M>/inbox"), on(".main .page")),
    ("archive", at("/m/<M>/archive"), on(".main .page")),
    ("new-task", at("/m/<M>/new?kind=task&project=p"), on(".create .composer textarea")),
    ("new-sprint", at("/m/<M>/new?kind=sprint&project=p"), on(".create .composer textarea")),
    ("project-board", at("/m/<M>/p/p/board"), on(".board-wrap .card")),
    ("project-list", at("/m/<M>/p/p/list"), on(".l-work .l-page-body > *")),
    ("git", at("/m/<M>/p/p/git"), on(".git-sum") + " && document.querySelector('.hrow')"),
    ("code", at("/m/<M>/p/p/code?file=<FILE>"), on(".cm-content") + " && document.querySelector('.ecrumb button')"),
    ("worktrees", at("/m/<M>/p/p/worktrees"), on(".l-work .l-page-body > *")),
    ("knowledge", at("/m/<M>/p/p/knowledge"), on(".l-work .l-page-body > *")),
    ("project-settings", at("/m/<M>/p/p/settings"), on(".l-work .l-page-body > *")),
    ("sprint-planning", at("/m/<M>/p/p/s/sp/planning"), on(".brief")),
    ("sprint-plan", at("/m/<M>/p/p/s/sp/plan"), on(".sp-plan:not([hidden]) .pt-row")),
    # A created sprint's chat: the closed composer says its one note in its writing area, and its tools keep their size.
    ("sprint-chat-created", at("/m/<M>/p/p/s/paused/planning"), on(".sp-conv:not([hidden]) .composer textarea:disabled")),
    ("sprint-summary", at("/m/<M>/p/p/s/sp/summary"), on(".main .page")),
    ("task-conversation", at("/m/<M>/t/task-2"), on(".brief")),
    ("task-workers", at("/m/<M>/t/r-now/workers"), on(".l-task .l-page-body > *")),
    ("task-code", at("/m/<M>/t/task-2/code"), on(".ed")),
) + tuple((f"settings-{s}", at(f"/settings/app/{s}"), on(".set-wrap")) for s in ("appearance", "voice", "updates", "shortcuts", "machines")) \
  + tuple((f"machine-{s}", at(MS + s), on(".set-wrap")) for s in ("overview", "connections", "worktrees", "storage", "diagnostics", "remote")) \
  + tuple((f"orchestration-{p}", at(MS + "orchestration/" + p), on(".set-wrap")) for p in ("defaults", "models", "routing", "effective", "policy", "tools", "profiles", "files"))
SCREENS += tuple((name, query, ready, OWNER) for name, query, ready in PAGES) + tuple((name + "-dark", query + "&dark=1", ready, ((1280, 800), )) for name, query, ready in PAGES if name in ("home", "git", "settings-appearance", "orchestration-routing"))
# The sprint plan's Graph view on a created 60-task plan, from the window minimum up: in the pane, in Full view (FLOWS), and
# with Details docked beside it. The panels give way by one rule, so nothing may overlap or narrow below its minimum.
GRAPH_AT = "graph=1&" + at("/m/<M>/p/p/s/big/plan")
GRAPH_READY = on(".gsv .n") + " && document.querySelector('.gv-band')"
GRAPH_SIZES = FULL + ((1920, 1080), )
GRAPH_PICK = "const n=document.querySelector('.gsv .n[data-id=\"t:big-12\"]');for(const t of ['pointerdown','pointerup'])n.dispatchEvent(new PointerEvent(t,{bubbles:true,button:0}))"
SCREENS += (("sprint-graph", GRAPH_AT, GRAPH_READY, GRAPH_SIZES), ("sprint-graph-dark", GRAPH_AT + "&dark=1", GRAPH_READY, ((1280, 800), )))

# The smallest window the desktop app allows (its min_inner_size, which layout-fit.mjs reads from desktop.rs) and 960 px, with the
# window's own controls in the title band. The task screens are held to the full rules there in each state their title band can
# hold, in both themes; every other page's title band is held to them on its own (title_gate).
MINIMUM = tuple(int(n) for n in os.environ["CW_MIN_WINDOW"].split("x"))
NARROW = tuple(dict.fromkeys(((960, 540), MINIMUM)))
task_at = lambda path: at("/m/<M>/t/" + path) + "&chrome=1"
TASKS = (
    ("task-question", task_at("n-ask"), on(".brief")),
    ("task-running", task_at("r-now"), on(".brief")),
    ("task-paused", task_at("p-stop"), on(".brief")),
    ("task-not-started", task_at("task-2"), on(".brief")),
    ("task-question-workers", task_at("n-ask/workers"), on(".l-task .l-page-body > *")),
    ("task-question-code", task_at("n-ask/code"), on(".ed")),
)
SCREENS += tuple((name + theme, query + more, ready, NARROW + OWNER) for name, query, ready in TASKS for theme, more in (("", ""), ("-dark", "&dark=1")))
# The same at the narrow sizes with the sidebar dragged to its widest (prefs.ts SIDEBAR_MAX), which leaves the title band its least room.
SCREENS += tuple((name + "-wide-sidebar", query + "&sidebar=" + os.environ["CW_SIDEBAR_MAX"], ready, NARROW) for name, query, ready in TASKS)
# A running task with a long Russian message and a pasted image in its queue: the row stays one line, its files are not shown in it.
QUEUED_ROW = on(".queue .qrow .qrow-end")
SCREENS += (("task-queued", task_at("r-now") + "&queued=r-now", QUEUED_ROW, FULL), ("task-queued-dark", task_at("r-now") + "&queued=r-now&dark=1", QUEUED_ROW, ((1280, 800), )))
# A working task's steps that recorded output far wider than any column, in Workers and in Conversation; OUTPUT_FLOWS opens them.
OUTPUT_SCREENS = (("task-output-workers", review_at("out/workers/w-out"), "document.querySelector('.stream button.fold')"), ("task-output-conversation", review_at("out"), "document.querySelector('.timeline button.fold')"))
SCREENS += tuple((name, query, ready, ((1280, 800), )) for name, query, ready in OUTPUT_SCREENS)
# Connected service, unconfirmed task: one notice, its explanation and the real read-only refresh.
UNKNOWN_AT = review_at("unk") + "&dark=1"
UNKNOWN_READY = "document.querySelector('.l-task .l-strip .l-strip-a button') && document.querySelector('.composer textarea:disabled')"
SCREENS += (("task-unknown-state", UNKNOWN_AT, UNKNOWN_READY, FULL),)
OPEN_STEPS = "for(const b of document.querySelectorAll('button.fold[aria-expanded=false]'))b.click();for(const b of document.querySelectorAll('button.step-row[aria-expanded=false]'))b.click()"
TITLE_BAND = tuple((name, query + "&chrome=1", ready) for name, query, ready in PAGES)

# Screens not moved to the primitives yet: name, route, ready, the bars that must keep their height. The row and reason rules apply.
KEPT = ()
ROUTE = {"git": "/m/<M>/p/p/git", "code": "/m/<M>/p/p/code?file=<FILE>", "sprint": "/m/<M>/p/p/s/sp/planning", "sprint-plan": "/m/<M>/p/p/s/sp/plan", "task": "/m/<M>/t/task-2"}

# The old queued row: the file's chip, squeezed to nothing, between the text and Edit, so its name is drawn over the controls.
QUEUED_CHIP = "for(const r of document.querySelectorAll('.queue .qrow')){const s=document.createElement('span');s.className='ws-atts';s.style.cssText='flex:0 1 0;min-width:0';s.innerHTML='<span class=\"chip ws-att\"><button class=\"ws-att-face\"><span class=\"trunc\">clipboard.png</span></button></span>';r.insertBefore(s,r.querySelector('.qrow-end'));}"

# Each rule has to be able to fail. Faults go in a layer named before the app's, so their !important outranks the primitives'.
FAULTS = (
    ("page scrolls", "primitives", "html,body{overflow:visible!important;height:auto!important}#root{height:3000px!important}", None, "page scrolls"),
    ("element outside its parent", "primitives", ".l-z-start{margin-left:-40px!important}", None, "outside"),
    ("text clipped with no way to read it", "primitives-long", ".l-strip-t{display:block!important;-webkit-line-clamp:unset!important;max-height:18px!important}", None, "clipped vertically"),
    ("text squeezed to nothing", "primitives", ".l-z-mid{flex:0 0 12px!important}", None, "text squeezed out"),
    ("text and controls overlap", "primitives", ".l-bar[data-bar=action] .l-bar-actions{position:relative!important;left:-260px!important}", None, "overlap"),
    ("control cut off", "primitives", ".l-bar[data-bar=action] .l-bar-actions{position:relative!important;left:200px!important}", None, "control cut off"),
    ("toolbar grows", "primitives-long", ".l-toolbar{height:auto!important;flex-wrap:wrap!important}.l-z-mid .trunc{white-space:normal!important}", None, "bar height toolbar"),
    ("bar height wrong", "primitives", ".l-bar[data-bar=action]{height:70px!important}", None, "bar height action"),
    ("bar overflows", "primitives", ".l-z-end{padding-right:3000px!important}", None, "bar overflow"),
    ("controls differ in height", "primitives", ".l-bar-actions .btn:first-child{height:20px!important}", None, "differ in height"),
    ("controls not centred", "primitives", ".l-bar-actions .btn:first-child{position:relative!important;top:5px!important}", None, "not centred"),
    ("labels off baseline", "primitives", ".l-bar-actions .btn:first-child{padding-top:10px!important}", None, "off baseline"),
    ("label wraps", "primitives", ".l-bar-actions .btn{white-space:normal!important;width:42px!important}", None, "wrapped label"),
    # The owner's two reports: a label drawn above the middle of its button, and a label broken onto two lines in a box that grows.
    ("label off centre", "primitives", ".l-bar-actions .btn{align-items:flex-start!important}", None, "label off centre: button"),
    ("label wraps in a growing box", "primitives", ".l-bar-actions .btn{white-space:normal!important;width:42px!important;height:auto!important}", None, "wrapped label: button"),
    ("strip out of bounds", "primitives", ".l-strip{min-height:10px!important;max-height:20px!important;padding:0!important}", None, "strip height"),
    ("reason not visible", "primitives", ".l-reason{visibility:hidden!important}", None, "no visible reason"),
    ("disabled primary flat", "primitives", ".btn-primary[aria-disabled=true]{background:none!important;box-shadow:none!important}", None, "lost its shape"),
    ("heading on top of the row above", "primitives", ".l-sect{margin-top:-14px!important}", None, "heading crowds"),
    ("heading off the row edge", "primitives", ".l-sect{padding-inline-start:40px!important}", None, "row edge"),
    ("region nested in a scroller", "primitives", ".l-page-body{overflow-y:scroll!important}", None, "another scroller"),
    ("more than three actions", "primitives", None, "const a=document.querySelector('.l-bar-actions');a.append(a.children[0].cloneNode(true),a.children[0].cloneNode(true),a.children[0].cloneNode(true));", "too many actions"),
    ("cut text with no way to read it", "primitives-long", None, "document.querySelectorAll('[data-whole]').forEach(e=>{e.removeAttribute('data-whole');e.removeAttribute('role');e.removeAttribute('tabindex')});", "cut with no way to read it"),
    # The title band as it was when the desktop journey found its title over the checkout chip: one crumbs box that could
    # shrink below what it held, and a band that did not measure itself. A state named with @WxH opens at that size.
    ("title over the checkout chip", "task-question-dark@960x540", ".tb-main{container-type:normal!important}.tb-main>.crumbs{display:flex!important;min-width:0!important}", None, "overlap"),
    # The queued row as the owner saw it: its file's chip squeezed in before Edit, its name running over the controls.
    ("queued row shows its file", "task-queued@960x540", None, QUEUED_CHIP, "overlap"),
    # A step's output as the owner saw it: long lines cut at the box's right edge with no way to scroll to them.
    ("output clipped sideways", "task-output-workers", ".step-sub .ws-out{overflow:hidden!important}", OPEN_STEPS + ";", "clipped sideways"),
    # The owner's 2026-11-10 reports: touch-size composer controls on a phone, a bare "?" beside Manual and Automatic, and a
    # graph menu opened half off the window.
    ("controls enlarged on a phone", "sprint-planning@390x844", ".mc .c-bar{--ctl-h:44px!important}", None, "larger than the desktop size"),
    ("help button without a background", "board-running", ".ws-hint{background:none!important;box-shadow:none!important;border:0!important}", None, "help button has no background"),
    ("popover off the window", "sprint-graph@960x540", "[popover]:popover-open{translate:600px 0!important}", "document.querySelector('[aria-label=\"Zoom presets\"]').click();", "popover outside the window"),
)

# Review states reached by pressing what the owner presses: a name, the screen, what must be ready, the steps (script, then what
# must hold after it), and the sizes. Each one is held to the full rules, a dialog in its own scope.
PRESS = "[...document.querySelectorAll(%s)].find(e=>e.textContent.trim()===%s).click()"
press = lambda selector, text: PRESS % (json.dumps(selector), json.dumps(text))
DIALOG_OPEN = "document.querySelector('dialog[open] [data-bar=dialog]')"
WHOLE = "document.querySelector('.rv-more').click()"
LONG_REQUEST = "Пожалуйста, верните прежний порядок пунктов бокового меню и оставьте подписи только у групп. " * 6
FILL = "const f=document.querySelector('dialog[open] textarea');f.focus();document.execCommand('insertText',false," + json.dumps(LONG_REQUEST) + ")"
FLOWS = (
    ("task-unknown-state-rechecked", UNKNOWN_AT, UNKNOWN_READY, ((press(".l-task .l-strip button", "Re-check state"), "document.querySelector('.l-task .l-strip-t').textContent.includes('Still not confirmed')"), ), FULL),
    ("changes-whole", review_at("rv/changes"), CHANGES, ((WHOLE, "document.querySelector('.rv-sum.open')"), ), OWNER),
    ("changes-git-whole", review_at("rg/changes"), CHANGES, ((WHOLE, "document.querySelector('.rv-sum.open')"), ), OWNER),
    ("dialog-request-empty", review_at("rg/changes"), CHANGES, ((press(".rv-bar button", "Request changes…"), DIALOG_OPEN), ), OWNER),
    ("dialog-request-filled", review_at("rg/changes"), CHANGES, ((press(".rv-bar button", "Request changes…"), DIALOG_OPEN), (FILL, "!document.querySelector('dialog[open] .btn-primary[aria-disabled]')")), OWNER),
    ("dialog-accept-open-checks", review_at("rv/changes"), CHANGES, ((press(".rv-bar button", "Accept v1"), "document.querySelector('dialog[open][aria-label=\"Accept with open checks\"] [data-bar=dialog]')"), ), OWNER),
    ("dialog-merge", review_at("rm/changes"), READY, ((press(".ready button", "Review and merge…"), "document.querySelector('dialog[open][aria-label=\"Finish task\"] [data-bar=dialog]')"), ), OWNER),
    ("dialog-close", review_at("rn/changes"), READY, ((press(".ready button", "Close as is…"), "document.querySelector('dialog[open][aria-label=\"Close task\"] [data-bar=dialog]')"), ), OWNER),
)
# The other screens' second states: what a press opens, at the owner's widths, on the same long names.
STATE_OF = {name: (query, ready) for name, query, ready in PAGES}
CLICK = "[...document.querySelectorAll(%s)].find(e=>e.textContent.trim()===%s).click()"
click_on = lambda selector, text: CLICK % (json.dumps(selector), json.dumps(text))
OPEN_TABS = "for(const n of ['canonical-exact','canonical-pricing','audit-company','check-model'])[...document.querySelectorAll('.tn')].find(e=>e.textContent.includes(n)).click()"
FIND = "const i=document.querySelector('.ws-find-in');i.value='line';i.dispatchEvent(new Event('input',{bubbles:true}))"
FLOWS += (
    ("git-branches",) + STATE_OF["git"] + (((click_on(".chg-tools button", "Branches and worktrees"), "document.querySelector('.gr')"), ), OWNER),
    ("code-tabs",) + STATE_OF["code"] + (((OPEN_TABS, "document.querySelectorAll('.etab').length >= 4"), ), OWNER),
    ("code-search",) + STATE_OF["code"] + (((click_on(".side-h .seg button", "Search"), "document.querySelector('.ws-find-in')"), (FIND, "document.querySelector('.res-file')")), OWNER),
    ("palette",) + STATE_OF["home"] + ((("document.querySelector('.ws-nav-btn').click()", "document.querySelector('dialog[open] .ws-pal-b')"), ), OWNER),
    ("sprint-read-more",) + STATE_OF["sprint-plan"] + ((("document.querySelector('.ws-more-l').click()", "document.querySelector('.ws-read .ws-read-d')"), ), OWNER),
    ("sprint-graph-full", GRAPH_AT, GRAPH_READY, ((click_on(".gv-bar button", "Full view"), "document.querySelector('.sp-body[data-full]')"), ), GRAPH_SIZES),
    ("sprint-graph-details", GRAPH_AT, GRAPH_READY, ((GRAPH_PICK + ";" + click_on(".gv-band button", "Details"), "document.querySelector('.ws-read .ws-read-d')"), ), GRAPH_SIZES),
    ("add-project-dialog",) + STATE_OF["home"] + ((("[...document.querySelectorAll('.sb-foot .btn')].find(e=>e.textContent.includes('Add project')).click()", "document.querySelector('dialog[open]')"), ), OWNER),
)
# The old fault in its most hidden form: a pseudo-element over the button that moves nothing. Only the reason rule may report it.
PSEUDO_FAULT = "[aria-disabled='true']{position:relative}[aria-disabled='true']::after{content:attr(data-why);position:absolute;inset:0;overflow:hidden;white-space:nowrap;background:inherit}"
DISABLED_BOXES = "return [...document.querySelectorAll('[aria-disabled=\"true\"][data-why]')].filter(e=>e.getClientRects().length).map(e=>{const r=e.getBoundingClientRect();return [e.textContent.trim(),r.left,r.top,r.width,r.height]})"
FLOWS += tuple((name + "-open", query, ready, ((OPEN_STEPS, "document.querySelectorAll('.ws-out').length >= " + ("3" if "workers" in name else "1")), ), FULL) for name, query, ready in OUTPUT_SCREENS)
STATES = {name: (query, ready) for name, query, ready, _ in SCREENS}


def main(fn):
    done, box = threading.Event(), {}
    def run():
        try:
            fn(box, done)
        except Exception as error:
            box["error"] = str(error)
            done.set()
        return False
    GLib.idle_add(run)
    assert done.wait(20), "WebKit callback timeout"
    if "error" in box:
        raise RuntimeError(box["error"])
    return box.get("value")


def js(code):
    def call(box, done):
        def finish(source, result, _):
            try:
                box["value"] = json.loads(source.evaluate_javascript_finish(result).to_string())
            except Exception as error:
                box["error"] = str(error)
            done.set()
        view.evaluate_javascript("JSON.stringify((()=>{" + code + "})()??null)", -1, None, None, None, finish, None)
    return main(call)


def wait(code):
    for _ in range(100):
        if js("return !!(" + code + ")"):
            return
        time.sleep(.1)
    raise TimeoutError(code)


def open_page(query, width, height, ready):
    global loads
    loads += 1
    main(lambda b, d: (win.resize(width, height), d.set()))
    main(lambda b, d: (view.load_uri(sys.argv[1] + f"?{query}&load={loads}"), d.set()))
    wait(f"window.fixture === '{loads}'")
    wait(ready)
    time.sleep(.5)
    actual = js("return [innerWidth, innerHeight]")
    assert actual == [width, height], (actual, width, height)


def load(name, width, height, ready):
    open_page("route=" + quote(ROUTE[name], safe=""), width, height, ready)


def capture(name):
    time.sleep(.3)  # Let WebKit paint the observed state before photographing it.
    def call(box, done):
        width, height = win.get_size()
        pixels = Gdk.pixbuf_get_from_window(win.get_window(), 0, 0, width, height)
        pixels.savev(str(evidence / (label + "-" + name + ".png")), "png", [], [])
        box["value"] = [width, height]
        done.set()
    return main(call)


def check(results, name):
    result = js(PROBE)
    results[name] = result
    capture(name)
    return result["problems"]


def check_strict(results, name):
    result = js(STRICT)
    results[name] = result
    capture(name)
    return result["problems"]


def inject(style):
    js("const s=document.createElement('style');s.id='fault';s.textContent=" + json.dumps("@layer fault{" + style + "}") + ";document.head.prepend(s);return 1")
    time.sleep(.25)


def open_state(name, width=1280, height=800):
    query, ready = STATES[name]
    open_page(query, width, height, ready)


# A press on a disabled control with no slot of its own opens its whole reason beside it and moves nothing.
TIP = """const t=document.querySelector('.l-why-tip'),b=document.querySelector(SEL).getBoundingClientRect(),r=t.getBoundingClientRect(),bar=document.querySelector(SEL).parentElement.getBoundingClientRect();
return {open:t.matches(':popover-open'),text:t.textContent,why:document.querySelector(SEL).dataset.why,box:[b.left,b.top,b.width,b.height],bar:[bar.left,bar.top,bar.width,bar.height],tip:[r.left,r.top,r.right,r.bottom],view:[innerWidth,innerHeight]}"""


def reason_tip(results, failures):
    # A reason too long for its strip: the two lines there are cut, so the press has to bring the whole of it.
    open_state("board-clamped")
    selector = ".l-toolbar .btn[aria-disabled='true'][data-why]"
    probe = TIP.replace("SEL", json.dumps(selector))
    before = js(probe)
    js("document.querySelector(" + json.dumps(selector) + ").click();return 1")
    time.sleep(.25)
    after = js(probe)
    capture("reason-tip")
    js("document.body.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}));return 1")
    time.sleep(.2)
    closed = js(probe)
    results["reason_tip"] = {"before": before, "after": after, "closed": closed}
    l, t, r, b = after["tip"]
    w, h = after["view"]
    failures["reason_tip"] = ([] if not before["open"] else ["open before the press"]) + ([] if after["open"] and after["text"] == after["why"] and after["why"] else ["the press did not open the whole reason"]) \
        + ([] if after["box"] == before["box"] and after["bar"] == before["bar"] else ["the press moved its control"]) + ([] if l >= 0 and t >= 0 and r <= w and b <= h else ["the reason left the window"]) + ([] if not closed["open"] else ["did not close on the next press"])


# What a screen must be like while the workspace loads: only the logo, centred, and nothing of the interface.
STARTUP = r"""
const logo = document.querySelector('.l-startup-logo img'), slot = document.querySelector('.l-startup-slot'), box = logo && logo.getBoundingClientRect();
const frame = [...document.querySelectorAll('.titlebar, .sidebar, .ws-tabstrip, .main, .card')].map(e => e.className);
const problems = [];
if (!logo || !logo.complete || !logo.naturalWidth) problems.push('startup logo missing');
if (box && Math.abs(box.left + box.width / 2 - innerWidth / 2) > .5 || box && Math.abs(box.top + box.height / 2 - innerHeight / 2) > .5) problems.push(`startup logo off centre: ${box.left},${box.top}`);
if (frame.length) problems.push('partial interface during startup: ' + frame.join(', '));
const win = [...document.querySelectorAll('.l-startup-band .win button')].map(b => b.getAttribute('aria-label'));
if (win.length !== 3) problems.push('window controls missing: ' + win.join(', '));
return {problems, logo: box && [box.left, box.top, box.width, box.height], slot: slot ? slot.textContent.trim() : null, win};
"""


def first_paint(results, logo):
    """The page before any script has run: the static logo on the theme surface, exactly where the startup screen keeps it."""
    def paint(theme):
        main(lambda b, d: (view.load_uri(sys.argv[1] + "boot.html"), d.set()))
        wait("document.querySelector('#boot img')?.complete && document.querySelector('#boot img').naturalWidth > 0 && document.documentElement.dataset.theme")
        js("localStorage.setItem('cw.theme'," + json.dumps(theme) + ");return 1")
        main(lambda b, d: (view.reload(), d.set()))
        time.sleep(.5)
        wait("document.querySelector('#boot img')?.complete && document.querySelector('#boot img').naturalWidth > 0 && document.documentElement.dataset.theme" + ("" if theme == "system" else " === " + json.dumps(theme)))
        capture(f"first-paint-{theme}")
        return js("const i=document.querySelector('#boot img'),r=i.getBoundingClientRect();return {logo:[r.left,r.top,r.width,r.height],body:getComputedStyle(document.body).backgroundColor,text:document.body.textContent.trim(),others:document.querySelectorAll('body *:not(script):not(#boot):not(#boot img):not(#root)').length,desktop:matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light',theme:document.documentElement.dataset.theme}")
    # The surface is the app's own theme: a choice of Dark or Light holds whatever the desktop prefers, and only System follows it.
    found = {theme: paint(theme) for theme in ("dark", "light", "system")}
    js("localStorage.removeItem('cw.theme');return 1")
    results["first_paint"] = found
    problems = []
    surface = {"dark": "rgb(24, 27, 26)", "light": "rgb(255, 255, 255)"}
    for theme, want in (("dark", surface["dark"]), ("light", surface["light"]), ("system", surface[found["system"]["desktop"]])):
        got = found[theme]
        if got["logo"] != logo:
            problems.append(f"first paint {theme}: logo at {got['logo']}, startup screen at {logo}")
        if got["body"] != want:
            problems.append(f"first paint {theme}: surface {got['body']}, want {want}")
        if got["text"] or got["others"]:
            problems.append(f"first paint {theme}: more than the logo: {got['text']!r} and {got['others']} other elements")
    return problems


def startup_gate(results, failures):
    """The screen while the service answers: quiet, slow, stalled, failed, and the hand-over to the interface in one frame."""
    def stage(name, text):
        for _ in range(170):
            if js("return document.querySelector('.l-startup-slot').textContent.includes(" + json.dumps(text) + ")"):
                break
            time.sleep(.1)
        found = js(STARTUP)
        capture(f"startup-{name}")
        results[f"startup_{name}"] = found
        return found["problems"] + ([] if text in (found["slot"] or "") else [f"startup {name}: no '{text}' in '{found['slot']}'"])
    open_state("startup")
    quiet = js(STARTUP)
    capture("startup-quiet")
    failures["first_paint"] = first_paint(results, quiet["logo"])
    open_state("startup")
    results["startup_quiet"] = quiet
    failures["startup_quiet"] = quiet["problems"] + ([] if not quiet["slot"] else [f"words on the first frames: {quiet['slot']}"])
    failures["startup_slow"] = stage("slow", "Loading workspace")
    failures["startup_long"] = stage("long", "Still waiting for the service")
    failures["startup_still"] = [] if results["startup_long"]["logo"] == quiet["logo"] == results["startup_slow"]["logo"] else ["the logo moved while waiting"]
    # Every frame is either the logo or the whole interface, never both and never neither.
    js("window.__frames=[];const tick=()=>{window.__frames.push([!!document.querySelector('.l-startup'),!!document.querySelector('.titlebar'),!!document.querySelector('.sidebar')]);window.__tick=requestAnimationFrame(tick)};tick();window.release();return 1")
    wait("document.querySelector('.titlebar') && window.__frames.length > 8")
    time.sleep(.4)
    frames = js("cancelAnimationFrame(window.__tick);return window.__frames")
    results["startup_handover"] = {"frames": len(frames), "logo": sum(1 for f in frames if f[0]), "interface": sum(1 for f in frames if f[1])}
    failures["startup_handover"] = [f"a frame showed {f}" for f in frames if f[0] == f[1] or f[1] != f[2]] + ([] if frames and frames[0][0] and frames[-1][1] else [f"frames ran from {frames[:1]} to {frames[-1:]}"]) + ([] if js("return !document.querySelector('.l-startup')") else ["the logo stayed"])
    # A failure stays, is explained and can be retried, and a retry that works reveals the interface.
    open_state("startup-failed")
    failures["startup_failed"] = stage("failed", "could not be loaded")
    failures["startup_failed"] += [] if js("return [...document.querySelectorAll('.l-startup-slot .btn')].some(b => b.textContent.trim() === 'Try again')") else ["no Try again"]
    js("[...document.querySelectorAll('.l-startup-slot .btn')].find(b => b.textContent.trim() === 'Try again').click();return 1")
    time.sleep(.6)
    still = js(STARTUP)
    failures["startup_failed"] += still["problems"] + ([] if "could not be loaded" in (still["slot"] or "") else ["the failure went away while still refused"])
    js("window.release();[...document.querySelectorAll('.l-startup-slot .btn')].find(b => b.textContent.trim() === 'Try again').click();return 1")
    wait("document.querySelector('.titlebar') && !document.querySelector('.l-startup')")
    # The rules above have to be able to fail.
    results["startup_faults"] = {}
    failures["startup_faults"] = []
    for fault, script, expect in (("interface mounted", "document.body.append(Object.assign(document.createElement('div'),{className:'titlebar'}))", "partial interface"), ("logo off centre", "document.querySelector('.l-startup-logo').style.marginTop='60px'", "off centre"), ("controls missing", "document.querySelector('.l-startup-band .win').remove()", "window controls missing")):
        open_state("startup")
        js(script + ";return 1")
        time.sleep(.2)
        found = js(STARTUP)["problems"]
        results["startup_faults"][fault] = found
        if not any(expect in p for p in found):
            failures["startup_faults"].append(f"not reported: {fault}")


# The board keeps its height, and the end of its toolbar stays put however the queue's state changes.
QUEUE_END = "return [...document.querySelectorAll('[aria-label=\"Sprint queue\"] .l-z-end > *')].filter(e=>e.getClientRects().length).map(e=>{const r=e.getBoundingClientRect();return [Math.round(r.left*2)/2,Math.round(r.top*2)/2,Math.round(r.width*2)/2,Math.round(r.height*2)/2]})"
BUDGET = "const b=document.querySelector('.board-wrap').getBoundingClientRect();return {top:b.top,height:b.height}"


def budget_problems(found, where):
    return ([] if found["top"] <= 220.5 else [f"chrome above the board in {where}: {found['top']}"]) + ([] if found["height"] >= 300 else [f"board too short in {where}: {found['height']}"])


def board_gate(results, failures):
    for name, query, ready, _ in SCREENS:
        if not name.startswith("board-"):
            continue
        open_page(query, 1280, 800, ready)
        found = js(BUDGET)
        results[f"budget_{name}"] = found
        failures[f"budget_{name}"] = budget_problems(found, name)
        results[f"end_{name}"] = js(QUEUE_END)
    ends = {name: results[f"end_{name}"] for name in ("board-paused", "board-running", "board-blocked", "board-idle")}
    failures["queue_end_stable"] = [] if all(e == ends["board-paused"] and len(e) >= 3 for e in ends.values()) else [json.dumps(ends)]
    open_state("board-paused")
    inject(".l-page-status{padding-top:220px!important}")
    found = js(BUDGET)
    failures["budget_fault"] = [] if budget_problems(found, "fault") else ["not reported: chrome over budget"]
    # The tab row folds by the pane's own width, so a wide window with a wide sidebar must fold as well.
    open_state("board-paused", 1024, 768)
    failures["areas_fold"] = [] if js("return getComputedStyle(document.querySelector('.pstrip [aria-label=\"More project areas\"]')).display !== 'none'") else ["no menu for the folded areas at 1024"]
    # The explanation is one press or one key from the queue's mode, and no longer takes a row.
    open_state("board-paused")
    hint = js("return {strip:[...document.querySelectorAll('.l-strip')].some(s=>s.textContent.includes('Passed checks leave')),review:document.body.textContent.includes('Review this task'),button:!!document.querySelector('.l-z-start .ws-hint')}")
    results["hint"] = hint
    js("document.querySelector('.l-z-start .ws-hint').focus();document.querySelector('.l-z-start .ws-hint').click();return 1")
    time.sleep(.3)
    opened = js("return [...document.querySelectorAll('.ws-hint-pop')].some(p=>p.matches(':popover-open') && p.textContent.includes('Passed checks leave'))")
    capture("board-hint-open")
    failures["hint"] = ([] if not hint["strip"] else ["the explanation still takes a strip"]) + ([] if not hint["review"] else ["the banner still says Review this task"]) + ([] if hint["button"] else ["no ? control"]) + ([] if opened else ["the ? did not open the explanation"])


# A target answers a press 11 px out from its centre on every side (half a pixel short of a 24 px edge, for sub-pixel rounding next
# to a neighbour), so its 24 px square or circle is its own (WCAG 2.5.8). The ? does it with a hit area around its 16 px box.
TARGETS = """return [...document.querySelectorAll(SEL)].filter(e=>e.getClientRects().length).map(el=>{const r=el.getBoundingClientRect(),x=r.left+r.width/2,y=r.top+r.height/2,s=getComputedStyle(el);
const hits=[[0,-11],[11,0],[0,11],[-11,0]].map(([dx,dy])=>{const h=document.elementFromPoint(x+dx,y+dy);return !!h&&(h===el||el.contains(h))});
return {label:(el.getAttribute('aria-label')||el.textContent).trim().slice(0,32),w:r.width,h:r.height,look:[parseFloat(s.width)-parseFloat(s.paddingLeft)-parseFloat(s.paddingRight),s.backgroundClip],hits}})"""
# The quiet line on whatever has focus now: its outline, and whether a placed focus marks it.
FOCUS_LINE = "const a=document.activeElement,s=getComputedStyle(a);return {label:(a.getAttribute('aria-label')||a.textContent).trim().slice(0,32),style:s.outlineStyle,width:s.outlineWidth,placed:a.hasAttribute('data-placed-focus'),keyboard:document.documentElement.hasAttribute('data-keyboard')}"
POINTER_PRESS = "const e=SEL;e.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}));e.focus();"
KEY = "document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:%s,bubbles:true,cancelable:true}));return 1"
TIP_STATE = "const t=document.querySelector('.l-why-tip');return {open:t.matches(':popover-open'),text:t.textContent}"


def keys_gate(results, failures):
    """Keyboard reach of cut text, the focus line a dialog or a panel places, press targets and the popovers' roles."""
    # Enter on a cut path and Space on a cut name in a panel open the whole text, as a press does; Escape closes it.
    open_state("changes-git")
    path = js("const e=[...document.querySelectorAll('.fhead .l-whole[role=button]')].find(e=>e.scrollWidth>e.clientWidth+1);if(!e)return null;e.focus();return {whole:e.dataset.whole,focused:document.activeElement===e}")
    before = js(TIP_STATE)
    js(KEY % json.dumps("Enter"))
    time.sleep(.3)
    entered = js(TIP_STATE)
    capture("keys-reveal-enter")
    js(KEY % json.dumps("Escape"))
    time.sleep(.2)
    escaped = js(TIP_STATE)
    open_state("git")
    name = js("const e=[...document.querySelectorAll('[data-whole][role=button]:not(.l-whole):not(.l-reason):not(.l-strip-t)')].find(e=>e.scrollWidth>e.clientWidth+1);if(!e)return null;e.focus();return {text:e.textContent.trim(),focused:document.activeElement===e}")
    js(KEY % json.dumps(" "))
    time.sleep(.3)
    spaced = js(TIP_STATE)
    capture("keys-reveal-space")
    results["keys_reveal"] = {"path": path, "before": before, "enter": entered, "escape": escaped, "name": name, "space": spaced}
    failures["keys_reveal"] = ([] if path and path["focused"] and not before["open"] else [f"no focusable cut path, or a script focus opened it: {path} {before}"]) \
        + ([] if path and entered["open"] and entered["text"] == path["whole"] else [f"Enter did not open the whole path: {entered}"]) + ([] if not escaped["open"] else ["Escape left it open"]) \
        + ([] if name and name["focused"] and spaced["open"] and spaced["text"] == name["text"] else [f"Space did not open a cut name in a panel: {name} {spaced}"])
    # A dialog opened by a pointer press: the press draws nothing, the control the dialog focuses carries the line, a press clears it.
    open_state("changes-no-git")
    trigger = "[...document.querySelectorAll('.rv-bar button')].find(e=>e.textContent.trim()==='Accept v1')"
    pressed = js(POINTER_PRESS.replace("SEL", trigger) + FOCUS_LINE)
    js(trigger + ".click();return 1")
    wait("document.querySelector('dialog[open][aria-label=\"Accept with open checks\"] [data-bar=dialog]')")
    time.sleep(.3)
    placed = js(FOCUS_LINE)
    capture("keys-dialog-focus")
    cleared = js(POINTER_PRESS.replace("SEL", "document.querySelector('dialog[open] .dlg-b')||document.querySelector('dialog[open]')").replace("e.focus();", "") + FOCUS_LINE)
    # A panel that opens with its close control focused does the same.
    open_state("sprint-plan")
    js(POINTER_PRESS.replace("SEL", "document.querySelector('.ws-more-l')") + "e.click();return 1")
    wait("document.querySelector('.ws-read') && document.activeElement && document.activeElement.getAttribute('aria-label')==='Close task details'")
    time.sleep(.3)
    panel = js(FOCUS_LINE)
    capture("keys-panel-focus")
    results["keys_focus"] = {"pressed": pressed, "placed": placed, "cleared": cleared, "panel": panel}
    line = lambda f: f["style"] not in ("none", "") and f["width"] == "1px"
    failures["keys_focus"] = ([] if not line(pressed) and not pressed["keyboard"] else [f"a pointer press drew a line: {pressed}"]) + ([] if line(placed) and placed["placed"] else [f"the dialog's focused control has no line: {placed}"]) \
        + ([] if not line(cleared) and not cleared["placed"] else [f"the next press kept the line: {cleared}"]) + ([] if line(panel) and panel["placed"] else [f"the panel's focused control has no line: {panel}"])
    # Press targets: the queue's ? is a filled 28 px control like the Manual/Automatic switch beside it (owner, 2026-11-10), in the
    # 44 px toolbar; a changed file's actions keep their 22 px look.
    open_state("board-paused")
    hint = js(TARGETS.replace("SEL", json.dumps(".l-z-start .ws-hint")))
    bar = js("return document.querySelector('[aria-label=\"Sprint queue\"]').getBoundingClientRect().height")
    capture("keys-hint-target")
    pops = js("return [...document.querySelectorAll('[popover]:not(.l-why-tip)')].map(p=>[p.id,p.getAttribute('role'),p.getAttribute('aria-label'),p.querySelector('[role=menuitem]')?'menu':'dialog'])")
    open_state("git")
    js("document.querySelectorAll('.gf .acts').forEach(a=>a.classList.add('on'));return 1")
    files = js(TARGETS.replace("SEL", json.dumps(".gf .ib")))
    capture("keys-git-targets")
    results["keys_targets"] = {"hint": hint, "bar": bar, "files": files[:6], "file_count": len(files), "popovers": pops}
    small = lambda found: [t for t in found if t["w"] < 24 or t["h"] < 24 or not all(t["hits"])]
    failures["keys_targets"] = ([] if hint and all(all(t["hits"]) and t["w"] == t["h"] == 28 for t in hint) else [f"the ? target: {hint}"]) + ([] if abs(bar - 44) <= .5 else [f"the queue toolbar is {bar} px"]) \
        + ([] if files and not small(files) and all(t["look"] == [22, "content-box"] for t in files) else [f"changed-file actions: {small(files)[:3] or files[:2]}"])
    failures["popover_roles"] = ([] if pops else ["no popovers on the board"]) + [f"popover {p[0]} has role {p[1]}, want {p[3]}" for p in pops if p[1] != p[3]] + [f"popover {p[0]} label not exposed" for p in pops if p[2] and p[1] not in ("menu", "dialog")]


# What the owner's screens showed wrong: the comment bar where nothing can be commented, an empty panel and a sentence said twice,
# a void above the bottom bar, Accept and Request changes out of line, headings on top of the counts above them.
REVIEW_FACTS = r"""
const panel = document.querySelector('[aria-label="Changes"]'), foot = document.querySelector('[aria-label="Review comments"]');
const body = document.querySelector('.rv-panel .chg-body'), acts = [...document.querySelectorAll('.rv-bar .l-bar-actions > .btn')].map(b => { const r = b.getBoundingClientRect(); return [b.textContent.trim(), r.top, r.height]; });
const textTop = el => { const g = document.createRange(); g.selectNodeContents(el); return g.getBoundingClientRect().top; };
const gap = (above, head) => above && head ? Math.round((textTop(head) - above.getBoundingClientRect().bottom) * 100) / 100 : null;
return {foot: !!foot, footTop: foot ? foot.getBoundingClientRect().top : null, body: body ? [body.getBoundingClientRect().bottom, body.getBoundingClientRect().height] : null,
  notePanels: panel ? panel.querySelectorAll('.note-panel').length : null, applied: panel ? (panel.textContent.match(/Changes were applied in the project folder/g) || []).length : null, acts,
  cardGap: gap(document.querySelector('.rcard .rv-outc'), document.querySelector('.rcard .rv-checks .l-sect-t')),
  evidenceGap: gap(document.querySelector('.evid > summary'), document.querySelector('.evid .rv-checks .l-sect-t')),
  rawHeadings: [...document.querySelectorAll('.rcard h3, .rv-panel h3, dialog[open] h3')].filter(h => !h.matches('.l-sect-t, .ws-result-copy h3')).length};
"""


def review_problems(found, where, git):
    out = []
    if git:
        out += [] if found["foot"] else [f"{where}: no comment bar"]
        out += [] if found["body"] and found["footTop"] is not None and abs(found["body"][0] - found["footTop"]) <= 1 else [f"{where}: space between the diff and the bottom bar: {found['body']} {found['footTop']}"]
        out += [] if found["body"] and found["body"][1] >= 239.5 else [f"{where}: diff squeezed below 240px: {found['body']}"]
    else:
        out += [] if not found["foot"] else [f"{where}: comment bar without Git"]
        out += [] if found["applied"] == 1 else [f"{where}: the no-Git sentence said {found['applied']} times"]
    out += [] if found["notePanels"] in (0, None) else [f"{where}: an empty panel takes the page"]
    tops, heights = {round(a[1], 1) for a in found["acts"]}, {round(a[2], 1) for a in found["acts"]}
    out += [] if len(found["acts"]) >= 2 and len(tops) == 1 and len(heights) == 1 else [f"{where}: Request changes and Accept not in one row: {found['acts']}"]
    out += [] if found["evidenceGap"] is None or found["evidenceGap"] >= 4 else [f"{where}: Check manually crowds Evidence: {found['evidenceGap']}"]
    out += [] if not found["rawHeadings"] else [f"{where}: {found['rawHeadings']} unstyled headings"]
    return out


def flow_gate(results, failures):
    """Every FLOWS entry at its sizes: open the screen, press what the owner presses, hold the result to the full rules."""
    for name, query, ready, steps, sizes in FLOWS:
        for width, height in sizes:
            key = f"{name}-{width}x{height}"
            open_page(query, width, height, ready)
            for script, after in steps:
                js(script + ";return 1")
                wait(after)
                time.sleep(.3)
            failures[key] = check_strict(results, key)


def review_gate(results, failures):
    for width, height in OWNER:
        for name, git in (("changes-no-git", False), ("changes-git", True), ("changes-whole", False), ("changes-git-whole", True)):
            query, ready = STATES.get(name) or next((q, r) for n, q, r, _, _ in FLOWS if n == name)
            open_page(query, width, height, ready)
            if name.endswith("whole"):
                js(WHOLE + ";return 1")
                wait("document.querySelector('.rv-sum.open')")
                time.sleep(.3)
            found = js(REVIEW_FACTS)
            results[f"review-{name}-{width}"] = found
            failures[f"review-{name}-{width}"] = review_problems(found, f"{name} {width}", git)
        open_state("result-conversation", width, height)
        found = js(REVIEW_FACTS)
        results[f"review-result-{width}"] = found
        failures[f"review-result-{width}"] = ([] if found["cardGap"] is not None and found["cardGap"] >= 4 else [f"Check manually crowds the counts: {found['cardGap']}"]) + ([] if not found["rawHeadings"] else ["unstyled headings"])
    # The diff keeps its room and meets the bottom bar: a summary allowed to take the whole height must be caught.
    open_state("changes-git")
    js(WHOLE + ";return 1")
    inject(".rv-top{max-height:none!important;flex:none!important}")
    failures["review_fault"] = [] if review_problems(js(REVIEW_FACTS), "fault", True) else ["not reported: the summary took the diff's room"]
    # Cut text: a press on a cut path shows the whole of it, a press on a cut check name opens its row.
    open_state("changes-git")
    path = js("const e=[...document.querySelectorAll('.fhead .l-whole')].find(e=>e.scrollWidth>e.clientWidth+1);if(!e)return null;const tip=document.querySelector('.l-why-tip');const before=tip.matches(':popover-open');e.click();return {whole:e.dataset.whole,role:e.getAttribute('role'),before}")
    time.sleep(.3)
    tip = js("const t=document.querySelector('.l-why-tip');return {open:t.matches(':popover-open'),text:t.textContent}")
    capture("reveal-path")
    js("document.body.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}));return 1")
    row = js("const t=[...document.querySelectorAll('.rv-checks .l-row-t[role=button]')][0];if(!t)return null;const h=t.closest('.l-row').getBoundingClientRect().height;t.click();return {h}")
    time.sleep(.3)
    opened = js("const t=document.querySelector('.l-row[data-open] .l-row-t');return t?{cut:t.scrollWidth>t.clientWidth+1,h:t.closest('.l-row').getBoundingClientRect().height}:null")
    capture("reveal-row")
    results["reveal"] = {"path": path, "tip": tip, "row": row, "opened": opened}
    failures["reveal"] = ([] if path and path["role"] == "button" and not path["before"] else [f"no cut path to press: {path}"]) + ([] if path and tip["open"] and tip["text"] == path["whole"] else [f"the press did not show the whole path: {tip}"]) \
        + ([] if row and opened and not opened["cut"] and opened["h"] > row["h"] else [f"a cut check name did not open: {row} {opened}"])
    # Enabling Send and reopen changes the words in the composer's reason slot and moves nothing.
    open_state("changes-git")
    js(press(".rv-bar button", "Request changes…") + ";return 1")
    wait(DIALOG_OPEN)
    time.sleep(.3)
    # Measured within the composer's tools row: the field above grows with its text, so the dialog may grow, but the row must not change.
    boxes = "const bar=document.querySelector('dialog[open] .mc .c-bar'),f=bar.getBoundingClientRect();return {slot:bar.querySelector('.l-reason').textContent,bar:[f.width,f.height],boxes:[...bar.querySelectorAll('.btn, .ib')].map(b=>{const r=b.getBoundingClientRect();return [(b.getAttribute('aria-label')||b.textContent).trim(),r.left-f.left,r.top-f.top,r.width,r.height,b.getAttribute('aria-disabled')]})}"
    before = js(boxes)
    js(FILL + ";return 1")
    time.sleep(.3)
    after = js(boxes)
    results["dialog_flip"] = {"before": before, "after": after}
    moved = [b for b, a in zip(before["boxes"], after["boxes"]) if b[:5] != a[:5]]
    failures["dialog_flip"] = ([] if before["slot"] == "Say what should change" else [f"empty: slot says {before['slot']!r}"]) + ([] if any(b[0] == "Send and reopen" and b[5] == "true" for b in before["boxes"]) else ["Send and reopen is not a disabled button"]) \
        + ([] if any(b[0] == "Send and reopen" and b[5] is None for b in after["boxes"]) else ["Send and reopen did not enable"]) + [f"moved: {m}" for m in moved] + ([] if after["slot"] != before["slot"] else ["the footer kept the reason"]) + ([] if after["bar"] == before["bar"] else [f"the footer changed size: {before['bar']} {after['bar']}"])


# Dark and light differ in colour only: every visible box must sit at the same place, at the same size, in both.
BOXES = "return [...document.querySelectorAll('body *')].filter(e=>e.getClientRects().length && !e.closest('svg') && getComputedStyle(e).visibility!=='hidden').map(e=>{const r=e.getBoundingClientRect();return e.tagName+'.'+String(e.className).split(' ').slice(0,2).join('.')+' '+[r.left,r.top,r.width,r.height].map(n=>Math.round(n*2)/2).join(',')})"


def theme_gate(results, failures):
    for name, query, ready in PAGES:
        boxes = {}
        # Back and Forward in the title band follow the view's own history, so both themes are measured after a load of their own.
        open_page(query, 1280, 800, ready)
        for theme, q in (("dark", query + "&dark=1"), ("light", query)):
            open_page(q, 1280, 800, ready)
            boxes[theme] = js(BOXES)
        moved = [f"{a} | {b}" for a, b in zip(boxes["light"], boxes["dark"]) if a != b]
        results[f"theme-{name}"] = {"boxes": len(boxes["light"]), "moved": moved[:5]}
        failures[f"theme-{name}"] = ([] if len(boxes["light"]) == len(boxes["dark"]) else [f"{len(boxes['light'])} boxes in light, {len(boxes['dark'])} in dark"]) + moved[:5]


# The full rules on the title band alone.
TITLE_STRICT = STRICT.replace("const scope = document.querySelector('dialog[open]') || document.body;", "const scope = document.querySelector('.titlebar');")
assert TITLE_STRICT != STRICT


def title_gate(results, failures, wanted=lambda name: True, narrow=lambda sizes: sizes):
    """Every page's title band at the narrow sizes, with the window's own controls: nothing overlaps, is cut off or runs out of it."""
    for name, query, ready in TITLE_BAND:
        for width, height in narrow(NARROW) if wanted("title-" + name) else ():
            key = f"title-{name}-{width}x{height}"
            open_page(query, width, height, ready)
            result = js(TITLE_STRICT)
            results[key] = result
            capture(key)
            failures[key] = result["problems"] + ([] if js("return document.querySelectorAll('.titlebar .win[role=group] button').length") == 3 else ["window controls missing from the title band"])


# The queued row: one line of text that stops before its own controls, which keep their box, and no file shown in it.
QUEUE_ROW = r"""
const problems = [], rows = [...document.querySelectorAll('.queue .qrow')];
if (!rows.length) problems.push('no queued row');
if (document.querySelector('.queue .ws-att, .queue .ws-atts')) problems.push('a file is shown in the queued row');
const R = n => Math.round(n * 100) / 100, out = [];
for (const row of rows) { const r = row.getBoundingClientRect(), t = row.querySelector('.qrow-t'), e = row.querySelector('.qrow-end');
  if (!t || !e) { problems.push('queued row lost its text or controls'); continue; }
  const tr = t.getBoundingClientRect(), er = e.getBoundingClientRect(), g = document.createRange(); g.selectNodeContents(t);
  const lines = new Set([...g.getClientRects()].filter(x => x.width > 1).map(x => Math.round(x.top))).size;
  out.push({row: [R(r.left), R(r.top), R(r.width), R(r.height)], text: [R(tr.left), R(tr.right)], controls: [R(er.left), R(er.right)], lines, cut: t.scrollWidth > t.clientWidth + 1, reachable: t.getAttribute('role') === 'button'});
  if (Math.abs(r.height - 40) > .5) problems.push(`queued row height ${R(r.height)} != 40`);
  // One row size: the grip, Edit, Send now and remove share one height, so none reads as a smaller, broken control.
  const heights = [...row.querySelectorAll('.grip, .qrow-end > *')].filter(b => b.getClientRects().length).map(b => R(b.getBoundingClientRect().height));
  if (new Set(heights).size > 1) problems.push(`queued controls differ in height: ${heights.join(', ')}`);
  if (lines !== 1) problems.push(`queued text on ${lines} lines`);
  // Every piece of text and every control in the row keeps clear of every other one, as the owner reads them.
  const items = [], walk = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
  // Text is measured where it is drawn: inside every box above it that cuts what overflows.
  const clip = el => { let c = {left: -1e9, top: -1e9, right: 1e9, bottom: 1e9}; for (let a = el; a && a !== document.documentElement; a = a.parentElement) { const s = getComputedStyle(a); if (s.overflowX !== 'visible' || s.overflowY !== 'visible') { const q = a.getBoundingClientRect(); c = {left: Math.max(c.left, q.left), top: Math.max(c.top, q.top), right: Math.min(c.right, q.right), bottom: Math.min(c.bottom, q.bottom)}; } } return c; };
  for (let n = walk.nextNode(); n; n = walk.nextNode()) { if (!n.nodeValue.trim()) continue; const k = document.createRange(), c = clip(n.parentElement); k.selectNodeContents(n); for (const q of k.getClientRects()) { const x = {left: Math.max(q.left, c.left), top: Math.max(q.top, c.top), right: Math.min(q.right, c.right), bottom: Math.min(q.bottom, c.bottom)}; if (x.right - x.left > 1 && x.bottom - x.top > 1) items.push({node: n.parentElement, label: n.nodeValue.trim().slice(0, 24), x}); } }
  for (const b of row.querySelectorAll('button')) if (b.getClientRects().length) items.push({node: b, label: (b.getAttribute('aria-label') || b.textContent).trim().slice(0, 24), x: b.getBoundingClientRect()});
  for (let i = 0; i < items.length; i++) for (let j = i + 1; j < items.length; j++) { const a = items[i], b = items[j]; if (a.node === b.node || a.node.contains(b.node) || b.node.contains(a.node)) continue;
    const w = Math.min(a.x.right, b.x.right) - Math.max(a.x.left, b.x.left), h = Math.min(a.x.bottom, b.x.bottom) - Math.max(a.x.top, b.x.top); if (w > .5 && h > .5) problems.push(`queued row: "${a.label}" runs into "${b.label}"`); }
  for (const b of row.querySelectorAll('.qrow-end > *')) { const x = b.getBoundingClientRect(); if (x.left < r.left - .5 || x.right > r.right + .5 || x.top < r.top - .5 || x.bottom > r.bottom + .5) problems.push(`queued control outside its row: "${(b.getAttribute('aria-label') || b.textContent).trim().slice(0, 24)}"`); }
  if (t.scrollWidth > t.clientWidth + 1 && t.getAttribute('role') !== 'button') problems.push('cut queued text has no way to read it'); }
return {problems: [...new Set(problems)], rows: out};
"""


def queue_gate(results, failures, narrow=lambda sizes: sizes):
    """The queued row at every width it is held to, and its own fault: the old chip squeezed in before Edit must be reported."""
    for width, height in narrow(FULL):
        key = f"queue-row-{width}x{height}"
        open_state("task-queued", width, height)
        results[key] = js(QUEUE_ROW)
        failures[key] = results[key]["problems"]
        capture(key)
    open_state("task-queued", 960, 540)
    js(QUEUED_CHIP + "return 1")
    time.sleep(.2)
    found = js(QUEUE_ROW)["problems"]
    capture("queue-row-fault")
    results["queue_row_fault"] = found
    failures["queue_row_fault"] = [] if any("file is shown" in p for p in found) and any("runs into" in p for p in found) else ["the old queued row was not reported: " + json.dumps(found)]


# Each opened output box: wider content than its box, which scrolls sideways when moved, takes keyboard focus, keeps a name, and
# stays inside its column, which itself never scrolls sideways; nor does the page.
OUTPUT_BOX = r"""
const problems = [], R = n => Math.round(n * 100) / 100, out = [], doc = document.documentElement;
const boxes = [...document.querySelectorAll('.ws-out')].filter(b => b.getClientRects().length);
if (!boxes.length) problems.push('no output box');
if (doc.scrollWidth > doc.clientWidth + 1) problems.push(`page scrolls sideways: ${doc.scrollWidth}>${doc.clientWidth}`);
for (const b of boxes) { const s = getComputedStyle(b), name = b.getAttribute('aria-label') || '(no name)', col = b.closest('.steps');
  const before = b.scrollLeft; b.scrollLeft = 160; const moved = b.scrollLeft - before; b.scrollLeft = before;
  b.focus(); const focused = document.activeElement === b; b.blur();
  const r = b.getBoundingClientRect(), c = col ? col.getBoundingClientRect() : null;
  out.push({name, box: [R(r.left), R(r.right)], column: c && [R(c.left), R(c.right)], scroll: [b.scrollWidth, b.clientWidth], moved, focused, space: s.whiteSpace, x: s.overflowX});
  if (!b.getAttribute('aria-label') || b.getAttribute('role') !== 'region') problems.push(`output box has no name: ${name}`);
  if (b.scrollWidth <= b.clientWidth + 1) problems.push(`output not wider than its box: ${name} ${b.scrollWidth}<=${b.clientWidth}`);
  if (moved <= 0) problems.push(`output does not scroll sideways: ${name}`);
  if (!focused || b.tabIndex !== 0) problems.push(`output box takes no keyboard focus: ${name}`);
  if (s.whiteSpace !== 'pre') problems.push(`output lines wrap: ${name} ${s.whiteSpace}`);
  if (!c) problems.push(`output box outside a step column: ${name}`);
  else { if (r.left < c.left - 1 || r.right > c.right + 1) problems.push(`output box outside its column: ${name} ${R(r.left)}..${R(r.right)} vs ${R(c.left)}..${R(c.right)}`);
    if (col.scrollWidth > col.clientWidth + 1) problems.push(`step column scrolls sideways: ${col.scrollWidth}>${col.clientWidth}`); } }
return {problems: [...new Set(problems)], boxes: out};
"""


def output_gate(results, failures, narrow=lambda sizes: sizes):
    """The opened output boxes in Workers and Conversation at every width they are held to."""
    for name, query, ready in OUTPUT_SCREENS:
        for width, height in narrow(FULL):
            key = f"{name}-scroll-{width}x{height}"
            open_page(query, width, height, ready)
            js(OPEN_STEPS + ";return 1")
            wait("document.querySelector('.ws-out')")
            time.sleep(.3)
            results[key] = js(OUTPUT_BOX)
            failures[key] = results[key]["problems"]
            # The capture shows the first box moved sideways, so the end of its long lines is on screen.
            js("const b=document.querySelector('.ws-out');b.scrollIntoView({block:'nearest'});b.scrollLeft=b.scrollWidth;return 1")
            capture(key)


# Sizes and edges at the phone width and the window minimum: the compact desktop controls, filled help buttons and menus kept
# inside the window. Only these rules apply here; the full rules hold the same pages from 960 px up.
FIT_RULES = ("larger than the desktop size", "help button has no background", "popover outside the window")
FIT_PAGES = ("sprint-planning", "sprint-plan", "sprint-chat-created", "task-question", "board-running", "new-task", "new-sprint", "orchestration-routing", "settings-voice")
FIT_SIZES = ((390, 844), (960, 540))
MORE_LAYERS = "document.querySelector('[aria-label=\"More graph controls\"]').click()"
PICK_LAYERS = "[...document.querySelectorAll('[popover]:popover-open .mi')].find(e=>e.textContent.trim()==='Layers').click()"
# The owner's menu: Layers opened from the graph's overflow menu, at the widths where the toolbar folds it there.
FIT_FLOWS = (("sprint-graph-layers", GRAPH_AT, GRAPH_READY, ((MORE_LAYERS, "document.querySelector('[popover]:popover-open .mi')"), (PICK_LAYERS, "document.querySelector('.gv-layers:popover-open')")), ((960, 540), (1024, 768))),)


def fit_gate(results, failures, wanted=lambda name: True):
    for name in FIT_PAGES:
        if not wanted(name):
            continue
        for width, height in FIT_SIZES:
            key = f"fit-{name}-{width}x{height}"
            open_state(name, width, height)
            found = js(STRICT)
            results[key] = {"problems": found["problems"]}
            failures[key] = [p for p in found["problems"] if any(rule in p for rule in FIT_RULES)]
            capture(key)
    for name, query, ready, steps, sizes in FIT_FLOWS:
        if not wanted(name):
            continue
        for width, height in sizes:
            key = f"{name}-{width}x{height}"
            open_page(query, width, height, ready)
            for script, after in steps:
                js(script + ";return 1")
                wait(after)
                time.sleep(.3)
            # An open menu lies over the page on purpose; it is held to the edge rules, not to the overlap rule.
            found = check_strict(results, key)
            failures[key] = [p for p in found if any(rule in p for rule in FIT_RULES + ("outside viewport",))]
            box = js("const r=document.querySelector('.gv-layers:popover-open').getBoundingClientRect();return [r.left,r.top,r.right,r.bottom]")
            results[key]["layers"] = box
            if box[0] < 0 or box[2] > width or box[3] > height:
                failures[key].append(f"layers menu outside the window: {box}")


def strict_gate(results, failures):
    """Every SCREENS entry, every title band at the narrow sizes, the disabled-to-enabled flip, the pseudo-element guard and the fault injections."""
    for name, query, ready, sizes in SCREENS:
        for width, height in sizes:
            key = f"{name}-{width}x{height}"
            open_page(query, width, height, ready)
            failures[key] = check_strict(results, key)
    title_gate(results, failures)
    # Enabling a control changes the words in its bar and nothing else.
    open_state("primitives")
    before = js(STRICT)
    js("window.flip();return 1")
    time.sleep(.3)
    after = js(STRICT)
    compared = [k for k in before["stable"] if k in after["stable"]]
    moved = {k: [before["stable"][k], after["stable"][k]] for k in compared if before["stable"][k] != after["stable"][k]}
    results["flip"] = {"compared": len(compared), "moved": moved, "slotsBefore": before["slots"], "slotsAfter": after["slots"]}
    failures["flip"] = ([] if compared else ["nothing to compare"]) + [f"moved when enabled: {k}" for k in moved] + ([] if before["slots"] != after["slots"] else ["the reason slot did not change"]) + after["problems"]
    reason_tip(results, failures)
    theme_gate(results, failures)
    flow_gate(results, failures)
    review_gate(results, failures)
    board_gate(results, failures)
    startup_gate(results, failures)
    keys_gate(results, failures)
    queue_gate(results, failures)
    output_gate(results, failures)
    fit_gate(results, failures)
    # A reason drawn over the button by a pseudo-element moves nothing, so only the reason rule can see it.
    open_state("primitives")
    boxes = js(DISABLED_BOXES)
    inject(PSEUDO_FAULT)
    found = js(STRICT)["problems"]
    expected = sorted({f'reason drawn inside control: "{b[0] or "More"}"' for b in boxes})
    results["pseudo_guard"] = {"controls": len(boxes), "unchanged": boxes == js(DISABLED_BOXES), "expected": expected, "found": found}
    failures["pseudo_guard"] = [] if boxes and boxes == js(DISABLED_BOXES) and sorted(found) == expected else [json.dumps(results["pseudo_guard"])]
    # Every rule above must report its own fault.
    results["faults"], failures["faults"] = {}, []
    for fault, state, style, script, expect in FAULTS:
        at, _, size = state.partition("@")
        open_state(at, *(map(int, size.split("x")) if size else ()))
        if style:
            inject(style)
        if script:
            js(script + "return 1")
            time.sleep(.2)
        problems = js(STRICT)["problems"]
        hits = [p for p in problems if expect in p]
        results["faults"][fault] = {"state": state, "reported": len(problems), "matching": len(hits), "first": hits[:2]}
        if not hits:
            failures["faults"].append(f"not reported: {fault} (expected '{expect}', got {problems[:3]})")


# A field that grows with its text: real editing commands, at a usual and at a short window. The controls must stay inside the window throughout.
# A name, the page, what must be ready, the press that opens the field (or None), the field, its controls and the feed under it.
grow_route = lambda route: "route=" + quote(route, safe="")
GROW = (
    ("new-task", grow_route("/m/<M>/new?kind=task&project=p"), None, None, ".create .composer textarea", ".create-foot .btn, .create .c-bar .ib", None),
    ("task", grow_route("/m/<M>/t/r-now"), None, None, ".dock .composer textarea:not(:disabled)", ".dock .c-bar .ib, .dock .c-bar .c-send", ".timeline"),
    # The planning chat is its own tab, with the whole width at every window size.
    ("sprint", grow_route("/m/<M>/p/p/s/sp/planning"), "document.querySelector('.sp-conv:not([hidden]) .composer textarea')", None, ".sp-conv:not([hidden]) .composer textarea", ".sp-conv .c-bar .ib, .sp-conv .c-bar .c-send", ".sp-conv .timeline"),
    # A dialog draft is the shared composer, whose layout must not stretch it while it is measured: emptied, it returns to its first height.
    ("request-changes", review_at("rg/changes"), CHANGES, press(".rv-bar button", "Request changes…"), "dialog[open] .composer textarea", "dialog[open] .c-bar .ib, dialog[open] [data-bar=dialog] .btn", None),
)
GROW_SIZES = ((1280, 800), (960, 540))
GROW_TEXT = "\n".join(f"Line {i} of a long pasted message" for i in range(80))
GROW_STATE = r"""
const f = document.querySelector(FIELD), r = f.getBoundingClientRect(), doc = document.documentElement, s = getComputedStyle(f);
const controls = [...document.querySelectorAll(CONTROLS)].filter(e => e.getClientRects().length).map(e => { const q = e.getBoundingClientRect(); return {name: (e.getAttribute('aria-label') || e.textContent || '').trim().slice(0, 24), top: q.top, bottom: q.bottom, left: q.left, right: q.right}; });
const feed = FEED && document.querySelector(FEED), dock = feed && document.querySelector('.dock');
let clear = null;
if (feed) { feed.scrollTop = feed.scrollHeight; const tail = [...feed.firstElementChild.children].filter(e => e.getClientRects().length).at(-1); clear = {tail: tail.getBoundingClientRect().bottom, dock: dock.getBoundingClientRect().top, space: getComputedStyle(feed).getPropertyValue('--dock-clear').trim(), height: dock.offsetHeight}; }
return {height: r.height, top: r.top, bottom: r.bottom, client: f.clientHeight, scroll: f.scrollHeight, overflow: getComputedStyle(f).overflowY, scrollbar: Math.round(f.offsetWidth - f.clientWidth - parseFloat(s.borderLeftWidth) - parseFloat(s.borderRightWidth)), cap: Math.round(innerHeight * .4), view: innerHeight, pageScrolls: doc.scrollHeight > doc.clientHeight + 1 || doc.scrollWidth > doc.clientWidth + 1, controls, clear};
"""


def grow_edit(field, command, value=None):
    # execCommand is the engine's own editing path, the one a paste takes: undo, caret and the input event are real.
    js("const f=document.querySelector(" + json.dumps(field) + ");f.focus();f.select();document.execCommand(" + json.dumps(command) + ",false," + json.dumps(value) + ");return 1")
    time.sleep(.3)


def grow_state(field, controls, feed):
    return js(GROW_STATE.replace("FIELD", json.dumps(field)).replace("CONTROLS", json.dumps(controls)).replace("FEED", json.dumps(feed)))


def grow_problems(name, floor, short, at_cap, cleared):
    out = []
    state = lambda s: f"{name}: " + s
    if not floor["height"] < short["height"] < at_cap["height"]:
        out.append(state(f"did not grow: {floor['height']} {short['height']} {at_cap['height']}"))
    if short["overflow"] != "hidden" or short["scrollbar"] != 0:
        out.append(state(f"scrolls below the cap: {short['overflow']} {short['scrollbar']}"))
    if abs(at_cap["height"] - at_cap["cap"]) > 1:
        out.append(state(f"cap {at_cap['height']}, want {at_cap['cap']}"))
    if at_cap["overflow"] != "auto" or at_cap["scroll"] <= at_cap["client"]:
        out.append(state(f"no inner scroll at the cap: {at_cap['overflow']} {at_cap['scroll']}/{at_cap['client']}"))
    if cleared["height"] != floor["height"] or cleared["overflow"] != "hidden":
        out.append(state(f"did not shrink back: {cleared['height']} from {floor['height']}, {cleared['overflow']}"))
    for label, found in (("floor", floor), ("short", short), ("cap", at_cap), ("cleared", cleared)):
        if found["pageScrolls"]:
            out.append(state(f"window scrolls at {label}"))
        out += [state(f"control off the window at {label}: {c['name']} {c['top']}..{c['bottom']}") for c in found["controls"] if c["top"] < 0 or c["bottom"] > found["view"] or c["left"] < 0]
        if not found["controls"]:
            out.append(state(f"no controls found at {label}"))
        if found["clear"] and found["clear"]["tail"] > found["clear"]["dock"] + 1:
            out.append(state(f"dock covers the end of the feed at {label}: {found['clear']}"))
        if found["clear"] and found["clear"]["space"] != f"{found['clear']['height'] + 16}px":
            out.append(state(f"feed not told the dock's height at {label}: {found['clear']}"))
    return out


def grow_open(query, ready, opener, field, width, height):
    open_page(query, width, height, ready or f"document.querySelector({json.dumps(field)})")
    if opener:
        js(opener + ";return 1")
        wait(f"document.querySelector({json.dumps(field)})")
        time.sleep(.3)


def grow_gate(results, failures):
    for name, query, ready, opener, field, controls, feed in GROW:
        for width, height in GROW_SIZES:
            key = f"grow-{name}-{width}x{height}"
            grow_open(query, ready, opener, field, width, height)
            floor = grow_state(field, controls, feed)
            grow_edit(field, "insertText", "\n".join(["one", "two", "three", "four", "five"]))
            short = grow_state(field, controls, feed)
            grow_edit(field, "insertText", GROW_TEXT)
            at_cap = grow_state(field, controls, feed)
            capture(key)
            # At the cap the field scrolls by itself and typing leaves both it and the page where they are.
            js("const f=document.querySelector(" + json.dumps(field) + ");f.scrollTop=120;const p=f.closest('.create');if(p)p.scrollTop=40;f.setSelectionRange(0,0);document.execCommand('insertText',false,'x');return 1")
            time.sleep(.3)
            kept = js("const f=document.querySelector(" + json.dumps(field) + ");const p=f.closest('.create');return {field:f.scrollTop,page:p?p.scrollTop:null,canPage:p?p.scrollHeight>p.clientHeight+40:false}")
            grow_edit(field, "delete")
            cleared = grow_state(field, controls, feed)
            results[key] = {"floor": floor, "short": short, "cap": at_cap, "cleared": cleared, "kept": kept}
            failures[key] = grow_problems(key, floor, short, at_cap, cleared) + ([] if kept["page"] is None or not kept["canPage"] or kept["page"] == 40 else [f"{key}: the page jumped while typing: {kept}"])
    # The rules above have to be able to fail.
    results["grow_faults"], failures["grow_faults"] = {}, []
    for fault, style, size, expect in (("no cap", "textarea[data-grow]{max-height:none!important}", (1280, 800), "cap"), ("footer in the flow", ".create-foot{position:static!important}", (960, 540), "control off the window")):
        name, query, ready, opener, field, controls, feed = GROW[0]
        grow_open(query, ready, opener, field, size[0], size[1])
        inject(style)
        floor = grow_state(field, controls, feed)
        grow_edit(field, "insertText", "\n".join(["one", "two", "three", "four", "five"]))
        short = grow_state(field, controls, feed)
        grow_edit(field, "insertText", GROW_TEXT)
        at_cap = grow_state(field, controls, feed)
        grow_edit(field, "delete")
        problems = grow_problems(fault, floor, short, at_cap, grow_state(field, controls, feed))
        results["grow_faults"][fault] = problems[:3]
        if not any(expect in p for p in problems):
            failures["grow_faults"].append(f"not reported: {fault}: {problems[:3]}")


def click(selector, text=None):
    needle = "e.textContent.trim()===" + json.dumps(text) if text else "true"
    wait("[...document.querySelectorAll(" + json.dumps(selector) + ")].some(e=>" + needle + ")")
    js("[...document.querySelectorAll(" + json.dumps(selector) + ")].find(e=>" + needle + ").click()")
    time.sleep(.3)


def run():
    global exit_code
    results, failures = {}, {}
    try:
        if sys.argv[3:] == ["keys"]:
            # A quick pass on the keyboard, focus-line, target and popover-role rules alone; the full run includes them.
            keys_gate(results, failures)
            results["failures"] = {k: v for k, v in failures.items() if v}
            (evidence / (label + "-metrics.json")).write_text(json.dumps(results, indent=2, ensure_ascii=False) + "\n")
            print(json.dumps({"checked": len(failures), "results": {k: results[k] for k in ("keys_reveal", "keys_focus", "keys_targets")}, "failed": results["failures"]}, ensure_ascii=False, indent=1), flush=True)
            assert not results["failures"], "rendered layout problems"
            return
        if sys.argv[3:4] != ["only"]:
            grow_gate(results, failures)
        if sys.argv[3:] == ["grow"]:
            results["failures"] = {k: v for k, v in failures.items() if v}
            (evidence / (label + "-metrics.json")).write_text(json.dumps(results, indent=2) + "\n")
            print(json.dumps({"checked": len(failures), "sizes": {k: [v[f]["height"] for f in ("floor", "short", "cap", "cleared")] for k, v in results.items() if k.startswith("grow-")}, "failed": results["failures"]}, ensure_ascii=False, indent=1), flush=True)
            assert not results["failures"], "rendered layout problems"
            return
        if sys.argv[3:4] == ["only"]:
            # A quick pass while working on a screen: the SCREENS and FLOWS whose names start with the given words, full rules only.
            # A word like @1280 keeps only that width.
            widths = {int(w[1:]) for w in sys.argv[4:] if w.startswith("@")}
            wanted = lambda name: any(name.startswith(w) for w in sys.argv[4:] if not w.startswith("@"))
            narrow = lambda sizes: [s for s in sizes if not widths or s[0] in widths]
            outline = "const m=document.querySelector('.main')||document.body;return [...m.querySelectorAll(':scope > *, :scope > * > *, :scope > * > * > *')].slice(0,40).map(e=>e.tagName.toLowerCase()+'.'+String(e.className).split(' ').join('.'))"
            for name, query, ready, sizes in SCREENS:
                for width, height in narrow(sizes) if wanted(name) else ():
                    key = f"{name}-{width}x{height}"
                    try:
                        open_page(query, width, height, ready)
                    except TimeoutError:
                        failures[key] = ["not ready: " + json.dumps(js(outline))]
                        capture(key)
                        continue
                    failures[key] = check_strict(results, key)
            for name, query, ready, steps, sizes in FLOWS:
                for width, height in narrow(sizes) if wanted(name) else ():
                    key = f"{name}-{width}x{height}"
                    open_page(query, width, height, ready)
                    for script, after in steps:
                        js(script + ";return 1")
                        wait(after)
                        time.sleep(.3)
                    failures[key] = check_strict(results, key)
            title_gate(results, failures, wanted, narrow)
            if wanted("task-queued"):
                queue_gate(results, failures, narrow)
            if wanted("task-output"):
                output_gate(results, failures, narrow)
            if "fit" in sys.argv[4:]:
                fit_gate(results, failures)
            if "themes" in sys.argv[4:]:
                theme_gate(results, failures)
            results["failures"] = {k: v for k, v in failures.items() if v}
            (evidence / (label + "-metrics.json")).write_text(json.dumps(results, indent=2) + "\n")
            print(json.dumps({"checked": len(failures), "failed": {k: {"count": len(v), "first": v[:12]} for k, v in results["failures"].items()}}, ensure_ascii=False, indent=1), flush=True)
            assert not results["failures"], "rendered layout problems"
            return
        for width, height in SIZES:
            size = f"{width}x{height}"
            load("git", width, height, "document.querySelector('.git-sum') && document.querySelector('.hrow')")
            failures[f"git-{size}"] = check(results, f"git-{size}")
            click(".chg-tools button", "Branches and worktrees")
            wait("document.querySelector('.gr')")
            failures[f"branches-{size}"] = check(results, f"branches-{size}")
            load("code", width, height, "document.querySelector('.cm-content') && document.querySelector('.ecrumb button')")
            click(".side-h .seg button", "Files")
            wait("document.querySelector('.tn')")
            failures[f"code-{size}"] = check(results, f"code-{size}")
            for name in ("canonical-exact", "canonical-pricing", "audit-company", "check-model"):
                js("[...document.querySelectorAll('.tn')].find(e=>e.textContent.includes(" + json.dumps(name) + ")).click()")
                time.sleep(.2)
            wait("document.querySelectorAll('.etab').length >= 4")
            failures[f"code-tabs-{size}"] = check(results, f"code-tabs-{size}")
            click(".side-h .seg button", "Search")
            js("const i=document.querySelector('.ws-find-in');i.value='line';i.dispatchEvent(new Event('input',{bubbles:true}))")
            wait("document.querySelector('.res-file')")
            failures[f"search-{size}"] = check(results, f"search-{size}")
        for name, route, ready, bars in KEPT:
            for width, height in SIZES:
                key = f"{name}-{width}x{height}"
                open_page("route=" + quote(route, safe=""), width, height, ready)
                failures[key] = check(results, key)
                for selector, want in bars.items():
                    got = js("return Math.round(document.querySelector(" + json.dumps(selector) + ").getBoundingClientRect().height * 100) / 100")
                    if abs(got - want) > .5:
                        failures[key].append(f"bar height {selector}: {got} != {want}")
        strict_gate(results, failures)
        load("sprint", 1280, 800, "document.querySelector('.brief')")
        failures["sprint-1280x800"] = check(results, "sprint-1280x800")
        results["sprint"] = js("return {brief: document.querySelector('.brief').innerHTML, ol: document.querySelectorAll('.brief ol li').length, br: document.querySelectorAll('.brief br').length, note: document.querySelectorAll('.sp-conv .ai br').length, mine: document.querySelectorAll('.sp-conv .mine br').length}")
        capture("sprint-brief")
        load("sprint-plan", 1280, 800, "document.querySelector('.sp-plan:not([hidden]) .ws-more-l')")
        click(".ws-more-l")
        wait("document.querySelector('.ws-read-d')")
        failures["read-more-1280x800"] = check(results, "read-more-1280x800")
        results["read_more"] = js("return {h1: document.querySelectorAll('.ws-read-d h1').length, ol: document.querySelectorAll('.ws-read-d ol li').length, br: document.querySelectorAll('.ws-read-d br').length, accept: document.querySelectorAll('.ws-read-d li').length}")
        capture("read-more")
        load("task", 1280, 800, "document.querySelector('.brief')")
        results["task"] = js("return {ol: document.querySelectorAll('.brief ol li').length, br: document.querySelectorAll('.brief br').length}")
        capture("task-brief")
        failures["text"] = [] if results["sprint"]["ol"] == 2 and results["sprint"]["br"] >= 1 and results["sprint"]["note"] >= 1 and results["sprint"]["mine"] >= 1 and results["read_more"]["h1"] == 1 and results["read_more"]["ol"] == 2 and results["read_more"]["br"] >= 1 and results["read_more"]["accept"] >= 4 and results["task"]["ol"] == 2 and results["task"]["br"] >= 1 else [json.dumps({k: results[k] for k in ("sprint", "read_more", "task")})]
        # The probe proves it can fail: the same page with reasons back inside the toolbar and names free to wrap must be caught.
        load("git", 1280, 800, "document.querySelector('.git-sum')")
        js("const s=document.createElement('style');s.textContent='.git-sum [data-why]::after{content:attr(data-why)!important}.git-sum .cap{white-space:normal!important;overflow:visible!important}.ws-gf .p,.hrow .trunc{white-space:normal!important;overflow:visible!important}';document.head.append(s)")
        time.sleep(.3)
        results["probe_catches_faults"] = len(js(PROBE)["problems"])
        failures["probe"] = [] if results["probe_catches_faults"] > 0 else ["the probe did not notice an injected fault"]
        results["failures"] = {k: v for k, v in failures.items() if v}
        (evidence / (label + "-metrics.json")).write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps({"checked": len(failures), "grow_faults": results["grow_faults"], "faults": {k: v["matching"] for k, v in results["faults"].items()}, "pseudo_guard": results["pseudo_guard"]["controls"], "flip": results["flip"]["compared"], "failed": {k: {"count": len(v), "first": v[:4]} for k, v in results["failures"].items()}}, ensure_ascii=False, indent=1), flush=True)
        assert not results["failures"], "rendered layout problems"
    except Exception as error:
        exit_code = 1
        print(type(error).__name__ + ": " + str(error), file=sys.stderr, flush=True)
    finally:
        GLib.idle_add(Gtk.main_quit)


threading.Thread(target=run, daemon=True).start()
try:
    Gtk.main()
finally:
    win.destroy()
    xvfb.terminate()
    try:
        xvfb.wait(timeout=5)
    except subprocess.TimeoutExpired:
        xvfb.kill()
        xvfb.wait(timeout=5)
sys.exit(exit_code)
