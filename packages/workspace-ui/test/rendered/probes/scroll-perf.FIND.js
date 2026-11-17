const out = [];
for (const el of document.querySelectorAll('body *')) {
  if (el.closest('svg')) continue;
  const s = getComputedStyle(el), r = el.getBoundingClientRect();
  if (r.width < 120 || r.height < 120 || !el.getClientRects().length) continue;
  const y = (s.overflowY === 'auto' || s.overflowY === 'scroll') && el.scrollHeight > el.clientHeight + 300;
  const x = !y && (s.overflowX === 'auto' || s.overflowX === 'scroll') && el.scrollWidth > el.clientWidth + 300;
  if (!x && !y) continue;
  el.dataset.perf = String(out.length);
  const name = el.tagName.toLowerCase() + (typeof el.className === 'string' && el.className.trim() ? '.' + el.className.trim().split(/\s+/).slice(0, 2).join('.') : '');
  out.push({id: out.length, name, axis: y ? 'y' : 'x', range: y ? el.scrollHeight - el.clientHeight : el.scrollWidth - el.clientWidth,
    reverse: s.display.includes('flex') && s.flexDirection === 'column-reverse', box: [r.left, r.top, r.width, r.height].map(Math.round)});
}
return {scrollers: out, nodes: document.querySelectorAll('*').length, idleCallback: typeof requestIdleCallback, contentVisibility: CSS.supports('content-visibility', 'auto')};
