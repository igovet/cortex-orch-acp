/** Exact bindings for Paseo's header-keyed, all-member question form. */
export type QuestionBatch = {
  batchId: string;
  questionId: string;
  questions: Array<{ questionId: string; question: string; options: string[] }>;
};
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
      || new Set(member.options.map((value: string) => value.trim().toLowerCase())).size !== member.options.length) {
      throw new Error("Invalid Cortex question batch member");
    }
    ids.add(member.questionId);
    return { questionId: member.questionId, question: member.question, options: [...member.options] as string[] };
  });
  if (members[0].questionId !== questionId) throw new Error("Invalid Cortex question batch anchor");
  return { batchId, questionId, questions: members };
}
export function batchForm(batch: Pick<QuestionBatch, "questions">) {
  return { questions: batch.questions.map((member) => ({
    question: member.question, header: member.questionId,
    options: member.options.map((label) => ({ label })), multiSelect: false, allowOther: true,
  })) };
}
export function batchAnswers(batch: Pick<QuestionBatch, "questions">, answers: unknown) {
  if (!answers || typeof answers !== "object" || Array.isArray(answers)
    || Object.keys(answers).length !== batch.questions.length) throw new Error("Answer every Cortex question before submitting");
  const values = answers as Record<string, unknown>;
  return batch.questions.map(({ questionId }) => {
    const answer = Object.hasOwn(values, questionId) ? values[questionId] : undefined;
    if (typeof answer !== "string" || !answer.trim() || answer.length > 32000) throw new Error("Answer every Cortex question before submitting");
    return { questionId, answer };
  });
}
