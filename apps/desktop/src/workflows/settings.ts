import { useCallback, useState } from "react";
import type { DependencyOverride, TicketPurpose, TicketType } from "@luuna/api-client";
import { DEFAULT_PLAN_BUDGETS } from "@luuna/api-client";
import type { TaskRow } from "./model";

/**
 * What the next plan, approval and runtime action use: the single-task form (paths, writes) or
 * the task graph (ADR 0017), the dependency overrides (ADR 0023), the approver, the budgets and
 * the provider and model for the next action.
 */
export interface RunSettings {
  paths: string;
  write: boolean;
  /** The task graph editor: empty is the single-task form; adding a row opens the graph. */
  taskRows: TaskRow[];
  maxWorkers: string;
  /** Shown once the runtime refuses a plan for a relationship, or when a row exists. */
  overrideRows: DependencyOverride[];
  approver: string;
  provider: string;
  model: string;
  attempts: string;
  timeoutSeconds: string;
  costDollars: string;
  autoRepair: boolean;
}

export const DEFAULT_RUN_SETTINGS: RunSettings = {
  paths: "src",
  write: true,
  taskRows: [],
  maxWorkers: "1",
  overrideRows: [],
  approver: "developer",
  provider: "",
  model: "",
  attempts: String(DEFAULT_PLAN_BUDGETS.max_attempts),
  timeoutSeconds: String(DEFAULT_PLAN_BUDGETS.timeout_seconds),
  costDollars: (DEFAULT_PLAN_BUDGETS.cost_usd_micros / 1_000_000).toFixed(2),
  autoRepair: false,
};

export type Patch<T> = Partial<T> | ((current: T) => Partial<T>);

/** State with a merge-patch setter, so a form can change one field without restating the rest. */
export function usePatchState<T extends object>(initial: T): [T, (patch: Patch<T>) => void] {
  const [value, setValue] = useState<T>(initial);
  const update = useCallback((patch: Patch<T>) => setValue((current) => ({ ...current, ...(typeof patch === "function" ? patch(current) : patch) })), []);
  return [value, update];
}

/** The provider and model the next runtime action uses, sent only when given. */
export function modelSettings(settings: Pick<RunSettings, "provider" | "model">): { provider?: string; model?: string } {
  return { ...(settings.provider.trim() && { provider: settings.provider.trim() }), ...(settings.model.trim() && { model: settings.model.trim() }) };
}

/** The ticket form as typed. */
export interface TicketFormState {
  title: string;
  intent: string;
  criteria: string;
  purpose: TicketPurpose;
  repositories: string[];
  /** Missions type, priority and comma-separated labels (ADR 0058). */
  ticketType: TicketType;
  priority: number;
  labels: string;
}

export function emptyTicketForm(repositoryId: string): TicketFormState {
  return { title: "", intent: "", criteria: "", purpose: "feature", repositories: [repositoryId], ticketType: "task", priority: 2, labels: "" };
}
