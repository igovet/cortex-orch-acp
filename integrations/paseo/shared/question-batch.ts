/** Exact bindings for Paseo's header-keyed, all-member question form. */
export type QuestionBatch = {
  batchId: string;
  questionId: string;
  questions: Array<{ questionId: string; question: string; options: string[]; topic?: string }>;
};
/** A short tab label when the runtime sent none (an older batch, or a model
 * that omitted the optional field): the question's own first meaningful
 * words, bounded to the same 24-character budget as an explicit topic --
 * never the raw question id, which carries no meaning to a reader. */
export function fallbackTopic(question: string): string {
  const words = question.trim().replace(/^[-*#>`]+\s*/, "").split(/\s+/).filter(Boolean);
  let label = "";
  for (const word of words) {
    const next = label ? `${label} ${word}` : word;
    if (next.length > 24) break;
    label = next;
  }
  return (label || words[0]?.slice(0, 24) || "Question").replace(/[?:,;.!]+$/, "");
}
export function parseQuestionBatch(input: Record<string, unknown>): QuestionBatch {
  const { batchId, questionId, questions } = input;
  if (typeof batchId !== "string" || !/^qb_[a-zA-Z0-9]+$/.test(batchId)
    || typeof questionId !== "string" || !Array.isArray(questions)
    || questions.length < 2 || questions.length > 8) throw new Error("Invalid Cortex question batch");
  const ids = new Set<string>();
  const members = questions.map((member) => {
    if (!member || typeof member !== "object" || typeof member.questionId !== "string"
      || !/^pq_[a-zA-Z0-9]+$/.test(member.questionId) || ids.has(member.questionId)
      || typeof member.question !== "string" || !member.question.trim() || member.question.length > 4000
      || !Array.isArray(member.options) || member.options.length < 2 || member.options.length > 8
      || member.options.some((value: unknown) => typeof value !== "string" || !value.trim() || value.length > 1000)
      || new Set(member.options.map((value: string) => value.trim().toLowerCase())).size !== member.options.length
      || (member.topic !== undefined && (typeof member.topic !== "string" || !member.topic.trim() || member.topic.length > 24))) {
      throw new Error("Invalid Cortex question batch member");
    }
    ids.add(member.questionId);
    return {
      questionId: member.questionId, question: member.question, options: [...member.options] as string[],
      ...(typeof member.topic === "string" && member.topic.trim() ? { topic: member.topic.trim() } : {}),
    };
  });
  if (members[0].questionId !== questionId) throw new Error("Invalid Cortex question batch anchor");
  return { batchId, questionId, questions: members };
}
export function batchForm(batch: Pick<QuestionBatch, "questions">) {
  const headers = new Set<string>();
  return { questions: batch.questions.map((member) => {
    const topic = member.topic || fallbackTopic(member.question) || "Question";
    let header = topic;
    for (let ordinal = 2; headers.has(header); ordinal++) {
      const suffix = ` ${ordinal}`;
      header = topic.slice(0, 24 - suffix.length).trimEnd() + suffix;
    }
    headers.add(header);
    return {
      question: member.question, header,
      options: member.options.map((label) => ({ label })), multiSelect: false, allowOther: true,
    };
  }) };
}
export function batchAnswers(batch: Pick<QuestionBatch, "questions">, answers: unknown) {
  if (!answers || typeof answers !== "object" || Array.isArray(answers)
    || Object.keys(answers).length !== batch.questions.length) throw new Error("Answer every Cortex question before submitting");
  const values = answers as Record<string, unknown>;
  const form = batchForm(batch);
  const headerKeyed = form.questions.every(({ header }) => Object.hasOwn(values, header));
  // Older integrations submitted identities directly. Accept one complete
  // key space, never a partial mixture of headers and identities.
  const idKeyed = batch.questions.every(({ questionId }) => Object.hasOwn(values, questionId));
  if (!headerKeyed && !idKeyed) throw new Error("Answer every Cortex question before submitting");
  return batch.questions.map(({ questionId }, index) => {
    const key = headerKeyed ? form.questions[index].header : questionId;
    const answer = values[key];
    if (typeof answer !== "string" || !answer.trim() || answer.length > 32000) throw new Error("Answer every Cortex question before submitting");
    return { questionId, answer };
  });
}
