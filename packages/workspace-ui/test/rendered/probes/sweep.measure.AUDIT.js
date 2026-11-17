window.__audit = (root, page) => {
  const out = [], seen = new Set();
  const sign = el => { const c = typeof el.className === 'string' ? el.className.trim().split(/\s+/).slice(0, 3).join('.') : ''; return el.tagName.toLowerCase() + (c ? '.' + c : ''); };
  const add = (rule, el, detail) => { const key = rule + '|' + sign(el) + '|' + String(detail).slice(0, 60); if (!seen.has(key)) { seen.add(key); out.push({ rule, at: sign(el), detail: String(detail).slice(0, 160) }); } };
  const shown = el => { const r = el.getBoundingClientRect(), s = getComputedStyle(el); return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const ID = /\b(?:xr|cn|xa|es|pq|ch|ar|ara|cr|cg|lo|ws)_[0-9a-f]{6,}\b|\b[trd]_[0-9a-f]{8,}\b|\b[0-9a-f]{20,}\b|sha256|mock-(?:machine|project|task)/;
  const CORTEX = /\bCortex\b/;
  const FILLER = /^(?:—|-|–|n\/a|N\/A|null|undefined|NaN|All 0|0\/0)$/;
  const diagnostics = /\/diagnostics$/.test(page);
  const say = (el, text) => { if (!diagnostics && ID.test(text)) add('technical id', el, text); if (CORTEX.test(text) && !el.closest('.ws-user')) add('says Cortex', el, text); };

  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const el = n.parentElement, text = n.nodeValue.trim();
    if (!text || !el || el.closest('script, style, .ws-demo-banner, .ws-demo-menu, .xterm, .cm-editor, .ws-live')) continue;
    if (el.closest('[popover]:not(:popover-open)') || el.closest('.ws-hint-pop')) continue;
    if (!shown(el)) continue;
    say(el, text);
    if (FILLER.test(text) && !el.closest('.dl, .sl, .sbs, .cl, .code')) add('empty filler', el, text);
    if (/[Ѐ-ӿ]/.test(text) && el.closest('.set-main, .set-side, .ws-hint-pop')) add('not English', el, text);
    // One piece that breaks across lines: a path, a name, an id-like value.
    if (!/\s/.test(text) && text.length >= 8 && !el.closest('pre, code, .code, .sbs, .cl, .dl, .args, .ai, .ws-pre, .mine, .tl-in .md, .md, .feed-in, .stream-in')) {
      const range = document.createRange(); range.selectNodeContents(n);
      const tops = new Set([...range.getClientRects()].map(r => Math.round(r.top)));
      if (tops.size > 1) add('one piece wraps', el, text);
    }
    if (el.closest('.main, .inspector, dialog, .set-main, [popover]') && !el.closest('button, a, label, summary, [role=button], [role=tab], [role=option], [role=menuitem], [role=treeitem], [draggable=true], .titlebar, .sidebar, .col-h, .ws-resize, .kbd, .cl > .n, .dl > .n, .sl > .n')) {
      const s = getComputedStyle(el);
      if ((s.webkitUserSelect ?? s.userSelect) === 'none' && (s.userSelect ?? 'none') === 'none') add('text cannot be selected', el, text);
    }
  }
  for (const el of root.querySelectorAll('[title], [aria-label], [placeholder]')) {
    if (el.closest('.ws-demo-banner, .ws-demo-menu')) continue;
    for (const a of ['title', 'aria-label', 'placeholder']) { const v = el.getAttribute(a); if (v) say(el, v); }
  }
  for (const el of root.querySelectorAll('input:not([type=checkbox]):not([type=radio]):not([type=file])')) if (shown(el) && el.value && !diagnostics && ID.test(el.value)) add('technical id', el, el.value);
  for (const el of root.querySelectorAll('.ws-hint-pop')) { if (/[Ѐ-ӿ]/.test(el.textContent)) add('not English', el, el.textContent.slice(0, 80)); if (CORTEX.test(el.textContent)) add('says Cortex', el, el.textContent.match(/.{0,50}\bCortex\b.{0,30}/)?.[0]); }

  for (const el of root.querySelectorAll('.count, .n, .sig, .badge, .task-row.all')) if (shown(el) && /^(?:All\s+)?0(?:\/0)?$/.test(el.textContent.trim())) add('zero counter', el, el.textContent.trim());

  for (const el of root.querySelectorAll('input[type=checkbox], input[type=radio]')) {
    if (el.closest('.sw') || !shown(el)) continue;
    const r = el.getBoundingClientRect();
    if (Math.abs(r.width - r.height) > .5 || r.width > 18) add('box is not square', el, `${r.width}x${r.height}`);
  }

  // Left and right gaps inside the nearest thing with an edge. A row flush on both sides is fine, one flush side never is.
  const framed = p => { const s = getComputedStyle(p); return parseFloat(s.borderLeftWidth) > 0 || parseFloat(s.borderRightWidth) > 0 || s.backgroundColor !== 'rgba(0, 0, 0, 0)' || /auto|scroll/.test(s.overflowY); };
  const ROWS = '.nav, .snav, .proj-wrap, .task-row, .mi, .lrow, .card, .opt, .ws-pal-o, .pt-row, .surface, .strip, .note-box, .composer, .dc, .sb-usage, .tree-row, .frow, .wrow, .gf, .rc, .ws-add, .col-add, .snav-h, .sb-h';
  for (const el of root.querySelectorAll(ROWS)) {
    if (!shown(el) || el.closest('.v-row[style*="display: none"]')) continue;
    let p = el.parentElement; while (p && p !== document.body && !framed(p)) p = p.parentElement;
    if (!p || p === document.body) continue;
    const r = el.getBoundingClientRect(), b = p.getBoundingClientRect(), s = getComputedStyle(p);
    const left = r.left - b.left - parseFloat(s.borderLeftWidth), right = b.right - parseFloat(s.borderRightWidth) - r.right, inner = b.width - parseFloat(s.borderLeftWidth) - parseFloat(s.borderRightWidth);
    if (r.width < inner * .5 || left > 48 || right > 48 || left < -1 || right < -1) continue;
    if (Math.abs(left - right) > 1) add('uneven gaps', el, `left ${left.toFixed(1)} right ${right.toFixed(1)} in ${sign(p)}`);
  }
  for (const el of root.querySelectorAll('.gr, .srow, .mi, .nav, .snav:not(.sub), .opt, .card-t, .strip')) {
    if (!shown(el)) continue;
    const s = getComputedStyle(el);
    if (Math.abs(parseFloat(s.paddingLeft) - parseFloat(s.paddingRight)) > 1) add('uneven padding', el, `${s.paddingLeft} ${s.paddingRight}`);
  }

  // A label is one line, and whatever is cut has a way to be read whole.
  for (const el of root.querySelectorAll('.btn, .chip, .tab, .st, .seg button, .pick, .sig, .kbd, .gr.th > span, .set-h')) {
    if (!shown(el) || !el.textContent.trim()) continue;
    const range = document.createRange(); range.selectNodeContents(el);
    const tops = new Set([...range.getClientRects()].filter(r => r.width > 1 && r.height > 6).map(r => Math.round(r.top + r.height / 2)));
    if (tops.size > 1 && el.getBoundingClientRect().height > 40) add('label wraps', el, el.textContent.trim());
  }
  for (const el of root.querySelectorAll('*')) {
    if (el.children.length > 3 || !shown(el) || el.closest('.xterm, .cm-editor')) continue;
    const s = getComputedStyle(el);
    if (s.textOverflow !== 'ellipsis' || s.overflowX === 'visible' || el.scrollWidth <= el.clientWidth + 1) continue;
    el.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
    const whole = el.textContent.trim(), help = el.closest('[title]')?.getAttribute('title') ?? '';
    if (!help.includes(whole)) add('cut with no way to read it', el, whole);
    if (el.closest('.orrow, .okrow')) add('routing table cuts a value', el, whole);
  }
  return out;
};
