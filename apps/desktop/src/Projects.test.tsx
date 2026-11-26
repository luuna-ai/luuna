import { expect, test } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { BaselineEvidence, CheckResult, ProjectView } from "@luuna/api-client";
import { BaselineList, RepositoryBaselines, baselineArtifacts, baselineRunFailure, currentProposal, latestBaselines, registrationRequest } from "./Projects";

const result = (recipe_id: string, status: CheckResult["status"], artifact_refs: string[]): CheckResult => ({ schema_version: "0.1", id: `${recipe_id}-result`, run_id: "baseline-run", repository_id: "app", recipe_id, candidate_hash: "", commit: "c".repeat(40), environment_fingerprint: "sha256:e", command: ["python", "-m", "unittest"], status, exit_code: status === "passed" ? 0 : 1, artifact_refs });
const baseline = (status: BaselineEvidence["status"], results: CheckResult[], reason?: string): BaselineEvidence => ({ id: `b-${status}`, run_id: "baseline-run", project_id: "p", manifest_revision: 1, identity: { manifest_hash: "sha256:m", repository_id: "app", commit: "c".repeat(40), tree: "t".repeat(40) }, environment_fingerprint: "sha256:e", status, required_checks: results.map((item) => item.recipe_id), results, started_at: "", ...(reason && { reason }) });

test("a spent discovery proposal is not offered for confirmation again", () => {
  const view = (projectRevision: number, proposedRevision: number) => ({ project: { revision: projectRevision }, discovery: { proposed_manifest: { revision: proposedRevision } } }) as unknown as ProjectView;
  expect(currentProposal(view(0, 1))).not.toBeNull();
  expect(currentProposal(view(1, 1))).toBeNull();
  expect(currentProposal(view(2, 2))).toBeNull();
  expect(currentProposal({ ...view(1, 2), discovery: null })).toBeNull();
});

test("baseline logs open by the identity the artifact route serves, for current and earlier records", () => {
  expect(baselineArtifacts(result("unit", "failed", ["unit.stdout", "unit.stderr"]))).toEqual(["unit.stdout", "unit.stderr"]);
  // Records from before contract 9 carry `artifact:` references the route never served.
  expect(baselineArtifacts(result("unit", "failed", ["artifact:baseline-run:unit:stdout", "artifact:baseline-run:unit:stderr"]))).toEqual(["unit.stdout", "unit.stderr"]);
  expect(baselineArtifacts(result("../escape", "failed", []))).toEqual([]);
  const html = renderToStaticMarkup(<BaselineList baselines={[baseline("failed", [result("unit", "failed", ["artifact:baseline-run:unit:stdout"]), result("lint", "passed", ["lint.stdout", "lint.stderr"])], "required check unit failed")]} busy={false} onLog={() => {}} />);
  for (const label of ["View unit stdout", "View unit stderr", "View lint stdout", "View lint stderr"]) expect(html).toContain(label);
  expect(html).toContain("required check unit failed");
  expect(html).toContain("Open the failing recipe&#x27;s log, fix the repository or its recipe and commit, then run the baseline again.");
  expect(html).not.toContain("repair the candidate");
  const blocked = renderToStaticMarkup(<BaselineList baselines={[baseline("blocked", [result("unit", "blocked", ["unit.stdout"])], "missing dependency: command `vitest` was not found (exit code 127); add a dependency-preparation recipe")]} busy={false} onLog={() => {}} />);
  expect(blocked).toContain("Add a dependency-preparation recipe");
  expect(renderToStaticMarkup(<BaselineList baselines={[]} busy={false} onLog={() => {}} />)).toBe("");
});

test("a baseline run that settles before any record shows the reason its terminal event recorded", () => {
  const identity = baselineRunFailure("failed", "repository app identity changed since registration: remote changed from https://example.invalid/a.git to https://example.invalid/b.git");
  expect(identity).toContain("Baseline run failed: repository app identity changed since registration");
  expect(identity).toContain("Rebind repository");
  expect(baselineRunFailure("interrupted", "interrupted by runtime shutdown")).toContain("Run the baseline again");
  expect(baselineRunFailure("failed", "")).toContain("before a baseline was recorded");
});

test("registration sends every repository row the developer added, through the builder the TUI shares", () => {
  expect(registrationRequest("Walkthrough", "walkthrough", [{ id: "app", path: "/srv/app" }, { id: "web", path: "/srv/web" }])).toEqual({ name: "Walkthrough", id: "walkthrough", repositories: [{ id: "app", path: "/srv/app" }, { id: "web", path: "/srv/web" }] });
  expect(registrationRequest("One", "", [{ id: "app", path: "/srv/app" }])).toEqual({ name: "One", repositories: [{ id: "app", path: "/srv/app" }] });
  expect(() => registrationRequest("One", "", [{ id: "app", path: "/srv/app" }, { id: "app", path: "/srv/other" }])).toThrow("unique");
  expect(() => registrationRequest("One", "", [{ id: "app", path: "" }])).toThrow("absolute root path");
});

test("every registered repository gets its own baseline control and shows its own latest status", () => {
  const app = { ...baseline("passed", [result("unit", "passed", ["unit.stdout"])]), id: "b-app-1", run_id: "run-app-1", identity: { manifest_hash: "sha256:m", repository_id: "app", commit: "a".repeat(40), tree: "t".repeat(40) } };
  const appLater = { ...app, id: "b-app-2", run_id: "run-app-2", status: "failed" as const, reason: "required check unit failed" };
  const web = { ...app, id: "b-web", run_id: "run-web", identity: { ...app.identity, repository_id: "web", commit: "b".repeat(40) } };
  // The API lists baselines newest first; the later (failed) record comes before the older passed one.
  const latest = latestBaselines([appLater, web, app], ["app", "web", "shared"]);
  expect(latest.app?.id).toBe("b-app-2");
  expect(latest.web?.id).toBe("b-web");
  expect(latest.shared).toBeUndefined();
  const started: string[] = [];
  const html = renderToStaticMarkup(<RepositoryBaselines repositoryIds={["app", "web", "shared"]} baselines={[appLater, web, app]} running={{ web: "run-web-live" }} busy={false} manifestConfirmed onRun={(id) => started.push(id)} onCancel={() => {}} />);
  expect(html.match(/data-capability="baselines.start"/g)).toHaveLength(3);
  for (const label of ["Run baseline for app", "Run baseline for web", "Run baseline for shared"]) expect(html).toContain(label);
  expect(html).toContain("Cancel baseline for web");
  expect(html).not.toContain("Cancel baseline for app");
  expect(html).toMatch(/<code>app<\/code>[\s\S]*?<span class="state" data-tone="danger">failed<\/span>/);
  expect(html).toMatch(/<code>web<\/code>[\s\S]*?<span class="state" data-tone="success">passed<\/span>/);
  expect(html).toMatch(/<code>shared<\/code>[\s\S]*?<span class="state" data-tone="attention">no baseline<\/span>/);
  // A running baseline disables only its own repository's control.
  expect(html).toMatch(/<button type="button" disabled="" data-capability="baselines.start">Run baseline for web/);
  expect(html).toMatch(/<button type="button" data-capability="baselines.start">Run baseline for app/);
});

test("a blocked baseline with the runtime's testing-setup offer shows the offer instead of the generic host step", () => {
  const offer = { kind: "testing_setup" as const, project_id: "p", repository_id: "app", baseline_id: "b-blocked", reason: "the manifest names no required check for repository app", next: "POST /api/v1/projects/p/tickets with purpose testing_setup" };
  const html = renderToStaticMarkup(<BaselineList baselines={[{ ...baseline("blocked", [], "no required check"), prerequisite: offer }]} busy={false} onLog={() => {}} />);
  expect(html).toContain("Testing setup needed for <code>app</code>: the manifest names no required check for repository app");
  expect(html).toContain("POST /api/v1/projects/p/tickets with purpose testing_setup");
  expect(html).not.toContain("could not run meaningfully on this host");
  expect(html).toMatch(/<p class="baseline-offer callout" data-tone="attention" role="status">/);
  expect(renderToStaticMarkup(<BaselineList baselines={[baseline("blocked", [], "no required check")]} busy={false} onLog={() => {}} />)).toContain("could not run meaningfully on this host");
});

import { requestTarget } from "./Projects";
test("a request from Home or the Board lands on the project heading, the ticket form or the ticket", () => {
  expect(requestTarget({})).toBe("projects-heading");
  expect(requestTarget({ ticketForm: true })).toBe("ticket-title");
  expect(requestTarget({ ticketId: "ticket-1", ticketForm: true })).toBe("ticket-ticket-1");
});

import { RemoveProject } from "./projects/RemoveProject";

test("removing a project asks for confirmation and says its records stay", () => {
  const view = { project: { id: "walkthrough", name: "Walkthrough" } } as unknown as ProjectView;
  const html = renderToStaticMarkup(<RemoveProject view={view} busy={false} onRemove={async () => true} />);
  expect(html).toContain("Remove Walkthrough from every list and free its folders.");
  expect(html).toContain("adding the same folders again brings the project back");
  expect(html).toMatch(/<button[^>]*disabled=""[^>]*data-capability="projects.remove"/);
});
