import { expect, test } from "vitest";
import type { BaselineEvidence } from "../src/index";
import { latestBaselinePerRepository } from "../src/projections";

const baseline = (id: string, status: BaselineEvidence["status"], repository_id = "app"): BaselineEvidence => ({
  id, run_id: id, project_id: "p", manifest_revision: 1,
  identity: { repository_id, manifest_hash: "sha256:manifest", commit: "commit", tree: "tree" },
  environment_fingerprint: "sha256:environment", status, required_checks: [], results: [], started_at: "",
});

test("a new passing baseline replaces the older blocked summary in runtime response order", () => {
  const newest = baseline("new", "passed");
  expect(latestBaselinePerRepository([baseline("other", "failed", "other"), newest, baseline("old", "blocked")], ["app", "missing"]))
    .toEqual({ app: newest, missing: undefined });
});

test("a newer failure is not hidden by an older passing baseline", () => {
  const newest = baseline("new", "failed");
  expect(latestBaselinePerRepository([newest, baseline("old", "passed")], ["app"]).app).toBe(newest);
});
