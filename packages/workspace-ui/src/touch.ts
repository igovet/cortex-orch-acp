/**
 * A touch screen has no pointer resting over anything, yet the engine keeps `:hover` on the last thing touched until something
 * else is touched: a row stays lit after the tap that opened it. On the phone every hover rule is therefore taken out of the
 * loaded style sheets. The desktop never calls this.
 *
 * Two kinds of hover rule are not highlights, and are kept in the form a touch screen can use:
 * - "not hovered" (`:not(:hover)`) is always true without a pointer, so the condition is dropped and the rule stays;
 * - a rule that reveals something inside the hovered element (it sets opacity, visibility or display on a descendant) would
 *   leave that control unreachable, so it applies always.
 */

/** Selector list split at its top-level commas (never inside `:is(a, b)` and the like). */
function parts(selector: string): string[] {
  const out: string[] = [];
  let depth = 0, from = 0;
  for (let i = 0; i < selector.length; i++) {
    const c = selector[i];
    if (c === "(" || c === "[") depth++;
    else if (c === ")" || c === "]") depth--;
    else if (c === "," && depth === 0) { out.push(selector.slice(from, i)); from = i + 1; }
  }
  out.push(selector.slice(from));
  return out.map(part => part.trim()).filter(Boolean);
}

/** True when `:hover` stands on an ancestor: something follows it across a combinator. */
function onAncestor(part: string): boolean {
  const rest = part.slice(part.lastIndexOf(":hover") + ":hover".length).replace(/\([^)]*\)|\[[^\]]*\]/g, "");
  return /[\s>+~]\s*\S/.test(rest);
}

/**
 * What a hover selector becomes on a touch screen: the same selector when it has no hover in it, a selector without the hover
 * condition, or null when nothing of the rule is left. `reveals` says the rule sets opacity, visibility or display.
 */
export function touchSelector(selector: string, reveals: boolean): string | null {
  if (!selector.includes(":hover")) return selector;
  const kept: string[] = [];
  for (const raw of parts(selector)) {
    const part = raw.replaceAll(":not(:hover)", "");
    if (!part.includes(":hover")) { if (part) kept.push(part); continue; }
    if (reveals && onAncestor(part)) kept.push(part.replaceAll(":hover", ""));
  }
  return kept.length ? kept.join(", ") : null;
}

type RuleList = { cssRules: CSSRuleList; deleteRule(index: number): void };
function strip(owner: RuleList): void {
  let rules: CSSRuleList;
  try { rules = owner.cssRules; } catch { return; }
  for (let i = rules.length - 1; i >= 0; i--) {
    const rule = rules[i] as CSSRule & Partial<CSSStyleRule> & Partial<RuleList>;
    // A style rule may itself hold nested rules; a group (@media, @container, @layer, @supports) only holds rules.
    if (rule.cssRules && typeof rule.deleteRule === "function") strip(rule as RuleList);
    if (typeof rule.selectorText !== "string" || !rule.selectorText.includes(":hover")) continue;
    const style = rule.style;
    const reveals = !!style && ["opacity", "visibility", "display"].some(name => style.getPropertyValue(name) !== "");
    const next = touchSelector(rule.selectorText, reveals);
    if (next === null) owner.deleteRule(i);
    else if (next !== rule.selectorText) {
      try { rule.selectorText = next; } catch { owner.deleteRule(i); }
    }
  }
}

const done = new WeakSet<object>();
/**
 * Takes hover out of every style sheet of the document, now and as later ones arrive (a page's styles load with the page).
 * Returns what stops watching for new sheets; sheets already changed stay changed.
 */
export function dropHover(doc: Document = document): () => void {
  const sweep = () => {
    for (const sheet of Array.from(doc.styleSheets)) {
      // A sheet still loading has no rules to read yet: it is swept again when it arrives.
      let ready = true;
      try { void sheet.cssRules; } catch { ready = false; }
      if (!ready || done.has(sheet)) continue;
      done.add(sheet);
      strip(sheet);
    }
  };
  sweep();
  if (typeof MutationObserver !== "function" || !doc.head) return () => undefined;
  const loaded = (event: Event) => { if ((event.target as Element | null)?.localName === "link") sweep(); };
  const watch = new MutationObserver(sweep);
  watch.observe(doc.head, { childList: true });
  doc.addEventListener("load", loaded, true);
  return () => { watch.disconnect(); doc.removeEventListener("load", loaded, true); };
}
