import type { ComponentContext, CreatePlanRequest, DependencyOverride, PlanBudgetRequest, RunRecord, SetupCheckRequest, TaskRecord, TaskRole, TaskState, TestingPrerequisite, Ticket, TicketPurpose, TicketType, UpdateTicketRequest } from "@luuna/api-client";
import { TASK_ROLES, advisoryRole, planRequestForTasks } from "@luuna/api-client";
import { retryIdentity } from "../ProjectComponents";
import { utf8Bytes } from "../failures";

/** The runtime's ticket and approver limits, in UTF-8 bytes. */
export const TEXT_LIMITS = { title: 200, intent: 16_000, criterion: 16_000, approver: 200 } as const;

/** Budget fields as typed; the cost is entered in dollars and sent in micro-dollars. */
export function parseBudgets(attempts: string, timeoutSeconds: string, costDollars: string): PlanBudgetRequest {
  const number = (text: string) => (text.trim() === "" ? Number.NaN : Number(text));
  return { max_attempts: number(attempts), timeout_seconds: number(timeoutSeconds), cost_usd_micros: Math.round(number(costDollars) * 1_000_000) };
}

/**
 * One row of the desktop's task-graph editor (ADR 0017): a repository, its writer paths and the
 * rows it waits for; since ADR 0023 also its role (an advisory role reads only) and the advisory
 * task of an earlier revision it derives from.
 */
export interface TaskRow { repository_id: string; paths: string; write: boolean; depends_on: string[]; role?: TaskRole; derived_from_task?: string }

/**
 * The plan request the desktop sends, through the builder the TUI shares (R02): one row is the
 * single-task form in its pre-M4 shape; several rows are a task graph whose task ids are the
 * repository ids, with `max_workers` sent only when it is not the default. Roles and derivations
 * travel only when given, `overrides` are the plan's dependency overrides (ADR 0023) and `checks`
 * a testing-setup plan's proposed recipes (ADR 0022).
 */
export function planRequestFromRows(rows: TaskRow[], budgets: PlanBudgetRequest, componentContext?: ComponentContext, overrides: DependencyOverride[] = [], checks: SetupCheckRequest[] = []): Omit<CreatePlanRequest, "idempotency_key"> {
  return planRequestForTasks(rows.map((row) => ({
    repository_id: row.repository_id,
    paths: row.paths.split(","),
    operations: row.write && !advisoryRole(row.role) ? ["read", "write"] : ["read"],
    depends_on: row.depends_on,
    ...(row.role && { role: row.role }),
    ...(row.derived_from_task?.trim() && { derived_from_task: row.derived_from_task }),
  })), budgets, { componentContext, dependencyOverrides: overrides, checks });
}

/** One row of the setup-plan form: a check recipe as the developer types it (ADR 0022). */
export interface CheckRow { id: string; executable: string; args: string; cwd: string; timeout: string }

export const EMPTY_CHECK_ROW: CheckRow = { id: "unit", executable: "", args: "", cwd: "", timeout: "600" };

/** The recipes a setup plan proposes, in the manifest-check shape; rows without an id or an executable are skipped. */
export function setupChecksFromRows(rows: CheckRow[]): SetupCheckRequest[] {
  return rows.filter((row) => row.id.trim() && row.executable.trim()).map((row) => ({
    id: row.id.trim(),
    executable: row.executable.trim(),
    args: row.args.split(/\s+/).filter(Boolean),
    ...(row.cwd.trim() && { cwd: row.cwd.trim() }),
    timeout_seconds: Number(row.timeout),
  }));
}

/** The ticket the offer pre-fills: purpose testing_setup, scoped to the offered repository, naming the runtime's reason and next step. */
export function setupTicketDraft(offer: TestingPrerequisite): { purpose: TicketPurpose; title: string; intent: string; criteria: string; repositoryIds: string[] } {
  return {
    purpose: "testing_setup",
    title: `Establish the testing setup for ${offer.repository_id}`,
    intent: `Repository ${offer.repository_id} has no usable testing setup: ${offer.reason}. Add the test harness and at least one real test so that the proposed required checks execute a counted test in the candidate worktree. Runtime next step: ${offer.next}`,
    criteria: ["The proposed required check recipes run in the candidate worktree and exit 0", "A recognised test runner reports at least one executed test", "No feature code changes"].join("\n"),
    repositoryIds: [offer.repository_id],
  };
}

/**
 * The advisory tasks a plan row of this ticket can derive from (ADR 0042): only those of runs of
 * this ticket's own plan, because the runtime accepts a derivation only from an earlier revision of
 * the same ticket's plan. Tasks are known only for runs whose task graph was opened.
 */
export function advisoryTasksForTicket(planIds: readonly string[], runs: readonly RunRecord[], inspected: Readonly<Record<string, TaskRecord[]>>): TaskRecord[] {
  const own = new Set(runs.filter((run) => planIds.includes(String(run.metadata.plan_id ?? ""))).map((run) => run.id));
  return Object.entries(inspected).filter(([runId]) => own.has(runId)).flatMap(([, tasks]) => tasks).filter((task) => advisoryRole(task.role));
}

/** The rows the derive convenience pre-fills from an advisory task's proposal (ADR 0023 point 4): every task derives from the advisory task. */
export function rowsFromProposal(task: TaskRecord): TaskRow[] {
  const proposed = task.advisory_result?.proposed_tasks ?? [];
  const ids = proposed.map((item) => item.repository_id);
  return proposed.map((item) => {
    const role = TASK_ROLES.includes(item.role as TaskRole) ? item.role as TaskRole : "implementer";
    const depends_on = (item.depends_on ?? []).map((dependency) => proposed.find((other) => other.id === dependency)?.repository_id ?? dependency).filter((dependency) => ids.includes(dependency));
    return { repository_id: item.repository_id, paths: (item.paths ?? []).join(",") || "src", write: !advisoryRole(role), depends_on, role, derived_from_task: task.task_id };
  });
}

/** Task states `POST …/implement` with `task_id` resumes (ADR 0017 point 8). */
export const RESUMABLE_TASK_STATES: readonly TaskState[] = ["failed", "blocked", "interrupted"];

/** Whether a task of this run can be resumed under the original approval: the run settled failed or interrupted and the task did not complete. */
export function taskResumable(run: Pick<RunRecord, "state">, task: Pick<TaskRecord, "state">): boolean {
  return (run.state === "failed" || run.state === "interrupted") && RESUMABLE_TASK_STATES.includes(task.state);
}

/** The Missions issue types and priorities a ticket takes (ADR 0058). */
export const TICKET_TYPES: { value: TicketType; label: string }[] = [
  { value: "task", label: "task" },
  { value: "bug", label: "bug" },
  { value: "feature", label: "feature" },
  { value: "epic", label: "epic" },
];
export const TICKET_PRIORITIES = [0, 1, 2, 3, 4].map((p) => ({ value: String(p), label: `P${p} ${["critical", "high", "medium", "low", "backlog"][p]}` }));

/** Comma-separated labels as the runtime takes them: trimmed, lowercased (as Missions stores them) and without repeats. */
export function parseLabels(text: string): string[] {
  return [...new Set(text.split(",").map((label) => label.trim().toLowerCase()).filter(Boolean))];
}

/** The type, priority and labels of a new ticket, leaving out the defaults the runtime omits. */
export function triageFields(type: TicketType, priority: number, labels: string): { type?: TicketType; priority?: number; labels?: string[] } {
  const list = parseLabels(labels);
  return { ...(type !== "task" && { type }), ...(priority !== 2 && { priority }), ...(list.length && { labels: list }) };
}

/** The draft of a ticket edit as the form holds it. Absent triage fields are left as they are. */
export interface TicketDraft { title: string; intent: string; criteria: string; repositoryIds: string[]; type?: TicketType; priority?: number; labels?: string }

/**
 * The edit request for a ticket: only the fields whose value differs from the ticket, at the
 * revision the panel displayed; `null` when nothing changed.
 */
export function ticketEditRequest(ticket: Ticket, draft: TicketDraft): UpdateTicketRequest | null {
  const title = draft.title.trim();
  const intent = draft.intent.trim();
  const criteria = draft.criteria.split("\n").map((line) => line.trim()).filter(Boolean);
  const repositories = draft.repositoryIds.filter(Boolean);
  const same = (a: string[], b: string[]) => a.length === b.length && a.every((item, index) => item === b[index]);
  const request: UpdateTicketRequest = {
    expected_revision: ticket.revision,
    ...(title && title !== ticket.title && { title }),
    ...(intent && intent !== ticket.intent && { intent }),
    ...(criteria.length && !same(criteria, ticket.acceptance_criteria) && { acceptance_criteria: criteria }),
    ...(repositories.length && !same(repositories, ticket.repository_ids) && { repository_ids: repositories }),
    ...(draft.type !== undefined && draft.type !== (ticket.type ?? "task") && { type: draft.type }),
    ...(draft.priority !== undefined && draft.priority !== (ticket.priority ?? 2) && { priority: draft.priority }),
    ...(draft.labels !== undefined && !same(parseLabels(draft.labels), ticket.labels ?? []) && { labels: parseLabels(draft.labels) }),
  };
  return Object.keys(request).length > 1 ? request : null;
}

/** Each ticket field measured as the runtime measures it; criteria count their longest line. */
export function ticketTextSizes(title: string, intent: string, criteria: string[]) {
  return { title: utf8Bytes(title), intent: utf8Bytes(intent), criterion: Math.max(0, ...criteria.map(utf8Bytes)) };
}

/** Whether any measured field exceeds the runtime's limit. */
export function tooLong(sizes: ReturnType<typeof ticketTextSizes>): boolean {
  return sizes.title > TEXT_LIMITS.title || sizes.intent > TEXT_LIMITS.intent || sizes.criterion > TEXT_LIMITS.criterion;
}

/**
 * One delivery identity per plan draft: a retry after a lost response replays the revision it
 * created. The draft is the request planned on top of `latestRevision`, so once the panel shows
 * the revision it created, planning again is a new draft with a new key.
 */
export function planRetryIdentity(previous: { content: string; key: string } | null, latestRevision: number, request: Omit<CreatePlanRequest, "idempotency_key">, create?: () => string) {
  return retryIdentity(previous, JSON.stringify({ after: latestRevision, request }), create);
}

/** A runtime state as the badge shows it. */
export function stateText(state: string): string {
  return state.replace(/_/g, " ");
}

/** Dollars from the runtime's micro-dollars, to the given precision. */
export function dollars(micros: number, digits: number): string {
  return `$${(micros / 1_000_000).toFixed(digits)}`;
}
