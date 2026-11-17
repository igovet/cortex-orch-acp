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
