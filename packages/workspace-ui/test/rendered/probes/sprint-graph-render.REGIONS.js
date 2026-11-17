const pick = {sidebar: '.sidebar', conversation: '.sp-conv', plan: '.sp-plan', details: '.ws-read', toolbar: '.gv-bar', canvas: '.gv-canvas', band: '.gv-band', head: '.sp-plan-h', tabs: '.tabs', strip: '.l-lane'};
const out = {}, boxes = [];
for (const [k, s] of Object.entries(pick)) { const el = document.querySelector(s); if (!el || !el.getClientRects().length) continue; const r = el.getBoundingClientRect(); if (r.width < 1 || r.height < 1) continue; out[k] = [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)]; boxes.push([k, el, r]); }
const overlaps = [];
for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) { const [a, ea, ra] = boxes[i], [b, eb, rb] = boxes[j]; if (ea.contains(eb) || eb.contains(ea)) continue;
  const w = Math.min(ra.right, rb.right) - Math.max(ra.left, rb.left), h = Math.min(ra.bottom, rb.bottom) - Math.max(ra.top, rb.top); if (w > 1 && h > 1) overlaps.push(`${a} over ${b} (${Math.round(w)}x${Math.round(h)})`); }
const z = document.querySelector('.gv-zoom')?.textContent, lod = document.querySelector('.gsv')?.dataset.lod;
const title = document.querySelector('.gsv .n .l1, .gsv .n .ts'); const fs = title && getComputedStyle(title).display !== 'none' ? title.getBoundingClientRect().height : null;
return {regions: out, overlaps, zoom: z, lod, sidebar: document.querySelector('.ws-app').dataset.sidebar, full: !!document.querySelector('.sp-body[data-full]'), map: !document.querySelector('.gv-map')?.hidden};
