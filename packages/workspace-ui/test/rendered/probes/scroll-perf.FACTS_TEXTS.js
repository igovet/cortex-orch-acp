(() => { const s = JSON.stringify; window.__facts = {}; window.__factsCost = []; JSON.stringify = function (v, ...rest) { const t0 = performance.now(), out = s.call(this, v, ...rest);
  if (typeof out === 'string' && out.startsWith('{"projectId"') && v && v.repo !== undefined) { const k = v.projectId, l = (__facts[k] ??= []); l.push({at: Math.round(performance.now()), text: out}); if (l.length > 2) l.shift();
    __factsCost.push([Math.round(t0), Math.round(performance.now() - t0), out.length]); } return out; }; })();
