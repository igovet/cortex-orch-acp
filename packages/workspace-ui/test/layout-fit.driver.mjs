// The rendered layout gate in WebKitGTK, on fake data and a private virtual screen. A problem on any screen fails the run.
// Run through layout-fit.mjs, inside the cortex-workspace-rendered image.
//
// To add a screen held to the full rules, add one entry to SCREENS below: a name, its route, what must be on screen when it is ready, and its sizes.
// Screens not moved to the layout primitives yet stay in KEPT and the flows in run(), under the row and reason rules only.
import {Host, startXvfb, engineEnv, main, sleep, probe, quote, join, writeJson, sub, TimeoutError} from "./rendered/webkit.mjs";

const base = process.argv[2], label = process.argv[3], extra = process.argv.slice(4), evidence = "/evidence";
const json = JSON.stringify;
const eq = (a, b) => json(a) === json(b);
const unique = xs => [...new Set(xs)];
const env = name => { const v = process.env[name]; if (v === undefined) throw Error(`${name} is required`); return v; };

// Screens and sizes the owner works at. Every row class here must stay one line, and every pane must keep its width.
const SIZES = [[1280, 800], [1920, 1080]];
const PROBE = probe("layout-fit.PROBE.js");
// The full rules, for screens built from the layout primitives. A string in `problems` is a failure.
const STRICT = sub(probe("layout-fit.STRICT.js"), "LABEL_FIT", env("CW_LABEL_FIT"));

// One line per screen held to the full rules above: name, query, ready expression, sizes.
const HARNESS = "document.querySelector('.l-page .l-bar') && window.flip";
const DIALOG = "document.querySelector('dialog[open] .l-bar') && window.flip";
const FULL = [[960, 540], [1024, 768], [1280, 800], [1872, 1100]];
const OWNER = [[1024, 768], [1280, 800], [1872, 1100]];
const board_at = state => "route=" + quote(`/m/<M>/p/p/s/${state}/board`);
const QUEUE = "document.querySelector('[aria-label=\"Sprint queue\"]') && document.querySelector('.card')";
const LOGO = "document.querySelector('.l-startup-logo img')?.complete && document.querySelector('.l-startup-logo img').naturalWidth > 0";
// The review screens: a result in the conversation, Changes with and without Git, Ready to merge. 33 checks, long Russian text.
const review_at = path => "world=review&route=" + quote(`/m/<M>/t/${path}`);
const automatic_board_at = "world=review&route=" + quote("/m/<M>/p/nogit/s/auto-sprint/board");
const stale_automatic_at = review_at("auto-stale/changes");
const stale_automatic_conversation_at = review_at("auto-stale");
const automatic_owner_hold_conversation_at = review_at("auto-owner-hold");
const automatic_finish_hold_conversation_at = review_at("auto-finish-hold");
const RESULT = "document.querySelector('.rcard [data-bar=card]') && document.querySelector('.rcard .rv-checks')";
const CHANGES = "document.querySelector('.rv-bar [data-bar=action]') && document.querySelector('.evid .l-row')";
const READY = "document.querySelector('.ready .rcard [data-bar=card]')";
const HOLD_CARD = "document.querySelector('.chat-review-hold [data-bar=card] button')";
const HOLD_HEADER = "[...document.querySelectorAll('.titlebar button')].some(b=>b.textContent.trim()==='See what is needed')";
const HOLD_CHANGES = CHANGES + " && " + HOLD_HEADER + " && document.querySelector('.rv-bar [role=note]')?.textContent.includes('pinned in its conversation')";
const STALE_CARD = HOLD_CARD + " && document.querySelector('.rcard .rv-stale')";
const REPAIR_SENT = "[...document.querySelectorAll('.chat-review-hold button[aria-disabled=true]')].some(b=>b.textContent.trim()==='Let the agent fix it' && b.dataset.why?.startsWith('Sent.'))";
const SCREENS = [
  ["result-conversation", review_at("rv"), RESULT, OWNER],
  ["changes-no-git", review_at("rv/changes"), CHANGES, OWNER],
  ["changes-git", review_at("rg/changes"), CHANGES + " && document.querySelector('.fhead')", OWNER],
  ["ready-review-and-merge", review_at("rm/changes"), READY, OWNER],
  ["ready-no-git", review_at("rn/changes"), READY, OWNER],
  // Automatic settlement blocked on a required check. The task must explain the hold
  // and keep an explicit Close as is action reachable from the linked task.
  ["automatic-hold", review_at("auto-hold/changes"), HOLD_CHANGES, OWNER],
  ["automatic-stale", stale_automatic_at, HOLD_CHANGES, OWNER],
  ["automatic-stale-conversation", stale_automatic_conversation_at, STALE_CARD, OWNER],
  ["automatic-owner-hold-conversation", automatic_owner_hold_conversation_at, HOLD_CARD, OWNER],
  ["automatic-finish-hold-conversation", automatic_finish_hold_conversation_at, HOLD_CARD, OWNER],
  ["board-paused", board_at("b-pause"), QUEUE, [[960, 540], ...OWNER]],
  ["board-running", board_at("b-run"), QUEUE, [[960, 540], ...OWNER]],
  ["board-blocked", board_at("b-need"), QUEUE, OWNER],
  ["board-idle", board_at("b-idle"), QUEUE, OWNER],
  ["board-manual", board_at("b-manual"), QUEUE, OWNER],
  ["board-no-next", board_at("b-none"), QUEUE, OWNER],
  ["board-complete", board_at("b-done"), QUEUE, OWNER],
  ["board-clamped", board_at("b-clamp"), QUEUE, OWNER],
  ["board-paused-dark", board_at("b-pause") + "&dark=1", QUEUE, [[1280, 800]]],
  ["startup", "startup=pending", LOGO, OWNER],
  ["startup-dark", "startup=pending&dark=1", LOGO, [[1280, 800]]],
  ["startup-failed", "startup=failed", LOGO + " && document.querySelector('.l-startup-slot .btn')", OWNER],
  ["primitives", "view=primitives", HARNESS, FULL],
  ["primitives-long", "view=primitives&stress=1", HARNESS, FULL],
  ["primitives-compact", "view=primitives&pane=780", HARNESS, [[1024, 768]]],
  ["primitives-tight", "view=primitives&pane=620", HARNESS, [[1024, 768]]],
  ["primitives-tight-long", "view=primitives&pane=620&stress=1", HARNESS, [[1024, 768]]],
  ["primitives-dialog", "view=primitives&dialog=1", DIALOG, [[960, 540], [1280, 800]]],
  ["primitives-dialog-long", "view=primitives&dialog=1&stress=1", DIALOG, [[960, 540], [1280, 800]]],
];

// Every other screen, on the same long names, at the owner's widths. Each is ready once its own main part is on screen.
const at = route => "route=" + quote(route);
const FRAME = "document.querySelector('.titlebar') && !document.querySelector('.l-startup')";
const on = selector => FRAME + " && document.querySelector(" + json(selector) + ")";
const MS = "/settings/machine/<M>/";
const PAGES = [
  ["home", at("/m/<M>"), on(".home")],
  ["inbox", at("/m/<M>/inbox"), on(".main .page")],
  ["archive", at("/m/<M>/archive"), on(".main .page")],
  ["new-task", at("/m/<M>/new?kind=task&project=p"), on(".create .composer textarea")],
  ["new-sprint", at("/m/<M>/new?kind=sprint&project=p"), on(".create .composer textarea")],
  ["project-board", at("/m/<M>/p/p/board"), on(".board-wrap .card")],
  ["project-list", at("/m/<M>/p/p/list"), on(".l-work .l-page-body > *")],
  ["git", at("/m/<M>/p/p/git"), on(".git-sum") + " && document.querySelector('.hrow')"],
  ["code", at("/m/<M>/p/p/code?file=<FILE>"), on(".cm-content") + " && document.querySelector('.ecrumb button')"],
  ["worktrees", at("/m/<M>/p/p/worktrees"), on(".l-work .l-page-body > *")],
  ["knowledge", at("/m/<M>/p/p/knowledge"), on(".l-work .l-page-body > *")],
  ["project-settings", at("/m/<M>/p/p/settings"), on(".l-work .l-page-body > *")],
  ["sprint-planning", at("/m/<M>/p/p/s/sp/planning"), on(".brief")],
  ["sprint-plan", at("/m/<M>/p/p/s/sp/plan"), on(".sp-plan:not([hidden]) .pt-row")],
  // A created sprint's chat: the closed composer says its one note in its writing area, and its tools keep their size.
  ["sprint-chat-created", at("/m/<M>/p/p/s/paused/planning"), on(".sp-conv:not([hidden]) .composer textarea:disabled")],
  ["sprint-summary", at("/m/<M>/p/p/s/sp/summary"), on(".main .page")],
  ["task-conversation", at("/m/<M>/t/task-2"), on(".brief")],
  ["task-workers", at("/m/<M>/t/r-now/workers"), on(".l-task .l-page-body > *")],
  ["task-code", at("/m/<M>/t/task-2/code"), on(".ed")],
  ...["appearance", "voice", "updates", "shortcuts", "machines"].map(s => ["settings-" + s, at(`/settings/app/${s}`), on(".set-wrap")]),
  ...["overview", "connections", "worktrees", "storage", "diagnostics", "remote"].map(s => ["machine-" + s, at(MS + s), on(".set-wrap")]),
  ...["defaults", "models", "routing", "effective", "policy", "tools", "profiles", "files"].map(p => ["orchestration-" + p, at(MS + "orchestration/" + p), on(".set-wrap")]),
];
SCREENS.push(...PAGES.map(([name, query, ready]) => [name, query, ready, OWNER]),
  ...PAGES.filter(([name]) => ["home", "git", "settings-appearance", "orchestration-routing"].includes(name)).map(([name, query, ready]) => [name + "-dark", query + "&dark=1", ready, [[1280, 800]]]));
// The sprint plan's Graph view on a created 60-task plan, from the window minimum up: in the pane, in Full view (FLOWS), and
// with Details docked beside it. The panels give way by one rule, so nothing may overlap or narrow below its minimum.
const GRAPH_AT = "graph=1&" + at("/m/<M>/p/p/s/big/plan");
const GRAPH_READY = on(".gsv .n") + " && document.querySelector('.gv-band')";
const GRAPH_SIZES = [...FULL, [1920, 1080]];
const GRAPH_PICK = "const n=document.querySelector('.gsv .n[data-id=\"t:big-12\"]');for(const t of ['pointerdown','pointerup'])n.dispatchEvent(new PointerEvent(t,{bubbles:true,button:0}))";
SCREENS.push(["sprint-graph", GRAPH_AT, GRAPH_READY, GRAPH_SIZES], ["sprint-graph-dark", GRAPH_AT + "&dark=1", GRAPH_READY, [[1280, 800]]]);

// The smallest window the desktop app allows (its min_inner_size, which layout-fit.mjs reads from desktop.rs) and 960 px, with the
// window's own controls in the title band. The task screens are held to the full rules there in each state their title band can
// hold, in both themes; every other page's title band is held to them on its own (title_gate).
const MINIMUM = env("CW_MIN_WINDOW").split("x").map(Number);
const NARROW = [[960, 540], MINIMUM].filter((s, i, all) => all.findIndex(o => eq(o, s)) === i);
const task_at = path => at("/m/<M>/t/" + path) + "&chrome=1";
const TASKS = [
  ["task-question", task_at("n-ask"), on(".brief")],
  ["task-running", task_at("r-now"), on(".brief")],
  ["task-paused", task_at("p-stop"), on(".brief")],
  ["task-not-started", task_at("task-2"), on(".brief")],
  ["task-question-workers", task_at("n-ask/workers"), on(".l-task .l-page-body > *")],
  ["task-question-code", task_at("n-ask/code"), on(".ed")],
];
for (const [name, query, ready] of TASKS) for (const [theme, more] of [["", ""], ["-dark", "&dark=1"]]) SCREENS.push([name + theme, query + more, ready, [...NARROW, ...OWNER]]);
// The same at the narrow sizes with the sidebar dragged to its widest (prefs.ts SIDEBAR_MAX), which leaves the title band its least room.
for (const [name, query, ready] of TASKS) SCREENS.push([name + "-wide-sidebar", query + "&sidebar=" + env("CW_SIDEBAR_MAX"), ready, NARROW]);
// A running task with a long Russian message and a pasted image in its queue: the row stays one line, its files are not shown in it.
const QUEUED_ROW = on(".queue .qrow .qrow-end");
SCREENS.push(["task-queued", task_at("r-now") + "&queued=r-now", QUEUED_ROW, FULL], ["task-queued-dark", task_at("r-now") + "&queued=r-now&dark=1", QUEUED_ROW, [[1280, 800]]]);
// Two real sidebar task rows with long automatic reasons. Their visible labels must retain width,
// keep the owner status on the same row, avoid protocol underscores, and link to distinct tasks.
const SIDEBAR_REASONS = "(()=>{const rows=[...document.querySelectorAll('.sidebar .task-row')].filter(r=>/\\/t\\/(auto-cont|auto-files)$/.test(r.getAttribute('href')||''));const data=rows.map(r=>{const title=r.querySelector('.trunc'),sig=r.querySelector('.sig'),rr=r.getBoundingClientRect(),tr=title?.getBoundingClientRect(),sr=sig?.getBoundingClientRect();return {location:r.closest('.sb-work')?'work':'project',project:r.closest('.tasks')?.previousElementSibling?.querySelector('.proj .trunc')?.textContent||'',href:r.getAttribute('href'),title:title?.textContent||'',sig:sig?.textContent||'',row:[rr.left,rr.right,rr.top,rr.bottom],titleBox:tr&&[tr.left,tr.right,tr.width],sigBox:sr&&[sr.left,sr.right,sr.width]}});const sb=document.querySelector('.sidebar .sb-scroll'),sideways=sb?sb.scrollWidth-sb.clientWidth:-1;return {data,sideways,ok:sideways===0&&data.length===2&&new Set(data.map(x=>x.href)).size===2&&data.every(x=>x.location===(document.querySelector('.sb-work')?'work':'project'))&&data.filter(x=>x.location==='work').every(x=>x.project.length>10)&&data.every(x=>x.title.length>10&&x.titleBox&&x.titleBox[2]>20&&!x.sig.includes('_')&&x.sigBox&&x.sigBox[2]>20&&x.titleBox[1]<=x.sigBox[0]+1)}})()";
SCREENS.push(["sidebar-automatic-reasons", at("/m/<M>/p/p/board") + "&sidebarRows=1", "document.querySelector('.sidebar .task-row a,.sidebar .task-row')", [[1280, 800], [1024, 768], [960, 540]]]);
SCREENS.push(["sidebar-phone-drawer", at("/m/<M>/p/p/board") + "&phoneapp&menu&sidebarRows=1&sidebarActive",
  "document.querySelector('.ws-drawer .sb-work .proj')", [[390, 844]]]);
SCREENS.push(["sidebar-sprint", board_at("b-run") + "&sidebar=260&sidebarActive",
  "document.querySelector('.sb-work .in-sprint') && document.querySelector('.sb-work [aria-current=page]')", [[1280, 800]]]);
// A working task's steps that recorded output far wider than any column, in Workers and in Conversation; OUTPUT_FLOWS opens them.
const OUTPUT_SCREENS = [["task-output-workers", review_at("out/workers/w-out"), "document.querySelector('.stream button.fold')"], ["task-output-conversation", review_at("out"), "document.querySelector('.timeline button.fold')"]];
for (const [name, query, ready] of OUTPUT_SCREENS) SCREENS.push([name, query, ready, [[1280, 800]]]);
// The runtime's stop question with long consequences, a Recommended mark and its targets: each answer is one row inside the card.
SCREENS.push(["stopq-conversation", "world=conversation&stopq=1&route=" + quote("/m/<M>/t/c-ask"), "document.querySelector('.dc-opts-rows .opt .sub') && document.querySelector('.ws-dc-targets .btn')", [...NARROW, ...OWNER]]);
// A sprint planned by a runtime task: that task's conversation (question, steps, team bar, composer) and its Workers, on the sprint.
SCREENS.push(["sprint-planning-task", "world=conversation&planningSprint=1&route=" + quote("/m/<M>/p/p/s/ps/planning"), "document.querySelector('section.task-panel[aria-label=Conversation] .dc-q') && document.querySelector('.ws-team')", [...NARROW, ...OWNER]],
  ["sprint-planning-workers", "world=conversation&planningSprint=1&route=" + quote("/m/<M>/p/p/s/ps/workers"), "document.querySelector('.wlist .wcard') && document.querySelector('.livepane')", [...NARROW, [1280, 800]]]);
// Connected service, unconfirmed task: one notice, its explanation and the real read-only refresh.
const UNKNOWN_AT = review_at("unk") + "&dark=1";
const UNKNOWN_READY = "document.querySelector('.l-task .l-strip .l-strip-a button') && document.querySelector('.composer textarea:disabled')";
SCREENS.push(["task-unknown-state", UNKNOWN_AT, UNKNOWN_READY, FULL]);
const OPEN_STEPS = "for(const b of document.querySelectorAll('button.fold[aria-expanded=false]'))b.click();for(const b of document.querySelectorAll('button.step-row[aria-expanded=false]'))b.click()";
const TITLE_BAND = PAGES.map(([name, query, ready]) => [name, query + "&chrome=1", ready]);

// Screens not moved to the primitives yet: name, route, ready, the bars that must keep their height. The row and reason rules apply.
const KEPT = [];
const ROUTE = {git: "/m/<M>/p/p/git", code: "/m/<M>/p/p/code?file=<FILE>", sprint: "/m/<M>/p/p/s/sp/planning", "sprint-plan": "/m/<M>/p/p/s/sp/plan", task: "/m/<M>/t/task-2"};

// The old queued row: the file's chip, squeezed to nothing, between the text and Edit, so its name is drawn over the controls.
const QUEUED_CHIP = "for(const r of document.querySelectorAll('.queue .qrow')){const s=document.createElement('span');s.className='ws-atts';s.style.cssText='flex:0 1 0;min-width:0';s.innerHTML='<span class=\"chip ws-att\"><button class=\"ws-att-face\"><span class=\"trunc\">clipboard.png</span></button></span>';r.insertBefore(s,r.querySelector('.qrow-end'));}";

// Each rule has to be able to fail. Faults go in a layer named before the app's, so their !important outranks the primitives'.
const FAULTS = [
  ["page scrolls", "primitives", "html,body{overflow:visible!important;height:auto!important}#root{height:3000px!important}", null, "page scrolls"],
  ["element outside its parent", "primitives", ".l-z-start{margin-left:-40px!important}", null, "outside"],
  ["text clipped with no way to read it", "primitives-long", ".l-strip-t{display:block!important;-webkit-line-clamp:unset!important;max-height:18px!important}", null, "clipped vertically"],
  ["text squeezed to nothing", "primitives", ".l-z-mid{flex:0 0 12px!important}", null, "text squeezed out"],
  ["text and controls overlap", "primitives", ".l-bar[data-bar=action] .l-bar-actions{position:relative!important;left:-260px!important}", null, "overlap"],
  ["control cut off", "primitives", ".l-bar[data-bar=action] .l-bar-actions{position:relative!important;left:200px!important}", null, "control cut off"],
  ["toolbar grows", "primitives-long", ".l-toolbar{height:auto!important;flex-wrap:wrap!important}.l-z-mid .trunc{white-space:normal!important}", null, "bar height toolbar"],
  ["bar height wrong", "primitives", ".l-bar[data-bar=action]{height:70px!important}", null, "bar height action"],
  ["bar overflows", "primitives", ".l-z-end{padding-right:3000px!important}", null, "bar overflow"],
  ["controls differ in height", "primitives", ".l-bar-actions .btn:first-child{height:20px!important}", null, "differ in height"],
  ["controls not centred", "primitives", ".l-bar-actions .btn:first-child{position:relative!important;top:5px!important}", null, "not centred"],
  ["labels off baseline", "primitives", ".l-bar-actions .btn:first-child{padding-top:10px!important}", null, "off baseline"],
  ["label wraps", "primitives", ".l-bar-actions .btn{white-space:normal!important;width:42px!important}", null, "wrapped label"],
  // The owner's two reports: a label drawn above the middle of its button, and a label broken onto two lines in a box that grows.
  ["label off centre", "primitives", ".l-bar-actions .btn{align-items:flex-start!important}", null, "label off centre: button"],
  ["label wraps in a growing box", "primitives", ".l-bar-actions .btn{white-space:normal!important;width:42px!important;height:auto!important}", null, "wrapped label: button"],
  ["strip out of bounds", "primitives", ".l-strip{min-height:10px!important;max-height:20px!important;padding:0!important}", null, "strip height"],
  ["reason not visible", "primitives", ".l-reason{visibility:hidden!important}", null, "no visible reason"],
  ["composer reason popup not visible", "result-conversation", ".l-why-tip{visibility:hidden!important}", null, 'no visible reason: "Send and reopen"'],
  ["disabled primary flat", "primitives", ".btn-primary[aria-disabled=true]{background:none!important;box-shadow:none!important}", null, "lost its shape"],
  ["heading on top of the row above", "primitives", ".l-sect{margin-top:-14px!important}", null, "heading crowds"],
  ["heading off the row edge", "primitives", ".l-sect{padding-inline-start:40px!important}", null, "row edge"],
  ["region nested in a scroller", "primitives", ".l-page-body{overflow-y:scroll!important}", null, "another scroller"],
  ["more than three actions", "primitives", null, "const a=document.querySelector('.l-bar-actions');a.append(a.children[0].cloneNode(true),a.children[0].cloneNode(true),a.children[0].cloneNode(true));", "too many actions"],
  ["cut text with no way to read it", "primitives-long", null, "document.querySelectorAll('[data-whole]').forEach(e=>{e.removeAttribute('data-whole');e.removeAttribute('role');e.removeAttribute('tabindex')});", "cut with no way to read it"],
  // The title band as it was when the desktop journey found its title over the checkout chip: one crumbs box that could
  // shrink below what it held, and a band that did not measure itself. A state named with @WxH opens at that size.
  ["title over the checkout chip", "task-question-dark@960x540", ".tb-main{container-type:normal!important}.tb-main>.crumbs{display:flex!important;min-width:0!important}", null, "overlap"],
  // The queued row as the owner saw it: its file's chip squeezed in before Edit, its name running over the controls.
  ["queued row shows its file", "task-queued@960x540", null, QUEUED_CHIP, "overlap"],
  // A step's output as the owner saw it: long lines cut at the box's right edge with no way to scroll to them.
  ["output clipped sideways", "task-output-workers", ".step-sub .ws-out{overflow:hidden!important}", OPEN_STEPS + ";", "clipped sideways"],
  // The owner's 2026-11-10 reports: touch-size composer controls on a phone, a bare "?" beside Manual and Automatic, and a
  // graph menu opened half off the window.
  ["controls enlarged on a phone", "sprint-planning@390x844", ".mc .c-bar{--ctl-h:44px!important}", null, "larger than the desktop size"],
  ["help button without a background", "board-running", ".ws-hint{background:none!important;box-shadow:none!important;border:0!important}", null, "help button has no background"],
  ["popover off the window", "sprint-graph@960x540", "[popover]:popover-open{translate:600px 0!important}", "document.querySelector('[aria-label=\"Zoom presets\"]').click();", "popover outside the window"],
];

// Review states reached by pressing what the owner presses: a name, the screen, what must be ready, the steps (script, then what
// must hold after it), and the sizes. Each one is held to the full rules, a dialog in its own scope.
const press = (selector, text) => `[...document.querySelectorAll(${json(selector)})].find(e=>e.textContent.trim()===${json(text)}).click()`;
const DIALOG_OPEN = "document.querySelector('dialog[open] [data-bar=dialog]')";
const WHOLE = "document.querySelector('.rv-more').click()";
const LONG_REQUEST = "Пожалуйста, верните прежний порядок пунктов бокового меню и оставьте подписи только у групп. ".repeat(6);
const FILL = "const f=document.querySelector('dialog[open] textarea');f.focus();document.execCommand('insertText',false," + json(LONG_REQUEST) + ")";
const OPEN_HOLD = [press(".titlebar button", "See what is needed"), HOLD_CARD];
const OPEN_STALE_HOLD = [OPEN_HOLD[0], STALE_CARD];
const CLOSE_HOLD = [press(".chat-review-hold button", "Close as is…"), 'document.querySelector(\'dialog[open][data-dialog="Close task"] [data-bar=dialog]\')'];
const REPAIR_HOLD = [press(".chat-review-hold button", "Let the agent fix it"), REPAIR_SENT];
const FLOWS = [
  ["task-unknown-state-rechecked", UNKNOWN_AT, UNKNOWN_READY, [[press(".l-task .l-strip button", "Re-check state"), "document.querySelector('.l-task .l-strip-t').textContent.includes('Still not confirmed')"]], FULL],
  ["changes-whole", review_at("rv/changes"), CHANGES, [[WHOLE, "document.querySelector('.rv-sum.open')"]], OWNER],
  ["changes-git-whole", review_at("rg/changes"), CHANGES, [[WHOLE, "document.querySelector('.rv-sum.open')"]], OWNER],
  ["dialog-request-empty", review_at("rg/changes"), CHANGES, [[press(".rv-bar button", "Request changes…"), DIALOG_OPEN]], OWNER],
  ["dialog-request-filled", review_at("rg/changes"), CHANGES, [[press(".rv-bar button", "Request changes…"), DIALOG_OPEN], [FILL, "!document.querySelector('dialog[open] .btn-primary[aria-disabled]')"]], OWNER],
  ["dialog-accept-open-checks", review_at("rv/changes"), CHANGES, [[press(".rv-bar button", "Accept v1"), "document.querySelector('dialog[open][data-dialog=\"Accept with open checks\"] [data-bar=dialog]')"]], OWNER],
  ["dialog-merge", review_at("rm/changes"), READY, [[press(".ready button", "Review and merge…"), "document.querySelector('dialog[open][data-dialog=\"Finish task\"] [data-bar=dialog]')"]], OWNER],
  ["dialog-close", review_at("rn/changes"), READY, [[press(".ready button", "Close as is…"), "document.querySelector('dialog[open][data-dialog=\"Close task\"] [data-bar=dialog]')"]], OWNER],
  // Queue decisions moved from the review strip into the pinned Conversation card. Start on Changes
  // where applicable and exercise the header shortcut before opening the same close dialog.
  ["automatic-hold-close", review_at("auto-hold/changes"), HOLD_CHANGES, [OPEN_HOLD, CLOSE_HOLD], OWNER],
  ["automatic-stale-actions", stale_automatic_at, HOLD_CHANGES, [OPEN_STALE_HOLD, CLOSE_HOLD], OWNER],
  ["automatic-stale-ask-agent", stale_automatic_conversation_at, STALE_CARD, [REPAIR_HOLD], OWNER],
  ["automatic-stale-conversation-close", stale_automatic_conversation_at, STALE_CARD, [CLOSE_HOLD], OWNER],
  ["automatic-owner-hold-ask-agent", automatic_owner_hold_conversation_at, HOLD_CARD, [REPAIR_HOLD], OWNER],
  ["automatic-finish-hold-authorize", automatic_finish_hold_conversation_at, HOLD_CARD, [[press(".chat-review-hold button", "Allow Automatic to finish…"), "document.querySelector('dialog[open]')?.textContent.includes('Staging check') && document.querySelector('dialog[open]')?.textContent.includes('main')"]], [[960, 540], [1280, 800]]],
  ["sidebar-automatic-reasons-check", at("/m/<M>/p/p/board") + "&sidebarRows=1", "document.querySelector('.sidebar .tasks .task-row')", [["window.__sidebarReasons=" + SIDEBAR_REASONS + ";return 1", "window.__sidebarReasons?.ok === true"]], [[1280, 800], [960, 540]]],
  ["sidebar-active-reasons-check", at("/m/<M>/p/p/board") + "&sidebarRows=1&sidebarActive", "document.querySelector('.sb-work .task-row')", [["window.__sidebarReasons=" + SIDEBAR_REASONS + ";return 1", "window.__sidebarReasons?.ok === true"]], [[1280, 800], [960, 540]]],
  ["automatic-board-open-task", automatic_board_at, "document.querySelector('.l-strip a')", [[press(".l-strip a", "Open task"), "document.querySelector('.chat-review-hold')"]], OWNER],
];
// The other screens' second states: what a press opens, at the owner's widths, on the same long names.
const STATE_OF = Object.fromEntries(PAGES.map(([name, query, ready]) => [name, [query, ready]]));
const click_on = (selector, text) => press(selector, text);
const OPEN_TABS = "for(const n of ['canonical-exact','canonical-pricing','audit-company','check-model'])[...document.querySelectorAll('.tn')].find(e=>e.textContent.includes(n)).click()";
const FIND = "const i=document.querySelector('.ws-find-in');i.value='line';i.dispatchEvent(new Event('input',{bubbles:true}))";
FLOWS.push(
  ["git-branches", ...STATE_OF.git, [[click_on(".chg-tools button", "Branches and worktrees"), "document.querySelector('.gr')"]], OWNER],
  ["code-tabs", ...STATE_OF.code, [[OPEN_TABS, "document.querySelectorAll('.etab').length >= 4"]], OWNER],
  ["code-search", ...STATE_OF.code, [[click_on(".side-h .seg button", "Search"), "document.querySelector('.ws-find-in')"], [FIND, "document.querySelector('.res-file')"]], OWNER],
  ["palette", ...STATE_OF.home, [["document.querySelector('.ws-nav-btn').click()", "document.querySelector('dialog[open] .ws-pal-b')"]], OWNER],
  ["sprint-read-more", ...STATE_OF["sprint-plan"], [["document.querySelector('.ws-more-l').click()", "document.querySelector('.ws-read .ws-read-d')"]], OWNER],
  ["sprint-graph-full", GRAPH_AT, GRAPH_READY, [[click_on(".gv-bar button", "Full view"), "document.querySelector('.sp-body[data-full]')"]], GRAPH_SIZES],
  ["sprint-graph-details", GRAPH_AT, GRAPH_READY, [[GRAPH_PICK + ";" + click_on(".gv-band button", "Details"), "document.querySelector('.ws-read .ws-read-d')"]], GRAPH_SIZES],
  ["add-project-dialog", ...STATE_OF.home, [["[...document.querySelectorAll('.sb-foot .btn')].find(e=>e.textContent.includes('Add project')).click()", "document.querySelector('dialog[open]')"]], OWNER],
);
// The old fault in its most hidden form: a pseudo-element over the button that moves nothing. Only the reason rule may report it.
const PSEUDO_FAULT = "[aria-disabled='true']{position:relative}[aria-disabled='true']::after{content:attr(data-why);position:absolute;inset:0;overflow:hidden;white-space:nowrap;background:inherit}";
const DISABLED_BOXES = "return [...document.querySelectorAll('[aria-disabled=\"true\"][data-why]')].filter(e=>e.getClientRects().length).map(e=>{const r=e.getBoundingClientRect();return [e.textContent.trim(),r.left,r.top,r.width,r.height]})";
for (const [name, query, ready] of OUTPUT_SCREENS) FLOWS.push([name + "-open", query, ready, [[OPEN_STEPS, "document.querySelectorAll('.ws-out').length >= " + (name.includes("workers") ? "3" : "1")]], FULL]);
const STATES = Object.fromEntries(SCREENS.map(([name, query, ready]) => [name, [query, ready]]));

await main(async () => {
  const display = await startXvfb();
  const host = await Host.start({width: 1280, height: 800, env: engineEnv(display)});
  const js = code => host.js(code), wait = code => host.wait(code, 100);
  let loads = 0;

  const open_page = async (query, width, height, ready) => {
    loads++;
    await host.resize(width, height);
    await host.load(base + `?${query}&load=${loads}`);
    await wait(`window.fixture === '${loads}'`);
    await wait(ready);
    await sleep(.5);
    const actual = await js("return [innerWidth, innerHeight]");
    if (!eq(actual, [width, height])) throw Error("AssertionError: " + json([actual, width, height]));
  };
  const load = (name, width, height, ready) => open_page("route=" + quote(ROUTE[name]), width, height, ready);
  const capture = async name => {
    await sleep(.3); // Let WebKit paint the observed state before photographing it.
    const size = await host.size();
    await host.capture(join(evidence, label + "-" + name + ".png"));
    return size;
  };
  const check = async (results, name) => { const result = await js(PROBE); results[name] = result; await capture(name); return result.problems; };
  const check_strict = async (results, name) => { const result = await js(STRICT); results[name] = result; await capture(name); return result.problems; };
  // A stale locator is a hard failure, but must not discard measurements from independent screens.
  // A dead host or other infrastructure error still aborts immediately.
  const measure = async (results, failures, key, run) => {
    try { await run(); }
    catch (error) {
      if (!(error instanceof TimeoutError) && !error.message.includes("TypeError:")) throw error;
      const problem = `measurement incomplete: ${error.name}: ${error.message}`;
      (results.measurement_errors ??= {})[key] = problem;
      (failures[key] ??= []).push(problem);
      await capture(key + "-incomplete");
    }
  };
  const inject = async style => {
    await js("const s=document.createElement('style');s.id='fault';s.textContent=" + json("@layer fault{" + style + "}") + ";document.head.prepend(s);return 1");
    await sleep(.25);
  };
  const open_state = async (name, width = 1280, height = 800) => {
    if (!STATES[name]) throw Error("KeyError: " + name);
    const [query, ready] = STATES[name];
    await open_page(query, width, height, ready);
  };

  // A press on a disabled control with no slot of its own opens its whole reason beside it and moves nothing.
  const TIP = `const t=document.querySelector('.l-why-tip'),c=document.querySelector(SEL),b=c.getBoundingClientRect(),r=t.getBoundingClientRect(),bar=c.parentElement.getBoundingClientRect();
const desc=document.getElementById(c.getAttribute('aria-describedby'))?.querySelector('.l-strip-t');
return {open:t.matches(':popover-open'),text:t.textContent,why:c.dataset.why,cut:!!desc&&desc.scrollHeight>desc.clientHeight+1,box:[b.left,b.top,b.width,b.height],bar:[bar.left,bar.top,bar.width,bar.height],tip:[r.left,r.top,r.right,r.bottom],view:[innerWidth,innerHeight]}`;

  async function reason_tip(results, failures) {
    // A reason too long for its strip: the two lines there are cut, so the press has to bring the whole of it.
    // The board's Continue queue can now be enabled on this saved state. The stressed
    // primitive always supplies a disabled toolbar control with a clamped description.
    await open_state("primitives-tight-long");
    const selector = ".l-toolbar .btn[aria-disabled='true'][data-why]";
    const probeCode = sub(TIP, "SEL", json(selector));
    const before = await js(probeCode);
    await js("document.querySelector(" + json(selector) + ").click();return 1");
    await sleep(.25);
    const after = await js(probeCode);
    await capture("reason-tip");
    await js("document.body.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}));return 1");
    await sleep(.2);
    const closed = await js(probeCode);
    results.reason_tip = {before, after, closed};
    const [l, t, r, b] = after.tip, [w, h] = after.view;
    failures.reason_tip = [...(before.cut ? [] : ["the fixture reason was not clamped"]), ...(!before.open ? [] : ["open before the press"]), ...(after.open && after.text === after.why && after.why ? [] : ["the press did not open the whole reason"]),
      ...(eq(after.box, before.box) && eq(after.bar, before.bar) ? [] : ["the press moved its control"]), ...(l >= 0 && t >= 0 && r <= w && b <= h ? [] : ["the reason left the window"]), ...(!closed.open ? [] : ["did not close on the next press"])];
  }

  // What a screen must be like while the workspace loads: only the logo, centred, and nothing of the interface.
  const STARTUP = probe("layout-fit.STARTUP.js");

  // The page before any script has run: the static logo on the theme surface, exactly where the startup screen keeps it.
  async function first_paint(results, logo) {
    const paint = async theme => {
      await host.load(base + "boot.html");
      await wait("document.querySelector('#boot img')?.complete && document.querySelector('#boot img').naturalWidth > 0 && document.documentElement.dataset.theme");
      await js("localStorage.setItem('cw.theme'," + json(theme) + ");return 1");
      await host.reload();
      await sleep(.5);
      await wait("document.querySelector('#boot img')?.complete && document.querySelector('#boot img').naturalWidth > 0 && document.documentElement.dataset.theme" + (theme === "system" ? "" : " === " + json(theme)));
      await capture(`first-paint-${theme}`);
      return js("const i=document.querySelector('#boot img'),r=i.getBoundingClientRect();return {logo:[r.left,r.top,r.width,r.height],body:getComputedStyle(document.body).backgroundColor,text:document.body.textContent.trim(),others:document.querySelectorAll('body *:not(script):not(#boot):not(#boot img):not(#root)').length,desktop:matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light',theme:document.documentElement.dataset.theme}");
    };
    // The surface is the app's own theme: a choice of Dark or Light holds whatever the desktop prefers, and only System follows it.
    const found = {};
    for (const theme of ["dark", "light", "system"]) found[theme] = await paint(theme);
    await js("localStorage.removeItem('cw.theme');return 1");
    results.first_paint = found;
    const problems = [];
    const surface = {dark: "rgb(24, 27, 26)", light: "rgb(255, 255, 255)"};
    for (const [theme, want] of [["dark", surface.dark], ["light", surface.light], ["system", surface[found.system.desktop]]]) {
      const got = found[theme];
      if (!eq(got.logo, logo)) problems.push(`first paint ${theme}: logo at ${json(got.logo)}, startup screen at ${json(logo)}`);
      if (got.body !== want) problems.push(`first paint ${theme}: surface ${got.body}, want ${want}`);
      if (got.text || got.others) problems.push(`first paint ${theme}: more than the logo: ${json(got.text)} and ${got.others} other elements`);
    }
    return problems;
  }

  // The screen while the service answers: quiet, slow, stalled, failed, and the hand-over to the interface in one frame.
  async function startup_gate(results, failures) {
    const stage = async (name, text) => {
      for (let i = 0; i < 170; i++) {
        if (await js("return document.querySelector('.l-startup-slot').textContent.includes(" + json(text) + ")")) break;
        await sleep(.1);
      }
      const found = await js(STARTUP);
      await capture(`startup-${name}`);
      results[`startup_${name}`] = found;
      return [...found.problems, ...(((found.slot ?? "")).includes(text) ? [] : [`startup ${name}: no '${text}' in '${found.slot}'`])];
    };
    await open_state("startup");
    const quiet = await js(STARTUP);
    await capture("startup-quiet");
    failures.first_paint = await first_paint(results, quiet.logo);
    await open_state("startup");
    results.startup_quiet = quiet;
    failures.startup_quiet = [...quiet.problems, ...(!quiet.slot ? [] : [`words on the first frames: ${quiet.slot}`])];
    failures.startup_slow = await stage("slow", "Loading workspace");
    failures.startup_long = await stage("long", "Still waiting for the service");
    failures.startup_still = eq(results.startup_long.logo, quiet.logo) && eq(quiet.logo, results.startup_slow.logo) ? [] : ["the logo moved while waiting"];
    // Every frame is either the logo or the whole interface, never both and never neither.
    await js("window.__frames=[];const tick=()=>{window.__frames.push([!!document.querySelector('.l-startup'),!!document.querySelector('.titlebar'),!!document.querySelector('.sidebar')]);window.__tick=requestAnimationFrame(tick)};tick();window.release();return 1");
    await wait("document.querySelector('.titlebar') && window.__frames.length > 8");
    await sleep(.4);
    const frames = await js("cancelAnimationFrame(window.__tick);return window.__frames");
    results.startup_handover = {frames: frames.length, logo: frames.filter(f => f[0]).length, interface: frames.filter(f => f[1]).length};
    failures.startup_handover = [...frames.filter(f => f[0] === f[1] || f[1] !== f[2]).map(f => `a frame showed ${json(f)}`),
      ...(frames.length && frames[0][0] && frames.at(-1)[1] ? [] : [`frames ran from ${json(frames.slice(0, 1))} to ${json(frames.slice(-1))}`]),
      ...(await js("return !document.querySelector('.l-startup')") ? [] : ["the logo stayed"])];
    // A failure stays, is explained and can be retried, and a retry that works reveals the interface.
    await open_state("startup-failed");
    failures.startup_failed = await stage("failed", "could not be loaded");
    failures.startup_failed.push(...(await js("return [...document.querySelectorAll('.l-startup-slot .btn')].some(b => b.textContent.trim() === 'Try again')") ? [] : ["no Try again"]));
    await js("[...document.querySelectorAll('.l-startup-slot .btn')].find(b => b.textContent.trim() === 'Try again').click();return 1");
    await sleep(.6);
    const still = await js(STARTUP);
    failures.startup_failed.push(...still.problems, ...((still.slot ?? "").includes("could not be loaded") ? [] : ["the failure went away while still refused"]));
    await js("window.release();[...document.querySelectorAll('.l-startup-slot .btn')].find(b => b.textContent.trim() === 'Try again').click();return 1");
    await wait("document.querySelector('.titlebar') && !document.querySelector('.l-startup')");
    // The rules above have to be able to fail.
    results.startup_faults = {};
    failures.startup_faults = [];
    for (const [fault, script, expect] of [["interface mounted", "document.body.append(Object.assign(document.createElement('div'),{className:'titlebar'}))", "partial interface"], ["logo off centre", "document.querySelector('.l-startup-logo').style.marginTop='60px'", "off centre"], ["controls missing", "document.querySelector('.l-startup-band .win').remove()", "window controls missing"]]) {
      await open_state("startup");
      await js(script + ";return 1");
      await sleep(.2);
      const found = (await js(STARTUP)).problems;
      results.startup_faults[fault] = found;
      if (!found.some(p => p.includes(expect))) failures.startup_faults.push(`not reported: ${fault}`);
    }
  }

  // The board keeps its height, and the end of its toolbar stays put however the queue's state changes.
  const QUEUE_END = "return [...document.querySelectorAll('[aria-label=\"Sprint queue\"] .l-z-end > *')].filter(e=>e.getClientRects().length).map(e=>{const r=e.getBoundingClientRect();return [Math.round(r.left*2)/2,Math.round(r.top*2)/2,Math.round(r.width*2)/2,Math.round(r.height*2)/2]})";
  const BUDGET = "const b=document.querySelector('.board-wrap').getBoundingClientRect();return {top:b.top,height:b.height}";
  const budget_problems = (found, where) => [...(found.top <= 220.5 ? [] : [`chrome above the board in ${where}: ${found.top}`]), ...(found.height >= 300 ? [] : [`board too short in ${where}: ${found.height}`])];

  async function board_gate(results, failures) {
    for (const [name, query, ready] of SCREENS) {
      if (!name.startsWith("board-")) continue;
      await open_page(query, 1280, 800, ready);
      const found = await js(BUDGET);
      results[`budget_${name}`] = found;
      failures[`budget_${name}`] = budget_problems(found, name);
      results[`end_${name}`] = await js(QUEUE_END);
    }
    const ends = Object.fromEntries(["board-paused", "board-running", "board-blocked", "board-idle"].map(name => [name, results[`end_${name}`]]));
    failures.queue_end_stable = Object.values(ends).every(e => eq(e, ends["board-paused"]) && e.length >= 3) ? [] : [json(ends)];
    await open_state("board-paused");
    await inject(".l-page-status{padding-top:220px!important}");
    const found = await js(BUDGET);
    failures.budget_fault = budget_problems(found, "fault").length ? [] : ["not reported: chrome over budget"];
    // The tab row folds by the pane's own width, so a wide window with a wide sidebar must fold as well.
    await open_state("board-paused", 1024, 768);
    failures.areas_fold = await js("return getComputedStyle(document.querySelector('.pstrip [aria-label=\"More project areas\"]')).display !== 'none'") ? [] : ["no menu for the folded areas at 1024"];
    // The explanation is one press or one key from the queue's mode, and no longer takes a row.
    await open_state("board-paused");
    const hint = await js("return {strip:[...document.querySelectorAll('.l-strip')].some(s=>s.textContent.includes('Tasks go to Done on their own in the current branch')),review:document.body.textContent.includes('Review this task'),button:!!document.querySelector('.l-z-start .ws-hint')}");
    results.hint = hint;
    await js("document.querySelector('.l-z-start .ws-hint').focus();document.querySelector('.l-z-start .ws-hint').click();return 1");
    await sleep(.3);
    const opened = await js("return [...document.querySelectorAll('.ws-hint-pop')].some(p=>p.matches(':popover-open') && p.textContent.includes('Tasks go to Done on their own in the current branch'))");
    await capture("board-hint-open");
    failures.hint = [...(!hint.strip ? [] : ["the explanation still takes a strip"]), ...(!hint.review ? [] : ["the banner still says Review this task"]), ...(hint.button ? [] : ["no ? control"]), ...(opened ? [] : ["the ? did not open the explanation"])];
  }

  // A target answers a press 11 px out from its centre on every side (half a pixel short of a 24 px edge, for sub-pixel rounding next
  // to a neighbour), so its 24 px square or circle is its own (WCAG 2.5.8). The ? does it with a hit area around its 16 px box.
  const TARGETS = `return [...document.querySelectorAll(SEL)].filter(e=>e.getClientRects().length).map(el=>{const r=el.getBoundingClientRect(),x=r.left+r.width/2,y=r.top+r.height/2,s=getComputedStyle(el);
const hits=[[0,-11],[11,0],[0,11],[-11,0]].map(([dx,dy])=>{const h=document.elementFromPoint(x+dx,y+dy);return !!h&&(h===el||el.contains(h))});
return {label:(el.getAttribute('aria-label')||el.textContent).trim().slice(0,32),w:r.width,h:r.height,look:[parseFloat(s.width)-parseFloat(s.paddingLeft)-parseFloat(s.paddingRight),s.backgroundClip],hits}})`;
  // The quiet line on whatever has focus now: its outline, and whether a placed focus marks it.
  const FOCUS_LINE = "const a=document.activeElement,s=getComputedStyle(a);return {label:(a.getAttribute('aria-label')||a.textContent).trim().slice(0,32),style:s.outlineStyle,width:s.outlineWidth,placed:a.hasAttribute('data-placed-focus'),keyboard:document.documentElement.hasAttribute('data-keyboard')}";
  const POINTER_PRESS = "const e=SEL;e.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}));e.focus();";
  const KEY = key => `document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:${json(key)},bubbles:true,cancelable:true}));return 1`;
  const TIP_STATE = "const t=document.querySelector('.l-why-tip');return {open:t.matches(':popover-open'),text:t.textContent}";

  // Keyboard reach of cut text, the focus line a dialog or a panel places, press targets and the popovers' roles.
  async function keys_gate(results, failures) {
    // Enter on a cut path and Space on a cut name in a panel open the whole text, as a press does; Escape closes it.
    await open_state("changes-git");
    const path = await js("const e=[...document.querySelectorAll('.fhead .l-whole[role=button]')].find(e=>e.scrollWidth>e.clientWidth+1);if(!e)return null;e.focus();return {whole:e.dataset.whole,focused:document.activeElement===e}");
    const before = await js(TIP_STATE);
    await js(KEY("Enter"));
    await sleep(.3);
    const entered = await js(TIP_STATE);
    await capture("keys-reveal-enter");
    await js(KEY("Escape"));
    await sleep(.2);
    const escaped = await js(TIP_STATE);
    await open_state("git");
    const name = await js("const e=[...document.querySelectorAll('[data-whole][role=button]:not(.l-whole):not(.l-reason):not(.l-strip-t)')].find(e=>e.scrollWidth>e.clientWidth+1);if(!e)return null;e.focus();return {text:e.textContent.trim(),focused:document.activeElement===e}");
    await js(KEY(" "));
    await sleep(.3);
    const spaced = await js(TIP_STATE);
    await capture("keys-reveal-space");
    results.keys_reveal = {path, before, enter: entered, escape: escaped, name, space: spaced};
    failures.keys_reveal = [...(path && path.focused && !before.open ? [] : [`no focusable cut path, or a script focus opened it: ${json(path)} ${json(before)}`]),
      ...(path && entered.open && entered.text === path.whole ? [] : [`Enter did not open the whole path: ${json(entered)}`]), ...(!escaped.open ? [] : ["Escape left it open"]),
      ...(name && name.focused && spaced.open && spaced.text === name.text ? [] : [`Space did not open a cut name in a panel: ${json(name)} ${json(spaced)}`])];
    // A dialog opened by a pointer press: the press draws nothing, the control the dialog focuses carries the line, a press clears it.
    await open_state("changes-no-git");
    const trigger = "[...document.querySelectorAll('.rv-bar button')].find(e=>e.textContent.trim()==='Accept v1')";
    const pressed = await js(sub(POINTER_PRESS, "SEL", trigger) + FOCUS_LINE);
    await js(trigger + ".click();return 1");
    await wait("document.querySelector('dialog[open][data-dialog=\"Accept with open checks\"] [data-bar=dialog]')");
    await sleep(.3);
    const placed = await js(FOCUS_LINE);
    await capture("keys-dialog-focus");
    const cleared = await js(sub(sub(POINTER_PRESS, "SEL", "document.querySelector('dialog[open] .dlg-b')||document.querySelector('dialog[open]')"), "e.focus();", "") + FOCUS_LINE);
    // A panel that opens with its close control focused does the same.
    await open_state("sprint-plan");
    await js(sub(POINTER_PRESS, "SEL", "document.querySelector('.ws-more-l')") + "e.click();return 1");
    await wait("document.querySelector('.ws-read') && document.activeElement && document.activeElement.getAttribute('aria-label')==='Close task details'");
    await sleep(.3);
    const panel = await js(FOCUS_LINE);
    await capture("keys-panel-focus");
    results.keys_focus = {pressed, placed, cleared, panel};
    const line = f => !["none", ""].includes(f.style) && f.width === "1px";
    failures.keys_focus = [...(!line(pressed) && !pressed.keyboard ? [] : [`a pointer press drew a line: ${json(pressed)}`]), ...(line(placed) && placed.placed ? [] : [`the dialog's focused control has no line: ${json(placed)}`]),
      ...(!line(cleared) && !cleared.placed ? [] : [`the next press kept the line: ${json(cleared)}`]), ...(line(panel) && panel.placed ? [] : [`the panel's focused control has no line: ${json(panel)}`])];
    // Press targets: the queue's ? is a filled 28 px control like the Manual/Automatic switch beside it (owner, 2026-11-10), in the
    // 44 px toolbar; a changed file's actions keep their 22 px look.
    await open_state("board-paused");
    const hint = await js(sub(TARGETS, "SEL", json(".l-z-start .ws-hint")));
    const bar = await js("return document.querySelector('[aria-label=\"Sprint queue\"]').getBoundingClientRect().height");
    await capture("keys-hint-target");
    const pops = await js("return [...document.querySelectorAll('[popover]:not(.l-why-tip)')].map(p=>[p.id,p.getAttribute('role'),p.getAttribute('aria-label'),p.querySelector('[role=menuitem]')?'menu':'dialog'])");
    await open_state("git");
    await js("document.querySelectorAll('.gf .acts').forEach(a=>a.classList.add('on'));return 1");
    const files = await js(sub(TARGETS, "SEL", json(".gf .ib")));
    await capture("keys-git-targets");
    results.keys_targets = {hint, bar, files: files.slice(0, 6), file_count: files.length, popovers: pops};
    const small = found => found.filter(t => t.w < 24 || t.h < 24 || !t.hits.every(Boolean));
    failures.keys_targets = [...(hint.length && hint.every(t => t.hits.every(Boolean) && t.w === 28 && t.h === 28) ? [] : [`the ? target: ${json(hint)}`]), ...(Math.abs(bar - 44) <= .5 ? [] : [`the queue toolbar is ${bar} px`]),
      ...(files.length && !small(files).length && files.every(t => eq(t.look, [22, "content-box"])) ? [] : [`changed-file actions: ${json(small(files).length ? small(files).slice(0, 3) : files.slice(0, 2))}`])];
    failures.popover_roles = [...(pops.length ? [] : ["no popovers on the board"]), ...pops.filter(p => p[1] !== p[3]).map(p => `popover ${p[0]} has role ${p[1]}, want ${p[3]}`), ...pops.filter(p => p[2] && !["menu", "dialog"].includes(p[1])).map(p => `popover ${p[0]} label not exposed`)];
  }

  // What the owner's screens showed wrong: the comment bar where nothing can be commented, an empty panel and a sentence said twice,
  // a void above the bottom bar, Accept and Request changes out of line, headings on top of the counts above them.
  const REVIEW_FACTS = probe("layout-fit.REVIEW_FACTS.js");
  const review_problems = (found, where, git) => {
    const out = [];
    if (git) {
      out.push(...(found.foot ? [] : [`${where}: no comment bar`]));
      out.push(...(found.body && found.footTop !== null && Math.abs(found.body[0] - found.footTop) <= 1 ? [] : [`${where}: space between the diff and the bottom bar: ${json(found.body)} ${found.footTop}`]));
      out.push(...(found.body && found.body[1] >= 239.5 ? [] : [`${where}: diff squeezed below 240px: ${json(found.body)}`]));
    } else {
      out.push(...(!found.foot ? [] : [`${where}: comment bar without Git`]));
      out.push(...(found.applied === 1 ? [] : [`${where}: the no-Git sentence said ${found.applied} times`]));
    }
    out.push(...(found.notePanels === 0 || found.notePanels === null ? [] : [`${where}: an empty panel takes the page`]));
    const r1 = n => Math.round(n * 10) / 10;
    const tops = new Set(found.acts.map(a => r1(a[1]))), heights = new Set(found.acts.map(a => r1(a[2])));
    out.push(...(found.acts.length >= 2 && tops.size === 1 && heights.size === 1 ? [] : [`${where}: Request changes and Accept not in one row: ${json(found.acts)}`]));
    out.push(...(found.evidenceGap === null || found.evidenceGap >= 4 ? [] : [`${where}: Check manually crowds Evidence: ${found.evidenceGap}`]));
    out.push(...(!found.rawHeadings ? [] : [`${where}: ${found.rawHeadings} unstyled headings`]));
    return out;
  };

  // Every FLOWS entry at its sizes: open the screen, press what the owner presses, hold the result to the full rules.
  async function flow_gate(results, failures) {
    for (const [name, query, ready, steps, sizes] of FLOWS) {
      for (const [width, height] of sizes) {
        const key = `${name}-${width}x${height}`;
        await measure(results, failures, key, async () => {
          await open_page(query, width, height, ready);
          for (const [script, after] of steps) { await js(script + ";return 1"); await wait(after); await sleep(.3); }
          failures[key] = await check_strict(results, key);
        });
      }
    }
  }

  async function review_gate(results, failures) {
    for (const [width, height] of OWNER) {
      for (const [name, git] of [["changes-no-git", false], ["changes-git", true], ["changes-whole", false], ["changes-git-whole", true]]) {
        const [query, ready] = STATES[name] ?? FLOWS.filter(f => f[0] === name).map(f => [f[1], f[2]])[0];
        await open_page(query, width, height, ready);
        if (name.endsWith("whole")) {
          await js(WHOLE + ";return 1");
          await wait("document.querySelector('.rv-sum.open')");
          await sleep(.3);
        }
        const found = await js(REVIEW_FACTS);
        results[`review-${name}-${width}`] = found;
        failures[`review-${name}-${width}`] = review_problems(found, `${name} ${width}`, git);
      }
      await open_state("result-conversation", width, height);
      const found = await js(REVIEW_FACTS);
      results[`review-result-${width}`] = found;
      failures[`review-result-${width}`] = [...(found.cardGap !== null && found.cardGap >= 4 ? [] : [`Check manually crowds the counts: ${found.cardGap}`]), ...(!found.rawHeadings ? [] : ["unstyled headings"])];
    }
    // The diff keeps its room and meets the bottom bar: a summary allowed to take the whole height must be caught.
    await open_state("changes-git");
    await js(WHOLE + ";return 1");
    await inject(".rv-top{max-height:none!important;flex:none!important}");
    failures.review_fault = review_problems(await js(REVIEW_FACTS), "fault", true).length ? [] : ["not reported: the summary took the diff's room"];
    // Cut text: a press on a cut path shows the whole of it, a press on a cut check name opens its row.
    await open_state("changes-git");
    const path = await js("const e=[...document.querySelectorAll('.fhead .l-whole')].find(e=>e.scrollWidth>e.clientWidth+1);if(!e)return null;const tip=document.querySelector('.l-why-tip');const before=tip.matches(':popover-open');e.click();return {whole:e.dataset.whole,role:e.getAttribute('role'),before}");
    await sleep(.3);
    const tip = await js("const t=document.querySelector('.l-why-tip');return {open:t.matches(':popover-open'),text:t.textContent}");
    await capture("reveal-path");
    await js("document.body.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}));return 1");
    const row = await js("const t=[...document.querySelectorAll('.rv-checks .l-row-t[role=button]')][0];if(!t)return null;const h=t.closest('.l-row').getBoundingClientRect().height;t.click();return {h}");
    await sleep(.3);
    const opened = await js("const t=document.querySelector('.l-row[data-open] .l-row-t');return t?{cut:t.scrollWidth>t.clientWidth+1,h:t.closest('.l-row').getBoundingClientRect().height}:null");
    await capture("reveal-row");
    results.reveal = {path, tip, row, opened};
    failures.reveal = [...(path && path.role === "button" && !path.before ? [] : [`no cut path to press: ${json(path)}`]), ...(path && tip.open && tip.text === path.whole ? [] : [`the press did not show the whole path: ${json(tip)}`]),
      ...(row && opened && !opened.cut && opened.h > row.h ? [] : [`a cut check name did not open: ${json(row)} ${json(opened)}`])];
    // Empty Send and reopen explains itself on press; once enabled it stops offering
    // that reason. Neither state moves the controls relative to their row.
    await open_state("changes-git");
    await js(press(".rv-bar button", "Request changes…") + ";return 1");
    await wait(DIALOG_OPEN);
    await sleep(.3);
    // Measured within the composer's tools row: the field above grows with its text, so the dialog may grow, but the row must not change.
    const boxes = "const bar=document.querySelector('dialog[open] .mc .c-bar'),f=bar.getBoundingClientRect();return {bar:[f.width,f.height],boxes:[...bar.querySelectorAll('.btn, .ib')].map(b=>{const r=b.getBoundingClientRect();return [(b.getAttribute('aria-label')||b.textContent).trim(),r.left-f.left,r.top-f.top,r.width,r.height,b.getAttribute('aria-disabled'),b.dataset.why??null]})}";
    const before = await js(boxes);
    await js("document.querySelector('dialog[open] .c-send').click();return 1");
    await wait("document.querySelector('.l-why-tip:popover-open')");
    const sendTip = await js(TIP_STATE);
    await js("document.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}));return 1");
    await js(FILL + ";return 1");
    await sleep(.3);
    const after = await js(boxes);
    results.dialog_flip = {before, after, tip: sendTip};
    const moved = before.boxes.filter((b, i) => !eq(b.slice(0, 5), after.boxes[i]?.slice(0, 5)));
    failures.dialog_flip = [...(sendTip.open && sendTip.text === "Say what should change" ? [] : [`empty: reason says ${json(sendTip)}`]), ...(before.boxes.some(b => b[0] === "Send and reopen" && b[5] === "true") ? [] : ["Send and reopen is not a disabled button"]),
      ...(after.boxes.some(b => b[0] === "Send and reopen" && b[5] === null && b[6] === null) ? [] : ["Send and reopen did not enable and clear its reason"]), ...moved.map(m => `moved: ${json(m)}`), ...(after.boxes.length === before.boxes.length ? [] : ["the controls changed number"]), ...(eq(after.bar, before.bar) ? [] : [`the footer changed size: ${json(before.bar)} ${json(after.bar)}`])];
  }

  // Dark and light differ in colour only: every visible box must sit at the same place, at the same size, in both.
  const BOXES = "return [...document.querySelectorAll('body *')].filter(e=>e.getClientRects().length && !e.closest('svg') && getComputedStyle(e).visibility!=='hidden').map(e=>{const r=e.getBoundingClientRect();return e.tagName+'.'+String(e.className).split(' ').slice(0,2).join('.')+' '+[r.left,r.top,r.width,r.height].map(n=>Math.round(n*2)/2).join(',')})";
  async function theme_gate(results, failures) {
    for (const [name, query, ready] of PAGES) {
      const boxes = {};
      // Back and Forward in the title band follow the view's own history, so both themes are measured after a load of their own.
      await open_page(query, 1280, 800, ready);
      for (const [theme, q] of [["dark", query + "&dark=1"], ["light", query]]) {
        await open_page(q, 1280, 800, ready);
        boxes[theme] = await js(BOXES);
      }
      const moved = boxes.light.flatMap((a, i) => i < boxes.dark.length && a !== boxes.dark[i] ? [`${a} | ${boxes.dark[i]}`] : []);
      results[`theme-${name}`] = {boxes: boxes.light.length, moved: moved.slice(0, 5)};
      failures[`theme-${name}`] = [...(boxes.light.length === boxes.dark.length ? [] : [`${boxes.light.length} boxes in light, ${boxes.dark.length} in dark`]), ...moved.slice(0, 5)];
    }
  }

  // The full rules on the title band alone.
  const TITLE_STRICT = STRICT.replace(/^const scope = .+;$/m, "const scope = document.querySelector('.titlebar');");
  if (TITLE_STRICT === STRICT) throw Error("AssertionError: the title band scope was not found in the strict probe");

  // The pinned owner card above the message box: what the owner sees for each kind of hold, at the usual and the smallest window.
  // Rules: seen without scrolling and above the composer, buttons whole and unclipped, no text running out of its box, no sideways
  // scroll, nothing like a code name in the words, and the card there even while the result is not loaded.
  const OWNER_CARD = `const kind=KIND,want=WANT,problems=[];const vw=innerWidth,vh=innerHeight;
const card=document.querySelector(kind==='quiet'?'.ws-hold-quiet':'.chat-review-hold');
if(!card)return {problems:['the card is not on the page']};
const r=card.getBoundingClientRect(),dock=document.querySelector('.dock');
if(!(r.width>0&&r.height>0))problems.push('the card has no size');
if(r.top<0||r.bottom>vh||r.left<0||r.right>vw)problems.push('the card is not whole in the window without scrolling '+JSON.stringify([r.left,r.top,r.right,r.bottom,vw,vh]));
if(!dock)problems.push('no message box');else{
 const d=dock.getBoundingClientRect(),feed=document.querySelector('.timeline').getBoundingClientRect(),composer=dock.querySelector('.composer');
 if(Math.abs(feed.bottom-d.top)>1)problems.push('the feed does not end at the dock '+JSON.stringify([feed.bottom,d.top]));
 if(!composer||r.bottom>composer.getBoundingClientRect().top+1)problems.push('the card is not above the message box');
 if(!(d.top>=0&&d.bottom<=vh+1))problems.push('the dock is outside the window');
 const ta=dock.querySelector('textarea,[contenteditable]');if(COMPOSER&&(!ta||ta.disabled||ta.getAttribute('aria-disabled')==='true'))problems.push('the message box is not enabled');}
const buttons=[...card.querySelectorAll('button,a.btn,a[role=button]')];
const names=buttons.map(b=>b.textContent.trim());
if(JSON.stringify(names)!==JSON.stringify(want))problems.push('buttons are '+JSON.stringify(names)+', wanted '+JSON.stringify(want));
for(const b of buttons){const q=b.getBoundingClientRect();
 if(!(q.width>0&&q.height>0))problems.push('button has no size: '+b.textContent);
 if(q.left<r.left-.5||q.right>r.right+.5||q.top<r.top-.5||q.bottom>r.bottom+.5)problems.push('button leaves the card: '+b.textContent);
 if(q.left<0||q.right>vw||q.top<0||q.bottom>vh)problems.push('button leaves the window: '+b.textContent);
 if(b.scrollWidth>b.clientWidth+1||b.scrollHeight>b.clientHeight+1)problems.push('button text is clipped: '+b.textContent);
 const at=document.elementFromPoint(q.left+q.width/2,q.top+q.height/2);if(!b.contains(at))problems.push('button is covered: '+b.textContent);}
for(const e of [card,...card.querySelectorAll('*')]){if(e.scrollWidth>e.clientWidth+1&&getComputedStyle(e).display!=='inline'&&e.clientWidth>0)problems.push('text runs out of its box: '+e.tagName+'.'+e.className+' '+JSON.stringify(e.textContent.slice(0,40)));}
const se=document.scrollingElement;if(se.scrollWidth>se.clientWidth+1)problems.push('the page scrolls sideways');
for(const e of document.querySelectorAll('.main,.chat,.l-task'))if(e.scrollWidth>e.clientWidth+1)problems.push('sideways scroll in '+e.className);
const text=card.innerText+' '+[...document.querySelectorAll('.l-task button')].map(b=>b.textContent).join(' ');
const bad=text.match(/\\b[A-Za-z]+_[A-Za-z_]+\\b/g);if(bad)problems.push('code-like words: '+bad.join(', '));
const seeBtn=[...document.querySelectorAll('button')].filter(b=>b.textContent.trim()==='See what is needed');
if(kind==='quiet'&&seeBtn.length)problems.push('the header asks the owner to look although nothing is needed');
if(kind!=='quiet'&&HEADER&&seeBtn.length!==1)problems.push('the header button See what is needed is missing');
return {problems,names,card:[r.left,r.top,r.right,r.bottom],text:card.innerText}`;
  const OWNER_CARDS = [
    ["allow", "oh-allow", "card", ["Allow Automatic to finish…", "Close as is…"], true, true],
    ["allow-nores", "oh-allow-nores", "card", ["Allow Automatic to finish…", "Close as is…"], true, true],
    ["repair", "oh-repair", "card", ["Let the agent fix it", "Close as is…"], true, true],
    ["unknown", "oh-unknown", "card", ["Close as is…"], true, true],
    ["agent", "oh-agent", "quiet", [], true, false],
    ["manual", "oh-manual", "card", ["Ask the agent to try again", "Close as is…"], true, true],
  ];
  async function owner_card_gate(results, failures, wanted = () => true, narrow = sizes => sizes) {
    for (const [name, task, kind, buttons, composer, header] of OWNER_CARDS) {
      for (const [width, height] of wanted("owner-card-" + name) ? narrow([[1280, 800], MINIMUM]) : []) {
        const key = `owner-card-${name}-${width}x${height}`;
        await open_page(review_at(task), width, height, kind === "quiet" ? "document.querySelector('.ws-hold-quiet')" : "document.querySelector('.chat-review-hold button')");
        const result = await js(sub(sub(sub(sub(OWNER_CARD, "KIND", json(kind)), "WANT", json(buttons)), "COMPOSER", json(composer)), "HEADER", json(header)));
        results[key] = result;
        const strict = await check_strict(results, key);
        results[key] = {...result, strict: results[key]};
        failures[key] = [...result.problems, ...strict];
      }
    }
    // The header's See what is needed from the Changes tab brings the owner back to the card.
    for (const [width, height] of wanted("owner-card-header") ? narrow([[1280, 800], MINIMUM]) : []) {
      const key = `owner-card-header-${width}x${height}`;
      await open_page(review_at("oh-allow/changes"), width, height, "[...document.querySelectorAll('button')].some(b=>b.textContent.trim()==='See what is needed')");
      await js(press("button", "See what is needed") + ";return 1");
      await wait("document.querySelector('.chat-review-hold button')");
      await sleep(.4);
      const result = await js(sub(sub(sub(sub(OWNER_CARD, "KIND", json("card")), "WANT", json(OWNER_CARDS[0][3])), "COMPOSER", "true"), "HEADER", "true"));
      results[key] = result;
      await capture(key);
      failures[key] = result.problems;
    }
    // Moving away from the end makes Latest appear. Its opaque band may cover the feed only, never the pinned decision.
    for (const [width, height] of wanted("owner-card-latest") ? narrow([MINIMUM, [1024, 768], [1280, 800]]) : []) {
      const key = `owner-card-latest-${width}x${height}`;
      await open_page(stale_automatic_conversation_at, width, height, STALE_CARD);
      await js("const feed=document.querySelector('.timeline');feed.dispatchEvent(new WheelEvent('wheel',{deltaY:-80}));feed.scrollTop=0;return 1");
      await wait("document.querySelector('.dock > .pill-band .new-pill')");
      const overlap = await js("const pill=document.querySelector('.pill-band').getBoundingClientRect(),card=document.querySelector('.chat-review-hold').getBoundingClientRect(); return {pillBottom:pill.bottom,cardTop:card.top}");
      const strict = await check_strict(results, key);
      results[key].latest = overlap;
      failures[key] = [...strict, ...(overlap.pillBottom <= overlap.cardTop + 1 ? [] : ["Latest covers the owner decision: " + json(overlap)])];
    }
  }
  // Every page's title band at the narrow sizes, with the window's own controls: nothing overlaps, is cut off or runs out of it.
  async function title_gate(results, failures, wanted = () => true, narrow = sizes => sizes) {
    for (const [name, query, ready] of TITLE_BAND) {
      for (const [width, height] of wanted("title-" + name) ? narrow(NARROW) : []) {
        const key = `title-${name}-${width}x${height}`;
        await open_page(query, width, height, ready);
        const result = await js(TITLE_STRICT);
        results[key] = result;
        await capture(key);
        failures[key] = [...result.problems, ...(await js("return document.querySelectorAll('.titlebar .win[role=group] button').length") === 3 ? [] : ["window controls missing from the title band"])];
      }
    }
  }

  // The queued row: one line of text that stops before its own controls, which keep their box, and no file shown in it.
  const QUEUE_ROW = probe("layout-fit.QUEUE_ROW.js");
  async function queue_gate(results, failures, narrow = sizes => sizes) {
    for (const [width, height] of narrow(FULL)) {
      const key = `queue-row-${width}x${height}`;
      await open_state("task-queued", width, height);
      results[key] = await js(QUEUE_ROW);
      failures[key] = results[key].problems;
      await capture(key);
    }
    await open_state("task-queued", 960, 540);
    await js(QUEUED_CHIP + "return 1");
    await sleep(.2);
    const found = (await js(QUEUE_ROW)).problems;
    await capture("queue-row-fault");
    results.queue_row_fault = found;
    failures.queue_row_fault = found.some(p => p.includes("file is shown")) && found.some(p => p.includes("runs into")) ? [] : ["the old queued row was not reported: " + json(found)];
  }

  // Each opened output box: wider content than its box, which scrolls sideways when moved, takes keyboard focus, keeps a name, and
  // stays inside its column, which itself never scrolls sideways; nor does the page.
  const OUTPUT_BOX = probe("layout-fit.OUTPUT_BOX.js");
  async function output_gate(results, failures, narrow = sizes => sizes) {
    for (const [name, query, ready] of OUTPUT_SCREENS) {
      for (const [width, height] of narrow(FULL)) {
        const key = `${name}-scroll-${width}x${height}`;
        await open_page(query, width, height, ready);
        await js(OPEN_STEPS + ";return 1");
        await wait("document.querySelector('.ws-out')");
        await sleep(.3);
        results[key] = await js(OUTPUT_BOX);
        failures[key] = results[key].problems;
        // The capture shows the first box moved sideways, so the end of its long lines is on screen.
        await js("const b=document.querySelector('.ws-out');b.scrollIntoView({block:'nearest'});b.scrollLeft=b.scrollWidth;return 1");
        await capture(key);
      }
    }
  }

  // Sizes and edges at the phone width and the window minimum: the compact desktop controls, filled help buttons and menus kept
  // inside the window. Only these rules apply here; the full rules hold the same pages from 960 px up.
  const FIT_RULES = ["larger than the desktop size", "help button has no background", "popover outside the window"];
  const FIT_PAGES = ["sprint-planning", "sprint-plan", "sprint-chat-created", "task-question", "board-running", "new-task", "new-sprint", "orchestration-routing", "settings-voice"];
  const FIT_SIZES = [[390, 844], [960, 540]];
  const MORE_LAYERS = "document.querySelector('[aria-label=\"More graph controls\"]').click()";
  const PICK_LAYERS = "[...document.querySelectorAll('[popover]:popover-open .mi')].find(e=>e.textContent.trim()==='Layers').click()";
  // The owner's menu: Layers opened from the graph's overflow menu, at the widths where the toolbar folds it there.
  const FIT_FLOWS = [["sprint-graph-layers", GRAPH_AT, GRAPH_READY, [[MORE_LAYERS, "document.querySelector('[popover]:popover-open .mi')"], [PICK_LAYERS, "document.querySelector('.gv-layers:popover-open')"]], [[960, 540], [1024, 768]]]];
  async function fit_gate(results, failures, wanted = () => true) {
    for (const name of FIT_PAGES) {
      if (!wanted(name)) continue;
      for (const [width, height] of FIT_SIZES) {
        const key = `fit-${name}-${width}x${height}`;
        await open_state(name, width, height);
        const found = await js(STRICT);
        results[key] = {problems: found.problems};
        failures[key] = found.problems.filter(p => FIT_RULES.some(rule => p.includes(rule)));
        await capture(key);
      }
    }
    for (const [name, query, ready, steps, sizes] of FIT_FLOWS) {
      if (!wanted(name)) continue;
      for (const [width, height] of sizes) {
        const key = `${name}-${width}x${height}`;
        await open_page(query, width, height, ready);
        for (const [script, after] of steps) { await js(script + ";return 1"); await wait(after); await sleep(.3); }
        // An open menu lies over the page on purpose; it is held to the edge rules, not to the overlap rule.
        const found = await check_strict(results, key);
        failures[key] = found.filter(p => [...FIT_RULES, "outside viewport"].some(rule => p.includes(rule)));
        const box = await js("const r=document.querySelector('.gv-layers:popover-open').getBoundingClientRect();return [r.left,r.top,r.right,r.bottom]");
        results[key].layers = box;
        if (box[0] < 0 || box[2] > width || box[3] > height) failures[key].push(`layers menu outside the window: ${json(box)}`);
      }
    }
  }

  // Every SCREENS entry, every title band at the narrow sizes, the disabled-to-enabled flip, the pseudo-element guard and the fault injections.
  async function strict_gate(results, failures) {
    for (const [name, query, ready, sizes] of SCREENS) {
      for (const [width, height] of sizes) {
        const key = `${name}-${width}x${height}`;
        await measure(results, failures, key, async () => {
          await open_page(query, width, height, ready);
          failures[key] = await check_strict(results, key);
        });
      }
    }
    await measure(results, failures, "title-gate", () => title_gate(results, failures));
    await measure(results, failures, "owner-card-gate", () => owner_card_gate(results, failures));
    // Enabling a control changes the words in its bar and nothing else.
    await open_state("primitives");
    const before = await js(STRICT);
    await js("window.flip();return 1");
    await sleep(.3);
    const after = await js(STRICT);
    const compared = Object.keys(before.stable).filter(k => k in after.stable);
    const moved = Object.fromEntries(compared.filter(k => !eq(before.stable[k], after.stable[k])).map(k => [k, [before.stable[k], after.stable[k]]]));
    results.flip = {compared: compared.length, moved, slotsBefore: before.slots, slotsAfter: after.slots};
    failures.flip = [...(compared.length ? [] : ["nothing to compare"]), ...Object.keys(moved).map(k => `moved when enabled: ${k}`), ...(!eq(before.slots, after.slots) ? [] : ["the reason slot did not change"]), ...after.problems];
    for (const [name, gate] of [
      ["reason-tip", reason_tip], ["themes", theme_gate], ["flows", flow_gate], ["review", review_gate],
      ["board", board_gate], ["startup", startup_gate], ["keys", keys_gate], ["queue", queue_gate],
      ["output", output_gate], ["fit", fit_gate],
    ]) await measure(results, failures, name + "-gate", () => gate(results, failures));
    // A reason drawn over the button by a pseudo-element moves nothing, so only the reason rule can see it.
    await open_state("primitives");
    const boxes = await js(DISABLED_BOXES);
    await inject(PSEUDO_FAULT);
    const found = (await js(STRICT)).problems;
    const expected = unique(boxes.map(b => `reason drawn inside control: "${b[0] || "More"}"`)).sort();
    const again = await js(DISABLED_BOXES);
    results.pseudo_guard = {controls: boxes.length, unchanged: eq(boxes, again), expected, found};
    failures.pseudo_guard = boxes.length && eq(boxes, again) && eq([...found].sort(), expected) ? [] : [json(results.pseudo_guard)];
    // Every rule above must report its own fault.
    results.faults = {}; failures.faults = [];
    for (const [fault, state, style, script, expect] of FAULTS) {
      const [named, , size] = state.split(/(@)/);
      await measure(results, failures, "fault-" + fault.replaceAll(" ", "-"), async () => {
        await open_state(named, ...(size ? size.split("x").map(Number) : []));
        if (style) await inject(style);
        if (script) { await js(script + "return 1"); await sleep(.2); }
        const problems = (await js(STRICT)).problems;
        const hits = problems.filter(p => p.includes(expect));
        results.faults[fault] = {state, reported: problems.length, matching: hits.length, first: hits.slice(0, 2)};
        if (!hits.length) failures.faults.push(`not reported: ${fault} (expected '${expect}', got ${json(problems.slice(0, 3))})`);
      });
    }
  }

  // A field that grows with its text: real editing commands, at a usual and at a short window. The controls must stay inside the window throughout.
  // A name, the page, what must be ready, the press that opens the field (or None), the field, its controls and the feed under it.
  const grow_route = route => "route=" + quote(route);
  const GROW = [
    ["new-task", grow_route("/m/<M>/new?kind=task&project=p"), null, null, ".create .composer textarea", ".create-foot .btn, .create .c-bar .ib", null],
    ["task", grow_route("/m/<M>/t/r-now"), null, null, ".dock .composer textarea:not(:disabled)", ".dock .c-bar .ib, .dock .c-bar .c-send", ".timeline"],
    // The planning chat is its own tab, with the whole width at every window size.
    ["sprint", grow_route("/m/<M>/p/p/s/sp/planning"), "document.querySelector('.sp-conv:not([hidden]) .composer textarea')", null, ".sp-conv:not([hidden]) .composer textarea", ".sp-conv .c-bar .ib, .sp-conv .c-bar .c-send", ".sp-conv .timeline"],
    // A dialog draft is the shared composer, whose layout must not stretch it while it is measured: emptied, it returns to its first height.
    ["request-changes", review_at("rg/changes"), CHANGES, press(".rv-bar button", "Request changes…"), "dialog[open] .composer textarea", "dialog[open] .c-bar .ib, dialog[open] [data-bar=dialog] .btn", null],
  ];
  const GROW_SIZES = [[1280, 800], [960, 540]];
  const GROW_TEXT = Array.from({length: 80}, (_, i) => `Line ${i} of a long pasted message`).join("\n");
  const GROW_STATE = probe("layout-fit.GROW_STATE.js");
  const grow_edit = async (field, command, value = null) => {
    // execCommand is the engine's own editing path, the one a paste takes: undo, caret and the input event are real.
    await js("const f=document.querySelector(" + json(field) + ");f.focus();f.select();document.execCommand(" + json(command) + ",false," + json(value) + ");return 1");
    await sleep(.3);
  };
  const grow_state = (field, controls, feed) => js(sub(sub(sub(GROW_STATE, "FIELD", json(field)), "CONTROLS", json(controls)), "FEED", json(feed)));
  const grow_problems = (name, floor, short, at_cap, cleared) => {
    const out = [];
    const state = s => `${name}: ` + s;
    if (!(floor.height < short.height && short.height < at_cap.height)) out.push(state(`did not grow: ${floor.height} ${short.height} ${at_cap.height}`));
    if (short.overflow !== "hidden" || short.scrollbar !== 0) out.push(state(`scrolls below the cap: ${short.overflow} ${short.scrollbar}`));
    if (Math.abs(at_cap.height - at_cap.cap) > 1) out.push(state(`cap ${at_cap.height}, want ${at_cap.cap}`));
    if (at_cap.overflow !== "auto" || at_cap.scroll <= at_cap.client) out.push(state(`no inner scroll at the cap: ${at_cap.overflow} ${at_cap.scroll}/${at_cap.client}`));
    if (cleared.height !== floor.height || cleared.overflow !== "hidden") out.push(state(`did not shrink back: ${cleared.height} from ${floor.height}, ${cleared.overflow}`));
    for (const [which, found] of [["floor", floor], ["short", short], ["cap", at_cap], ["cleared", cleared]]) {
      if (found.pageScrolls) out.push(state(`window scrolls at ${which}`));
      out.push(...found.controls.filter(c => c.top < 0 || c.bottom > found.view || c.left < 0).map(c => state(`control off the window at ${which}: ${c.name} ${c.top}..${c.bottom}`)));
      if (!found.controls.length) out.push(state(`no controls found at ${which}`));
      if (found.clear && found.clear.tail > found.clear.dock + 1) out.push(state(`dock covers the end of the feed at ${which}: ${json(found.clear)}`));
      // The dock is the feed's flow sibling below it: the feed's box, and so its scrollbar, ends at the dock's top edge.
      if (found.clear && Math.abs(found.clear.feed - found.clear.dock) > 1) out.push(state(`feed box does not end at the dock's top at ${which}: ${json(found.clear)}`));
    }
    return out;
  };
  const grow_open = async (query, ready, opener, field, width, height) => {
    await open_page(query, width, height, ready || `document.querySelector(${json(field)})`);
    if (opener) {
      await js(opener + ";return 1");
      await wait(`document.querySelector(${json(field)})`);
      await sleep(.3);
    }
  };
  async function grow_gate(results, failures) {
    for (const [name, query, ready, opener, field, controls, feed] of GROW) {
      for (const [width, height] of GROW_SIZES) {
        const key = `grow-${name}-${width}x${height}`;
        await grow_open(query, ready, opener, field, width, height);
        const floor = await grow_state(field, controls, feed);
        await grow_edit(field, "insertText", ["one", "two", "three", "four", "five"].join("\n"));
        const short = await grow_state(field, controls, feed);
        await grow_edit(field, "insertText", GROW_TEXT);
        const at_cap = await grow_state(field, controls, feed);
        await capture(key);
        // At the cap the field scrolls by itself and typing leaves both it and the page where they are.
        await js("const f=document.querySelector(" + json(field) + ");f.scrollTop=120;const p=f.closest('.create');if(p)p.scrollTop=40;f.setSelectionRange(0,0);document.execCommand('insertText',false,'x');return 1");
        await sleep(.3);
        const kept = await js("const f=document.querySelector(" + json(field) + ");const p=f.closest('.create');return {field:f.scrollTop,page:p?p.scrollTop:null,canPage:p?p.scrollHeight>p.clientHeight+40:false}");
        await grow_edit(field, "delete");
        const cleared = await grow_state(field, controls, feed);
        results[key] = {floor, short, cap: at_cap, cleared, kept};
        failures[key] = [...grow_problems(key, floor, short, at_cap, cleared), ...(kept.page === null || !kept.canPage || kept.page === 40 ? [] : [`${key}: the page jumped while typing: ${json(kept)}`])];
      }
    }
    // The rules above have to be able to fail.
    results.grow_faults = {}; failures.grow_faults = [];
    for (const [fault, style, size, expect] of [["no cap", "textarea[data-grow]{max-height:none!important}", [1280, 800], "cap"], ["footer in the flow", ".create-foot{position:static!important}", [960, 540], "control off the window"]]) {
      const [, query, ready, opener, field, controls, feed] = GROW[0];
      await grow_open(query, ready, opener, field, size[0], size[1]);
      await inject(style);
      const floor = await grow_state(field, controls, feed);
      await grow_edit(field, "insertText", ["one", "two", "three", "four", "five"].join("\n"));
      const short = await grow_state(field, controls, feed);
      await grow_edit(field, "insertText", GROW_TEXT);
      const at_cap = await grow_state(field, controls, feed);
      await grow_edit(field, "delete");
      const problems = grow_problems(fault, floor, short, at_cap, await grow_state(field, controls, feed));
      results.grow_faults[fault] = problems.slice(0, 3);
      if (!problems.some(p => p.includes(expect))) failures.grow_faults.push(`not reported: ${fault}: ${json(problems.slice(0, 3))}`);
    }
  }

  const click = async (selector, text = null) => {
    const needle = text ? "e.textContent.trim()===" + json(text) : "true";
    await wait("[...document.querySelectorAll(" + json(selector) + ")].some(e=>" + needle + ")");
    await js("[...document.querySelectorAll(" + json(selector) + ")].find(e=>" + needle + ").click()");
    await sleep(.3);
  };
  const summary = (results, failures) => { results.failures = Object.fromEntries(Object.entries(failures).filter(([, v]) => v.length)); };
  const save = async results => writeJson(join(evidence, label + "-metrics.json"), results, 2);
  const failIfAny = results => { if (Object.keys(results.failures).length) throw Error("AssertionError: rendered layout problems"); };

  const results = {}, failures = {};
  try {
    if (eq(extra, ["keys"])) {
      // A quick pass on the keyboard, focus-line, target and popover-role rules alone; the full run includes them.
      await keys_gate(results, failures);
      summary(results, failures);
      await save(results);
      console.log(json({checked: Object.keys(failures).length, results: {keys_reveal: results.keys_reveal, keys_focus: results.keys_focus, keys_targets: results.keys_targets}, failed: results.failures}, null, 1));
      failIfAny(results);
      return 0;
    }
    if (extra[0] !== "only") await grow_gate(results, failures);
    if (eq(extra, ["grow"])) {
      summary(results, failures);
      await save(results);
      console.log(json({checked: Object.keys(failures).length, sizes: Object.fromEntries(Object.entries(results).filter(([k]) => k.startsWith("grow-") && k !== "grow_faults").map(([k, v]) => [k, ["floor", "short", "cap", "cleared"].map(f => v[f].height)])), failed: results.failures}, null, 1));
      failIfAny(results);
      return 0;
    }
    if (extra[0] === "only") {
      // A quick pass while working on a screen: the SCREENS and FLOWS whose names start with the given words, full rules only.
      // A word like @1280 keeps only that width.
      const words = extra.slice(1);
      const widths = new Set(words.filter(w => w.startsWith("@")).map(w => Number(w.slice(1))));
      const wanted = name => words.filter(w => !w.startsWith("@")).some(w => name.startsWith(w));
      const narrow = sizes => sizes.filter(s => !widths.size || widths.has(s[0]));
      const outline = "const m=document.querySelector('.main')||document.body;return [...m.querySelectorAll(':scope > *, :scope > * > *, :scope > * > * > *')].slice(0,40).map(e=>e.tagName.toLowerCase()+'.'+String(e.className).split(' ').join('.'))";
      for (const [name, query, ready, sizes] of SCREENS) {
        for (const [width, height] of wanted(name) ? narrow(sizes) : []) {
          const key = `${name}-${width}x${height}`;
          try { await open_page(query, width, height, ready); }
          catch (error) {
            if (!(error instanceof TimeoutError)) throw error;
            failures[key] = ["not ready: " + json(await js(outline))];
            await capture(key);
            continue;
          }
          failures[key] = await check_strict(results, key);
        }
      }
      for (const [name, query, ready, steps, sizes] of FLOWS) {
        for (const [width, height] of wanted(name) ? narrow(sizes) : []) {
          const key = `${name}-${width}x${height}`;
          await open_page(query, width, height, ready);
          for (const [script, after] of steps) { await js(script + ";return 1"); await wait(after); await sleep(.3); }
          failures[key] = await check_strict(results, key);
        }
      }
      await title_gate(results, failures, wanted, narrow);
      await owner_card_gate(results, failures, wanted, narrow);
      if (wanted("task-queued")) await queue_gate(results, failures, narrow);
      if (wanted("task-output")) await output_gate(results, failures, narrow);
      if (words.includes("fit")) await fit_gate(results, failures);
      if (words.includes("themes")) await theme_gate(results, failures);
      summary(results, failures);
      await save(results);
      console.log(json({checked: Object.keys(failures).length, failed: Object.fromEntries(Object.entries(results.failures).map(([k, v]) => [k, {count: v.length, first: v.slice(0, 12)}]))}, null, 1));
      failIfAny(results);
      return 0;
    }
    for (const [width, height] of SIZES) {
      const size = `${width}x${height}`;
      await load("git", width, height, "document.querySelector('.git-sum') && document.querySelector('.hrow')");
      failures[`git-${size}`] = await check(results, `git-${size}`);
      await click(".chg-tools button", "Branches and worktrees");
      await wait("document.querySelector('.gr')");
      failures[`branches-${size}`] = await check(results, `branches-${size}`);
      await load("code", width, height, "document.querySelector('.cm-content') && document.querySelector('.ecrumb button')");
      await click(".side-h .seg button", "Files");
      await wait("document.querySelector('.tn')");
      failures[`code-${size}`] = await check(results, `code-${size}`);
      for (const name of ["canonical-exact", "canonical-pricing", "audit-company", "check-model"]) {
        await js("[...document.querySelectorAll('.tn')].find(e=>e.textContent.includes(" + json(name) + ")).click()");
        await sleep(.2);
      }
      await wait("document.querySelectorAll('.etab').length >= 4");
      failures[`code-tabs-${size}`] = await check(results, `code-tabs-${size}`);
      await click(".side-h .seg button", "Search");
      await js("const i=document.querySelector('.ws-find-in');i.value='line';i.dispatchEvent(new Event('input',{bubbles:true}))");
      await wait("document.querySelector('.res-file')");
      failures[`search-${size}`] = await check(results, `search-${size}`);
    }
    for (const [name, route, ready, bars] of KEPT) {
      for (const [width, height] of SIZES) {
        const key = `${name}-${width}x${height}`;
        await open_page("route=" + quote(route), width, height, ready);
        failures[key] = await check(results, key);
        for (const [selector, want] of Object.entries(bars)) {
          const got = await js("return Math.round(document.querySelector(" + json(selector) + ").getBoundingClientRect().height * 100) / 100");
          if (Math.abs(got - want) > .5) failures[key].push(`bar height ${selector}: ${got} != ${want}`);
        }
      }
    }
    await strict_gate(results, failures);
    await load("sprint", 1280, 800, "document.querySelector('.brief')");
    failures["sprint-1280x800"] = await check(results, "sprint-1280x800");
    results.sprint = await js("return {brief: document.querySelector('.brief').innerHTML, ol: document.querySelectorAll('.brief ol li').length, br: document.querySelectorAll('.brief br').length, note: document.querySelectorAll('.sp-conv .ai br').length, mine: document.querySelectorAll('.sp-conv .mine br').length}");
    await capture("sprint-brief");
    await load("sprint-plan", 1280, 800, "document.querySelector('.sp-plan:not([hidden]) .ws-more-l')");
    await click(".ws-more-l");
    await wait("document.querySelector('.ws-read-d')");
    failures["read-more-1280x800"] = await check(results, "read-more-1280x800");
    results.read_more = await js("return {h1: document.querySelectorAll('.ws-read-d h1').length, ol: document.querySelectorAll('.ws-read-d ol li').length, br: document.querySelectorAll('.ws-read-d br').length, accept: document.querySelectorAll('.ws-read-d li').length}");
    await capture("read-more");
    await load("task", 1280, 800, "document.querySelector('.brief')");
    results.task = await js("return {ol: document.querySelectorAll('.brief ol li').length, br: document.querySelectorAll('.brief br').length}");
    await capture("task-brief");
    const {sprint, read_more, task} = results;
    failures.text = sprint.ol === 2 && sprint.br >= 1 && sprint.note >= 1 && sprint.mine >= 1 && read_more.h1 === 1 && read_more.ol === 2 && read_more.br >= 1 && read_more.accept >= 4 && task.ol === 2 && task.br >= 1 ? [] : [json({sprint, read_more, task})];
    // The probe proves it can fail: the same page with reasons back inside the toolbar and names free to wrap must be caught.
    await load("git", 1280, 800, "document.querySelector('.git-sum')");
    await js("const s=document.createElement('style');s.textContent='.git-sum [data-why]::after{content:attr(data-why)!important}.git-sum .cap{white-space:normal!important;overflow:visible!important}.ws-gf .p,.hrow .trunc{white-space:normal!important;overflow:visible!important}';document.head.append(s)");
    await sleep(.3);
    results.probe_catches_faults = (await js(PROBE)).problems.length;
    failures.probe = results.probe_catches_faults > 0 ? [] : ["the probe did not notice an injected fault"];
    summary(results, failures);
    await save(results);
    console.log(json({checked: Object.keys(failures).length, grow_faults: results.grow_faults, faults: Object.fromEntries(Object.entries(results.faults).map(([k, v]) => [k, v.matching])), pseudo_guard: results.pseudo_guard.controls, flip: results.flip.compared,
      failed: Object.fromEntries(Object.entries(results.failures).map(([k, v]) => [k, {count: v.length, first: v.slice(0, 4)}]))}, null, 1));
    failIfAny(results);
    return 0;
  } catch (error) {
    console.error(error.name + ": " + error.message);
    if (!results.failures) {
      results.aborted = error.name + ": " + error.message;
      summary(results, failures);
      await save(results);
    }
    return 1;
  }
});
