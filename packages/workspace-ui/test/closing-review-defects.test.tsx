import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, readableCheckReason, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { buildIndex, closedAsIs } from "../src/selectors";
import { flush, projection, scriptedPort, withTask } from "./support";

// Defects seen in a paid live run of the dev build: a phantom "Result v2" with Accept right after Request changes, an
// accepted and verified result closed by the owner that read "not verified", and worker checks blamed on a missing tool.
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const world = (patch: Partial<Task>) => {
  const s = withTask(emptyWorkspace(), "Work");
  s.machines[0].connection = "connected";
  Object.assign(s.tasks[0], patch);
  return s;
};
async function mount(state: WorkspaceState, path: string) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}
const result = { version: 2, digest: "digest", summary: "Earlier result", checks: [{ name: "node test/counter.test.js", detail: "", outcome: "passed" as const }] };
const acceptButtons = () => [...document.querySelectorAll("button")].filter(b => /^Accept v/.test(b.textContent?.trim() ?? ""));

it("a task working again after Request changes offers no Accept, even if an older result is still held", async () => {
  await mount(world({ status: "working", process: "live", executionId: "e", workspaceDigest: "digest", result }), `/m/${LOCAL_MACHINE_ID}/t/t/changes`);
  expect(acceptButtons()).toEqual([]);
  dispose?.(); document.body.innerHTML = "";
  // The same result in Review still offers Accept.
  await mount(world({ status: "review", executionId: "e", workspaceDigest: "digest", result }), `/m/${LOCAL_MACHINE_ID}/t/t/changes`);
  expect(acceptButtons().map(b => b.textContent?.trim())).toContain("Accept v2");
});

it("an accepted, verified result closed as is reads as accepted, and only an unchecked one as not verified", () => {
  const base = world({}).tasks[0];
  const accepted: Task = { ...base, status: "done", closedAs: "as_is", notVerifiedByCortex: false, result: { ...result, version: 1 }, acceptance: { at: 1, digest: "digest", version: 1 } };
  expect(closedAsIs(accepted)).toBe("Closed as is, accepted v1");
  expect(closedAsIs({ ...accepted, acceptance: null, result: { ...result, automaticReady: true } })).toBe("Closed as is, verified");
  expect(closedAsIs({ ...accepted, notVerifiedByCortex: true })).toBe("Closed as is, not verified");
  expect(closedAsIs({ ...accepted, acceptance: null })).toBe("Closed as is, not verified");
  const state = world({ status: "done", closedAs: "as_is", notVerifiedByCortex: false, result: { ...result, version: 1 }, acceptance: { at: 1, digest: "digest", version: 1 } });
  expect(buildIndex(state, Date.now()).byId.get("t")!.fact.text).toBe("Closed as is, accepted v1");
});

it("names why the runtime kept a worker's passed claim unverified instead of blaming a missing tool", () => {
  const revision = readableCheckReason("Runtime kept this passed claim unverified: revision_mismatch.");
  expect(revision).toMatch(/before the files reached the state this result reports/);
  expect(revision).not.toMatch(/tool or service/);
  expect(readableCheckReason("Runtime kept this passed claim unverified: trivial_command.")).toMatch(/does not test the claim/);
  expect(readableCheckReason("revision_mismatch")).toMatch(/before the files reached/);
  // Anything else keeps its earlier reading.
  expect(readableCheckReason("verification_command_unavailable")).toMatch(/No executable command was recorded/);
  // An unknown code is named as what was recorded, never as a missing tool or service, and never read from the prototype.
  expect(readableCheckReason("constructor")).toBe("The runtime did not confirm this check (recorded reason: constructor). Ask Cortex to rerun it.");
});

it("live run: a reviewer's own words naming a runtime code read as that code's meaning, not as an outage", () => {
  const said = "The implementation report's native command receipt was rejected by the runtime for revision_mismatch; no independent same-revision receipt is available in this review.";
  const shown = readableCheckReason(said);
  expect(shown).toBe("The implementation report's native command receipt was rejected by the runtime for a run on an earlier version of the files; no independent same-revision receipt is available in this review.");
  expect(readableCheckReason("Rejected as trivial_command; a direct read confirms 38 lines.")).toBe("Rejected as a command that only shows something and does not test the claim; a direct read confirms 38 lines.");
  // A name that is not a runtime code (a file, a function) stays as written.
  expect(readableCheckReason("tests in test/my_module.test.js failed for parse_args")).toBe("tests in test/my_module.test.js failed for parse_args");
});

it.each([
  "revision_mismatch", "trivial_command", "tool_call_ref_missing", "tool_call_ref_unknown", "foreign_attempt", "ref_already_cited", "not_a_command",
  "engine_command_unverifiable", "receipt_missing", "ambiguous_ref", "call_incomplete", "result_missing", "result_truncated", "command_unrecorded",
  "status_variable_unproven", "later_run_unresolved", "later_run_failed", "nonzero_exit", "zero_exit",
  "spawn_failed", "spawn_or_capture_failed", "wait_failed", "owner_stopped", "timeout", "interrupted_or_timeout", "output_incomplete", "authority_changed",
  "deletion_not_confirmed", "verification_review_revision_changed", "verification_command_unavailable", "namespace_or_mount_setup_failed", "setup_timeout",
  "bubblewrap_launch_failed", "write_grant_outside_project",
])("the runtime code %s reads in plain words without claiming a missing tool or service", code => {
  // A plain word such as "timeout" inside a sentence is already English and stays.
  for (const said of [code, `Runtime kept this passed claim unverified: ${code}.`, ...(code.includes("_") ? [`Rejected for ${code} here.`] : [])]) {
    const shown = readableCheckReason(said);
    expect(shown).not.toMatch(/tool or service/);
    expect(shown).not.toContain(code);
  }
});
