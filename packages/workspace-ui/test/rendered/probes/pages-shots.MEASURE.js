const CTL = '.btn, .ib, .sel, .seg, .chip, .pick, select, input:not([type=checkbox]):not([type=radio]):not([type=range])';
const seen = e => e.getClientRects().length && getComputedStyle(e).visibility !== 'hidden' && !e.closest('[hidden], [inert]');
const mixed = [];
const parents = new Set([...document.querySelectorAll(CTL)].filter(seen).map(e => e.parentElement));
for (const p of parents) {
  const kids = [...p.children].filter(e => e.matches(CTL) && seen(e) && !e.closest('.seg > *'));
  if (kids.length < 2) continue;
  const rows = {};
  for (const k of kids) { const r = k.getBoundingClientRect(); const line = Math.round(r.top + r.height / 2); (rows[line] ??= new Set()).add(Math.round(r.height)); }
  for (const hs of Object.values(rows)) if (Math.max(...hs) - Math.min(...hs) > 1) mixed.push(p.tagName.toLowerCase() + '.' + String(p.className).split(' ').slice(0, 2).join('.') + ' ' + JSON.stringify([...hs]));
}
const named = [];
const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
for (let n = walker.nextNode(); n; n = walker.nextNode()) if (/cortex/i.test(n.textContent) && n.parentElement && seen(n.parentElement) && !n.parentElement.closest('pre, code, .cm-editor, textarea')) named.push(n.textContent.trim().slice(0, 80));
for (const e of document.querySelectorAll('[aria-label], [title], [placeholder], [data-why]')) for (const a of ['aria-label', 'title', 'placeholder', 'data-why']) { const v = e.getAttribute(a); if (v && /cortex/i.test(v)) named.push(a + ': ' + v.slice(0, 80)); }
const past = [...document.querySelectorAll('body *')].filter(e => seen(e) && e.getBoundingClientRect().right > innerWidth + 1 && !e.closest('.l-region, [data-scroll=sideways], pre, .cm-editor') && ![...e.children].some(c => c.getBoundingClientRect().right > innerWidth + 1)).slice(0, 6).map(e => e.tagName.toLowerCase() + '.' + String(e.className).split(' ').slice(0, 2).join('.') + ' ' + Math.round(e.getBoundingClientRect().right));
const fit = (LABEL_FIT)(document.querySelector('dialog[open]') || document.body);
// No text or control runs into another that is not its own ancestor or descendant, measured where each is drawn (inside every
// box that cuts it). The sprint graph's drawing is its own.
const clipOf = el => { let c = {l: 0, t: 0, r: innerWidth, b: innerHeight}; for (let a = el; a && a !== document.documentElement; a = a.parentElement) { const s = getComputedStyle(a); if (s.overflowX !== 'visible' || s.overflowY !== 'visible') { const q = a.getBoundingClientRect(); c = {l: Math.max(c.l, q.left), t: Math.max(c.t, q.top), r: Math.min(c.r, q.right), b: Math.min(c.b, q.bottom)}; } } return c; };
const cutTo = (r, c) => ({l: Math.max(r.left, c.l), t: Math.max(r.top, c.t), r: Math.min(r.right, c.r), b: Math.min(r.bottom, c.b)});
const runs = [], scopeEl = document.querySelector('dialog[open]') || document.body;
const tw = document.createTreeWalker(scopeEl, NodeFilter.SHOW_TEXT);
for (let n = tw.nextNode(); n; n = tw.nextNode()) { const p = n.parentElement; if (!n.nodeValue.trim() || !p || !seen(p) || p.closest('script, style, .gsv, .ws-live')) continue; const g = document.createRange(); g.selectNodeContents(n); const c = clipOf(p); for (const r of g.getClientRects()) { const x = cutTo(r, c); if (x.r - x.l > 1 && x.b - x.t > 1) runs.push({node: p, label: n.nodeValue.trim().slice(0, 32), ...x}); } }
for (const el of scopeEl.querySelectorAll('button, a, input, textarea')) { if (!seen(el) || el.closest('.gsv')) continue; const x = cutTo(el.getBoundingClientRect(), clipOf(el.parentElement)); if (x.r - x.l > 1 && x.b - x.t > 1) runs.push({node: el, label: (el.getAttribute('aria-label') || el.textContent || '').trim().slice(0, 32), ...x}); }
// A bar pinned over a scroller (a sticky footer with its own fill) hides what scrolls under it; that is not a collision.
const pinnedOf = el => { for (let a = el; a && a !== document.documentElement; a = a.parentElement) { const s = getComputedStyle(a); if ((s.position === 'sticky' || s.position === 'fixed') && s.backgroundColor !== 'rgba(0, 0, 0, 0)' && s.backgroundColor !== 'transparent') return a; } return null; };
const overlaps = [];
for (let i = 0; i < runs.length && overlaps.length < 12; i++) for (let j = i + 1; j < runs.length && overlaps.length < 12; j++) { const a = runs[i], b = runs[j]; if (a.node === b.node || a.node.contains(b.node) || b.node.contains(a.node)) continue; if (pinnedOf(a.node) !== pinnedOf(b.node)) continue; const w = Math.min(a.r, b.r) - Math.max(a.l, b.l), h = Math.min(a.b, b.b) - Math.max(a.t, b.t); if (w > 1 && h > 1) overlaps.push(`overlap "${a.label}" with "${b.label}" (${Math.round(w)}x${Math.round(h)})`); }
return {viewport: [innerWidth, innerHeight], sideways: document.documentElement.scrollWidth > innerWidth + 1, mixed: [...new Set(mixed)].slice(0, 12), named: [...new Set(named)].slice(0, 12), past, labels: [...fit.problems.slice(0, 30), ...overlaps], labelCount: fit.problems.length, worst: fit.labels, entries: fit.entries, covers: [...document.querySelectorAll(':popover-open')].map(p => { const r = p.getBoundingClientRect(); return [r.left, r.top, r.right, r.bottom]; })};
