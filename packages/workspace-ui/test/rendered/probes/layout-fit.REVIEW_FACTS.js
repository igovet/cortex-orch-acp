const panel = document.querySelector('[aria-label="Changes"]'), foot = document.querySelector('[aria-label="Review comments"]');
const body = document.querySelector('.rv-panel .chg-body'), acts = [...document.querySelectorAll('.rv-bar .l-bar-actions > .btn')].map(b => { const r = b.getBoundingClientRect(); return [b.textContent.trim(), r.top, r.height]; });
const textTop = el => { const g = document.createRange(); g.selectNodeContents(el); return g.getBoundingClientRect().top; };
const gap = (above, head) => above && head ? Math.round((textTop(head) - above.getBoundingClientRect().bottom) * 100) / 100 : null;
return {foot: !!foot, footTop: foot ? foot.getBoundingClientRect().top : null, body: body ? [body.getBoundingClientRect().bottom, body.getBoundingClientRect().height] : null,
  notePanels: panel ? panel.querySelectorAll('.note-panel').length : null, applied: panel ? (panel.textContent.match(/Changes were applied in the project folder/g) || []).length : null, acts,
  cardGap: gap(document.querySelector('.rcard .rv-outc'), document.querySelector('.rcard .rv-checks .l-sect-t')),
  evidenceGap: gap(document.querySelector('.evid > summary'), document.querySelector('.evid .rv-checks .l-sect-t')),
  rawHeadings: [...document.querySelectorAll('.rcard h3, .rv-panel h3, dialog[open] h3')].filter(h => !h.matches('.l-sect-t, .ws-result-copy h3')).length};
