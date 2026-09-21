# Paseo chat presentation and informational requests

Observed on 2026-09-22 against the installed Paseo 0.8 client and Cortex provider.
Private source conversations and raw reports are not copied into this document.

## Incident

A simple project-overview request became a plan with three worker attempts:
investigation, independent review, then a fix. The reviewer supported the project
description but requested a correction to an artifact hash in report metadata.
The user stopped the fix. The coordinator had already inspected relevant files
before delegating essentially the same inspection. A worker also used Russian
for progress/final text. This is not evidence of an infinite scheduler loop;
it is an unnecessarily expensive workflow for an informational question.

## Changes

- A coordinator can propose a read-only informational answer before any plan or
  worker exists. Runtime completion requires its exact retained, idempotent answer
  receipt. Existing plans cannot use this path to bypass review or verification.
- All 23 generated worker profiles and every worker turn carry the English
  communication rule. Coordinator turns reassert the selected message language
  after context. Ordinary, recovery and review turns are covered by scripted
  controller tests. This is instruction delivery, not guaranteed translation.
- Thinking and protocol bookkeeping/wait events are hidden in Paseo chat and worker
  panels. Redacted durable evidence remains. Cortex tools have readable labels and
  neutral colors. Read/Read skill are nonexpandable target/status rows. New host
  and worker display history omits their bodies; model responses are unchanged.
  Old reads are projected without rewriting storage. Raw data is never rendered;
  other calls expand only for readable output, errors or file diffs.
- Worker cards use a fixed-width icon and a two-line title with ellipsis.

## Retained checks and limits

- Full no-paid Python suite: **2,367 passed**, no skips, 147.70 seconds (seven multiprocessing/fork
  deprecation warnings). TypeScript and 21 interface tests passed. Includes
  installed CLI/adapter checks with a loopback scripted model provider.
- Direct-answer and advertised-schema checks: **41 passed**. Covers both English
  and Russian, streaming and non-streaming, read-only project access, no workers,
  no duplicate answer, exact retries, changed-key conflict and plan bypass denial.
- Existing production conversation inspected in real Paseo after plugin reload:
  Thinking rows absent from chat and worker panel; readable tool names shown;
  Read and Read skill were static text in both places. Shell output still expanded,
  without Raw data. A real new Edit displayed the retained change. Long worker
  cards kept their icon and bounded title. Historical Russian prose remains
  historical; it was not rewritten as if the old worker had spoken English.
- Plugin reload closes existing provider connections. The inspected idle agent
  was reloaded without sending a model prompt and its retained timeline restored.
- Two new user-authorized tasks ran against existing native accounts in separate
  disposable projects. No additional task was created during recovery. Exact
  retained identities and outcomes are below.
- No Zed/codeg runbook row is closed by these observations.

## Two-task live follow-up

- Overview: Paseo `463a9d4e-079d-4f87-9318-c92ce68240d5`, runtime
  `t_d9d9037cd8f7`, Codex Luna medium, COMPLETED at revision zero, no worker nodes.
  Russian user input produced an English answer with English selected. The model
  also made two denied command attempts while explaining how to run tests. This
  is retained as unnecessary work, not claimed as a clean minimal call sequence.
  The coordinator instruction now explicitly distinguishes explaining test commands
  from executing them; that instruction change was not given a third paid task.
- Change: Paseo `6a1083a9-7165-49e5-8059-b176ce1f8f54`, runtime
  `t_9cb6467a830c`, COMPLETED, revision one, one implementation node and one review.
  The route policy selected Luna medium (`xa_7be46fbe9748`) for implementation
  and Haiku (`xa_f0be020e8e8a`) for independent review. Both worker panels showed
  English prose. The Haiku coordinator answered in Russian with Russian selected,
  despite English input. Review APPROVE and runtime verification completed; all
  three existing tests passed and git diff changed only `a - b` to `a + b`.
- The change task initially failed before prompting because this campaign's Haiku
  selection incorrectly specified medium effort. Prompt-free inspection of the
  installed adapter confirmed no effort selector after selecting Haiku. Discovery
  now observes each Claude model separately and preserves an observed empty effort
  set. The isolated policy was corrected. Paseo retained the stale effort and its
  null-reset RPC also failed; the stopped disposable daemon's test-agent setting
  was backed up and cleared, then the same task resumed. No production account or
  stable Paseo settings were changed by that recovery.
- Twenty read display events and twelve worker read activity events in the change
  task retained no result bodies. This is display retention evidence, not removal
  of model-side tool results or old database payloads.
- Private command logs and the exact identity/result inventory are retained outside
  this repository under `/var/tmp/cortex-paseo-followup-5cpoxfxn`; they are not
  copied into public documentation. Historical failures remain retained.
