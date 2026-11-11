// One rule for every boxed label, shared by the layout gate (layout-fit.py) and the page captures (pages-shots.py), which receive
// it from their .mjs launcher. A label is the text of a control, tab, chip, badge, status word or table head that stands on one
// line in its own box. It must not wrap, and the middle of its capitals must sit in the middle of its box, within 1 px: the line
// box alone is not enough, since a face whose ascent exceeds its descent draws its letters above the middle of that box.
// Called as (scope) => {problems, labels}; `labels` keeps the worst offsets per kind for the evidence files.
(scope) => {
  const SINGLE = '.btn, .ib, .sel, .seg > *, .scope-btn, .ptab, .tab, .chip, .pick, .st, .c-send, [role=tab], [role=radio]:not(:has(p, small, .d)), th, .kbd, .key';
  const PRESSABLE = 'button, a[href], [role=button], [role=menuitem], [role=menuitemradio], [role=option], [role=switch], label, summary';
  // Not labels: code and fields, recorded output, the graph's drawing, and notices whose text wraps by design (a strip's
  // sentence, a reason beside its controls), which their own rules hold to two lines and a way to read the rest.
  // A row's description, and the Inspector's fact (the task's own words), reflow in their column.
  // A value in a list of terms stands on its term's line (.kv), not in a box; so do a text link and a cut piece of text that
  // a press reveals whole ([data-whole]), whose box is only their own words.
  const SKIP = 'svg, pre, code, textarea, input, .cm-editor, .ws-out, .ws-live, [data-wrap], .gsv, .l-strip-t, .l-reason, .l-row-d, .inspector .st, .kv > dd, a.lnk, [data-whole]:not(.btn, .chip, .pick)';
  const css = el => getComputedStyle(el);
  const seen = el => { if (el.closest('[hidden], details:not([open]) > :not(summary)')) return false; const s = css(el); if (s.display === 'none' || s.visibility === 'hidden' || +s.opacity === 0) return false; const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  const transparent = c => c === 'rgba(0, 0, 0, 0)' || c === 'transparent';
  // A box half scrolled out of its scroller is measured when it is whole again.
  const whole = el => { const r = el.getBoundingClientRect(); for (let a = el.parentElement; a && a !== document.documentElement; a = a.parentElement) { const s = css(a); if (s.overflowX === 'visible' && s.overflowY === 'visible') continue; const c = a.getBoundingClientRect(); if (r.top < c.top - .5 || r.bottom > c.bottom + .5 || r.left < c.left - .5 || r.right > c.right + .5) return false; } return r.top >= -.5 && r.bottom <= innerHeight + .5; };
  // A box that draws itself (a fill or a border) and is no taller than a control: a chip, badge or count, whatever its class.
  const drawn = el => { const s = css(el), r = el.getBoundingClientRect(); return r.height <= 32 && s.display !== 'inline' && (!transparent(s.backgroundColor) || parseFloat(s.borderTopWidth) > 0) && [...el.childNodes].some(n => n.nodeType === 3 && n.nodeValue.trim()); };
  const name = el => { const c = typeof el.className === 'string' ? el.className.trim().split(/\s+/).slice(0, 2).join('.') : ''; return el.tagName.toLowerCase() + (c ? '.' + c : ''); };
  const say = el => (el.getAttribute('aria-label') || el.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 40);
  const ctx = document.createElement('canvas').getContext('2d');
  const metric = {};
  const metrics = s => { const key = `${s.fontStyle} ${s.fontWeight} ${s.fontSize} ${s.fontFamily}`; if (!metric[key]) { ctx.font = key; const m = ctx.measureText('H'); metric[key] = {a: m.fontBoundingBoxAscent, d: m.fontBoundingBoxDescent, cap: m.actualBoundingBoxAscent}; } return metric[key]; };
  const boxes = new Set();
  for (const el of scope.querySelectorAll(SINGLE + ', ' + PRESSABLE)) boxes.add(el);
  for (const el of scope.querySelectorAll('span, b, em, i, small, div, li, td')) if (drawn(el)) boxes.add(el);
  const problems = [], worst = {}, entries = [];
  let measured = 0;
  for (const el of boxes) {
    if (el.closest(SKIP) || !seen(el)) continue;
    const r = el.getBoundingClientRect();
    if (r.height > 48 || !whole(el)) continue;
    // The label is the box's own text: text inside a nested box is that box's label.
    const texts = [], walk = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n = walk.nextNode(); n; n = walk.nextNode()) { const p = n.parentElement; if (!n.nodeValue.trim() || !p || !seen(p) || p.closest(SKIP)) continue; let own = true; for (let a = p; a && a !== el; a = a.parentElement) if (boxes.has(a)) { own = false; break; } if (own) texts.push(n); }
    if (!texts.length) continue;
    const lines = texts.map(n => { const g = document.createRange(); g.selectNodeContents(n); return {n, rects: [...g.getClientRects()].filter(x => x.width > 1 && x.height > 1)}; }).filter(t => t.rects.length);
    if (!lines.length) continue;
    const single = el.matches(SINGLE);
    // A link inside a sentence wraps with the sentence; it has no box of its own.
    const s = css(el);
    const wrapped = lines.filter(t => new Set(t.rects.map(x => Math.round(x.top))).size > 1);
    if (wrapped.length && s.display !== 'inline' && (single || r.height <= 40)) { problems.push(`wrapped label: ${name(el)} "${say(el)}"`); continue; }
    // Several lines of text (a title over a reason) are not one label; their box is laid out by its own rules.
    if (new Set(lines.map(t => Math.round(t.rects[0].top))).size > 1) continue;
    const bt = parseFloat(s.borderTopWidth), bb = parseFloat(s.borderBottomWidth);
    const mid = r.top + bt + (r.height - bt - bb) / 2;
    // Text in an inline or inline-block box (a link or a path in a sentence) sits on its line by the line's rules, not in a
    // box of its own: nothing to centre it in.
    if (s.display === 'inline' || s.display === 'inline-block') continue;
    measured++;
    for (const {n, rects} of lines) {
      const x = rects[0], m = metrics(css(n.parentElement));
      // The engine lays text out on whole-pixel ascents and descents; take them when the run's box shows it did.
      const a = Math.abs(x.height - (Math.round(m.a) + Math.round(m.d))) < .5 ? Math.round(m.a) : m.a;
      const base = x.top + a, ink = base - m.cap / 2, line = x.top + x.height / 2;
      const off = {ink: Math.round((ink - mid) * 100) / 100, line: Math.round((line - mid) * 100) / 100};
      const kind = name(el).split('.').slice(0, 2).join('.');
      // Where the letters are drawn, for the pixel check: the run's own box and the box it must sit in the middle of.
      // An underline is ink below the baseline, so an underlined label is left to the metric rule above.
      if (entries.length < 600 && !css(n.parentElement).textDecorationLine.includes('underline')) entries.push({kind, label: say(el), text: n.nodeValue.trim(), run: [x.left, x.top, Math.min(x.right, r.right), x.bottom], box: [r.left, r.top, r.right, r.bottom], mid, ink: off.ink});
      if (!worst[kind] || Math.abs(off.ink) > Math.abs(worst[kind].ink)) worst[kind] = {...off, label: say(el), h: Math.round(r.height * 100) / 100, font: css(n.parentElement).fontSize + '/' + css(n.parentElement).lineHeight};
      if (Math.abs(off.ink) > 1) problems.push(`label off centre: ${name(el)} "${say(el)}" letters ${off.ink > 0 ? 'low' : 'high'} by ${Math.abs(off.ink)}px`);
      // The mono face is drawn lower on purpose so that its capitals are centred (mockup-styles.css), so only the interface
      // face is also held to a centred line box.
      else if (Math.abs(off.line) > 1 && !/Cousine/.test(css(n.parentElement).fontFamily)) problems.push(`label off centre: ${name(el)} "${say(el)}" line box ${off.line > 0 ? 'low' : 'high'} by ${Math.abs(off.line)}px`);
      break;
    }
  }
  return {problems: [...new Set(problems)], labels: {measured, worst}, entries};
}
