import { afterEach, expect, it } from "vitest";
import { render } from "solid-js/web";
import App from "./App";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const mount = (path: string) => {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <App />, root);
  return root;
};

it("serves the Workspace at the root", async () => {
  const root = mount("/");
  await expect.poll(() => root.querySelector(".crumbs .cur")?.textContent, { timeout: 5000 }).toBe("Home");
  expect(location.pathname).toBe("/m/mock-machine-00000");
  expect(root.querySelector(".mock-chip")).toBeTruthy();
});

it.each(["/a", "/b", "/c", "/archive/a", "/archive/c", "/needs-you", "/p/cedar-notes/board", "/settings/about"])("no longer routes the earlier exploration at %s", async path => {
  const root = mount(path);
  await expect.poll(() => root.querySelector(".note-panel")?.textContent, { timeout: 5000 }).toContain("This page does not exist");
  expect(root.textContent).not.toContain("Archived exploration");
  expect(root.querySelector(".shell-controls")).toBeNull();
});
