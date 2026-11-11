# A simpler task screen: three concepts

Three exploratory prototypes of the Cortex task screen. They keep the current
look (colours, type, controls and cards come from
[`../mockups/styles.css`](../mockups/styles.css)) and change only how the screen
works: one obvious next action, few regions and plain labels.

These prototypes are exploratory. They do not change the accepted direction in
[`docs/cortex-workspace/DESIGN.md`](../../../docs/cortex-workspace/DESIGN.md) and
are not connected to the app.

## How to open

Open [`index.html`](index.html) in a browser and pick a concept. No build step,
no network access. If your browser blocks pages opened from disk, serve this
`prototypes/cortex-workspace` folder with any local static server, for example
`python3 -m http.server --bind 127.0.0.1` run from that folder, and open
`concepts/index.html`.

At the top of each concept, outside the simulated app:

- **Sample** switches between three states: *Before start* (a saved draft),
  *Needs your answer* (the "Fix token refresh race" task with a question from
  QA Engineer) and *Result, another task* (the separate "Rate-limit the export
  endpoint" task, whose result has a failing check).
- **Light theme / Dark theme** switches the theme. `?theme=light` in the address
  works too.

Everything is local: answering, starting or requesting changes only changes the
page.

## The three concepts

**Next action** ([`simple-a.html`](simple-a.html)). Open a task and immediately
see the one thing you need to do. The question, the Start button or the result
decision sits at the top of one column. Plan, Team, Activity and Task details
are buttons below it, not permanent panels. On a wide screen Team or Activity
opens beside the task; on a tablet or phone it replaces the task with a
Back to task button.

**Conversation** ([`simple-b.html`](simple-b.html)). Talk to Cortex in one
thread. Questions and results are messages you act on where they appear. One
"View question" or "View result" link in the header brings you to the waiting
item. While a question waits, the message box stays folded so a reply cannot go
to the wrong place; "Message Cortex instead" opens it without losing either
draft.

**Three steps** ([`simple-c.html`](simple-c.html)). Plan, Work and Result. Each
step says whether it is done, happening now or not yet; you see only the step
you open. The question lives in Work and the result decision lives in Result.
Steps are for reading: there is no Next button, and the screen never switches
step on its own.

## Shared rules

- Choosing an answer does not send it; **Send answer** does.
- Accept is unavailable while a check fails; **Request changes** is the main
  action instead. Accepting never commits or merges; Git stays separate.
- Saving a draft starts nothing; **Start task** does.
- Layouts reflow at desktop, tablet and phone widths. Phone controls are at
  least 44 px tall, keyboard focus is visible, and reduced motion stops the
  spinners.
