window.__idle = {bursts: [], slow: []}; const t0 = performance.now(); let n = 0, last = 0;
const where = n => { const el = n.nodeType === 1 ? n : n.parentElement; const at = el?.closest('[aria-label], section, aside, nav, main, header'); return at ? at.tagName.toLowerCase() + (at.getAttribute('aria-label') ? '[' + at.getAttribute('aria-label').slice(0, 30) + ']' : '.' + String(at.className).split(' ')[0]) : '?'; };
new MutationObserver(list => { const t = Math.round((performance.now() - t0) / 100) / 10; let b = __idle.bursts.at(-1);
  if (!b || b.t !== t) __idle.bursts.push(b = {t, n: 0, added: 0, removed: 0, attributes: 0, text: 0, targets: {}});
  for (const r of list) { b.n++; if (r.type === 'childList') { b.added += r.addedNodes.length; b.removed += r.removedNodes.length; } else if (r.type === 'attributes') { b.attributes++; b.names = b.names ?? {}; b.names[r.attributeName] = (b.names[r.attributeName] ?? 0) + 1;
      const v = r.target.getAttribute(r.attributeName), k = r.target.tagName + ' ' + r.attributeName + ': ' + r.oldValue + ' -> ' + v; b.changes = b.changes ?? {}; if (Object.keys(b.changes).length < 12 || b.changes[k]) b.changes[k] = (b.changes[k] ?? 0) + 1; } else b.text++;
    const w = where(r.target); b.targets[w] = (b.targets[w] ?? 0) + 1; } }).observe(document.body, {subtree: true, childList: true, attributes: true, attributeOldValue: true, characterData: true});
const tick = t => { if (last && t - last > 25) __idle.slow.push([Math.round((t - t0) / 100) / 10, Math.round(t - last)]); last = t; if (++n < 4300) requestAnimationFrame(tick); else __idle.done = true; };
requestAnimationFrame(tick); return 1;
