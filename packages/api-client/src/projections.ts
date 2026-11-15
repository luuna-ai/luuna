import type { BaselineEvidence, BaselineStatus, ProjectView, RunRecord, RunState, TestingPrerequisite, Ticket, TicketPurpose } from "./index.ts";

/**
 * Read-only projections of authoritative state for the experience surfaces of docs/09-experience.md
 * (Home and Kanban; ADR 0027 point 4). Both clients render these from the same records, so the
 * desktop panels and `luuna-tui home` / `luuna-tui board` show one projection. Nothing here
 * decides anything: a card opens the existing panel of its ticket, and no column can be entered
 * by moving a card, because the runtime's run state is the only source of a column.
 */

export type BoardColumnId = "backlog" | "testing_prerequisite" | "plan" | "approval" | "implementation" | "checks" | "review" | "prs";

export interface BoardColumn {
  id: BoardColumnId;
  title: string;
  /** Which authoritative states land here, in the runtime's words. */
  description: string;
}

/** The columns of docs/09-experience.md, in order. */
export const BOARD_COLUMNS: readonly BoardColumn[] = [
  { id: "backlog", title: "Backlog", description: "Tickets without a plan revision." },
  { id: "testing_prerequisite", title: "Testing prerequisite", description: "Repositories whose latest baseline shows no usable testing setup (the runtime offers a testing-setup ticket), and workflows blocked by a check that could not run (testing_blocked)." },
  { id: "plan", title: "Discovery / plan", description: "Workflows in discovery or exploration (discovered, exploring, queued, running), and settled workflows whose next step is the next plan revision (completed, failed, cancelled, interrupted)." },
  { id: "approval", title: "Approval", description: "Plan revisions waiting for the developer's exact-hash approval (plan_ready)." },
  { id: "implementation", title: "Implementation", description: "Approved plans and running writers (approved, implementing)." },
  { id: "checks", title: "Checks", description: "Candidates whose required checks run or wait to run (validating)." },
  { id: "review", title: "Review", description: "Independent review in progress, or a review that did not pass and waits for a repair or a new revision (reviewing, paused)." },
  { id: "prs", title: "PRs", description: "Reviewed candidates; publishing and merging stay with the developer (reviewed)." },
];

/** The sentence both clients show beside the board. */
export const BOARD_NOTE = "This board is a projection of the runtime's recorded run states. Cards are read-only and open the ticket's own panel; dragging a card is not implemented, because moving a card can never bypass a validation gate or an approval.";

/** The column a workflow run's authoritative state projects to; exhaustive, so a new state fails typecheck. */
export const RUN_COLUMN: Record<RunState, BoardColumnId> = {
  queued: "plan",
  running: "plan",
  discovered: "plan",
  exploring: "plan",
  completed: "plan",
  failed: "plan",
  cancelled: "plan",
  interrupted: "plan",
  testing_blocked: "testing_prerequisite",
  plan_ready: "approval",
  approved: "implementation",
  implementing: "implementation",
  validating: "checks",
  reviewing: "review",
  paused: "review",
  reviewed: "prs",
};

export interface BoardCard {
  column: BoardColumnId;
  kind: "ticket" | "prerequisite";
  project_id: string;
  title: string;
  ticket_id?: string;
  ticket_revision?: number;
  purpose?: TicketPurpose;
  run_id?: string;
  state?: RunState;
  plan_revision?: number;
  repository_id?: string;
  reason?: string;
}

export interface BoardInput {
  projects: ProjectView[];
  tickets: Ticket[];
  runs: RunRecord[];
}

export interface BoardProjection {
  columns: { column: BoardColumn; cards: BoardCard[] }[];
  note: string;
}

/** The workflow run of a ticket's latest plan revision, if any. */
export function latestWorkflow(ticket: Pick<Ticket, "id">, runs: RunRecord[]): RunRecord | undefined {
  let latest: RunRecord | undefined;
  for (const run of runs) {
    if (run.metadata.kind !== "workflow" || run.metadata.ticket_id !== ticket.id) continue;
    const revision = Number(run.metadata.plan_revision ?? 0);
    const seen = latest ? Number(latest.metadata.plan_revision ?? 0) : -1;
    if (!latest || revision > seen || (revision === seen && run.updated_at > latest.updated_at)) latest = run;
  }
  return latest;
}

/** Every ticket as a card in the column of its latest workflow's state, plus one card per testing-setup offer. */
export function boardProjection(input: BoardInput): BoardProjection {
  const cards: BoardCard[] = [];
  for (const view of input.projects) {
    for (const prerequisite of view.prerequisites ?? []) {
      cards.push({ column: "testing_prerequisite", kind: "prerequisite", project_id: view.project.id, title: `Testing setup needed for ${prerequisite.repository_id}`, repository_id: prerequisite.repository_id, reason: prerequisite.reason });
    }
  }
  for (const ticket of input.tickets) {
    const run = latestWorkflow(ticket, input.runs);
    cards.push({
      column: run ? RUN_COLUMN[run.state] : "backlog",
      kind: "ticket",
      project_id: ticket.project_id,
      title: ticket.title,
      ticket_id: ticket.id,
      ticket_revision: ticket.revision,
      ...(ticket.purpose && ticket.purpose !== "feature" && { purpose: ticket.purpose }),
      ...(run && { run_id: run.id, state: run.state, plan_revision: Number(run.metadata.plan_revision ?? 0) }),
    });
  }
  return { columns: BOARD_COLUMNS.map((column) => ({ column, cards: cards.filter((card) => card.column === column.id) })), note: BOARD_NOTE };
}

export interface RepositoryHealth {
  repository_id: string;
  local_path: string;
  /** The latest recorded baseline of this repository, or null before one exists. */
  baseline: { run_id: string; status: BaselineStatus; manifest_revision: number; commit: string; reason?: string } | null;
  prerequisite: TestingPrerequisite | null;
}

export interface HomeProject {
  id: string;
  name: string;
  revision: number;
  manifest_hash: string | null;
  repositories: RepositoryHealth[];
  tickets: number;
  /** Workflows of this project whose run has not settled. */
  active_workflows: number;
}

export interface HomeBlocker {
  kind: "prerequisite" | "workflow";
  project_id: string;
  title: string;
  detail: string;
  repository_id?: string;
  run_id?: string;
  state?: RunState;
  ticket_id?: string;
}

export interface HomeInput {
  projects: ProjectView[];
  /** Recorded baselines per project id. */
  baselines: Record<string, BaselineEvidence[]>;
  /** Tickets per project id. */
  tickets: Record<string, Ticket[]>;
  runs: RunRecord[];
}

export interface HomeSummary {
  projects: HomeProject[];
  blockers: HomeBlocker[];
}

/** Workflow states that wait on the developer before anything can continue. */
export const BLOCKING_RUN_STATES: readonly RunState[] = ["failed", "paused", "interrupted", "testing_blocked"];
const SETTLED: readonly RunState[] = ["completed", "failed", "cancelled", "interrupted", "reviewed"];

/** The latest baseline per registered repository, in runtime order (newest first). */
export function latestBaselinePerRepository(baselines: BaselineEvidence[], repositoryIds: string[]): Record<string, BaselineEvidence | undefined> {
  return Object.fromEntries(repositoryIds.map((id) => [id, baselines.find((baseline) => baseline.identity.repository_id === id)]));
}

/** Projects with their validation health (latest baseline per repository) and the blockers waiting on the developer. */
export function homeSummary(input: HomeInput): HomeSummary {
  const projects: HomeProject[] = [];
  const blockers: HomeBlocker[] = [];
  for (const view of input.projects) {
    const id = view.project.id;
    const repositoryIds = view.repositories.map((repository) => repository.repository_id);
    const latest = latestBaselinePerRepository(input.baselines[id] ?? [], repositoryIds);
    const prerequisites = view.prerequisites ?? [];
    const tickets = input.tickets[id] ?? [];
    const workflows = input.runs.filter((run) => run.project_id === id && run.metadata.kind === "workflow");
    projects.push({
      id,
      name: view.project.name,
      revision: view.project.revision,
      manifest_hash: view.manifest?.hash ?? null,
      repositories: view.repositories.map((repository) => {
        const baseline = latest[repository.repository_id];
        return {
          repository_id: repository.repository_id,
          local_path: repository.local_path,
          baseline: baseline ? { run_id: baseline.run_id, status: baseline.status, manifest_revision: baseline.manifest_revision, commit: baseline.identity.commit, ...(baseline.reason && { reason: baseline.reason }) } : null,
          prerequisite: prerequisites.find((offer) => offer.repository_id === repository.repository_id) ?? null,
        };
      }),
      tickets: tickets.length,
      active_workflows: workflows.filter((run) => !SETTLED.includes(run.state)).length,
    });
    for (const prerequisite of prerequisites) {
      blockers.push({ kind: "prerequisite", project_id: id, title: `Testing setup needed for ${prerequisite.repository_id}`, detail: prerequisite.reason, repository_id: prerequisite.repository_id });
    }
    for (const ticket of tickets) {
      const run = latestWorkflow(ticket, workflows);
      if (!run || !BLOCKING_RUN_STATES.includes(run.state)) continue;
      blockers.push({ kind: "workflow", project_id: id, title: ticket.title, detail: `workflow ${run.id} (plan r${String(run.metadata.plan_revision ?? "?")}) is ${run.state.replace(/_/g, " ")}`, run_id: run.id, state: run.state, ticket_id: ticket.id });
    }
  }
  return { projects, blockers };
}
