import { createComputed, createEffect, on, type Accessor } from "solid-js";

const MOVE = { duration: 280, easing: "cubic-bezier(.2,.7,.2,1)" } as const;
const still = () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

type Spot = { card: HTMLElement; column: string; box: DOMRect };
type Before = { cards: Map<string, Spot>; widths: Map<string, number> };
const columnsOf = (board: HTMLElement) => [...board.querySelectorAll<HTMLElement>(".col[data-col]")];
const cardsOf = (board: HTMLElement) => new Map([...board.querySelectorAll<HTMLElement>(".card[data-task]")].map((card): [string, Spot] => [card.dataset.task!, { card, column: card.closest<HTMLElement>(".col")?.dataset.col ?? "", box: card.getBoundingClientRect() }]));

/** A column clips what it scrolls, so a card crossing to another one flies as a copy above the board and lands on the real card. */
function fly(spot: Spot, from: DOMRect) {
  const to = spot.box, ghost = spot.card.cloneNode(true) as HTMLElement;
  ghost.removeAttribute("data-task");
  ghost.setAttribute("aria-hidden", "true");
  ghost.classList.add("ws-fly");
  Object.assign(ghost.style, { left: `${to.left}px`, top: `${to.top}px`, width: `${to.width}px`, height: `${to.height}px` });
  document.body.append(ghost);
  spot.card.style.visibility = "hidden";
  const flight = ghost.animate([{ transform: `translate(${from.left - to.left}px, ${from.top - to.top}px)` }, { transform: "none" }], MOVE);
  const land = () => { ghost.remove(); spot.card.style.visibility = ""; };
  return flight.finished.then(land, land);
}

function play(board: HTMLElement, first: Before) {
  const now = cardsOf(board);
  const crossed = [...now].filter(([id, spot]) => { const was = first.cards.get(id); return was && was.column !== spot.column; });
  if (!crossed.length) return;
  const moves: Promise<unknown>[] = [], columns = columnsOf(board);
  // The final layout is already measured, so a column can open from its old width while its cards keep their final one.
  for (const column of columns) {
    const from = first.widths.get(column.dataset.col!), to = column.getBoundingClientRect().width;
    if (from === undefined || Math.abs(from - to) < .5) continue;
    column.style.setProperty("--col-w", `${to}px`);
    const size = (w: number) => ({ flexGrow: 0, flexBasis: `${w}px`, minWidth: `${w}px`, maxWidth: `${w}px` });
    moves.push(column.animate([size(from), size(to)], MOVE).finished.catch(() => undefined));
  }
  for (const [id, spot] of now) {
    const was = first.cards.get(id);
    if (!was) continue;
    if (was.column !== spot.column) { moves.push(fly(spot, was.box)); continue; }
    // A neighbour closing the gap slides into it too.
    const shift = was.box.top - spot.box.top;
    if (Math.abs(shift) > .5) moves.push(spot.card.animate([{ transform: `translateY(${shift}px)` }, { transform: "none" }], MOVE).finished.catch(() => undefined));
  }
  board.setAttribute("data-flying", "");
  void Promise.all(moves).then(() => {
    board.removeAttribute("data-flying");
    for (const column of columns) column.style.removeProperty("--col-w");
  });
}

/**
 * A card that changes status crosses the board instead of vanishing and reappearing. The old places are read just before the
 * screen changes and the new ones just after, so nothing here decides where a card belongs. Reduced motion moves it at once.
 */
export function createBoardMotion(board: () => HTMLElement | undefined, placement: Accessor<string>) {
  let before: Before | null = null;
  createComputed(on(placement, (_, previous) => {
    const el = board();
    before = previous !== undefined && el && typeof el.animate === "function" && !still()
      ? { cards: cardsOf(el), widths: new Map(columnsOf(el).map(c => [c.dataset.col!, c.getBoundingClientRect().width])) } : null;
  }));
  createEffect(on(placement, () => {
    const el = board(), first = before;
    before = null;
    if (el && first) play(el, first);
  }, { defer: true }));
}
