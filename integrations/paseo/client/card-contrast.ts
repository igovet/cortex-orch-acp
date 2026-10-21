/**
 * Readable secondary text for the coordinator worker card.
 *
 * The card's role, route and usage lines use the host theme's
 * `foregroundMuted`, and its hovered/pressed state the theme's `surface2`.
 * In the installed light theme that pair measured 4.40:1 (rgb(113,113,122)
 * on rgb(244,244,245)), below the 4.5:1 docs/DESIGN.md requires for 11-12px
 * text. The plugin may only use the host's theme tokens and must not change
 * the host's global theme, so the card derives its secondary text colour
 * from those tokens: `foregroundMuted` itself when it already reaches 4.5:1
 * on every background the card can show (normal, hover, focus, pressed),
 * otherwise the smallest step from it toward `foreground` that does. One
 * colour serves every state, so a hover never changes the text colour.
 *
 * The same rule applies to the card's other coloured text: in the installed
 * default dark theme the accent (#20744A, the "Open worker" action) measures
 * about 2.5-3.0:1 on its surfaces, so status and action text are derived
 * the same way from their own token.
 */

export const MIN_TEXT_CONTRAST = 4.5;

type Rgb = { r: number; g: number; b: number };

function clampChannel(value: number): number {
  return Math.max(0, Math.min(255, value));
}

/** `#rgb`, `#rrggbb`, `#rrggbbaa` (alpha ignored), `rgb()`/`rgba()`;
 * `null` for anything else (named colours, `hsl()`, variables). */
export function parseColor(value: string): Rgb | null {
  const text = value.trim().toLowerCase();
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$/.exec(text);
  if (hex) {
    const digits = hex[1].length === 3 ? hex[1].split("").map((d) => d + d).join("") : hex[1].slice(0, 6);
    return { r: parseInt(digits.slice(0, 2), 16), g: parseInt(digits.slice(2, 4), 16), b: parseInt(digits.slice(4, 6), 16) };
  }
  const rgb = /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)(?:\s*[,/]\s*[\d.]+%?)?\s*\)$/.exec(text);
  if (rgb) return { r: clampChannel(Number(rgb[1])), g: clampChannel(Number(rgb[2])), b: clampChannel(Number(rgb[3])) };
  return null;
}

function luminance({ r, g, b }: Rgb): number {
  const channel = (value: number) => {
    const c = value / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG 2.x contrast ratio of two parsed colours. */
export function contrastRatio(a: Rgb, b: Rgb): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (light + 0.05) / (dark + 0.05);
}

function mix(from: Rgb, to: Rgb, amount: number): Rgb {
  return {
    r: Math.round(from.r + (to.r - from.r) * amount),
    g: Math.round(from.g + (to.g - from.g) * amount),
    b: Math.round(from.b + (to.b - from.b) * amount),
  };
}

function toRgbString({ r, g, b }: Rgb): string {
  return `rgb(${r}, ${g}, ${b})`;
}

/**
 * The text colour to draw on every one of `backgrounds`: the theme's own
 * `color` when it is already readable on all of them, else the least blend
 * toward `foreground` (searched in 5% steps) that is. If a token cannot be
 * parsed, the theme's `foreground` is used: an unknown colour is never
 * assumed readable.
 */
export function readableTextColor(color: string, foreground: string, backgrounds: readonly string[]): string {
  const muted = color;
  const from = parseColor(muted);
  const to = parseColor(foreground);
  const surfaces = backgrounds.map(parseColor);
  if (!from || !to || surfaces.some((surface) => surface === null)) return foreground;
  const readable = (color: Rgb) => surfaces.every((surface) => contrastRatio(color, surface as Rgb) >= MIN_TEXT_CONTRAST);
  if (readable(from)) return muted;
  for (let step = 1; step <= 20; step += 1) {
    const candidate = mix(from, to, step / 20);
    if (readable(candidate)) return toRgbString(candidate);
  }
  return foreground;
}
