import type { Message, Task, WorkspaceCommand, WorkspaceState } from "./types";
import { editable, entity, nextId, nonempty, online, pauseSprint, requireFact, validateChecks } from "./core";
import { continueTask } from "./tasks";
import { addEntry, addLine, cutCommands, isOpen, pushActivity, sessionFor, stopClock, validateSettings } from "./timeline";

/** A sent message becomes part of the record: the conversation for Cortex, the worker's history for a direct message. */
function post(state: WorkspaceState, task: Task, message: Message) {
  if (message.workerId) pushActivity(entity(state.workers, message.workerId), message.text, { kind: "message", messageId: message.id, at: message.at });
  else addEntry(state, task, { kind: "message", messageId: message.id });
}
function acceptsMessages(state: WorkspaceState, task: Task, resume: boolean | undefined, now: number | undefined) {
  editable(task); online(state, task);
  // Any message resumes a paused task, when the caller says that is what the owner did.
  if (resume && task.status === "paused") continueTask(state, task, now);
  requireFact(["working", "needs_you"].includes(task.status), "Task is not accepting messages; continue it first");
}
const queued = (state: WorkspaceState, id: string) => {
  const message = entity(state.messages, id);
  requireFact(message.delivery === "queued", "This message has already left the queue");
  return message;
};
const workerTask = (state: WorkspaceState, workerId: string) => {
  const worker = entity(state.workers, workerId), task = entity(state.tasks, worker.taskId);
  editable(task); online(state, task);
  return { worker, task };
};
/** A worker's question closing lets the task leave Needs you only if nothing else still waits for the owner. */
function settleDecisions(state: WorkspaceState, task: Task) {
  if (sessionFor(state, task).decisions.some(isOpen) || task.status !== "needs_you" || task.conflict !== "none") return;
  task.issue = null; task.status = "working"; task.process = "live";
}

function reviewOf(state: WorkspaceState, task: Task) {
  let review = state.reviews.find(r => r.taskId === task.id);
  if (!review) { review = { taskId: task.id, sha: "", files: [], scopes: [], comments: [], mock: true }; state.reviews.push(review); }
  return review;
}
const comment = (state: WorkspaceState, task: Task, id: string) => {
  const found = reviewOf(state, task).comments.find(c => c.id === id);
  requireFact(found, "Unknown comment");
  requireFact(!found.sent, "This comment was already sent");
  return found;
};
/** Sending a finished result back: Cortex works on it again, and any acceptance of that version goes away. */
function reopen(state: WorkspaceState, task: Task, text: string, now: number | undefined) {
  requireFact(["review", "ready_to_merge"].includes(task.status) && task.conflict === "none", "Only a finished result can be sent back");
  requireFact(task.integration?.method !== "squash", "This task is already merged");
  task.status = "working"; task.process = "live"; task.acceptance = null; task.resolvedBy = null; task.issue = null;
  const message: Message = { id: nextId(state, "message"), taskId: task.id, workerId: null, text: nonempty(text, "Message"), mode: "send_now", delivery: "recorded", at: now ?? null, deliveredAt: null, failure: null, mock: true };
  state.messages.push(message);
  post(state, task, message);
  return message;
}

/** Returns the entity id, or null when the command belongs to another module. */
export function handleSession(state: WorkspaceState, command: WorkspaceCommand): { id?: string } | null {
  if (command.type === "retry_delivery") return handleSession(state, { type: "simulate_delivery", messageId: command.messageId, delivery: "sending" });
  switch (command.type) {
    case "request_changes": {
      const task = entity(state.tasks, command.taskId); editable(task); online(state, task);
      return { id: reopen(state, task, command.text, command.now).id };
    }
    case "add_comment": {
      const task = entity(state.tasks, command.taskId); editable(task);
      requireFact(Number.isInteger(command.line) && command.line > 0, "Pick a line to comment on");
      const id = nextId(state, "comment");
      reviewOf(state, task).comments.push({ id, path: nonempty(command.path, "File"), line: command.line, text: nonempty(command.text, "Comment"), sent: false, mock: true });
      return { id };
    }
    case "edit_comment": {
      const task = entity(state.tasks, command.taskId); editable(task);
      comment(state, task, command.commentId).text = nonempty(command.text, "Comment");
      return { id: command.commentId };
    }
    case "remove_comment": {
      const task = entity(state.tasks, command.taskId); editable(task);
      const review = reviewOf(state, task), found = comment(state, task, command.commentId);
      review.comments = review.comments.filter(c => c !== found);
      return { id: command.commentId };
    }
    case "send_comments": {
      const task = entity(state.tasks, command.taskId); editable(task); online(state, task);
      const drafts = reviewOf(state, task).comments.filter(c => !c.sent);
      requireFact(drafts.length > 0, "No draft comments to send");
      // One batched clarification, each comment tied to its file and line.
      const text = [`${drafts.length} comment${drafts.length === 1 ? "" : "s"} on the diff:`, ...drafts.map(c => `${c.path}:${c.line} ${c.text}`)].join("\n");
      if (["working", "needs_you"].includes(task.status)) {
        const message: Message = { id: nextId(state, "message"), taskId: task.id, workerId: null, text, mode: "send_now", delivery: "recorded", at: command.now ?? null, deliveredAt: null, failure: null, mock: true };
        state.messages.push(message);
        post(state, task, message);
      } else reopen(state, task, text, command.now);
      drafts.forEach(c => { c.sent = true; });
      return {};
    }
    case "set_task_settings": {
      const task = entity(state.tasks, command.taskId); editable(task); online(state, task);
      requireFact(task.status !== "done", "A closed task keeps the settings it ran with");
      task.settings = validateSettings({ ...task.settings, ...command.settings });
      return { id: task.id };
    }
    case "send_message": {
      const task = entity(state.tasks, command.taskId);
      acceptsMessages(state, task, command.resume, command.now);
      const worker = command.workerId ? entity(state.workers, command.workerId) : null;
      if (worker) requireFact(worker.taskId === task.id && worker.status === "working", "Worker is not an active recipient for this task");
      const message: Message = {
        id: nextId(state, "message"), taskId: task.id, workerId: worker?.id ?? null, text: nonempty(command.text, "Message"), mode: command.mode,
        delivery: command.mode === "queue" ? "queued" : worker ? "sending" : "recorded", at: command.now ?? null, deliveredAt: null, failure: null, attachments: command.attachments ?? [], mock: true,
      };
      state.messages.push(message);
      if (command.mode === "send_now") post(state, task, message);
      return { id: message.id };
    }
    case "edit_queued": { queued(state, command.messageId).text = nonempty(command.text, "Message"); return { id: command.messageId }; }
    case "remove_queued": { queued(state, command.messageId).delivery = "withdrawn"; return { id: command.messageId }; }
    case "move_queued": {
      const message = queued(state, command.messageId);
      if (command.before === message.id) return { id: message.id };
      if (command.before) requireFact(queued(state, command.before).taskId === message.taskId, "That message belongs to another task");
      state.messages.splice(state.messages.indexOf(message), 1);
      const at = command.before ? state.messages.findIndex(m => m.id === command.before) : state.messages.length;
      state.messages.splice(at, 0, message);
      return { id: message.id };
    }
    case "send_queued": {
      const message = queued(state, command.messageId), task = entity(state.tasks, message.taskId);
      acceptsMessages(state, task, command.resume, command.now);
      const worker = message.workerId ? entity(state.workers, message.workerId) : null;
      if (worker) requireFact(worker.status === "working", "Worker is no longer active");
      message.delivery = worker ? "sending" : "recorded"; message.mode = "send_now"; message.at = command.now ?? message.at;
      post(state, task, message);
      return { id: message.id };
    }
    case "flush_queue": {
      const task = entity(state.tasks, command.taskId); editable(task); online(state, task);
      requireFact(task.status === "working", "Continue the task before flushing its client queue");
      state.messages.filter(m => m.taskId === task.id && m.delivery === "queued").forEach(message => {
        const live = !message.workerId || entity(state.workers, message.workerId).status === "working";
        message.delivery = !message.workerId ? "recorded" : live ? "sending" : "failed";
        message.at = command.now ?? message.at;
        if (live) post(state, task, message);
      });
      return {};
    }
    case "simulate_delivery": {
      const message = entity(state.messages, command.messageId);
      requireFact(message.workerId && ["sending", "recorded", "failed", "unverified"].includes(message.delivery), "Delivery is final or not a worker message");
      const worker = entity(state.workers, message.workerId);
      requireFact(worker.status === "working", "Worker no longer active");
      requireFact(["sending", "recorded", "delivered", "failed", "unverified"].includes(command.delivery), "Invalid worker delivery transition");
      message.delivery = command.delivery;
      message.failure = command.delivery === "failed" ? command.failure ?? "Could not send" : null;
      if (command.delivery === "delivered") message.deliveredAt = command.now ?? null;
      return { id: message.id };
    }
    case "withdraw_message": {
      const message = entity(state.messages, command.messageId);
      requireFact(message.workerId && ["sending", "recorded", "failed", "unverified"].includes(message.delivery), "Only an undelivered worker message can be withdrawn");
      message.delivery = "withdrawn";
      return { id: message.id };
    }
    case "redirect_message": {
      const message = entity(state.messages, command.messageId), task = entity(state.tasks, message.taskId);
      requireFact(message.workerId && ["failed", "unverified"].includes(message.delivery), "Only an undelivered worker message can go to the coordinator");
      acceptsMessages(state, task, false, command.now);
      message.delivery = "withdrawn";
      const copy: Message = { ...message, id: nextId(state, "message"), workerId: null, delivery: "recorded", at: command.now ?? null, deliveredAt: null, failure: null };
      state.messages.push(copy);
      post(state, task, copy);
      return { id: copy.id };
    }
    case "ask_decision": {
      const task = entity(state.tasks, command.taskId); editable(task); online(state, task);
      requireFact(["working", "needs_you"].includes(task.status), "Only a running task can ask");
      const worker = command.workerId ? entity(state.workers, command.workerId) : null;
      if (worker) { requireFact(worker.status === "working", "Worker is not active"); worker.status = "waiting_you"; worker.now = command.text; stopClock(worker, command.now); }
      const id = nextId(state, "decision");
      sessionFor(state, task).decisions.push({
        id, kind: "question", from: worker?.role ?? "", workerId: worker?.id ?? null, at: command.now ?? null,
        text: nonempty(command.text, "Question"), options: command.options, answer: null, withdrawn: false,
      });
      addEntry(state, task, { kind: "decision", decisionId: id });
      task.status = "needs_you"; task.issue = worker?.role ? `Question from ${worker.role}` : "Question waiting";
      pauseSprint(state, task, task.issue);
      return { id };
    }
    case "answer_decision": {
      const task = entity(state.tasks, command.taskId); editable(task); online(state, task);
      requireFact(["working", "needs_you"].includes(task.status), "Resume the task before answering; this question may be withdrawn");
      const decision = sessionFor(state, task).decisions.find(d => d.id === command.decisionId);
      requireFact(decision, "Unknown question");
      requireFact(isOpen(decision), "This question is already closed");
      decision.answer = { text: nonempty(command.answer, "Answer"), at: command.now ?? null };
      if (decision.workerId) {
        const worker = entity(state.workers, decision.workerId);
        if (worker.status === "waiting_you") { worker.status = "working"; worker.now = "Continuing with your answer"; worker.runningSince = command.now ?? null; }
      }
      settleDecisions(state, task);
      return { id: decision.id };
    }
    case "worker_activity": {
      const worker = entity(state.workers, command.workerId);
      online(state, entity(state.tasks, worker.taskId));
      requireFact(worker.status === "working", "Worker is not active");
      pushActivity(worker, nonempty(command.text, "Activity"), { at: command.now ?? null, ...command.entry });
      worker.now = command.text;
      return { id: worker.id };
    }
    case "finish_worker": {
      const worker = entity(state.workers, command.workerId);
      online(state, entity(state.tasks, worker.taskId));
      requireFact(worker.status === "working", "Worker is not active");
      validateChecks(command.checks);
      worker.status = command.outcome; stopClock(worker, command.now); worker.now = null;
      worker.result = { summary: nonempty(command.summary, "Summary"), checks: structuredClone(command.checks), mock: true };
      return { id: worker.id };
    }
    case "stop_worker": {
      const { worker, task } = workerTask(state, command.workerId);
      requireFact(["working", "waiting_you"].includes(worker.status), "Worker is not running");
      worker.status = "cancelled"; stopClock(worker, command.now); cutCommands(worker, "Worker stopped by you");
      worker.now = "Stopped by you";
      pushActivity(worker, command.reason ? `Stopped by you: ${command.reason}` : "Stopped by you", { kind: "divider", at: command.now ?? null });
      sessionFor(state, task).decisions.filter(d => d.workerId === worker.id && isOpen(d)).forEach(d => { d.withdrawn = true; });
      state.messages.filter(m => m.workerId === worker.id && ["sending", "recorded"].includes(m.delivery)).forEach(m => { m.delivery = "failed"; m.failure = "Not delivered: the worker was stopped first"; });
      addLine(state, task, "stop", `You stopped ${worker.role}. The task keeps running.`, command.now);
      settleDecisions(state, task);
      return { id: worker.id };
    }
    case "stop_command": {
      const { worker } = workerTask(state, command.workerId);
      requireFact(worker.status === "working", "Worker is not running");
      const item = worker.activity.find(a => a.sequence === command.sequence);
      requireFact(item?.command, "Unknown command");
      requireFact(item.command.state === "running", "This command is no longer running");
      item.command.state = "stopping"; item.command.reason = nonempty(command.reason, "Reason");
      return { id: worker.id };
    }
    case "settle_command": {
      const { worker } = workerTask(state, command.workerId);
      const item = worker.activity.find(a => a.sequence === command.sequence);
      requireFact(item?.command && ["running", "stopping"].includes(item.command.state), "This command already finished");
      item.command.state = command.state; item.command.exit = command.exit;
      return { id: worker.id };
    }
    default: return null;
  }
}
