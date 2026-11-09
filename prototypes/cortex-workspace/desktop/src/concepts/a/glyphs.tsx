import { For } from "solid-js";

// Candidate glyphs (icon family is an open owner decision). Always decorative and
// paired with visible text; colour is never the only signal.
const C = "M8 1.75a6.25 6.25 0 1 1 0 12.5a6.25 6.25 0 1 1 0-12.5z";
type P = { d: string; fill?: boolean };
const G = {
  none: [] as P[],
  check: [{ d: C }, { d: "M5 8.3l2 2 4-4.4" }],
  x: [{ d: C }, { d: "M5.7 5.7l4.6 4.6M10.3 5.7l-4.6 4.6" }],
  dash: [{ d: C }, { d: "M5.5 8h5" }],
  question: [{ d: C }, { d: "M6.3 6.4a1.8 1.8 0 1 1 2.6 1.6c-.6.3-.9.7-.9 1.3v.3M8 11.4v.1" }],
  half: [{ d: C }, { d: "M8 1.75v12.5a6.25 6.25 0 0 0 0-12.5z", fill: true }],
  pause: [{ d: "M5.8 4v8M10.2 4v8" }],
  minus: [{ d: "M4 8h8" }],
  slash: [{ d: C }, { d: "M3.6 12.4l8.8-8.8" }],
  key: [{ d: "M10.5 2.2a3.3 3.3 0 1 1-3 4.8L2.5 12v1.8h2v-1.3h1.3v-1.3h1.3l1.6-1.6" }, { d: "M11.2 4.8v.1" }],
  message: [{ d: "M2.2 3h11.6v7.6H7.6L4.6 13v-2.4H2.2z" }, { d: "M6.9 5.4a1.1 1.1 0 1 1 1.5 1c-.3.2-.4.4-.4.7M8 8.6v.1" }],
  filecheck: [{ d: "M4 1.8h5l3 3v9.4H4z" }, { d: "M6 9.6l1.5 1.5 2.8-2.9" }],
  eye: [{ d: "M1.5 8S4 3.6 8 3.6 14.5 8 14.5 8 12 12.4 8 12.4 1.5 8 1.5 8z" }, { d: "M8 6.2a1.8 1.8 0 1 0 0 3.6a1.8 1.8 0 1 0 0-3.6z" }],
  clipboard: [{ d: "M4.5 3h7v11.2h-7z" }, { d: "M6.5 1.8h3v2.2h-3z" }, { d: "M6.4 9l1.3 1.3 2.5-2.6" }],
  plug: [{ d: "M6 1.5v3M10 1.5v3M4.5 4.5h7v2.8a3.5 3.5 0 0 1-7 0z" }, { d: "M8 10.8v3.7" }],
  plugzap: [{ d: "M6 1.5v3M10 1.5v3M4.5 4.5h7v2.8a3.5 3.5 0 0 1-7 0z" }, { d: "M8 10.8v3.7M13.6 9.6l-1.4 2h2l-1.4 2" }],
  plugoff: [{ d: "M6 1.5v3M10 1.5v3M4.5 4.5h7v2.8a3.5 3.5 0 0 1-7 0z" }, { d: "M8 10.8v3.7M1.8 1.8l12.4 12.4" }],
  clock: [{ d: C }, { d: "M8 4.6V8l2.4 1.5" }],
  history: [{ d: "M2.4 8a5.6 5.6 0 1 0 1.7-4" }, { d: "M2.4 2.4v2.6H5M8 5v3l2 1.3" }],
  flask: [{ d: "M6 1.6h4M6.6 1.6v4.5L2.9 12.9a1 1 0 0 0 .9 1.5h8.4a1 1 0 0 0 .9-1.5L9.4 6.1V1.6" }],
  lock: [{ d: "M3.8 7.2h8.4v6.9H3.8z" }, { d: "M5.5 7.2V5a2.5 2.5 0 0 1 5 0v2.2" }],
};
export type GlyphName = keyof typeof G;

export function Glyph(props: { name: GlyphName }) {
  return <svg class="a-glyph" viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor"
    stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
    <For each={G[props.name] as P[]}>{p => <path d={p.d} fill={p.fill ? "currentColor" : "none"} stroke={p.fill ? "none" : "currentColor"} />}</For>
  </svg>;
}
