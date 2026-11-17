window.bench = null;
const canvas = document.querySelector('.gv-canvas'), r = canvas.getBoundingClientRect();
const frames = (kind) => new Promise(resolve => { const out = []; let last = 0, n = 0;
  const tick = t => { if (last) out.push(t - last); last = t; if (++n > 125) return resolve(out.slice(5));
    const t0 = performance.now();
    canvas.dispatchEvent(kind === 'zoom' ? new WheelEvent('wheel', {deltaY: n % 40 < 20 ? -25 : 25, ctrlKey: true, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2, bubbles: true, cancelable: true})
      : new WheelEvent('wheel', {deltaY: n % 40 < 20 ? 18 : -18, deltaX: n % 60 < 30 ? 6 : -6, bubbles: true, cancelable: true}));
    handler.push(performance.now() - t0);
    requestAnimationFrame(tick); };
  requestAnimationFrame(tick); });
const painted = run => new Promise(resolve => { const t0 = performance.now(); run(); const sync = performance.now() - t0; requestAnimationFrame(() => requestAnimationFrame(() => resolve({sync, frame: performance.now() - t0}))); });
const stats = xs => { const s = [...xs].sort((a, b) => a - b), q = p => s[Math.min(s.length - 1, Math.floor(p * s.length))]; return {n: s.length, median: +q(.5).toFixed(2), p95: +q(.95).toFixed(2), max: +s[s.length - 1].toFixed(2)}; };
let handler = [];
(async () => {
  const settle = () => new Promise(res => setTimeout(res, 300));
  const pan = await frames('pan'); const panHandler = stats(handler); handler = []; await settle();
  const zoom = await frames('zoom'); const zoomHandler = stats(handler); handler = []; await settle();
  const press = el => { for (const t of ['pointerdown', 'pointerup']) el.dispatchEvent(new PointerEvent(t, {bubbles: true, button: 0})); };
  const picks = []; for (const k of [5, 40, 77, 120, 160, 199]) { picks.push(await painted(() => press(document.querySelector(`.gsv .n[data-id="t:huge-${k}"]`)))); await new Promise(r => setTimeout(r, 400)); }
  press(canvas); await settle();
  const boxes = [...document.querySelectorAll('[aria-label="Show tasks by status"] input[type=checkbox]')];
  const filters = []; for (const b of boxes.slice(0, 3)) for (const on of [true, false]) { filters.push(await painted(() => { b.checked = on; b.dispatchEvent(new Event('change', {bubbles: true})); })); await settle(); }
  const keyZoom = []; for (const k of ['0', '-', '-', '+', 'f']) { const n = document.querySelector('.gsv .n[tabindex="0"]'); n.focus(); keyZoom.push(await painted(() => n.dispatchEvent(new KeyboardEvent('keydown', {key: k, bubbles: true, cancelable: true})))); await settle(); }
  window.bench = {nodes: document.querySelectorAll('.gsv .n').length, edges: document.querySelectorAll('.gsv path.e').length, canvas: [Math.round(r.width), Math.round(r.height)],
    pan: stats(pan), panHandler, zoom: stats(zoom), zoomHandler, selection: {sync: stats(picks.map(p => p.sync)), frame: stats(picks.map(p => p.frame))},
    filter: {sync: stats(filters.map(p => p.sync)), frame: stats(filters.map(p => p.frame))}, settleZoom: {sync: stats(keyZoom.map(p => p.sync)), frame: stats(keyZoom.map(p => p.frame))}};
})();
return 1;
