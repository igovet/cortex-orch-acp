// Climbs a feed that reads its earlier part as the reader scrolls up (scroll-perf.driver.mjs `earlier`), and records what the
// reader sees. MODE `steady` moves the feed up SPEED px every frame, so between two painted frames a row must move down by
// exactly what was scrolled: anything else is a jump. MODE `wheel` leaves the scrolling to the real wheel (xdotool) and only
// watches. Each frame is sampled after it painted (a task posted from the frame's callback runs after its rendering), so a
// correction made before paint is not counted as a jump and one made a frame late is.
window.__climb = null;
(() => {
  const box = document.querySelector('SELECTOR'), inner = box.firstElementChild, mode = 'MODE', speed = SPEED, limit = LIMIT_MS;
  const channel = new MessageChannel();
  const rows = () => [...inner.children].filter(n => !n.hasAttribute('data-follow-skip'));
  const frames = [], slowAt = [], times = [], jumpsAt = [], samples = [], prepends = [], recreated = [];
  let pendingExpected = 0, anchor = null, anchorY = 0, last = 0, start = performance.now(), expected = 0, topWait = 0, topSince = null, stopped = false, lastTop = box.scrollTop;
  let height = box.scrollHeight, firstRow = rows()[0], known = new WeakSet(rows());
  const more = () => !!box.querySelector('.ws-earlier');
  const pick = () => {
    const top = box.getBoundingClientRect().top;
    for (const n of rows()) { const r = n.getBoundingClientRect(); if (r.height > 0 && r.top >= top) return n; }
    return null;
  };
  channel.port1.onmessage = () => {
    // After paint: where the anchor row is now, against where it was plus what this frame scrolled.
    const top = box.getBoundingClientRect().top, now = performance.now();
    const grown = box.scrollHeight - height, first = rows()[0];
    if (anchor && !anchor.isConnected) { recreated.push(Math.round(now - start)); anchor = null; }
    let jump = 0;
    // What every frame since the last sample scrolled: a sample can come after the next frame's callback already ran.
    if (anchor) { const y = anchor.getBoundingClientRect().top - top; jump = y - anchorY - pendingExpected; anchorY = y; }
    pendingExpected = 0;
    if (first !== firstRow || grown > 0) {
      // Rows were put above (the reserve may have given them its room, so the feed need not grow): those not seen before are
      // the new page.
      const fresh = rows().filter(n => !known.has(n)).length;
      if (first !== firstRow && fresh > 0) prepends.push({i: samples.length, t: Math.round(now - start), added: grown, rows: fresh, scrollTopBefore: Math.round(lastTop), scrollTopAfter: Math.round(box.scrollTop), jump: Math.round(jump * 10) / 10});
      for (const n of rows()) known.add(n);
      firstRow = first;
    }
    height = box.scrollHeight; lastTop = box.scrollTop;
    samples.push(Math.round(jump * 10) / 10); times.push(now);
    if (Math.abs(jump) > 1 && jumpsAt.length < 30) jumpsAt.push([Math.round(now - start), Math.round(jump), Math.round(box.scrollTop), grown]);
    if (!anchor || anchorY < 0 || anchorY > box.clientHeight - 40) { anchor = pick(); if (anchor) anchorY = anchor.getBoundingClientRect().top - top; }
    if (box.scrollTop < 1 && more()) { topSince ??= now; } else if (topSince !== null) { topWait += now - topSince; topSince = null; }
  };
  const frame = t => {
    if (stopped) return;
    if (last) { frames.push(t - last); if (t - last > 32) slowAt.push([Math.round(t - start), Math.round(t - last)]); }
    last = t;
    const before = box.scrollTop;
    if (mode === 'steady') box.scrollTop = Math.max(0, before - speed);
    // What the content should move down by in this frame: what the reader scrolled (none at the top).
    expected = mode === 'steady' ? before - box.scrollTop : 0;
    pendingExpected += expected;
    channel.port2.postMessage(0);
    const done = performance.now() - start > limit || (!more() && box.scrollTop < 1);
    if (done) { stopped = true; finish(); return; }
    requestAnimationFrame(frame);
  };
  const finish = () => {
    // What one small change at the feed's top costs to lay out again, with every row read so far drawn below it.
    const edge = box.querySelector('.ws-earlier') ?? inner.firstElementChild, relayout = [];
    for (let k = 0; k < 6; k++) { edge.style.paddingTop = k % 2 ? '' : '1px'; const s0 = performance.now(); void inner.offsetHeight; relayout.push(performance.now() - s0); }
    edge.style.paddingTop = '';
    if (topSince !== null) topWait += performance.now() - topSince;
    const sorted = [...frames].sort((a, b) => a - b), q = p => sorted.length ? Math.round(sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] * 10) / 10 : 0;
    // Under the wheel a frame's own motion is unknown: a prepend frame is compared with the frames around it.
    const median = xs => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[s.length >> 1] : 0; };
    // Under the wheel each frame moves by what the wheel's animation gives it, which varies from frame to frame: a frame
    // with a page put above is a jump by what it moved beyond the most any of the three frames before or after it moved, and
    // a stall when it stood still while they moved.
    for (const p of prepends) if (mode !== 'steady') {
      const around = [...[1, 2, 3].map(d => p.i - d), ...[1, 2, 3].map(d => p.i + d)].filter(k => k >= 0 && k < samples.length).map(k => Math.abs(samples[k]));
      const most = Math.max(0, ...around), dy = Math.abs(samples[p.i] ?? 0);
      p.jump = Math.round(Math.max(0, dy - most) * 10) / 10;
      p.stall = median(around) > 8 && dy < .25 * median(around);
    }
    const jumps = mode === 'steady' ? samples.map(Math.abs) : prepends.map(p => Math.abs(p.jump));
    window.__climb = {mode, ms: Math.round(performance.now() - start), frames: frames.length, p50: q(.5), p95: q(.95), max: Math.round(Math.max(0, ...frames)),
      over32: frames.filter(f => f > 32).length, over50: frames.filter(f => f > 50).length, slow: frames.filter(f => f > 32).map(Math.round).slice(0, 20),
      maxJump: Math.max(0, ...jumps), stalls: prepends.filter(p => p.stall).length, jumpsOver1: jumps.filter(j => j > 1).length, prepends, jumpsAt, slowAt: slowAt.slice(0, 30), recreated: recreated.length, topWaitMs: Math.round(topWait),
      reachedStart: !more(), rows: rows().length, relayoutMs: Math.round([...relayout].sort((a, b) => a - b)[3] * 10) / 10, scrollTop: Math.round(box.scrollTop), scrollHeight: box.scrollHeight};
  };
  requestAnimationFrame(frame);
})();
return 1;
