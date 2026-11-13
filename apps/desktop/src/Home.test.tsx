import { expect, test, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { BaselineEvidence, ProjectView, RunRecord, Ticket } from "@luuna/api-client";
import { homeSummary } from "@luuna/api-client";
import { HomeSummaryView, readHome } from "./Home";
import type { OpenRequest } from "./Home";

export const offer = { kind: "testing_setup" as const, project_id: "two", repository_id: "lib", baseline_id: "b2", reason: "the manifest names no required check for repository lib", next: "POST /api/v1/projects/two/tickets with purpose testing_setup" };
export const views: ProjectView[] = [
  { project: { id: "one", name: "One", revision: 2, manifest_hash: "sha256:m", created_at: "", updated_at: "" }, repositories: [{ project_id: "one", repository_id: "app", local_path: "/srv/app", registered_at: "", observation: {} as never }], manifest: { project_id: "one", revision: 2, hash: "sha256:m", manifest: {} as never, base_commits: {}, confirmed_at: "" }, discovery: null, prerequisites: [] },
  { project: { id: "two", name: "Two", revision: 1, manifest_hash: "sha256:n", created_at: "", updated_at: "" }, repositories: [{ project_id: "two", repository_id: "lib", local_path: "/srv/lib", registered_at: "", observation: {} as never }], manifest: { project_id: "two", revision: 1, hash: "sha256:n", manifest: {} as never, base_commits: {}, confirmed_at: "" }, discovery: null, prerequisites: [offer] },
];
const baselineOf = (project_id: string, repository: string, status: BaselineEvidence["status"], extra: Partial<BaselineEvidence> = {}): BaselineEvidence => ({ id: `b-${repository}`, run_id: `run-b-${repository}`, project_id, manifest_revision: 1, identity: { manifest_hash: "sha256:m", repository_id: repository, commit: "a".repeat(40), tree: "t".repeat(40) }, environment_fingerprint: "sha256:e", status, required_checks: [], results: [], started_at: "", ...extra });
export const baselines: Record<string, BaselineEvidence[]> = { one: [baselineOf("one", "app", "passed", { id: "b-app-2", run_id: "run-b-app-2", manifest_revision: 2 }), baselineOf("one", "app", "failed")], two: [baselineOf("two", "lib", "blocked", { reason: "no required check", prerequisite: offer })] };
const ticketOf = (id: string, project_id: string, title: string, extra: Partial<Ticket> = {}): Ticket => ({ schema_version: "0.1", id, project_id, revision: 1, source: "luuna_sqlite_temporary", title, intent: "", acceptance_criteria: [], repository_ids: ["app"], created_at: "", updated_at: "", ...extra });
export const tickets: Record<string, Ticket[]> = { one: [ticketOf("t1", "one", "Greeting"), ticketOf("t2", "one", "Unplanned")], two: [ticketOf("t3", "two", "Establish tests", { revision: 2, purpose: "testing_setup", repository_ids: ["lib"] })] };
export const runs: RunRecord[] = [
  { id: "run-1", project_id: "one", state: "paused", revision: 5, metadata: { kind: "workflow", ticket_id: "t1", plan_id: "plan-1", plan_revision: 2 }, created_at: "", updated_at: "2" },
  { id: "run-0", project_id: "one", state: "failed", revision: 3, metadata: { kind: "workflow", ticket_id: "t1", plan_id: "plan-1", plan_revision: 1 }, created_at: "", updated_at: "1" },
  { id: "run-3", project_id: "two", state: "approved", revision: 2, metadata: { kind: "workflow", ticket_id: "t3", plan_id: "plan-3", plan_revision: 1 }, created_at: "", updated_at: "3" },
  { id: "run-s", project_id: "one", state: "completed", revision: 2, metadata: { prompt: "hello" }, created_at: "", updated_at: "4" },
];

/** A client double over the fixture; every read is recorded. */
export function fixtureClient() {
  const calls: string[] = [];
  const client = {
    listProjects: vi.fn(async () => { calls.push("GET /projects"); return views.map((view) => view.project); }),
    project: vi.fn(async (id: string) => { calls.push(`GET /projects/${id}`); return views.find((view) => view.project.id === id)!; }),
    listBaselines: vi.fn(async (id: string) => { calls.push(`GET /projects/${id}/baselines`); return baselines[id]; }),
    listTickets: vi.fn(async (id: string) => { calls.push(`GET /projects/${id}/tickets`); return tickets[id]; }),
    listRuns: vi.fn(async () => { calls.push("GET /runs"); return runs; }),
  };
  return { client, calls };
}

test("the Home summary projects validation health per repository and the blockers waiting on the developer, the same projection the TUI prints", async () => {
  const { client, calls } = fixtureClient();
  const summary = await readHome(client, new AbortController().signal);
  expect(summary).toEqual(homeSummary({ projects: views, baselines, tickets, runs }));
  expect(calls.every((call) => call.startsWith("GET "))).toBe(true);
  expect(summary.projects.map((project) => [project.id, project.repositories[0].baseline?.status, project.repositories[0].baseline?.manifest_revision, project.repositories[0].prerequisite !== null, project.tickets, project.active_workflows])).toEqual([["one", "passed", 2, false, 2, 1], ["two", "blocked", 1, true, 1, 1]]);
  expect(summary.blockers.map((blocker) => [blocker.kind, blocker.project_id, blocker.run_id ?? blocker.repository_id, blocker.state])).toEqual([["workflow", "one", "run-1", "paused"], ["prerequisite", "two", "lib", undefined]]);
  const opened: OpenRequest[] = [];
  const html = renderToStaticMarkup(<HomeSummaryView summary={summary} busy={false} onOpen={(request) => opened.push(request)} />);
  expect(html).toContain('aria-label="Projects and validation health"');
  expect(html).toMatch(/<span class="list-row-title"><code>app<\/code><\/span>.*?<span class="state" data-tone="success">passed<\/span>/);
  expect(html).toMatch(/<span class="list-row-title"><code>lib<\/code><\/span>.*?<span class="state" data-tone="danger">blocked<\/span>/);
  expect(html).toContain("testing setup needed</span>");
  expect(html).toContain("2 ticket(s) · 1 active workflow(s)");
  expect(html).toContain("Open One");
  expect(html).toContain('aria-label="New ticket in Two"');
  // What waits on the developer comes before the project list.
  expect(html.indexOf("Waiting on you")).toBeLessThan(html.indexOf("Projects and validation health"));
  expect(html).toContain('aria-label="Blockers waiting on you"');
  expect(html).toMatch(/<span class="state" data-tone="attention">paused<\/span><\/span><span class="list-row-main"><span class="list-row-title">Greeting<\/span>/);
  expect(html).toContain("workflow run-1 (plan r2) is paused");
  expect(html).toContain("Testing setup needed for lib");
  expect(html).toContain(offer.reason);
  // Green is earned: only the passed baseline is success; a completed workflow never is.
  expect(html.match(/data-tone="success"/g)).toHaveLength(1);
  expect(renderToStaticMarkup(<HomeSummaryView summary={{ projects: [], blockers: [] }} busy={false} onOpen={() => {}} />)).toContain("No projects yet");
  expect(renderToStaticMarkup(<HomeSummaryView summary={{ ...summary, blockers: [] }} busy={false} onOpen={() => {}} />)).toContain("Nothing is waiting on you.");
});

test("Home shows four untoned metric cards derived from the summary, with the final numbers in the markup", async () => {
  const { client } = fixtureClient();
  const summary = await readHome(client, new AbortController().signal);
  const html = renderToStaticMarkup(<HomeSummaryView summary={summary} busy={false} onOpen={() => {}} />);
  const sum = (pick: (project: (typeof summary.projects)[number]) => number) => summary.projects.reduce((total, project) => total + pick(project), 0);
  expect(html).toMatch(/<div role="group" aria-label="Home summary counts" data-cols="4" data-cols-compact="2" class="prim-grid/);
  const values = [...html.matchAll(/<dl class="metric-card"><dt>([^<]+)<\/dt><dd class="metric-value">(\d+)<\/dd>/g)].map((match) => [match[1], Number(match[2])]);
  expect(values).toEqual([["Projects", summary.projects.length], ["Waiting on you", summary.blockers.length], ["Tickets", sum((project) => project.tickets)], ["Active workflows", sum((project) => project.active_workflows)]]);
  expect(sum((project) => project.tickets)).toBe(3);
  expect(html.match(/class="metric-card"[^>]*data-tone/)).toBeNull();
  expect(html).toContain('aria-label="Blockers waiting on you"');
  const empty = renderToStaticMarkup(<HomeSummaryView summary={{ projects: [], blockers: [] }} busy={false} onOpen={() => {}} onAddProject={() => {}} />);
  expect(empty).not.toContain("metric-card");
  expect(empty).toContain("No projects yet");
  expect(empty.match(/<button/g)).toHaveLength(1);
});
