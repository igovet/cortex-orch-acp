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
