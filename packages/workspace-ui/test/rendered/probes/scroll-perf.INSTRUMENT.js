(() => {
  const P = window.__perf = {on: false};
  let frames, last, flush, mutations, handlers, handlerMs, handlerMax, ro, roMs, roMax, scrolls, wheels, lat, wheelAt, slow, run = 0;
  const reset = () => { frames = []; last = 0; flush = []; mutations = 0; handlers = 0; handlerMs = 0; handlerMax = 0; ro = 0; roMs = 0; roMax = 0; scrolls = 0; wheels = 0; lat = []; wheelAt = null; slow = []; };
  reset();
  const add = EventTarget.prototype.addEventListener, remove = EventTarget.prototype.removeEventListener, wraps = new WeakMap();
  const timed = fn => { let w = wraps.get(fn); if (!w) { w = function (e) { if (!P.on) return fn.call(this, e); const t = performance.now(); try { return fn.call(this, e); } finally { const d = performance.now() - t; handlers++; handlerMs += d; if (d > handlerMax) handlerMax = d; } }; wraps.set(fn, w); } return w; };
  EventTarget.prototype.addEventListener = function (type, fn, o) { return add.call(this, type, (type === 'scroll' || type === 'wheel') && typeof fn === 'function' ? timed(fn) : fn, o); };
  EventTarget.prototype.removeEventListener = function (type, fn, o) { return remove.call(this, type, (typeof fn === 'function' && wraps.get(fn)) || fn, o); };
  const RO = window.ResizeObserver;
  window.ResizeObserver = class extends RO { constructor(cb) { super(function (entries, o) { if (!P.on) return cb.call(this, entries, o); const t = performance.now(); try { return cb.call(this, entries, o); } finally { const d = performance.now() - t; ro++; roMs += d; if (d > roMax) roMax = d; } }); } };
  new MutationObserver(list => { if (P.on) mutations += list.length; }).observe(document.documentElement, {subtree: true, childList: true, attributes: true, characterData: true});
  // A notch that moved nothing (the end was reached) is not waiting for a scroll any more once the next one comes.
  add.call(window, 'wheel', () => { if (!P.on) return; wheels++; const now = performance.now(); if (wheelAt === null || now - wheelAt > 100) wheelAt = now; }, {capture: true, passive: true});
  add.call(window, 'scroll', () => { if (!P.on) return; scrolls++; if (wheelAt !== null) { lat.push(performance.now() - wheelAt); wheelAt = null; } }, {capture: true, passive: true});
  const tick = (t, mine) => { if (!P.on || mine !== run) return; if (last) { frames.push(t - last); if (t - last > 25) slow.push(Math.round(t - last)); } last = t;
    const s = performance.now(); void document.documentElement.offsetHeight; flush.push(performance.now() - s); requestAnimationFrame(t => tick(t, mine)); };
  const q = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s.length ? +s[Math.min(s.length - 1, Math.floor(p * s.length))].toFixed(1) : 0; };
  const r1 = n => +n.toFixed(1);
  P.start = () => { reset(); P.on = true; const mine = ++run; requestAnimationFrame(t => tick(t, mine)); };
  P.stop = () => { P.on = false; return {frames: frames.length, p50: q(frames, .5), p95: q(frames, .95), max: r1(Math.max(0, ...frames)),
    dropped: frames.filter(f => f > 25).length, long: frames.filter(f => f > 50).length, slow: slow.slice(0, 12),
    layoutMs: r1(flush.reduce((a, b) => a + b, 0)), layoutMax: r1(Math.max(0, ...flush)), layoutFrames: flush.filter(f => f > 1).length,
    mutations, handlers, handlerMs: r1(handlerMs), handlerMax: r1(handlerMax), resizeCallbacks: ro, resizeMs: r1(roMs), resizeMax: r1(roMax),
    scrolls, wheels, latencyP95: q(lat, .95), latencyMax: r1(Math.max(0, ...lat))}; };
})();
