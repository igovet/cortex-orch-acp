const supports = c => CSS.supports(c), probe = document.createElement("div");
probe.style.cssText = "position:absolute;visibility:hidden;width:60px;height:60px;overflow:scroll";
document.body.append(probe);
const scrollbar = probe.offsetWidth - probe.clientWidth;
probe.remove();
await document.fonts.load('14px "Noto Sans Variable"', "Привет");
await document.fonts.load('12.5px "Cousine"', "Привет");
const measure = family => { const s = document.createElement("span"); s.textContent = "Привет, проект"; s.style.cssText = `position:absolute;visibility:hidden;font:14px ${family}`; document.body.append(s); const w = s.getBoundingClientRect().width; s.remove(); return +w.toFixed(2); };
return JSON.stringify({
  popover: typeof HTMLElement.prototype.showPopover === "function", dialog: typeof HTMLDialogElement.prototype.showModal === "function",
  colorMix: supports("color: color-mix(in srgb, red 50%, transparent)"), has: supports("selector(:has(a))"), focusVisible: supports("selector(:focus-visible)"), container: supports("container-type: inline-size"),
  scrollbarGutter: supports("scrollbar-gutter: stable"), mask: supports("mask: none") || supports("-webkit-mask: none"), positionArea: supports("position-area: bottom"),
  scrollbar, cyrillicDesign: measure('"Noto Sans Variable"'), cyrillicSans: measure("sans-serif"),
  pageMenu: !document.body.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true })),
  linkDrag: getComputedStyle(document.querySelector(".sidebar a")).webkitUserDrag,
  faces: [...document.fonts].filter(f => f.status === "loaded").length,
});
