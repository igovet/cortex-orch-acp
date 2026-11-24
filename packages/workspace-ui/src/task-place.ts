import { createContext, useContext } from "solid-js";
import type { Sprint } from "@cortex/workspace-domain";
import type { Route } from "./router";

/**
 * Where a task's conversation and workers are read. An ordinary task reads on its own page. The task that plans a sprint has
 * no page of its own: the same conversation and workers read on its sprint's page, so every link inside them leads there,
 * and its messages, answers, Stop and Resume go to the sprint's planning, which continues that task.
 */
export type TaskPlace = { sprint: () => Sprint; changingRequest?: () => boolean };
export const PlanningPlace = createContext<TaskPlace | null>(null);

/** The sprint whose planning task is on view, or null on an ordinary task's page. */
export function usePlanningSprint(): () => Sprint | null {
  const place = useContext(PlanningPlace);
  return () => place?.sprint() ?? null;
}

/** The routes inside a task's conversation and workers: its own page for an ordinary task, its sprint's tabs for a planning task. */
export function useTaskRoutes(task: () => { machine: string; id: string }) {
  const sprint = usePlanningSprint();
  return {
    conversation(): Route {
      const s = sprint();
      return s ? { name: "sprint", machine: task().machine, project: s.projectId, sprint: s.id, tab: "planning" } : { name: "task", machine: task().machine, task: task().id };
    },
    workers(worker?: string): Route {
      const s = sprint();
      return s ? { name: "sprint", machine: task().machine, project: s.projectId, sprint: s.id, tab: "workers", ...(worker ? { worker } : {}) }
        : { name: "task", machine: task().machine, task: task().id, tab: "workers", ...(worker ? { worker } : {}) };
    },
  };
}
