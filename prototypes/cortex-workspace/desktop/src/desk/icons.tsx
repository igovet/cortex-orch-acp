/** Inline outline icons (Lucide-style geometry, 24-unit grid, 1.5px rendered stroke at 16px). Decorative: always paired with text or an accessible name. */
const c = (cx: number, cy: number, r: number) => `M${cx - r} ${cy}a${r} ${r} 0 1 0 ${2 * r} 0a${r} ${r} 0 1 0 ${-2 * r} 0`;
const r = (x: number, y: number, w: number, h: number) => `M${x + 2} ${y}h${w - 4}a2 2 0 0 1 2 2v${h - 4}a2 2 0 0 1-2 2h${-(w - 4)}a2 2 0 0 1-2-2v${-(h - 4)}a2 2 0 0 1 2-2z`;

const PATHS = {
  plus: "M12 5v14M5 12h14",
  search: `${c(11, 11, 7)}M20 20l-3.5-3.5`,
  inbox: "M22 12h-6l-2 3h-4l-2-3H2M5.5 5.1 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.5-6.9A2 2 0 0 0 16.8 4H7.2a2 2 0 0 0-1.7 1.1z",
  grid: "M4 4h6v6H4zM14 4h6v6h-6zM14 14h6v6h-6zM4 14h6v6H4z",
  plug: "M12 22v-5M9 8V2M15 8V2M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8z",
  plugOff: "M12 22v-5M9 8V2M15 8V2M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8zM3 3l18 18",
  gitMerge: `${c(18, 18, 3)}${c(6, 6, 3)}M6 21V9a9 9 0 0 0 9 9`,
  gitBranch: `M6 3v12${c(18, 6, 3)}${c(6, 18, 3)}M18 9a9 9 0 0 1-9 9`,
  gitCommit: `${c(12, 12, 3)}M3 12h6M15 12h6`,
  xCircle: `${c(12, 12, 10)}M15 9l-6 6M9 9l6 6`,
  checkCircle: `${c(12, 12, 10)}M8.5 12l2.5 2.5 4.5-5`,
  notRun: `${c(12, 12, 10)}M8 12h8`,
  help: `${c(12, 12, 10)}M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3M12 17h.01`,
  running: `${c(12, 12, 10)}M12 2a10 10 0 0 1 0 20z`,
  octagonPause: "M7.9 2h8.2L22 7.9v8.2L16.1 22H7.9L2 16.1V7.9zM10 15V9M14 15V9",
  pause: `${c(12, 12, 10)}M10 15V9M14 15V9`,
  complete: `${c(12, 12, 10)}M8.5 12l2.5 2.5 4.5-5`,
  key: `${c(7.5, 15.5, 5.5)}M21 2l-9.6 9.6M15.5 7.5l3 3L22 7l-3-3`,
  question: "M7.9 20A9 9 0 1 0 4 16.1L2 22zM9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3M12 17h.01",
  fileCheck: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M9 15l2 2 4-4",
  eye: `M2 12s3-7 10-7 10 7 10 7-3 7-10 7S2 12 2 12z${c(12, 12, 3)}`,
  history: "M3 12a9 9 0 1 0 9-9 9.8 9.8 0 0 0-6.7 2.7L3 8M3 3v5h5M12 7v5l4 2",
  clock: `${c(12, 12, 10)}M12 6v6l4 2`,
  flask: "M9 3h6M10 3v7.4L4.6 19.2A1.2 1.2 0 0 0 5.6 21h12.8a1.2 1.2 0 0 0 1-1.8L14 10.4V3M7 15h10",
  sun: `${c(12, 12, 4)}M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M6.3 17.7l-1.4 1.4M19.1 4.9l-1.4 1.4`,
  moon: "M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9z",
  monitor: `${r(2, 3, 20, 14)}M8 21h8M12 17v4`,
  settings: `M12.2 2h-.4a2 2 0 0 0-2 2v.2a2 2 0 0 1-1 1.7l-.4.3a2 2 0 0 1-2 0l-.2-.1a2 2 0 0 0-2.7.7l-.2.4a2 2 0 0 0 .7 2.7l.2.1a2 2 0 0 1 1 1.7v.5a2 2 0 0 1-1 1.7l-.2.1a2 2 0 0 0-.7 2.7l.2.4a2 2 0 0 0 2.7.7l.2-.1a2 2 0 0 1 2 0l.4.3a2 2 0 0 1 1 1.7v.2a2 2 0 0 0 2 2h.4a2 2 0 0 0 2-2v-.2a2 2 0 0 1 1-1.7l.4-.3a2 2 0 0 1 2 0l.2.1a2 2 0 0 0 2.7-.7l.2-.4a2 2 0 0 0-.7-2.7l-.2-.1a2 2 0 0 1-1-1.7v-.5a2 2 0 0 1 1-1.7l.2-.1a2 2 0 0 0 .7-2.7l-.2-.4a2 2 0 0 0-2.7-.7l-.2.1a2 2 0 0 1-2 0l-.4-.3a2 2 0 0 1-1-1.7V4a2 2 0 0 0-2-2z${c(12, 12, 3)}`,
  chevronRight: "M9 18l6-6-6-6",
  chevronDown: "M6 9l6 6 6-6",
  chevronLeft: "M15 18l-6-6 6-6",
  arrowLeft: "M19 12H5M12 19l-7-7 7-7",
  arrowRight: "M5 12h14M12 5l7 7-7 7",
  arrowUp: "M12 19V5M5 12l7-7 7 7",
  arrowDown: "M12 5v14M19 12l-7 7-7-7",
  panelLeft: `${r(3, 3, 18, 18)}M9 3v18`,
  panelList: `${r(3, 3, 18, 18)}M9 3v18M13 8h4M13 12h4`,
  panelBottom: `${r(3, 3, 18, 18)}M3 15h18`,
  more: "M5 12h.01M12 12h.01M19 12h.01",
  filter: "M22 3H2l8 9.5V19l4 2v-8.5z",
  x: "M18 6L6 18M6 6l12 12",
  copy: `${r(8, 8, 14, 14)}M4 16a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2`,
  lock: `${r(3, 11, 18, 11)}M7 11V7a5 5 0 0 1 10 0v4`,
  circleSlash: `${c(12, 12, 10)}M4.9 4.9l14.2 14.2`,
  file: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6",
  folder: "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.7-.9L9.6 3.9A2 2 0 0 0 7.9 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2z",
  terminal: "M4 17l6-6-6-6M12 19h8",
  sort: "M3 6h18M7 12h10M10 18h4",
  check: "M20 6L9 17l-5-5",
  pencil: "M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z",
  alert: "M21.7 18l-8-14a2 2 0 0 0-3.5 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.7-3zM12 9v4M12 17h.01",
  refresh: "M3 12a9 9 0 0 1 9-9 9.8 9.8 0 0 1 6.7 2.7L21 8M21 3v5h-5M21 12a9 9 0 0 1-9 9 9.8 9.8 0 0 1-6.7-2.7L3 16M8 16H3v5",
  keyboard: `${r(2, 4, 20, 16)}M6 8h.01M10 8h.01M14 8h.01M18 8h.01M8 12h.01M12 12h.01M16 12h.01M7 16h10`,
  info: `${c(12, 12, 10)}M12 16v-4M12 8h.01`,
  board: `${r(3, 3, 18, 18)}M9 3v18M15 3v18`,
  list: "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
  overview: `${r(3, 3, 18, 18)}M3 9h18M9 21V9`,
  user: `${c(12, 8, 4)}M20 21a8 8 0 0 0-16 0`,
  bot: `${r(3, 8, 18, 12)}M12 8V4M8 14h.01M16 14h.01`,
  phone: `${r(5, 2, 14, 20)}M12 18h.01`,
  send: "M22 2L11 13M22 2l-7 20-4-9-9-4z",
  plugZap: "M6.3 20.3a2.4 2.4 0 0 0 3.4 0L12 18l-6-6-2.3 2.3a2.4 2.4 0 0 0 0 3.4zM2 22l3-3M7.5 13.5L10 11M10.5 16.5L13 14M12 6l6 6 2.3-2.3a2.4 2.4 0 0 0 0-3.4l-2.6-2.6a2.4 2.4 0 0 0-3.4 0z",
  dot: c(12, 12, 4),
} as const;
export type IconName = keyof typeof PATHS;

export function Icon(props: { name: IconName; size?: number; class?: string }) {
  const size = () => props.size ?? 16;
  return <svg class={`icon ${props.class ?? ""}`} width={size()} height={size()} style={{ width: `${size() / 16}rem`, height: `${size() / 16}rem` }} viewBox="0 0 24 24" fill="none" stroke="currentColor"
    stroke-width={size() <= 14 ? 2.4 : 2.25} stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d={PATHS[props.name]} />
    {props.name === "running" ? <path d="M12 2a10 10 0 0 1 0 20z" fill="currentColor" stroke="none" /> : null}
    {props.name === "dot" ? <path d={PATHS.dot} fill="currentColor" stroke="none" /> : null}
  </svg>;
}
