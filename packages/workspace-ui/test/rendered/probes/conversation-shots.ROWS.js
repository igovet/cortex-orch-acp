const rows = {'title band': '.tb-actions', 'composer tools': '.dock .mc .c-bar', 'answer tools': '.dc .mc .c-bar', 'queued row': '.queue .qrow', 'task bar': '.taskbar'};
const out = {};
for (const [name, sel] of Object.entries(rows)) { const row = document.querySelector(sel); if (!row) continue;
  const hs = [...row.querySelectorAll('.btn, .ib, .sel, .seg, .chip')].filter(e => e.getClientRects().length && !e.closest('.seg > *') && getComputedStyle(e).visibility !== 'hidden').map(e => Math.round(e.getBoundingClientRect().height * 10) / 10);
  out[name] = [...new Set(hs)]; }
// What runs past the right edge of the window, if anything: the deepest such boxes, by class.
out['past the right edge'] = [...document.querySelectorAll('body *')].filter(e => e.getClientRects().length && e.getBoundingClientRect().right > innerWidth + 1 && ![...e.children].some(c => c.getBoundingClientRect().right > innerWidth + 1)).slice(0, 6).map(e => e.tagName.toLowerCase() + '.' + String(e.className).split(' ').slice(0, 2).join('.') + ' ' + Math.round(e.getBoundingClientRect().right));
return out;
