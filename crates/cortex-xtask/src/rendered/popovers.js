const pause = () => new Promise(r => setTimeout(r, 60));
const css = el => getComputedStyle(el), out = [];
const triggers = [...document.querySelectorAll("button[popovertarget]")].filter(el => el.getClientRects().length);
const seen = new Set();
for (const trigger of triggers) {
  const id = trigger.getAttribute("popovertarget"), pop = document.getElementById(id);
  if (seen.has(id) || !pop) continue;
  seen.add(id);
  trigger.click();
  await pause();
  const r = pop.getBoundingClientRect(), s = css(pop);
  out.push({ id, supported: typeof pop.showPopover === "function", open: pop.matches(":popover-open"), display: s.display, background: s.backgroundColor,
    rect: [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)], inside: r.left >= 0 && r.top >= 0 && r.right <= innerWidth + .5 && r.bottom <= innerHeight + .5, items: pop.querySelectorAll(".mi, button, a, dt").length });
  pop.hidePopover?.();
  await pause();
}
return JSON.stringify(out);
