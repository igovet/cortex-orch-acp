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
