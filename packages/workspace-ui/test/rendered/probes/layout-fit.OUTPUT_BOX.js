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
