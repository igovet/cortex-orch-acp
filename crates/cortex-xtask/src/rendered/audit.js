const vw = innerWidth, vh = innerHeight, theme = document.documentElement.dataset.theme;
const css = el => getComputedStyle(el);
const box = el => { const r = el.getBoundingClientRect(); return { x: +r.x.toFixed(1), y: +r.y.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1) }; };
const name = el => `${el.tagName.toLowerCase()}${el.id ? "#" + el.id : ""}${typeof el.className === "string" && el.className ? "." + el.className.trim().split(/\s+/).slice(0, 3).join(".") : ""}`;
const token = (t, property = "background-color") => { const p = document.createElement("i"); p.style.setProperty(property, `var(${t})`); document.body.append(p); const v = css(p).getPropertyValue(property); p.remove(); return v; };
const parse = v => {
  let m = /rgba?\(([\d.]+)[, ]+([\d.]+)[, ]+([\d.]+)(?:[,/ ]+([\d.]+))?/.exec(v);
  if (m) return [+m[1], +m[2], +m[3], m[4] === undefined ? 1 : +m[4]];
  m = /color\(srgb ([\d.]+) ([\d.]+) ([\d.]+)(?: \/ ([\d.]+))?/.exec(v);
  return m ? [m[1] * 255, m[2] * 255, m[3] * 255, m[4] === undefined ? 1 : +m[4]] : [0, 0, 0, 0];
};
const lum = ([r, g, b]) => (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
const seen = el => { const s = css(el), r = el.getBoundingClientRect(); return el.getClientRects().length > 0 && s.visibility !== "hidden" && s.display !== "none" && r.width > 0 && r.height > 0 && r.right > 0 && r.bottom > 0 && r.left < vw && r.top < vh; };
const surface = token("--bg-canvas");
const root = document.getElementById("root"), bar = document.querySelector(".titlebar");
const page = {
  theme, surface, viewport: [vw, vh], html: css(document.documentElement).backgroundColor, body: css(document.body).backgroundColor, root: css(root).backgroundColor,
  rootBox: box(root), htmlBox: box(document.documentElement), bodyBox: box(document.body), bodyMargin: css(document.body).margin,
  scroll: [document.documentElement.scrollWidth, document.documentElement.scrollHeight],
};
const barBox = bar ? box(bar) : null, inBar = bar ? [...bar.querySelectorAll("a, button")].filter(seen) : [];
const outside = inBar.filter(el => { const r = el.getBoundingClientRect(), b = bar.getBoundingClientRect(); return r.left < b.left - .5 || r.right > b.right + .5 || r.top < b.top - .5 || r.bottom > b.bottom + .5; }).map(name);
const overlaps = [];
inBar.forEach((a, i) => inBar.slice(i + 1).forEach(b => { if (a.contains(b) || b.contains(a)) return; const p = a.getBoundingClientRect(), q = b.getBoundingClientRect(); if (Math.min(p.right, q.right) - Math.max(p.left, q.left) > 1 && Math.min(p.bottom, q.bottom) - Math.max(p.top, q.top) > 1) overlaps.push(name(a) + "+" + name(b)); }));
const split = document.querySelector(".titlebar .split");
const win = [...document.querySelectorAll(".win")];
const chrome = {
  barBox, outside, overlaps, controls: inBar.length,
  winGroups: win.length, winButtons: win.flatMap(w => [...w.querySelectorAll("button")]).length, winHidden: win.some(w => w.getAttribute("aria-hidden") === "true"),
  split: split ? { box: box(split), display: css(split).display, parts: [...split.children].map(box) } : null,
};
const texts = new Map(), faces = {};
const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
const dashes = [];
for (let n; (n = walker.nextNode());) {
  const t = n.textContent.trim(), p = n.parentElement;
  if (!t || !p || !seen(p) || ["SCRIPT", "STYLE"].includes(p.tagName)) continue;
  const first = css(p).fontFamily.split(",")[0].replace(/["']/g, "").trim();
  faces[first] = (faces[first] ?? 0) + 1;
  if (/^[-‒–—―]{1,3}$/.test(t)) dashes.push(name(p));
}
const light = [], native = [], shadows = [], themed = [], clipped = [];
for (const el of document.querySelectorAll("body, body *")) {
  if (!seen(el)) continue;
  const s = css(el), c = parse(s.backgroundColor), r = el.getBoundingClientRect();
  if (theme === "dark" && c[3] > .5 && lum(c) > .55 && !el.classList.contains("i") && !el.classList.contains("tick") && !el.closest(".btn-primary") && !el.classList.contains("mc-send") && !el.closest(".bar")) light.push(`${name(el)} ${s.backgroundColor} ${Math.round(r.width)}x${Math.round(r.height)}`);
  if (el.matches("input, textarea, select, button") && !(el.type === "checkbox" || el.type === "radio")) {
    if (["outset", "inset"].includes(s.borderTopStyle) || (el.matches("select") && s.appearance !== "none") || s.appearance === "button" || s.appearance === "push-button") native.push(`${name(el)} ${s.appearance} ${s.borderTopStyle}`);
  }
  for (const m of s.boxShadow.matchAll(/(?:rgba?\([^)]*\)|color\([^)]*\))\s+(-?[\d.]+)px\s+(-?[\d.]+)px\s+([\d.]+)px/g)) { const blur = +m[3]; if (blur > 0) shadows.push(blur); }
  if (el.matches("a, button, [role=tab], [role=menuitem]") && /theme/i.test(`${el.title} ${el.getAttribute("aria-label") ?? ""} ${el.textContent}`)) themed.push(name(el));
  if (el.matches(".btn, .nav, .chip, .snav") && el.scrollWidth > el.clientWidth + 1 && !el.querySelector(".trunc") && !el.classList.contains("trunc")) clipped.push(name(el));
}
const loaded = [...document.fonts].filter(f => f.status === "loaded").map(f => `${f.family.replace(/["']/g, "")} ${f.weight}`);
const ruler = (family, weight = 400) => { const s = document.createElement("span"); s.textContent = "Cortex Workspace Rg 0123456789"; s.style.cssText = `position:absolute;visibility:hidden;font:${weight} 14px ${family}`; document.body.append(s); const w = s.getBoundingClientRect().width; s.remove(); return +w.toFixed(2); };
const fonts = {
  stack: css(document.body).fontFamily.split(",")[0].replace(/["']/g, ""), faces, loaded: [...new Set(loaded)], check: document.fonts.check('14px "Noto Sans Variable"'),
  widths: { design: ruler('"Noto Sans Variable"'), sans: ruler("sans-serif"), medium: ruler('"Noto Sans Variable"', 500) },
};
const icons = [...document.querySelectorAll(".i")].filter(seen);
const unmasked = icons.filter(el => { const s = css(el); return (s.maskImage ?? s.webkitMaskImage ?? "none") === "none" && (s.webkitMaskImage ?? "none") === "none"; });
const plainIcons = unmasked.length;
// Which icon, what the engine computed for it, and whether it paints a solid block: --m is empty when no .i-<name> rule exists.
const plainIconDetail = unmasked.slice(0, 8).map(el => { const s = css(el), b = box(el); return { icon: el.className, mask: s.maskImage ?? null, webkitMask: s.webkitMaskImage ?? null, m: s.getPropertyValue("--m").slice(0, 40), background: s.backgroundColor, box: [b.w, b.h], row: (el.parentElement?.textContent ?? "").trim().slice(0, 40) }; });
const known = new Set();
const rules = list => { for (const rule of list) { if (rule.selectorText) for (const m of rule.selectorText.matchAll(/\.([\w-]+)/g)) known.add(m[1]); if (rule.cssRules) rules(rule.cssRules); } };
for (const sheet of document.styleSheets) rules(sheet.cssRules);
const unknown = new Set();
for (const el of document.querySelectorAll("body *")) if (seen(el) && typeof el.className === "string") for (const c of el.classList) if (!known.has(c)) unknown.add(c);
const ui = {
  dialogs: [...document.querySelectorAll("dialog[open]")].map(name),
  navs: [...document.querySelectorAll(".sidebar .nav")].filter(seen).map(el => { const b = box(el); return [el.textContent.trim().slice(0, 14), b.x, b.y, b.w, b.h, css(el).fontWeight]; }),
  crumb: document.querySelector(".crumbs")?.textContent ?? "",
  main: document.querySelector(".main") ? box(document.querySelector(".main")) : null,
  mainText: (document.querySelector(".main")?.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 90),
  strips: [...document.querySelectorAll(".main .strip")].filter(seen).map(el => [name(el), box(el).y, box(el).h]),
  icons: icons.length, plainIcons, plainIconDetail, userSelect: css(document.body).webkitUserSelect,
};
return JSON.stringify({ page, chrome, fonts, ui, unknown: [...unknown].slice(0, 30), light: light.slice(0, 12), native: native.slice(0, 12), maxShadow: Math.max(0, ...shadows), themed: themed.slice(0, 8), dashes: dashes.slice(0, 12), clipped: clipped.slice(0, 12) });
