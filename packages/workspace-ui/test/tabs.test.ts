import { describe, expect, it } from "vitest";
import { parseRoute } from "../src/router";
import { activeOf, addTab, closeTab, cycleTab, encodeTabs, FRESH_HREF, firstTabs, hrefIn, isStoredTabs, jumpTab, moveTab, selectTab, startTabs, stepTab, tabOf, tabTitle, visitTab, type TabList } from "../src/tabs";

const three = (): TabList => addTab(addTab(firstTabs("/a"), "/b"), "/c");
const hrefs = (list: TabList) => list.tabs.map(hrefIn);

describe("the tab list", () => {
  it("opens a new tab at the end and makes it active", () => {
    const list = three();
    expect(hrefs(list)).toEqual(["/a", "/b", "/c"]);
    expect(hrefIn(activeOf(list))).toBe("/c");
  });
  it("closing the active tab moves to its right neighbour, or its left at the end", () => {
    let list = selectTab(three(), 2);
    list = closeTab(list, 2);
    expect(hrefIn(activeOf(list))).toBe("/c");
    list = closeTab(list, 3);
    expect(hrefIn(activeOf(list))).toBe("/a");
  });
  it("closing another tab leaves the active one", () => expect(closeTab(selectTab(three(), 1), 3).active).toBe(1));
  it("closing the last tab leaves one fresh tab, never an empty window", () => {
    const list = closeTab(firstTabs("/a"), 1);
    expect(list.tabs).toHaveLength(1);
    expect(hrefIn(list.tabs[0])).toBe(FRESH_HREF);
    expect(list.tabs[0].id).not.toBe(1);
  });
  it("reorders by drag position and clamps past the end", () => {
    expect(hrefs(moveTab(three(), 1, 2))).toEqual(["/b", "/c", "/a"]);
    expect(hrefs(moveTab(three(), 3, 0))).toEqual(["/c", "/a", "/b"]);
    expect(hrefs(moveTab(three(), 1, 99))).toEqual(["/b", "/c", "/a"]);
  });
  it("cycles with wrap-around, and jumps by position with 9 as the last", () => {
    expect(cycleTab(three(), 1).active).toBe(1);
    expect(cycleTab(selectTab(three(), 1), -1).active).toBe(3);
    expect(jumpTab(three(), 2).active).toBe(2);
    expect(jumpTab(selectTab(three(), 1), 9).active).toBe(3);
    expect(jumpTab(three(), 7)).toEqual(three());
  });
});

describe("a tab's own history", () => {
  it("goes back and forward, and a new visit drops what was ahead", () => {
    let list = firstTabs("/a");
    list = visitTab(list, 1, "/b", false);
    list = visitTab(list, 1, "/c", false);
    list = stepTab(list, 1, -1);
    expect(hrefIn(list.tabs[0])).toBe("/b");
    list = visitTab(list, 1, "/d", false);
    expect(list.tabs[0].entries).toEqual(["/a", "/b", "/d"]);
    expect(stepTab(list, 1, 1)).toBe(list);
  });
  it("replaces in place, and ignores a visit to where it already is", () => {
    const list = visitTab(firstTabs("/a"), 1, "/b", true);
    expect(list.tabs[0].entries).toEqual(["/b"]);
    expect(visitTab(list, 1, "/b", false)).toBe(list);
  });
  it("another tab's history is untouched", () => {
    let list = addTab(firstTabs("/a"), "/b");
    list = visitTab(list, 1, "/a2", false);
    expect(tabOf(list, 2)!.entries).toEqual(["/b"]);
  });
});

describe("surviving a restart", () => {
  it("restores routes and the active tab", () => {
    const stored = encodeTabs(selectTab(three(), 2));
    const list = startTabs(stored, FRESH_HREF);
    expect(hrefs(list)).toEqual(["/a", "/b", "/c"]);
    expect(list.active).toBe(2);
  });
  it("a deep link opens in the tab that was active, with Back to where it was", () => {
    const list = startTabs(encodeTabs(selectTab(three(), 2)), "/deep");
    expect(hrefIn(activeOf(list))).toBe("/deep");
    expect(stepTab(list, 2, -1).tabs[1].entries[0]).toBe("/b");
    expect(hrefIn(list.tabs[0])).toBe("/a");
  });
  it("refuses a damaged record", () => {
    expect(isStoredTabs(encodeTabs(three()))).toBe(true);
    for (const bad of [null, [], { tabs: [] }, { tabs: [{ id: 1, href: "//evil" }], active: 1 }, { tabs: [{ id: 1, href: "/a" }], active: 2 }, { tabs: [{ id: 1, href: "/a" }, { id: 1, href: "/b" }], active: 1 }])
      expect(isStoredTabs(bad)).toBe(false);
  });
});

describe("tab titles", () => {
  const names = { project: () => "Cedar", task: () => "Fix the race", sprint: () => undefined };
  const title = (path: string, search = "") => tabTitle(parseRoute(path, search), names);
  it("name the screen", () => {
    expect(title("/m/x")).toBe("Home");
    expect(title("/m/x/new")).toBe("New task");
    expect(title("/m/x/new", "?kind=sprint")).toBe("New sprint");
    expect(title("/settings/app/appearance")).toBe("Settings");
    expect(title("/m/x/p/p/board")).toBe("Cedar");
    expect(title("/m/x/p/p/code")).toBe("Cedar: Code");
    expect(title("/m/x/t/t")).toBe("Fix the race");
    expect(title("/m/x/p/p/s/s1/planning")).toBe("Sprint");
  });
});
