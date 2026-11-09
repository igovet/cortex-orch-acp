// A classic script, so it runs before the first paint: a deferred module could paint the wrong surface first. System is the default.
try {
  var pref = localStorage.getItem("cw.theme");
  var dark = pref === "dark" || ((!pref || pref === "system") && matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  document.documentElement.style.colorScheme = dark ? "dark" : "light";
  var size = JSON.parse(localStorage.getItem("cw.textSize") || "100");
  if (typeof size === "number" && [100, 125, 150, 200].indexOf(size) >= 0) document.documentElement.style.fontSize = (16 * size / 100) + "px";
} catch (e) { document.documentElement.dataset.theme = "dark"; }
