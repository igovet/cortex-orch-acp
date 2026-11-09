import { readdirSync, readFileSync } from "node:fs";
import ts from "typescript";
import { afterEach, expect, it } from "vitest";

afterEach(() => { document.body.innerHTML = ""; document.head.querySelector("#cursor-styles")?.remove(); });
const controls = [
  '<button>Button</button>', '<a href="#">Link</a>', '<summary>Summary</summary>', '<select><option>Choice</option></select>',
  '<input type="button">', '<input type="checkbox">', '<input type="radio">', '<input type="range">',
  '<label>Toggle</label>', '<div role="button"></div>', '<div role="tab"></div>', '<div role="option"></div>',
  '<div role="menuitem"></div>', '<div role="treeitem"></div>', '<div role="switch"></div>', '<div role="checkbox"></div>',
  '<g class="ws-node"></g>', '<article class="card"></article>', '<div class="ws-pointer"></div>', '<div class="ws-tr"></div>', '<div class="pt-row"></div>',
];
it("every custom clickable source element uses a covered pointer family", () => {
  const uncovered: string[] = [];
  for (const name of readdirSync("src").filter(n => n.endsWith(".tsx"))) {
    const file = ts.createSourceFile(name, readFileSync("src/" + name, "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    const visit = (node: ts.Node) => {
      if (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) {
        const tag = node.tagName.getText(file), attrs = node.attributes.properties;
        const click = attrs.find(a => ts.isJsxAttribute(a) && a.name.getText(file) === "onClick");
        const activator = click?.getText(file);
        if (activator && /^[a-z]/.test(tag) && !/^(button|a|input|select|textarea|summary|label)$/.test(tag)) {
          const role = attrs.find(a => ts.isJsxAttribute(a) && a.name.getText(file) === "role")?.getText(file) ?? "";
          const cls = attrs.find(a => ts.isJsxAttribute(a) && a.name.getText(file) === "class")?.getText(file) ?? "";
          // Propagation guards activate nothing; dialog's handler dismisses only its backdrop.
          const guard = /^onClick=\{\w+ => \w+\.stopPropagation\(\)\}$/.test(activator);
          const backdrop = tag === "dialog" && activator.includes("e.target === el");
          if (!guard && !backdrop && !/role="(?:button|tab|option|menuitem|treeitem|switch|checkbox|radio|link)"/.test(role) && !/\b(?:ws-pointer|ws-tr|pt-row|ws-node)\b/.test(cls))
            uncovered.push(name + ":" + (file.getLineAndCharacterOfPosition(node.pos).line + 1));
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(file);
  }
  expect(uncovered).toEqual([]);
});
function style() {
  const s = document.createElement("style"); s.id = "cursor-styles";
  s.textContent = readFileSync("src/mockup-styles.css", "utf8") + readFileSync("src/workspace.css", "utf8"); document.head.append(s);
}
it("every clickable control family has a pointer in the actual shared cascade", () => {
  style(); document.body.innerHTML = controls.join("");
  const wrong = [...document.body.children].filter(e => getComputedStyle(e).cursor !== "pointer").map(e => e.outerHTML);
  expect(wrong).toEqual([]);
});
it("disabled controls are not pointers and text fields retain text cursors", () => {
  style(); document.body.innerHTML = '<button disabled>Off</button><a href="#" aria-disabled="true">Off</a><div role="button" data-busy>Busy</div><input type="text"><textarea></textarea><div contenteditable="true"></div>';
  for (const e of [...document.body.children].slice(0, 3)) expect(["default", "not-allowed"]).toContain(getComputedStyle(e).cursor);
  for (const e of [...document.body.children].slice(3)) expect(getComputedStyle(e).cursor).toBe("text");
});
