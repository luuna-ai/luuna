import { expect, test } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ByteCount, EMPTY_CHECK_ROW, PlanReview, SetupChecksEditor, TEXT_LIMITS, TicketCard, TicketEditor, Workflows, parseBudgets, planRequestFromRows, setupChecksFromRows, ticketEditRequest, ticketTextSizes } from "../Workflows";
import { planBudgetProblem, planRequestForTasks } from "@luuna/api-client";
import type { PlanRevision, RunState, Ticket } from "@luuna/api-client";
import { planBlocker, planSupersedes } from "../workflow-state";
import { triageFields } from "./model";
import { card, noActions, pendingEvidence, planOf, runOf, ticket } from "./fixtures.test-support";

const planButton = (html: string) => html.match(/<button[^>]*>Plan r\d+<\/button>/)?.[0] ?? "";

// The ticket screen as rendered: the plan review, the ticket card, the forms and the setup plan.
test("approval presents every immutable field and full hash before acknowledgment", () => {
  const plan = { id: "plan-1", revision: 7, hash: `sha256:${"abcdef01".repeat(8)}`, permissions: { operations: ["write"] }, required_checks: [{ command: "private-check" }], tasks: [{ acceptance_criteria: ["all criteria visible"] }], manifest_hash: "manifest-identity", future_field: "also visible", component_context: { memory: [{ id: "memory-1", scope: "user", hash: "sha256:memory", content: "Explicitly selected knowledge", provenance: "Developer supplied note" }], prompt: { name: "guidance", version: 2, hash: "sha256:prompt", body: "Exact selected prompt content" } } } as unknown as PlanRevision;
  const html = renderToStaticMarkup(<PlanReview plan={plan} canApprove busy={false} onApprove={() => { throw new Error("unreviewed approval"); }} />);
  expect(html).toContain(plan.hash);
  for (const value of ["permissions", "operations", "private-check", "all criteria visible", "manifest-identity", "also visible", "Explicitly selected knowledge", "Developer supplied note", "Exact selected prompt content", "sha256:memory", "sha256:prompt"]) expect(html).toContain(value);
  expect(html).toMatch(/<button[^>]*disabled/);
  expect(html).toContain("I reviewed this complete plan and hash.");
});

test("a new plan revision is held back while the latest revision's work is active, and confirmed when it leaves work behind", () => {
  const writing = card(runOf("run-1", "implementing"), { evidence: null, active: true, repair_allowed: false, revalidate_allowed: false });
  expect(planButton(writing)).toContain("disabled");
  expect(writing).toContain("Plan r2 is available once r1&#x27;s active work settles; cancel it to stop sooner.");
  expect(writing).not.toContain("anyway");
  const reviewing = card(runOf("run-1", "validating"), { evidence: { ...pendingEvidence, status: "reviewing" }, active: true, repair_allowed: false, revalidate_allowed: false });
  expect(planButton(reviewing)).toContain("disabled");
  // A pending candidate is never a dead end: when the runtime refuses its validation (a moved
  // manifest, spent budgets), confirming the new revision is the way on.
  const waiting = card(runOf("run-1", "validating"), { evidence: pendingEvidence, active: false, repair_allowed: false, revalidate_allowed: false, validate_allowed: false, decision_reasons: { validate: "manifest changed; re-plan", repair: null, revalidate: null } });
  expect(planButton(waiting)).toContain("disabled");
  expect(waiting).not.toContain("is available once");
  expect(waiting).toMatch(/<label class="plan-supersede"><input type="checkbox"[^>]*\/> Plan r2 anyway: r1&#x27;s candidate is waiting for validation and can no longer be validated once r2 exists.<\/label>/);
  for (const [state, left] of [["approved", "approval no longer authorizes"], ["reviewed", "reviewed candidate keeps its evidence"]] as const) {
    const html = card(runOf("run-1", state), { evidence: state === "reviewed" ? { ...pendingEvidence, status: "reviewed" } : null, active: false, repair_allowed: false, revalidate_allowed: false });
    expect(planButton(html), state).toContain("disabled");
    expect(html, state).toContain(left);
  }
  for (const state of ["paused", "failed", "interrupted", "cancelled", "plan_ready"] as RunState[]) {
    const settled = card(runOf("run-1", state), { evidence: state === "plan_ready" ? null : { ...pendingEvidence, status: state === "cancelled" ? "cancelled" : "failed" }, active: false, repair_allowed: false, revalidate_allowed: false });
    expect(planButton(settled), state).not.toContain("disabled");
    expect(settled, state).not.toContain("is available once");
    expect(settled, state).not.toContain("anyway");
  }
  expect(planBlocker(1)).toBe("");
  expect(planSupersedes(1)).toBe("");
  expect(planSupersedes(1, runOf("run-1", "validating"), { evidence: { ...pendingEvidence, status: "validating" }, active: true, repair_allowed: false, revalidate_allowed: false })).toBe("");
});

test("the first writer attempt can be cancelled from the ticket card and says what runs", () => {
  const writing = card(runOf("run-1", "implementing"), { evidence: null, active: true, repair_allowed: false, revalidate_allowed: false });
  expect(writing).toContain("Cancel workflow");
  expect(writing).toContain("The runtime is running the scoped writer for plan r1. Closing this view does not cancel the workflow.");
  // Once a candidate exists its review owns the cancel, so the card never shows two.
  const repairing = card(runOf("run-1", "implementing"), { evidence: pendingEvidence, active: true, repair_allowed: false, revalidate_allowed: false });
  expect(repairing.match(/Cancel workflow/g)).toHaveLength(1);
  expect(card(runOf("run-1", "approved"), { evidence: null, active: false, repair_allowed: false, revalidate_allowed: false })).not.toContain("Cancel workflow");
});

test("a recorded reason under the ticket names the refusal instead of a refused action, and earlier revisions stay visible", () => {
  const paused = card(runOf("run-2", "paused", 1, 2), { evidence: { ...pendingEvidence, status: "paused" }, active: false, repair_allowed: false, revalidate_allowed: false, decision_reasons: { validate: "x", repair: "repair limit reached: 2 of 2 repairs used", revalidate: "x" } }, "independent review did not pass; repair or re-plan required", [{ plan: planOf(1), run: runOf("run-1", "cancelled") }]);
  expect(paused).toContain("Repair is not available: repair limit reached: 2 of 2 repairs used. Read the findings, then create the next plan revision.");
  expect(paused).not.toContain("repair the candidate or");
  expect(paused).toContain("Earlier revisions: <span>r1 <span class=\"state\" data-tone=\"neutral\">cancelled</span></span>");
  expect(paused).toContain("budget 2 attempt(s), 600s, $1.00");
  const noChange = card(runOf("run-1", "failed"), { evidence: null, active: false, repair_allowed: false, revalidate_allowed: false }, "writer settled without changing any file");
  expect(noChange).toContain("The writer changed nothing. Create the next plan revision with the paths the change needs");
});

test("plan budgets are entered with the runtime's defaults and bounds", () => {
  const html = renderToStaticMarkup(<Workflows client={null} projectId="p" repositoryId="app" manifestConfirmed onChanged={() => {}} />);
  for (const field of ["Writer attempts<input type=\"number\" inputMode=\"numeric\" min=\"1\" max=\"10\" step=\"1\" value=\"2\"", "Time budget (seconds)<input type=\"number\" inputMode=\"numeric\" min=\"1\" max=\"86400\" step=\"1\" value=\"600\"", "Cost budget (USD)<input type=\"number\" inputMode=\"decimal\" min=\"0.000001\" max=\"1000\" step=\"any\" value=\"1.00\""]) expect(html.replace(/ id="[^"]*" name="[^"]*"/g, "")).toContain(field);
  expect(html).toContain("cumulative across the writer, checks, independent review and repairs");
  expect(parseBudgets("3", "1800", "2.5")).toEqual({ max_attempts: 3, timeout_seconds: 1800, cost_usd_micros: 2_500_000 });
  expect(parseBudgets("2", "600", "1.00")).toEqual({ max_attempts: 2, timeout_seconds: 600, cost_usd_micros: 1_000_000 });
  expect(planBudgetProblem(parseBudgets("", "600", "1"))).toBe("budgets.max_attempts must be between 1 and 10");
  expect(planBudgetProblem(parseBudgets("2", "90000", "1"))).toBe("budgets.timeout_seconds must be between 1 and 86400");
  expect(planBudgetProblem(parseBudgets("2", "600", "1000.01"))).toBe("budgets.cost_usd_micros must be between 1 and 1000000000");
  expect(planBudgetProblem(parseBudgets("2", "600", "abc"))).toBe("budgets.cost_usd_micros must be between 1 and 1000000000");
});

test("ticket text is counted in UTF-8 bytes with a visible counter, as the runtime counts it", () => {
  const html = renderToStaticMarkup(<Workflows client={null} projectId="p" repositoryId="app" manifestConfirmed onChanged={() => {}} />);
  expect(html).toContain("0 of 200 bytes");
  expect(html).toContain("Longest criterion: 0 of 16000 bytes");
  expect(html).not.toMatch(/maxLength="200"/);
  const cjk = ticketTextSizes("漢".repeat(100), "intent", ["short", "é".repeat(10)]);
  expect(cjk).toEqual({ title: 300, intent: 6, criterion: 20 });
  expect(cjk.title > TEXT_LIMITS.title).toBe(true);
  expect(renderToStaticMarkup(<ByteCount id="x" bytes={300} limit={200} />)).toContain("300 of 200 bytes · too long");
  expect(ticketTextSizes("", "", []).criterion).toBe(0);
});

test("the approver sees cumulative usage including unpriced provider work", () => {
  const html = renderToStaticMarkup(<TicketCard ticket={{ ...ticket, usage: { workflows: 3, attempts: 4, repairs: 1, elapsed_seconds: 77, cost_usd_micros: 250000, cost_unreported: true } }} revisions={[{ plan: planOf(2), run: runOf("run-1", "plan_ready") }]} snapshots={new Map()} diffs={{}} logs={{}} busy={false} connected actions={noActions} />);
  expect(html).toContain("3 workflow(s)");
  expect(html).toContain("$0.2500 recorded or reserved");
  expect(html).toContain("Some provider usage has no reported price");
  expect(html.indexOf("Cumulative ticket usage")).toBeLessThan(html.indexOf("Approve r2"));
});

test("the task-graph editor and the ticket scope appear only for a multi-repository project; the single-task form stays the default", () => {
  const single = renderToStaticMarkup(<Workflows client={null} projectId="p" repositoryId="app" manifestConfirmed onChanged={() => {}} />);
  expect(single).toContain("Writer paths (comma separated)");
  expect(single).not.toContain("Plan a task graph across repositories");
  expect(single).not.toContain("Repositories the ticket may touch");
  const multi = renderToStaticMarkup(<Workflows client={null} projectId="p" repositoryId="app" repositoryIds={["app", "web"]} manifestConfirmed onChanged={() => {}} />);
  expect(multi).toContain("Writer paths (comma separated)");
  expect(multi).toContain("Plan a task graph across repositories");
  expect(multi).toContain('aria-label="Repositories the ticket may touch"');
  expect(multi).toMatch(/<input type="checkbox"[^>]*checked=""[^>]*\/> app/);
  expect(multi).toMatch(/<input type="checkbox"[^>]*\/> web/);
});

test("a ticket edit sends only the changed fields at the displayed revision, and the editor names the plans it stales", () => {
  expect(ticketEditRequest(ticket, { title: ticket.title, intent: ticket.intent, criteria: "prints", repositoryIds: ["app"] })).toBeNull();
  expect(ticketEditRequest(ticket, { title: " Edited ", intent: ticket.intent, criteria: "prints", repositoryIds: ["app"] })).toEqual({ expected_revision: 1, title: "Edited" });
  expect(ticketEditRequest({ ...ticket, revision: 3 }, { title: ticket.title, intent: "Print it twice", criteria: "prints\n\n twice ", repositoryIds: ["app", "web"] })).toEqual({ expected_revision: 3, intent: "Print it twice", acceptance_criteria: ["prints", "twice"], repository_ids: ["app", "web"] });
  // Type, priority and labels are sent only when they differ (ADR 0058); labels are lowercased and deduplicated.
  const base = { title: ticket.title, intent: ticket.intent, criteria: ticket.acceptance_criteria.join("\n"), repositoryIds: ticket.repository_ids };
  expect(ticketEditRequest(ticket, { ...base, type: "task", priority: 2, labels: "" })).toBeNull();
  expect(ticketEditRequest(ticket, { ...base, type: "bug", priority: 0, labels: "UI, docs, ui" })).toEqual({ expected_revision: 1, type: "bug", priority: 0, labels: ["ui", "docs"] });
  expect(ticketEditRequest({ ...ticket, labels: ["ui"] }, { ...base, labels: "" })).toEqual({ expected_revision: 1, labels: [] });
  expect(triageFields("task", 2, " ")).toEqual({});
  expect(triageFields("epic", 4, "a,b")).toEqual({ type: "epic", priority: 4, labels: ["a", "b"] });
  expect(ticketEditRequest(ticket, { title: "", intent: "", criteria: "", repositoryIds: [] })).toBeNull();
  const html = renderToStaticMarkup(<TicketEditor ticket={ticket} repositoryIds={["app", "web"]} busy={false} latestRevision={2} onSave={() => { throw new Error("no unchanged save"); }} />);
  expect(html).toContain("Edit ticket r1");
  expect(html).toContain('data-capability="tickets.update"');
  expect(html).toMatch(/<button class="primary" disabled=""[^>]*>Save ticket r2<\/button>/);
  expect(html).toContain("plan r2 was made for r1 and must be re-planned");
  expect(html).toMatch(/<input type="checkbox"[^>]*checked=""[^>]*\/> app/);
  expect(html).toMatch(/<input type="checkbox"[^>]*\/> web/);
  const card = renderToStaticMarkup(<TicketCard ticket={ticket} revisions={[]} snapshots={new Map()} diffs={{}} logs={{}} busy={false} connected actions={noActions} />);
  expect(card).toContain('data-capability="tickets.update"');
});

test("a setup plan sends the typed check recipes in the manifest-check shape, the body the TUI sends for --check", () => {
  const rows = [{ id: "unit", executable: "python", args: "-m unittest", cwd: "", timeout: "120" }, { id: " ", executable: "ignored", args: "", cwd: "", timeout: "1" }, { id: "lint", executable: "ruff", args: "", cwd: "src", timeout: "60" }];
  expect(setupChecksFromRows(rows)).toEqual([{ id: "unit", executable: "python", args: ["-m", "unittest"], timeout_seconds: 120 }, { id: "lint", executable: "ruff", args: [], cwd: "src", timeout_seconds: 60 }]);
  const budgets = parseBudgets("2", "600", "1.00");
  const request = planRequestFromRows([{ repository_id: "app", paths: "tests", write: true, depends_on: [] }], { ...budgets, max_workers: 1 }, undefined, [], setupChecksFromRows(rows));
  expect(request).toEqual({ task: { repository_id: "app", paths: ["tests"], operations: ["read", "write"] }, budgets: { max_attempts: 2, timeout_seconds: 600, cost_usd_micros: 1_000_000 }, checks: [{ id: "unit", executable: "python", args: ["-m", "unittest"], timeout_seconds: 120, capabilities: [], required: true }, { id: "lint", executable: "ruff", args: [], cwd: "src", timeout_seconds: 60, capabilities: [], required: true }] });
  expect(request).toEqual(planRequestForTasks([{ repository_id: "app", paths: ["tests"], operations: ["read", "write"], depends_on: [] }], { ...budgets, max_workers: 1 }, { checks: setupChecksFromRows(rows) }));
  // A feature plan carries no checks key.
  expect(planRequestFromRows([{ repository_id: "app", paths: "src", write: true, depends_on: [] }], budgets)).not.toHaveProperty("checks");
  const setupTicket: Ticket = { ...ticket, purpose: "testing_setup", title: "Establish tests" };
  const planned: unknown[] = [];
  const html = renderToStaticMarkup(<TicketCard ticket={setupTicket} revisions={[]} snapshots={new Map()} diffs={{}} logs={{}} busy={false} connected actions={{ ...noActions, plan: (...args) => planned.push(args) }} />);
  expect(html).toContain("testing setup</span>");
  expect(html).toContain('aria-label="Setup check recipes for ticket-1"');
  expect(html).toContain("Plan setup</button>");
  expect(renderToStaticMarkup(<SetupChecksEditor ticket={setupTicket} rows={[EMPTY_CHECK_ROW]} busy={false} onChange={() => {}} />)).toContain('value="600"');
  expect(renderToStaticMarkup(<TicketCard ticket={ticket} revisions={[]} snapshots={new Map()} diffs={{}} logs={{}} busy={false} connected actions={noActions} />)).not.toContain("Setup check recipes");
});
