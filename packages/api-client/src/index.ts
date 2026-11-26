export type RunState =
  | "queued" | "running" | "completed" | "failed" | "cancelled" | "interrupted"
  | "discovered" | "exploring" | "testing_blocked" | "plan_ready" | "approved" | "implementing" | "validating" | "reviewing" | "reviewed" | "paused";

export interface RunRecord {
  id: string;
  project_id: string;
  state: RunState;
  revision: number;
  metadata: { prompt?: string; provider?: string; model?: string; [key: string]: unknown };
  created_at: string;
  updated_at: string;
}

export interface Health {
  status: string;
  pi_version: string;
  /** Approved writers and required checks run with the developer's host permissions. */
  native_execution?: boolean;
  /** Content digest of the sources the runtime binary was built from. */
  build_identity?: string;
  /** Release and commit of the compiler that built the runtime binary. */
  build_rustc?: string;
  /** Any rustc wrapper Cargo ran the compiler through, or "none". */
  build_rustc_sha256?: string;
  build_rustc_wrapper?: string;
  /** Profile, optimization and RUSTFLAGS the runtime binary was compiled with. */
  build_flags?: string;
}

/** One item of the runtime's doctor report: `ok`, `warning`, or `missing` (which fails a required item). */
export type DoctorStatus = "ok" | "warning" | "missing";

export interface DoctorItem {
  name: string;
  /** Git, Pi and a writable data directory; a missing required item fails the report. */
  required: boolean;
  status: DoctorStatus;
  /** One line; credential items name the variable and say whether it is set, never its value. */
  detail: string;
}

/**
 * The host diagnostic `luuna-runtime doctor` prints, served by the running runtime for its own
 * configuration (`GET /api/v1/doctor`, ADR 0027 point 4): the clients' settings surface. Nothing
 * here is editable; credentials are configured in the runtime's environment.
 */
export interface DoctorReport {
  ok: boolean;
  items: DoctorItem[];
}

export interface RunRequest {
  prompt: string;
  provider?: string;
  model?: string;
}

// Keep raw engine payloads available: Pi may add fields without a UI release.
export interface EventEnvelope {
  schema_version: string;
  event_id: string;
  sequence: number;
  timestamp: string;
  project_id: string;
  run_id: string;
  type: string;
  payload: Record<string, unknown>;
}

export type ExecutionProfile = "native" | "container";

export interface ManifestCheck {
  id: string;
  executable: string;
  args: string[];
  required: boolean;
  timeout_seconds: number;
  capabilities: string[];
  /** Defaults to `.` (the repository root) when omitted. */
  cwd?: string;
}

export interface RepositoryManifest {
  id: string;
  remote: string;
  default_branch: string;
  profiles: string[];
  execution: ExecutionProfile;
  checks: ManifestCheck[];
}

export interface Relationship {
  producer: string;
  consumer: string;
  kind: "api" | "package" | "generated-contract" | "runtime" | "integration";
  contract: string;
  confirmed: boolean;
  evidence: string[];
}

export interface ProjectPolicy {
  approval: "plan";
  testing_prerequisite: true;
  publish: "automatic_pr";
  merge: "human";
  max_explorers: number;
  max_implementers: number;
  max_task_repairs: number;
}

/** The versioned, machine-independent project map (schemas/project.schema.json). */
export interface ProjectManifest {
  schema_version: string;
  id: string;
  name: string;
  revision: number;
  repositories: RepositoryManifest[];
  relationships: Relationship[];
  policy: ProjectPolicy;
}

export interface DirtyState {
  staged: number;
  unstaged: number;
  untracked: number;
  conflicted: number;
  paths: string[];
  truncated: boolean;
}

export interface RepositoryObservation {
  local_path: string;
  identity: { remote: string | null; push_remote?: string | null; root_commits: string[]; shallow: boolean };
  head_commit: string;
  branch: string | null;
  remote_default_branch: string | null;
  local_branches: string[];
  dirty: DirtyState;
  git_version: string;
  observed_at: string;
}

export interface ProjectRecord {
  id: string;
  name: string;
  revision: number;
  manifest_hash: string | null;
  created_at: string;
  updated_at: string;
}

export interface RepositoryRegistration {
  project_id: string;
  repository_id: string;
  local_path: string;
  observation: RepositoryObservation;
  registered_at: string;
}

export interface ManifestRecord {
  project_id: string;
  revision: number;
  hash: string;
  manifest: ProjectManifest;
  base_commits: Record<string, string>;
  confirmed_at: string;
}

export interface ProposedCheck {
  check: ManifestCheck;
  evidence: string[];
  confidence: "observed" | "inferred";
  /** Whether this host has the check's tools; absent in discoveries stored before probing existed. */
  capability?: "available" | "missing";
  missing_tools?: string[];
}

export interface HostCapabilities {
  probed_at: string;
  os: string;
  arch: string;
  tools: ToolProbe[];
}

export interface DiscoveryQuestion {
  repository_id: string | null;
  topic: string;
  detail: string;
}

/** A read-only proposal; it executed nothing and authorizes nothing. */
export interface Discovery {
  schema_version: string;
  id: string;
  project_id: string;
  created_at: string;
  repositories: { repository_id: string; observation: RepositoryObservation; profiles: string[]; evidence: string[]; proposed_checks: ProposedCheck[] }[];
  proposed_manifest: ProjectManifest;
  manifest_problems: string[];
  questions: DiscoveryQuestion[];
  host_capabilities?: HostCapabilities;
}

export interface ProjectView {
  project: ProjectRecord;
  repositories: RepositoryRegistration[];
  manifest: ManifestRecord | null;
  discovery: Discovery | null;
  /** One offer per registered repository whose latest baseline shows no usable testing setup (absent from runtimes that predate ADR 0022). */
  prerequisites?: TestingPrerequisite[];
}

export type CheckStatus = "passed" | "failed" | "blocked" | "cancelled" | "interrupted";
export type BaselineStatus = "running" | CheckStatus;

/** Deterministic evidence for one recipe execution (schemas/result.schema.json). */
export interface CheckResult {
  schema_version: string;
  id: string;
  run_id: string;
  repository_id: string;
  recipe_id: string;
  candidate_hash: string;
  commit: string;
  environment_fingerprint: string;
  command: string[];
  status: CheckStatus;
  exit_code: number | null;
  artifact_refs: string[];
  cwd?: string;
  started_at?: string;
  finished_at?: string;
  reason?: string;
  artifact_hashes?: Record<string, string>;
  tests_detected?: number;
  /** Artifact references whose stream reached the executor's size cap. */
  truncated_artifacts?: string[];
}

/** A repository's required checks at an exact commit, tree, manifest and host. */
export interface BaselineEvidence {
  id: string;
  run_id: string;
  project_id: string;
  manifest_revision: number;
  identity: { manifest_hash: string; repository_id: string; commit: string; tree: string };
  environment_fingerprint: string;
  status: BaselineStatus;
  required_checks: string[];
  results: CheckResult[];
  started_at: string;
  finished_at?: string;
  reason?: string;
  /** A blocked baseline that shows no usable testing setup carries the runtime's offer (ADR 0022). */
  prerequisite?: TestingPrerequisite;
}

/**
 * The runtime's deterministic offer of the testing-setup prerequisite (ADR 0022): recorded on a
 * blocked baseline whose repository has no required check or whose checks executed no test. It is
 * returned on the baseline list, on the project view (`prerequisites`) and on the 422 that refuses
 * `implement` for a feature plan of that repository (`ApiError.prerequisite`).
 */
export interface TestingPrerequisite {
  kind: "testing_setup";
  project_id: string;
  repository_id: string;
  baseline_id: string;
  /** Why the baseline shows no usable testing setup. */
  reason: string;
  /** The route and request that starts the prerequisite workflow, in the runtime's words. */
  next: string;
}

/**
 * The handoff `GET /runs/{id}/candidate` carries for a testing-setup workflow: the recipes the
 * candidate must pass, whether it is reviewed with a passed verdict, and the developer's next step
 * (merge, confirm the recipes in the manifest, run a baseline). The runtime never confirms or merges.
 */
export interface SetupHandoff {
  kind: "testing_setup";
  repository_id: string;
  checks: ManifestCheck[];
  candidate_commit: string | null;
  reviewed: boolean;
  next: string;
}

export interface StartBaselineRequest {
  repository_id: string;
  include_optional?: boolean;
}

/** Legacy local records migrate to the authoritative Missions projection. */
export type TicketSource = "luuna_sqlite_temporary" | "missions";

export interface TicketUsage {
  workflows: number; attempts: number; elapsed_seconds: number; cost_usd_micros: number; cost_unreported: boolean; repairs: number;
}

/** `feature` (the default, omitted from serialization) or the testing-setup prerequisite of ADR 0022. */
export type TicketPurpose = "feature" | "testing_setup";

/** The Missions issue type (ADR 0058); `task`, the default, is omitted from serialization. */
export type TicketType = "task" | "bug" | "feature" | "epic";

export interface Ticket {
  usage?: TicketUsage;
  schema_version: string;
  id: string;
  project_id: string;
  revision: number;
  source: TicketSource;
  title: string;
  intent: string;
  acceptance_criteria: string[];
  repository_ids: string[];
  /** Absent for a feature ticket (the runtime omits the default so earlier records hash unchanged). */
  purpose?: TicketPurpose;
  /** Absent for a task. */
  type?: TicketType;
  /** 0 (critical) to 4 (backlog); absent means 2. */
  priority?: number;
  /** Lowercase labels; absent means none. */
  labels?: string[];
  created_at: string;
  updated_at: string;
}

export interface TicketSourceStatus {
  source?: TicketSource;
  source_id: string | null;
  source_revision: string | null;
  reconciliation_required: boolean;
  workspace: string | null;
}

export interface CreateTicketRequest {
  /** Repeating a delivery with the same key returns the same ticket. */
  idempotency_key: string;
  title: string;
  intent: string;
  acceptance_criteria: string[];
  repository_ids: string[];
  /** `testing_setup` for the prerequisite ticket the runtime offers; omitted for a feature. */
  purpose?: TicketPurpose;
  type?: TicketType;
  priority?: number;
  labels?: string[];
}

/** `PUT /tickets/{id}`: the named fields replace the current ones; the revision must match what the client read. */
export interface UpdateTicketRequest {
  expected_revision: number;
  title?: string;
  intent?: string;
  acceptance_criteria?: string[];
  repository_ids?: string[];
  type?: TicketType;
  priority?: number;
  /** The whole new list; `[]` removes every label. */
  labels?: string[];
}

export interface ComponentStatus {
  enabled: boolean;
  workspace_path?: string;
  reason?: string;
}

export type MemoryScope = "project" | "process" | "user";

export interface MemoryRecord {
  id: string;
  scope: MemoryScope;
  content: string;
  provenance: string;
  hash: string;
  created_at?: string;
}

export interface CreateMemoryRequest {
  idempotency_key: string;
  scope: MemoryScope;
  content: string;
  provenance: string;
}

export interface PromoteMemoryRequest {
  idempotency_key: string;
  expected_hash: string;
  to_scope: "project" | "user";
}

export interface PromotedMemory {
  promoted: MemoryRecord;
  from: { id: string; scope: MemoryScope };
}

/** Corrects a record's content at its displayed hash; the record keeps its scope and gains provenance naming the correction. */
export interface UpdateMemoryRequest {
  idempotency_key: string;
  expected_hash: string;
  content: string;
}

/** The personal export: user-scope memory with provenance, and nothing of any project. */
export interface UserMemoryExport {
  schema_version: string;
  kind: "luuna-user-memory-export";
  exported_at: string;
  scope: "user";
  memory: MemoryRecord[];
  omitted: string[];
}

/** Longest age in days a record of each scope may reach before `applyMemoryRetention` deletes it; null keeps records. User memory is never subject to a project policy. */
export interface MemoryRetentionPolicy {
  process_max_age_days: number | null;
  project_max_age_days: number | null;
}

export interface ApplyMemoryRetentionRequest {
  idempotency_key: string;
}

export interface MemoryRetentionResult {
  applied_at: string;
  policy: MemoryRetentionPolicy;
  deleted: { id: string; scope: MemoryScope; created_at: string; hash: string }[];
}

export interface ProjectExport {
  schema_version: string;
  kind: "luuna-project-export";
  exported_at: string;
  project: { id: string; name: string; manifest_revision: number; manifest_hash: string | null };
  manifest: ProjectManifest | null;
  /** Machine-independent identity of each registered repository, keyed by repository id. */
  repository_identities: Record<string, { remote: string | null; push_remote?: string | null; root_commits: string[]; shallow: boolean }>;
  tickets: Ticket[];
  memory: MemoryRecord[];
  prompts: RenderedPrompt[];
  omitted: string[];
  /** luuna-json-v1 hash of every other field; an import recomputes and refuses a mismatch. */
  export_hash: string;
}

/** An export plus this machine's checkout of each exported repository. */
export interface ProjectImportRequest {
  export: ProjectExport;
  repositories: { id: string; path: string }[];
  idempotency_key: string;
}

export interface ProjectImportResult {
  project: ProjectView;
  export_hash: string;
  exported_at: string;
  manifest_hash: string;
  components_enabled: boolean;
  counts: { tickets: number; memory: number; prompts: number };
  mapping: {
    manifest: { exported_revision: number; imported_revision: number };
    tickets: { id: string; exported_revision: number; imported_revision: number }[];
    memory: { exported_id: string; imported_id: string; scope: MemoryScope; exported_hash: string; imported_hash: string }[];
    prompts: { name: string; exported_version: number; imported_version: number; exported_hash: string; imported_hash: string; sections_flattened: boolean }[];
  };
  replayed: boolean;
}

export type ScaffoldProfile = "node" | "node-typescript" | "react" | "nextjs" | "python" | "go" | "rust" | "flutter" | "android" | "ios";

/** Orbit (ADR 0036): a project's repositories seen together. Read-only status. */
export interface Upstream {
  name: string;
  ahead: number;
  behind: number;
}

export interface OrbitRepository {
  repository_id: string;
  local_path: string;
  present: boolean;
  branch: string | null;
  head_commit: string | null;
  upstream: Upstream | null;
  dirty: DirtyState | null;
  remote: string | null;
  recipes: string[];
  error: string | null;
}

export interface OrbitView {
  project_id: string;
  name: string;
  observed_at: string;
  manifest_revision: number | null;
  repositories: OrbitRepository[];
  recipes: string[];
}

/** Orbit outcomes are the developer's commands, never check evidence. */
export type OrbitStatus = "succeeded" | "failed" | "blocked" | "cancelled" | "skipped";

export interface OrbitResult {
  repository_id: string;
  status: OrbitStatus;
  /** Sync only: cloned, fetched or unchanged. */
  action?: "cloned" | "fetched" | "unchanged";
  exit_code: number | null;
  reason: string | null;
  command: string[];
  stdout_excerpt: string;
  stderr_excerpt: string;
  started_at: string;
  finished_at: string;
}

export interface OrbitExecution {
  id: string;
  project_id: string;
  kind: "exec" | "recipe" | "sync";
  command: string[];
  started_at: string;
  finished_at: string;
  results: OrbitResult[];
}

export interface OrbitExecRequest {
  executable: string;
  args?: string[];
  /** Repository ids; omitted or empty selects every registered repository. */
  repositories?: string[];
  cwd?: string;
  timeout_seconds?: number;
  parallel?: boolean;
}

export interface OrbitRunRequest {
  recipe: string;
  repositories?: string[];
  parallel?: boolean;
}

export interface OrbitSyncRequest {
  repositories?: string[];
  /** Fetch present checkouts (default true); missing ones are cloned. */
  fetch?: boolean;
}

export type SandboxBackend = "bwrap" | "seatbelt" | "app_container" | "unavailable";
export type SandboxNetwork = "open" | "none";

export interface SandboxRepository {
  repository_id: string;
  host_path: string;
  sandbox_path: string;
}

/** What `luuna-sbx` may reach for one project, as the runtime decided it. */
export interface SandboxPlan {
  version: number;
  project_id: string;
  backend: SandboxBackend;
  available: boolean;
  reason: string | null;
  workdir: string;
  host_root: string | null;
  home: string;
  repositories: SandboxRepository[];
  network: SandboxNetwork;
  container_name: string | null;
  notes: string[];
}

/** Exec timeout bounds the runtime enforces, in seconds. */
export const ORBIT_TIMEOUT_LIMITS = [1, 3600] as const;

/** Splits a command line typed by the developer into executable and arguments (quotes group words); no shell runs it. */
export function orbitCommand(line: string): { executable: string; args: string[] } {
  const words: string[] = [];
  const pattern = /"([^"]*)"|'([^']*)'|(\S+)/g;
  for (const match of line.matchAll(pattern)) words.push(match[1] ?? match[2] ?? match[3]);
  if (words.length === 0) throw new Error("Type a command to run in each repository.");
  return { executable: words[0], args: words.slice(1) };
}

/** The terminal command that opens the sandbox a plan describes. */
export function sandboxCommand(projectId: string, network: SandboxNetwork = "open"): string {
  return `lu sbx ${projectId}${network === "none" ? " --network none" : ""}`;
}

export interface ScaffoldProposalRequest {
  profile: ScaffoldProfile;
  name: string;
  parent_dir: string;
}

export interface ScaffoldGenerator {
  id: string;
  executable: string;
  args: string[];
  capabilities: string[];
  network: boolean;
  description: string;
}

export interface ToolProbe {
  tool: string;
  status: "available" | "missing";
  version?: string;
  detail?: string;
}

export interface ScaffoldProposal {
  schema_version: string;
  profile: ScaffoldProfile;
  name: string;
  parent_dir: string;
  target_dir: string;
  status: "ready" | "blocked";
  blocked_reason?: string;
  files: { path: string; contents: string }[];
  generator?: ScaffoldGenerator;
  preparation: ScaffoldGenerator[];
  network_steps: string[];
  baseline: ManifestCheck[];
  required_tools: ToolProbe[];
  questions: DiscoveryQuestion[];
  hash: string;
}

export interface ScaffoldRequest {
  proposal_hash: string;
  idempotency_key: string;
  confirm: true;
  confirm_network?: boolean;
}

export interface ScaffoldCheckOutcome {
  recipe_id: string;
  status: CheckStatus;
  exit_code: number | null;
  reason: string | null;
  command: string[];
  tool_version: string | null;
  tests_detected: number | null;
  stdout_excerpt: string;
  stderr_excerpt: string;
}

export interface ScaffoldResult {
  proposal_hash: string;
  idempotency_key: string;
  profile: ScaffoldProfile;
  target_dir: string;
  commit: string;
  generator?: ScaffoldCheckOutcome;
  preparation: ScaffoldCheckOutcome[];
  check: ScaffoldCheckOutcome;
  baseline_passed: boolean;
  project: ProjectView;
  discovery: Discovery;
  replayed: boolean;
}

export interface PromptRecord {
  name: string;
  version: number;
  hash?: string;
}

export interface RenderedPrompt {
  name: string;
  version: number;
  hash: string;
  body: string;
}

export interface CreatePromptRequest {
  idempotency_key: string;
  name: string;
  body: string;
  expected_version?: number;
}

/** Explicit selection only: nothing is attached from personal stores automatically. */
export interface ComponentContext {
  memory_ids: string[];
  prompt?: { name: string; version: number; hash: string } | null;
}

/** Exact runtime-resolved resource content covered by the immutable plan hash. */
export interface ComponentContextSnapshot {
  memory: Array<{ id: string; scope: MemoryScope; hash: string; content: string; provenance: string }>;
  prompt?: { name: string; version: number; hash: string; body: string } | null;
}

export type Operation = "read" | "write" | "shell" | "network" | "publish";

export type TaskRole = "implementer" | "repository_lead" | "project_orchestrator";

/** A justified departure from the confirmed repository relationships (ADR 0023). */
export interface DependencyOverride {
  consumer: string;
  producer: string;
  action: "waive" | "add";
  justification: string;
}

export interface PlanTask {
  id: string;
  repository_id: string;
  role: TaskRole | string;
  depends_on: string[];
  paths: string[];
  operations: Operation[];
  acceptance_criteria: string[];
  execution_profile: string;
  /** The advisory task of an earlier plan revision this task was copied from. */
  derived_from_task?: string;
}

export interface CheckRecipe {
  id: string;
  repository_id: string;
  executable: string;
  args: string[];
  cwd: string;
  timeout_seconds: number;
  required: boolean;
  capabilities: string[];
  execution_profile: string;
}

export interface Budgets {
  max_workers: number;
  max_attempts: number;
  timeout_seconds: number;
  cost_usd_micros: number;
}

export interface PlanRevision {
  schema_version: string;
  canonicalization: string;
  id: string;
  revision: number;
  project_id: string;
  ticket_id: string;
  ticket_revision: number;
  manifest_hash: string;
  base_commits: Record<string, string>;
  candidate_hash: string | null;
  permissions: { repositories: Record<string, string[]>; operations: Operation[] };
  tasks: PlanTask[];
  required_checks: CheckRecipe[];
  budgets: Budgets;
  hash: string;
  component_context?: ComponentContextSnapshot;
  /** Copied from the ticket when the plan is made; part of the sealed hash (ADR 0022). Absent means `feature`. */
  purpose?: TicketPurpose;
  /** Justified departures from the confirmed relationships, hashed with the plan (ADR 0023). */
  dependency_overrides?: DependencyOverride[];
}

/**
 * A check recipe a testing-setup plan proposes as the repository's baseline (ADR 0022): the
 * manifest-check shape, always required. `cwd` defaults to the repository root, `args` and
 * `capabilities` to none; `timeout_seconds` must be given.
 */
export interface SetupCheckRequest {
  id: string;
  executable: string;
  args?: string[];
  cwd?: string;
  timeout_seconds: number;
  capabilities?: string[];
  /** Accepted for symmetry with the manifest check it becomes; `false` is refused by the runtime. */
  required?: true;
}

/** The structured result of an advisory task (repository lead or project orchestrator), never a commit (ADR 0023). */
export interface AdvisoryResult {
  findings: string[];
  proposed_tasks: ProposedTask[];
  risks: string[];
}

/** A task an advisory result proposes; it becomes real only through a plan revision the developer creates and approves. */
export interface ProposedTask {
  id: string;
  repository_id: string;
  role?: TaskRole | string;
  depends_on?: string[];
  paths?: string[];
  acceptance_criteria?: string[];
  rationale?: string;
}

/** Who assigned a task: the developer's approval of the exact plan revision (R06, ADR 0023). */
export interface AssignmentTrace {
  approved_by: string;
  approval_id: string;
  approved_at: string;
  plan_id: string;
  plan_revision: number;
  plan_hash: string;
}

export interface PlanBudgetRequest {
  /** Concurrent writers for a multi-task plan (1 to the number of tasks, at most 4); a single-task plan allows exactly 1. */
  max_workers?: number;
  max_attempts?: number;
  /** Cumulative across the writer, checks, review and repairs of the workflow. */
  timeout_seconds?: number;
  cost_usd_micros?: number;
}

export interface PlanTaskRequest {
  id?: string;
  repository_id: string;
  paths: string[];
  operations: Operation[];
  acceptance_criteria?: string[];
  /** Task ids this task waits for; its writer and checks receive the producers' candidate worktrees and commits. */
  depends_on?: string[];
  /** Defaults to `implementer`; an advisory role reads only and records an advisory result. */
  role?: TaskRole;
  /** The advisory task of an earlier plan revision whose proposal this task copies. */
  derived_from_task?: string;
}

export interface CreatePlanRequest {
  /** One task, or `tasks` for a dependency-ordered multi-repository plan (never both). */
  task?: PlanTaskRequest;
  tasks?: PlanTaskRequest[];
  permissions?: Record<string, string[]>;
  /** Justified departures from the confirmed repository relationships (ADR 0023). */
  dependency_overrides?: DependencyOverride[];
  budgets?: PlanBudgetRequest;
  component_context?: ComponentContext;
  /** Repeating a delivery with the same key and request returns the plan it created. */
  idempotency_key?: string;
  /**
   * Testing-setup plans only (ADR 0022): the recipes the setup candidate must pass and that the
   * developer then confirms as the repository's required checks. Absent, the confirmed manifest's
   * required recipes are used; refused on a feature plan.
   */
  checks?: SetupCheckRequest[];
}

/** The runtime's defaults when a plan names no budgets. */
export const DEFAULT_PLAN_BUDGETS: Required<PlanBudgetRequest> = { max_workers: 1, max_attempts: 2, timeout_seconds: 600, cost_usd_micros: 1_000_000 };

/** Inclusive bounds the runtime enforces for each plan budget. */
export const PLAN_BUDGET_LIMITS: Record<keyof PlanBudgetRequest, readonly [number, number]> = {
  max_workers: [1, 4],
  max_attempts: [1, 10],
  timeout_seconds: [1, 86_400],
  cost_usd_micros: [1, 1_000_000_000],
};

/** The runtime's refusal for an out-of-range budget, or null when every named budget is in range. */
export function planBudgetProblem(budgets: PlanBudgetRequest): string | null {
  for (const [field, [low, high]] of Object.entries(PLAN_BUDGET_LIMITS) as [keyof PlanBudgetRequest, readonly [number, number]][]) {
    const value = budgets[field];
    if (value !== undefined && (!Number.isSafeInteger(value) || value < low || value > high)) return `budgets.${field} must be between ${low} and ${high}`;
  }
  return null;
}

/** Idempotency keys are 1 to 128 visible ASCII characters. */
export function validIdempotencyKey(key: string): boolean {
  return /^[\x21-\x7e]{1,128}$/.test(key);
}

/** One task of a multi-repository plan as a client collects it: `id` defaults to the repository id. */
export interface PlanTaskDraft {
  id?: string;
  repository_id: string;
  paths: string[];
  operations: Operation[];
  acceptance_criteria?: string[];
  depends_on?: string[];
  /** `implementer` (the default, omitted so single-task requests keep their pre-M4 identity) or an advisory role (ADR 0023). */
  role?: TaskRole;
  /** The advisory task of an earlier plan revision whose proposal this task copies (R06). */
  derived_from_task?: string;
}

export const TASK_ROLES: readonly TaskRole[] = ["implementer", "repository_lead", "project_orchestrator"];

/** An advisory role reads only: its session has read-only tools and records an advisory result, never a commit. */
export function advisoryRole(role: TaskRole | string | undefined): boolean {
  return role === "repository_lead" || role === "project_orchestrator";
}

/** Everything a plan request may carry beside its tasks and budgets. */
export interface PlanRequestOptions {
  componentContext?: ComponentContext;
  permissions?: Record<string, string[]>;
  /** Justified departures from the confirmed relationships; each names two tasks of this plan (ADR 0023). */
  dependencyOverrides?: DependencyOverride[];
  /** A testing-setup plan's proposed baseline recipes (ADR 0022); never sent empty. */
  checks?: SetupCheckRequest[];
}

/**
 * The plan request both clients send for the same tasks (ADR 0017): one `task` for a single-task
 * plan (its pre-M4 shape, so the plan hashes exactly as before), `tasks` with `depends_on` for a
 * task graph, and `budgets.max_workers` only when it is not the default. Empty paths default to
 * `src`; every task must name a distinct repository; `depends_on` must name other tasks of this
 * plan. A task's `role` is sent only when it is not `implementer` and `derived_from_task` only when
 * given (ADR 0023), so earlier requests keep their idempotency identity; an advisory task may only
 * read. Dependency overrides must name two distinct tasks of the plan with a justification;
 * testing-setup `checks` need an id, an executable and a timeout and are always required. The
 * runtime validates everything again; this only refuses what no runtime would accept.
 */
export function planRequestForTasks(drafts: PlanTaskDraft[], budgets: PlanBudgetRequest = {}, options: PlanRequestOptions = {}): Omit<CreatePlanRequest, "idempotency_key"> {
  if (drafts.length === 0) throw new Error("A plan needs at least one task.");
  const tasks: PlanTaskRequest[] = drafts.map((draft) => {
    const id = (draft.id ?? draft.repository_id).trim();
    const repository = draft.repository_id.trim();
    if (!repository) throw new Error("Every task names a registered repository.");
    const paths = draft.paths.map((path) => path.trim()).filter(Boolean);
    const depends = (draft.depends_on ?? []).map((task) => task.trim()).filter(Boolean);
    const criteria = (draft.acceptance_criteria ?? []).map((line) => line.trim()).filter(Boolean);
    const role = draft.role ?? "implementer";
    if (!TASK_ROLES.includes(role)) throw new Error(`Task ${id} has an unknown role ${role}; a role is implementer, repository_lead or project_orchestrator.`);
    if (advisoryRole(role) && (draft.operations.length !== 1 || draft.operations[0] !== "read")) throw new Error(`Task ${id} has the advisory role ${role}, which reads only: its operations must be exactly read.`);
    // A single task carries no id and the runtime names it `implement`.
    const effective = drafts.length > 1 ? id : "implement";
    const derived = draft.derived_from_task?.trim();
    if (derived !== undefined && derived === effective) throw new Error(`Task ${effective} cannot derive from itself.`);
    return {
      ...(drafts.length > 1 && { id }),
      repository_id: repository,
      paths: paths.length ? paths : ["src"],
      operations: draft.operations,
      ...(criteria.length && { acceptance_criteria: criteria }),
      ...(depends.length && { depends_on: depends }),
      ...(role !== "implementer" && { role }),
      ...(derived && { derived_from_task: derived }),
    };
  });
  const ids = tasks.map((task) => task.id ?? "implement");
  const repositories = tasks.map((task) => task.repository_id);
  if (new Set(repositories).size !== repositories.length) throw new Error("Each task of a plan targets a distinct repository; a dependent change inside one repository is one task.");
  if (new Set(ids).size !== ids.length) throw new Error("Task ids must be unique.");
  for (const task of tasks) for (const dependency of task.depends_on ?? []) {
    if (dependency === task.id) throw new Error(`Task ${task.id} cannot depend on itself.`);
    if (!ids.includes(dependency)) throw new Error(`Task ${task.id} depends on ${dependency}, which is not a task of this plan.`);
  }
  for (const task of tasks) if (task.derived_from_task && ids.includes(task.derived_from_task)) throw new Error(`Task ${task.id ?? ids[0]} derives from ${task.derived_from_task}, a task of this same plan; it must name an advisory task of an earlier revision.`);
  const overrides = (options.dependencyOverrides ?? []).map((override) => ({ consumer: override.consumer.trim(), producer: override.producer.trim(), action: override.action, justification: override.justification.trim() }));
  const pairs = new Set<string>();
  for (const override of overrides) {
    if (override.action !== "waive" && override.action !== "add") throw new Error(`A dependency override's action is waive or add, not ${String(override.action)}.`);
    for (const name of [override.consumer, override.producer]) if (!ids.includes(name)) throw new Error(`Dependency override names ${name || "an empty task id"}, which is not a task of this plan.`);
    if (override.consumer === override.producer) throw new Error(`Dependency override names ${override.consumer} as both consumer and producer.`);
    if (advisoryRole(tasks[ids.indexOf(override.consumer)].role) || advisoryRole(tasks[ids.indexOf(override.producer)].role)) throw new Error(`Dependency override ${override.consumer}/${override.producer} names an advisory task; a dependency on an advisory task needs no override.`);
    if (!override.justification) throw new Error(`Dependency override ${override.consumer}/${override.producer} needs a justification.`);
    const pair = `${override.consumer}\u0000${override.producer}`;
    if (pairs.has(pair)) throw new Error(`Dependency override ${override.consumer}/${override.producer} is repeated.`);
    pairs.add(pair);
  }
  const checks = (options.checks ?? []).map((check) => {
    const id = check.id.trim();
    const executable = check.executable.trim();
    if (!id || !executable) throw new Error("Every testing-setup check needs an id and an executable.");
    if ((check.required as boolean | undefined) === false) throw new Error(`Check ${id} is optional; a testing-setup plan proposes required checks only.`);
    if (!Number.isSafeInteger(check.timeout_seconds) || check.timeout_seconds < 1) throw new Error(`Check ${id} needs a timeout in seconds of at least 1.`);
    const args = (check.args ?? []).map((arg) => arg.trim()).filter(Boolean);
    const capabilities = (check.capabilities ?? []).map((capability) => capability.trim()).filter(Boolean);
    const cwd = check.cwd?.trim();
    return { id, executable, args, ...(cwd && cwd !== "." && { cwd }), timeout_seconds: check.timeout_seconds, capabilities, required: true as const };
  });
  if (new Set(checks.map((check) => check.id)).size !== checks.length) throw new Error("Testing-setup check ids must be unique.");
  if (checks.length && tasks.length !== 1) throw new Error("A testing-setup plan is exactly one task on the repository whose baseline it establishes.");
  const { max_workers, ...rest } = budgets;
  const named: PlanBudgetRequest = { ...rest, ...(max_workers !== undefined && max_workers !== DEFAULT_PLAN_BUDGETS.max_workers && { max_workers }) };
  if (named.max_workers !== undefined && named.max_workers > tasks.length) throw new Error(`budgets.max_workers must be between 1 and ${tasks.length} for ${tasks.length} task(s).`);
  const problem = planBudgetProblem(named);
  if (problem) throw new Error(problem);
  return {
    ...(tasks.length === 1 ? { task: tasks[0] } : { tasks }),
    ...(overrides.length && { dependency_overrides: overrides }),
    ...(Object.keys(named).length && { budgets: named }),
    ...(options.permissions && { permissions: options.permissions }),
    ...(options.componentContext && (options.componentContext.memory_ids.length > 0 || options.componentContext.prompt) ? { component_context: options.componentContext } : {}),
    ...(checks.length && { checks }),
  };
}

/**
 * The override the runtime's relationship refusal quotes (ADR 0023 point 5), or null when the
 * reason is not that refusal: the developer then records it with a real justification.
 */
export function overrideFromRefusal(reason: string): DependencyOverride | null {
  const match = /dependency_overrides\s*(\[\{.*?\}\])/.exec(reason);
  if (!match) return null;
  try {
    const parsed: unknown = JSON.parse(match[1]);
    const first = Array.isArray(parsed) ? parsed[0] as Record<string, unknown> : null;
    if (!first || typeof first.consumer !== "string" || typeof first.producer !== "string" || (first.action !== "waive" && first.action !== "add")) return null;
    return { consumer: first.consumer, producer: first.producer, action: first.action, justification: "" };
  } catch {
    return null;
  }
}

export interface PlanOpened {
  plan: PlanRevision;
  run: RunRecord;
}

export interface Approval {
  schema_version: string;
  id: string;
  plan_id: string;
  plan_revision: number;
  plan_hash: string;
  approved_by: string;
  approved_at: string;
  permissions: { repositories: Record<string, string[]>; operations: Operation[] };
}

export interface ApprovePlanRequest {
  plan_revision: number;
  /** The exact hash the approver saw; anything else is a stale approval. */
  plan_hash: string;
  approved_by: string;
}

export interface PlanApproved {
  approval: Approval;
  run: RunRecord;
}

export interface ImplementRequest {
  task_id?: string;
  provider?: string;
  model?: string;
}

/** The runtime's immutable identity for an exact set of repository commits. */
export type TaskState = "queued" | "running" | "completed" | "failed" | "interrupted" | "blocked" | "cancelled";

/** Durable per-task state of a multi-repository workflow (ADR 0017). */
export interface TaskRecord {
  run_id: string;
  task_id: string;
  repository_id: string;
  role?: TaskRole | string;
  state: TaskState;
  attempt_id?: string;
  candidate_commit?: string;
  candidate_tree?: string;
  depends_on: string[];
  reason?: string;
  /** Who assigned the task: the approval of the exact plan revision (ADR 0023). */
  assigned_by?: AssignmentTrace | null;
  derived_from_task?: string | null;
  /** The plan's overrides that name this task as consumer or producer. */
  dependency_overrides?: DependencyOverride[];
  /** An advisory task's structured result once it completed. */
  advisory_result?: AdvisoryResult | null;
  updated_at: string;
}

export interface CandidateSet {
  schema_version: string;
  id: string;
  revision: number;
  plan_hash: string;
  repository_commits: Record<string, string>;
  dependency_bindings: Record<string, string>;
  hash: string;
}

export interface ReviewFinding {
  severity: "info" | "warning" | "blocking";
  path: string;
  line: number;
  explanation: string;
  evidence: string[];
}

/** A structured, independently recorded review of one exact candidate. */
export interface ReviewResult {
  schema_version: string;
  id: string;
  candidate_hash: string;
  reviewer_session: string;
  findings: ReviewFinding[];
  verdict: "passed" | "changes_required" | "blocked";
  evidence: string[];
}

export type CandidateStatus = "pending" | "validating" | "reviewing" | "reviewed" | "failed" | "testing_blocked" | "paused" | "cancelled" | "interrupted";

export interface CandidateEvidence {
  candidate: CandidateSet;
  run_id: string;
  implementer_session: string;
  repository_trees: Record<string, string>;
  status: CandidateStatus;
  results: CheckResult[];
  review?: ReviewResult | null;
  started_at: string;
  finished_at?: string;
  reason?: string;
  elapsed_seconds: number;
  cost_usd_micros: number;
  validation_generation: number;
}

/** The refusal each candidate route would give now, or null when it would admit the request. */
export interface CandidateDecisionReasons {
  validate: string | null;
  repair: string | null;
  revalidate: string | null;
}

/** The whole workflow's usage (writer attempts, validation and review) against its approved plan. */
export interface WorkflowUsage {
  attempts: number;
  max_attempts: number;
  elapsed_seconds: number;
  timeout_seconds: number;
  cost_usd_micros: number;
  cost_budget_usd_micros: number;
  /** A writer or reviewer reported tokens without a cost, so the cost budget did not meter them. */
  cost_unreported: boolean;
  repairs: number;
  max_repairs: number | null;
}

export interface CandidateView {
  evidence: CandidateEvidence | null;
  /** Whether the runtime currently owns a worker for this workflow. */
  active: boolean;
  /** The runtime's decision after checking state, repair and attempt budgets. */
  repair_allowed: boolean;
  /**
   * Whether a failed candidate may start a fresh validation generation in place: its required
   * checks passed but no reviewer verdict was recorded, and the approved budgets still have
   * headroom. Same revision, same approval, fresh checks and a fresh reviewer.
   */
  revalidate_allowed: boolean;
  /** The validate route's own gate (absent from runtimes that predate it). */
  validate_allowed?: boolean;
  /** Why each refused action is refused (absent from runtimes that predate it). */
  decision_reasons?: CandidateDecisionReasons;
  /** Absent from runtimes that predate workflow usage reporting. */
  usage?: WorkflowUsage;
  /** The testing-setup handoff of a setup workflow; null for a feature workflow (ADR 0022). */
  prerequisite?: SetupHandoff | null;
  /** The plan's departures from the confirmed relationships (ADR 0023). */
  dependency_overrides?: DependencyOverride[];
  /** The task records with their assignment traces and advisory results (ADR 0023). */
  tasks?: TaskRecord[];
}

export type PublicationState = "pending" | "pushing" | "pr_open" | "failed" | "integrated";
export type PublicationStep = "pending" | "done" | "failed";

export interface PublishRequest {
  expected_hash: string;
  idempotency_key: string;
}

export interface CiObservation {
  state: "pending" | "success" | "failure" | "unknown";
  observed_at?: string;
  checks: { name: string; status: string; conclusion?: string }[];
  reason?: string;
}

export interface MergeObservation {
  merged: boolean;
  merged_at?: string;
  merged_by?: string;
  observed_at?: string;
  reason?: string;
}

export interface RepositoryPublication {
  branch: string;
  commit: string;
  remote: string;
  owner: string;
  repo: string;
  base_branch: string;
  push: PublicationStep;
  pull_request_step: PublicationStep;
  pull_request?: { number: number; url: string };
  reason?: string;
  ci: CiObservation;
  merge: MergeObservation;
}

/** Mirrors schemas/publication.schema.json. */
export interface Publication {
  schema_version: string;
  ticket_id: string;
  candidate_hash: string;
  per_repository_branch: Record<string, string>;
  pull_request_urls: Record<string, string>;
  publication_state: PublicationState;
}

export interface PublicationRecord {
  schema_version: string;
  run_id: string;
  project_id: string;
  ticket_id: string;
  candidate_hash: string;
  idempotency_key: string;
  publication_state: PublicationState;
  repositories: Record<string, RepositoryPublication>;
  reason?: string;
  created_at: string;
  updated_at: string;
  observed_at?: string;
  /** The developer sent this published candidate to a bounded repair after its pull request failed CI; a later revision publishes under its own record whose pull request body names this one. */
  superseded_by_repair?: boolean;
  superseded_reason?: string;
  superseded_at?: string;
  contract: Publication;
  active: boolean;
}

/** The response of a CI-failure repair: the superseded publication record plus the workflow run now implementing the next candidate revision. */
export interface PublicationRepair extends PublicationRecord {
  run: RunRecord;
}

export interface CandidateDiff {
  candidate_hash: string;
  repository_id: string;
  base_commit: string;
  commit: string;
  diff: string;
  truncated?: boolean;
  files?: { path: string; status: string; shown: "text" | "summary" | "omitted" }[];
  worktree_drift?: string | null;
}

export interface CandidateActionRequest {
  /** The full candidate hash shown to the developer; stale requests are refused. */
  candidate_hash: string;
  provider?: string;
  model?: string;
}

export interface ValidateCandidateRequest extends CandidateActionRequest {
  /** Opt in to bounded automatic repair under this immutable plan's approval. */
  auto_repair?: boolean;
}

export type LeaseState = "active" | "released" | "abandoned";

export interface WorktreeLease {
  id: string;
  run_id: string;
  project_id: string;
  repository_id: string;
  path: string;
  base_commit: string;
  state: LeaseState;
  candidate_commit?: string;
  created_at: string;
  released_at?: string;
  reason?: string;
}

export type AttemptState = "running" | "completed" | "failed" | "cancelled" | "interrupted";

export interface AttemptRecord {
  id: string;
  run_id: string;
  task_id: string;
  number: number;
  state: AttemptState;
  payload: Record<string, unknown>;
  started_at: string;
  finished_at?: string;
}

export interface RegisterProjectRequest {
  id?: string;
  name: string;
  repositories: { id: string; path: string }[];
}

/**
 * The registration both clients send for the same repositories: every row with a repository id and
 * an absolute root path, none repeated. Paths stay in the machine registry; the manifest never
 * carries them.
 */
export function registerProjectRequest(name: string, repositories: { id: string; path: string }[], id?: string): RegisterProjectRequest {
  const rows = repositories.map((row) => ({ id: row.id.trim(), path: row.path.trim() })).filter((row) => row.id || row.path);
  if (!name.trim()) throw new Error("A project needs a name.");
  if (rows.length === 0) throw new Error("Register at least one repository: an id and its absolute root path.");
  for (const row of rows) if (!row.id || !row.path) throw new Error("Every repository row needs an id and an absolute root path.");
  if (new Set(rows.map((row) => row.id)).size !== rows.length) throw new Error("Repository ids must be unique within a project.");
  return { name: name.trim(), ...(id?.trim() && { id: id.trim() }), repositories: rows };
}

/** Sending it is the developer's confirmation; the registered path is re-inspected when omitted. */
export interface RebindRepositoryRequest {
  local_path?: string;
}

/** The largest page a paged read may request. */
export const MAX_PAGE = 10_000;

export interface PageOptions {
  /** At most this many records (1 to MAX_PAGE); omitted reads every record. */
  limit?: number;
}

export class ApiError extends Error {
  readonly status: number;
  /**
   * `fromRuntime` is true when the message is the runtime's own short reason from a JSON
   * `{error}` body, false when the client substituted a generic sentence.
   */
  readonly fromRuntime: boolean;
  /**
   * The testing-setup offer a 422 refusal of `implement` carries beside its reason (ADR 0022):
   * the one structured detail a refusal body may add, so a client can act on it.
   */
  readonly prerequisite?: TestingPrerequisite;

  // No parameter properties: Node loads this source directly (type stripping) for the TUI.
  constructor(status: number, message: string, fromRuntime = false, prerequisite?: TestingPrerequisite) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.fromRuntime = fromRuntime;
    if (prerequisite) this.prerequisite = prerequisite;
  }
}

/** One parsed `text/event-stream` frame. */
interface SseFrame { event: string; data: string; id: string }

/** Parses SSE frames from a byte stream; a frame is dispatched at each blank line. */
async function* sseFrames(body: ReadableStream<Uint8Array>): AsyncGenerator<SseFrame, void, undefined> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffered = "";
  let frame: SseFrame = { event: "", data: "", id: "" };
  let lines: string[] = [];
  try {
    for (;;) {
      const { done, value } = await reader.read();
      buffered += done ? decoder.decode() : decoder.decode(value, { stream: true });
      const parts = buffered.split(/\r\n|\r|\n/);
      buffered = done ? "" : parts.pop() ?? "";
      for (const line of parts) {
        if (line === "") {
          if (lines.length) { frame.data = lines.join("\n"); yield frame; }
          frame = { event: "", data: "", id: "" };
          lines = [];
          continue;
        }
        if (line.startsWith(":")) continue;
        const colon = line.indexOf(":");
        const field = colon === -1 ? line : line.slice(0, colon);
        const value = colon === -1 ? "" : line.slice(colon + 1).replace(/^ /, "");
        if (field === "event") frame.event = value;
        else if (field === "data") lines.push(value);
        else if (field === "id") frame.id = value;
      }
      if (done) return;
    }
  } finally {
    reader.releaseLock();
  }
}

function pause(milliseconds: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) { reject(signal.reason); return; }
    const aborted = () => { clearTimeout(timer); reject(signal?.reason); };
    const timer = setTimeout(() => { signal?.removeEventListener("abort", aborted); resolve(); }, milliseconds);
    signal?.addEventListener("abort", aborted, { once: true });
  });
}

/** What a live follower is told while it waits for the runtime to answer again. */
export interface FollowHooks {
  outage?: (failure: unknown) => void;
  reconnected?: () => void;
  wait?: (milliseconds: number, signal?: AbortSignal) => Promise<void>;
}

function pageQuery(page: PageOptions, separator: "?" | "&"): string {
  if (page.limit === undefined) return "";
  if (!Number.isSafeInteger(page.limit) || page.limit < 1 || page.limit > MAX_PAGE) throw new Error(`limit must be an integer from 1 to ${MAX_PAGE}.`);
  return `${separator}limit=${page.limit}`;
}

/**
 * Every event after `after`, read in pages of `limit` from a cursor until a page comes back empty.
 * A short page is not the end: the runtime bounds run-event pages by count and bytes (ADR 0015).
 * A runtime that ignores `limit` answers with one long page and an empty one.
 */
export async function readEventPages(read: (after: number, page: PageOptions) => Promise<EventEnvelope[]>, after: number, limit = 1000): Promise<EventEnvelope[]> {
  const events: EventEnvelope[] = [];
  let cursor = after;
  for (;;) {
    const page = await read(cursor, { limit });
    events.push(...page);
    const last = page.at(-1)?.sequence;
    if (last === undefined || last <= cursor) return events;
    cursor = last;
  }
}

/** Tokens may only be sent to the developer's local runtime. */
export function loopbackUrl(value: string): string {
  const url = new URL(value);
  if (url.protocol !== "http:" || !["127.0.0.1", "localhost", "[::1]"].includes(url.hostname)) {
    throw new Error("Use an HTTP loopback runtime URL (127.0.0.1, localhost or [::1]).");
  }
  if (url.username || url.password || url.search || url.hash || url.pathname !== "/") {
    throw new Error("Runtime URL must contain only a loopback address and optional port.");
  }
  return url.origin;
}

export function isTerminal(state: RunState): boolean {
  return ["completed", "failed", "cancelled", "interrupted", "reviewed"].includes(state);
}

/** A workflow whose writer has settled with a candidate and now waits for validation (M3). */
export function awaitsValidation(state: RunState): boolean {
  return state === "validating";
}

/** Native Roots records keep their original wire names and additional fields. */
export interface IntentNode { id: string; kind: string; slug: string; status: string; author: string; createdAt: string; decidedBy?: string; [key: string]: unknown }
export interface IntentEdge { id: string; from: string; to: string; rel: string; by: string; createdAt: string; [key: string]: unknown }
export interface IntentProposal { id: string; kind: string; from?: string; to?: string; rel?: string | null; reason?: string; cites: { node: string; quote: string }[]; by: string; status: string; createdAt: string; decidedBy?: string; [key: string]: unknown }
export interface IntentQuestion { id: string; node: string; text: string; by: string; status: string; createdAt: string; [key: string]: unknown }
export interface IntentSettings { project: string; version: string; tier: number; agent_timeout_seconds: number; proposal_limit: number; sprout_limit: number; questions_per_session: number; proposal_ttl_days: number; sprout_ttl_days: number; view_write: boolean; flow_heading: boolean }
export interface IntentView { settings: IntentSettings; nodes: IntentNode[]; edges: IntentEdge[] }
export interface IntentHumanRequest { by: string }
export interface IntentRejectRequest extends IntentHumanRequest { reason?: string }
export interface IntentAcceptRequest extends IntentRejectRequest { rel?: string; keep?: string }
export interface IntentDecision { id: string; action: string; proposals: IntentProposal[]; edge: IntentEdge | null; statusChanges: { node: string; from: string; to: string }[]; think: { node: string; guidance: string } | null }

function intentHuman<T extends IntentHumanRequest>(request: T): T {
  const by = request.by.trim();
  if (!by || new TextEncoder().encode(by).length > 200 || by.startsWith("agent:") || by.startsWith("roots:") || by === "roots") throw new Error("Supply your explicit human name, at most 200 UTF-8 bytes; agent and Roots identities are refused.");
  return { ...request, by };
}

export class LuunaClient {
  readonly baseUrl: string;
  private readonly token: string;
  private readonly fetcher: typeof fetch;

  constructor(baseUrl: string, token: string, fetcher: typeof fetch = fetch) {
    this.baseUrl = loopbackUrl(baseUrl);
    if (!token.trim()) throw new Error("Enter the local service token.");
    this.token = token;
    this.fetcher = fetcher;
  }

  private async request<T>(path: string, init: RequestInit = {}): Promise<T> {
    const headers = new Headers(init.headers);
    headers.set("Authorization", `Bearer ${this.token}`);
    headers.set("Accept", "application/json");
    if (init.body) headers.set("Content-Type", "application/json");
    const response = await this.fetcher.call(globalThis, `${this.baseUrl}/api/v1${path}`, {
      ...init,
      headers,
      cache: "no-store",
      credentials: "omit",
      redirect: "error",
    });
    if (!response.ok) throw await failureOf(response);
    return await response.json() as T;
  }

  health(signal?: AbortSignal): Promise<Health> {
    return this.request("/health", { signal });
  }

  /** The running runtime's doctor report: environment and credential presence by name, never values. */
  doctor(signal?: AbortSignal): Promise<DoctorReport> {
    return this.request("/doctor", { signal });
  }

  /** Asks the runtime to stop in an orderly way (ADR 0010); it answers 202 and drains. */
  shutdown(signal?: AbortSignal): Promise<{ status: string }> {
    return this.request("/shutdown", { method: "POST", signal });
  }

  /**
   * One SSE connection to a run's durable event stream, resumed after `after` through
   * `Last-Event-ID`. It yields every event in sequence order and returns when the runtime closes
   * the stream: the run settled, or the runtime is shutting down. Transport failures throw.
   */
  async *streamEvents(id: string, after = 0, signal?: AbortSignal): AsyncGenerator<EventEnvelope, void, undefined> {
    if (!Number.isSafeInteger(after) || after < 0) throw new Error("Invalid event cursor.");
    const headers = new Headers({ Authorization: `Bearer ${this.token}`, Accept: "text/event-stream" });
    if (after > 0) headers.set("Last-Event-ID", String(after));
    const response = await this.fetcher.call(globalThis, `${this.baseUrl}/api/v1/runs/${encodeURIComponent(id)}/stream?after=${after}`, {
      headers,
      cache: "no-store",
      credentials: "omit",
      redirect: "error",
      signal,
    });
    if (!response.ok) throw await failureOf(response);
    if (!response.body) throw new Error("The runtime returned no event stream.");
    let cursor = after;
    for await (const frame of sseFrames(response.body)) {
      if (frame.event !== "run_event" || !frame.data) continue;
      const record = JSON.parse(frame.data) as EventEnvelope;
      if (!record.event_id || record.run_id !== id || !Number.isSafeInteger(record.sequence) || record.sequence <= cursor) {
        throw new Error("Runtime returned an event for a different run or out of sequence.");
      }
      cursor = record.sequence;
      yield record;
    }
  }

  /**
   * Follows a run's events live until it settles, reconnecting after an outage from the last
   * received sequence (the runtime comes back by itself). A rejected token, a denied action or a
   * forgotten run ends the follow with its error; everything else is reported through `hooks.outage`.
   */
  async *followEvents(id: string, after = 0, signal?: AbortSignal, hooks: FollowHooks = {}): AsyncGenerator<EventEnvelope, void, undefined> {
    let cursor = after;
    let lost = false;
    const wait = hooks.wait ?? pause;
    while (!signal?.aborted) {
      try {
        for await (const event of this.streamEvents(id, cursor, signal)) {
          if (lost) { lost = false; hooks.reconnected?.(); }
          cursor = event.sequence;
          yield event;
        }
        // A closed stream means the run settled, or the runtime is stopping: the run says which.
        const run = await this.run(id, signal);
        if (lost) { lost = false; hooks.reconnected?.(); }
        if (isTerminal(run.state)) return;
        await wait(1000, signal);
      } catch (failure) {
        if (signal?.aborted) return;
        if (failure instanceof ApiError && [401, 403, 404].includes(failure.status)) throw failure;
        lost = true;
        hooks.outage?.(failure);
        await wait(2500, signal).catch(() => undefined);
      }
    }
  }

  /** Newest first; `limit` keeps the most recent records. */
  listRuns(signal?: AbortSignal, page: PageOptions = {}): Promise<RunRecord[]> {
    return this.request(`/runs${pageQuery(page, "?")}`, { signal });
  }

  runsPage(before?: string, project?: string, signal?: AbortSignal): Promise<RunRecord[]> {
    const query = new URLSearchParams({ limit: "200" });
    if (before) query.set("before", before);
    if (project) query.set("project", project);
    return this.request(`/runs?${query}`, { signal });
  }

  run(id: string, signal?: AbortSignal): Promise<RunRecord> {
    return this.request(`/runs/${encodeURIComponent(id)}`, { signal });
  }

  startRun(request: RunRequest, signal?: AbortSignal): Promise<RunRecord> {
    return this.request("/runs", { method: "POST", body: JSON.stringify(request), signal });
  }

  cancelRun(id: string, signal?: AbortSignal): Promise<RunRecord> {
    return this.request(`/runs/${encodeURIComponent(id)}/cancel`, { method: "POST", signal });
  }

  events(id: string, after = 0, signal?: AbortSignal, page: PageOptions = {}): Promise<EventEnvelope[]> {
    if (!Number.isSafeInteger(after) || after < 0) throw new Error("Invalid event cursor.");
    return this.request(`/runs/${encodeURIComponent(id)}/events?after=${after}${pageQuery(page, "&")}`, { signal });
  }

  eventPage(id: string, after = 0, before?: number, signal?: AbortSignal): Promise<EventEnvelope[]> {
    if (!Number.isSafeInteger(after) || after < 0 || (before !== undefined && (!Number.isSafeInteger(before) || before < 1))) throw new Error("Invalid event cursor.");
    return this.request(`/runs/${encodeURIComponent(id)}/events?after=${after}&limit=200${before === undefined ? "" : `&before=${before}`}`, { signal });
  }

  listProjects(signal?: AbortSignal): Promise<ProjectRecord[]> {
    return this.request("/projects", { signal });
  }

  project(id: string, signal?: AbortSignal): Promise<ProjectView> {
    return this.request(`/projects/${encodeURIComponent(id)}`, { signal });
  }

  /** Registers existing repositories by absolute path; the runtime inspects them read-only. */
  registerProject(request: RegisterProjectRequest, signal?: AbortSignal): Promise<ProjectView> {
    return this.request("/projects", { method: "POST", body: JSON.stringify(request), signal });
  }

  discover(id: string, signal?: AbortSignal): Promise<Discovery> {
    return this.request(`/projects/${encodeURIComponent(id)}/discover`, { method: "POST", signal });
  }

  /** Saves the confirmed map; `expectedRevision` is the project's current manifest revision. */
  confirmManifest(id: string, expectedRevision: number, manifest: ProjectManifest, signal?: AbortSignal): Promise<ProjectView> {
    if (!Number.isSafeInteger(expectedRevision) || expectedRevision < 0) throw new Error("Invalid manifest revision.");
    return this.request(`/projects/${encodeURIComponent(id)}/manifest`, {
      method: "PUT",
      body: JSON.stringify({ expected_revision: expectedRevision, manifest }),
      signal,
    });
  }

  projectEvents(id: string, after = 0, signal?: AbortSignal, page: PageOptions = {}): Promise<EventEnvelope[]> {
    if (!Number.isSafeInteger(after) || after < 0) throw new Error("Invalid event cursor.");
    return this.request(`/projects/${encodeURIComponent(id)}/events?after=${after}${pageQuery(page, "&")}`, { signal });
  }

  /**
   * Re-points a moved or re-identified checkout after re-inspecting it read-only; the request is
   * the developer's explicit intent. A manifest whose remote no longer matches must be confirmed again.
   */
  rebindRepository(projectId: string, repositoryId: string, request: RebindRepositoryRequest = {}, signal?: AbortSignal): Promise<ProjectView> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/repositories/${encodeURIComponent(repositoryId)}/rebind`, { method: "POST", body: JSON.stringify(request), signal });
  }

  listBaselines(id: string, signal?: AbortSignal): Promise<BaselineEvidence[]> {
    return this.request(`/projects/${encodeURIComponent(id)}/baselines`, { signal });
  }

  /** Starts a baseline run; it is an ordinary run with events, cancellation and replay. */
  startBaseline(id: string, request: StartBaselineRequest, signal?: AbortSignal): Promise<RunRecord> {
    return this.request(`/projects/${encodeURIComponent(id)}/baselines`, { method: "POST", body: JSON.stringify(request), signal });
  }

  /** Creates a ticket through the project's authoritative source. */
  createTicket(projectId: string, request: CreateTicketRequest, signal?: AbortSignal): Promise<Ticket> {
    if (!request.idempotency_key.trim()) throw new Error("An idempotency key is required.");
    return this.request(`/projects/${encodeURIComponent(projectId)}/tickets`, { method: "POST", body: JSON.stringify(request), signal });
  }

  listTickets(projectId: string, signal?: AbortSignal): Promise<Ticket[]> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/tickets`, { signal });
  }

  ticket(id: string, signal?: AbortSignal): Promise<Ticket> {
    return this.request(`/tickets/${encodeURIComponent(id)}`, { signal });
  }

  reconcileTicket(id: string, expectedRevision: number, signal?: AbortSignal): Promise<Ticket> {
    if (!Number.isSafeInteger(expectedRevision) || expectedRevision < 1) throw new Error("Invalid ticket revision.");
    return this.request(`/tickets/${encodeURIComponent(id)}/reconcile`, {
      method: "POST", body: JSON.stringify({ expected_revision: expectedRevision }), signal,
    });
  }

  /** Edits a ticket at the revision the client read; the runtime advances the revision and stales earlier plans. */
  updateTicket(id: string, request: UpdateTicketRequest, signal?: AbortSignal): Promise<Ticket> {
    if (!Number.isSafeInteger(request.expected_revision) || request.expected_revision < 1) throw new Error("Invalid ticket revision.");
    if (request.title === undefined && request.intent === undefined && request.acceptance_criteria === undefined && request.repository_ids === undefined) throw new Error("Name at least one ticket field to change.");
    return this.request(`/tickets/${encodeURIComponent(id)}`, { method: "PUT", body: JSON.stringify(request), signal });
  }

  ticketSource(id: string, signal?: AbortSignal): Promise<TicketSourceStatus> {
    return this.request(`/tickets/${encodeURIComponent(id)}/source`, { signal });
  }

  components(projectId: string, signal?: AbortSignal): Promise<ComponentStatus> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/components`, { signal });
  }

  enableComponents(projectId: string, signal?: AbortSignal): Promise<ComponentStatus> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/components`, { method: "POST", signal });
  }

  listMemory(projectId: string, options: { query?: string; includeUser?: boolean } = {}, signal?: AbortSignal): Promise<MemoryRecord[]> {
    const parameters = [options.query ? `q=${encodeURIComponent(options.query)}` : "", options.includeUser ? "include_user=true" : ""].filter(Boolean);
    const search = parameters.length ? `?${parameters.join("&")}` : "";
    return this.request(`/projects/${encodeURIComponent(projectId)}/memory${search}`, { signal });
  }

  createMemory(projectId: string, request: CreateMemoryRequest, signal?: AbortSignal): Promise<MemoryRecord> {
    if (!request.idempotency_key.trim()) throw new Error("An idempotency key is required.");
    return this.request(`/projects/${encodeURIComponent(projectId)}/memory`, { method: "POST", body: JSON.stringify(request), signal });
  }

  deleteMemory(projectId: string, id: string, expectedHash: string, signal?: AbortSignal): Promise<{ deleted: boolean }> {
    if (!expectedHash.startsWith("sha256:")) throw new Error("Delete memory by its exact content hash.");
    return this.request(`/projects/${encodeURIComponent(projectId)}/memory/${encodeURIComponent(id)}?expected_hash=${encodeURIComponent(expectedHash)}`, { method: "DELETE", signal });
  }

  /** Corrects a memory's content at its exact displayed hash; the record keeps its scope. */
  updateMemory(projectId: string, id: string, request: UpdateMemoryRequest, signal?: AbortSignal): Promise<MemoryRecord> {
    if (!request.expected_hash.startsWith("sha256:")) throw new Error("Correct memory by its exact content hash.");
    if (!request.idempotency_key.trim()) throw new Error("An idempotency key is required.");
    if (!request.content.trim()) throw new Error("Memory content is required.");
    return this.request(`/projects/${encodeURIComponent(projectId)}/memory/${encodeURIComponent(id)}`, { method: "PUT", body: JSON.stringify(request), signal });
  }

  /** The personal export: user memory with provenance; the runtime refuses one carrying a credential value or a host path. */
  exportUserMemory(signal?: AbortSignal): Promise<UserMemoryExport> {
    return this.request("/memory/user/export", { signal });
  }

  memoryRetention(projectId: string, signal?: AbortSignal): Promise<MemoryRetentionPolicy> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/memory/retention`, { signal });
  }

  /** Replaces the policy; nothing is deleted until `applyMemoryRetention`. */
  setMemoryRetention(projectId: string, policy: MemoryRetentionPolicy, signal?: AbortSignal): Promise<MemoryRetentionPolicy> {
    for (const days of [policy.process_max_age_days, policy.project_max_age_days]) {
      if (days !== null && (!Number.isSafeInteger(days) || days < 1 || days > 3650)) throw new Error("A retention limit is between 1 and 3650 days, or null to keep records.");
    }
    return this.request(`/projects/${encodeURIComponent(projectId)}/memory/retention`, { method: "PUT", body: JSON.stringify(policy), signal });
  }

  /** Deletes expired project and process memory through the adapter; replaying the key returns the recorded result. */
  applyMemoryRetention(projectId: string, request: ApplyMemoryRetentionRequest, signal?: AbortSignal): Promise<MemoryRetentionResult> {
    if (!request.idempotency_key.trim()) throw new Error("An idempotency key is required.");
    return this.request(`/projects/${encodeURIComponent(projectId)}/memory/retention/apply`, { method: "POST", body: JSON.stringify(request), signal });
  }

  /** Widens a memory's scope one step (process -> project, project -> user); the narrower record is deleted after the wider one exists. */
  promoteMemory(projectId: string, id: string, request: PromoteMemoryRequest, signal?: AbortSignal): Promise<PromotedMemory> {
    if (!request.expected_hash.startsWith("sha256:")) throw new Error("Promote memory by its exact content hash.");
    return this.request(`/projects/${encodeURIComponent(projectId)}/memory/${encodeURIComponent(id)}/promote`, { method: "POST", body: JSON.stringify(request), signal });
  }

  /** The scope-safe export: manifest, tickets, project/process memory and prompts; the runtime refuses one that would carry user memory, credentials or host paths. */
  exportProject(projectId: string, signal?: AbortSignal): Promise<ProjectExport> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/export`, { signal });
  }

  /** Registers the exported project from local checkouts of its repositories, confirms its manifest and brings tickets, memory and prompts in; the same key replays. */
  importProject(request: ProjectImportRequest, signal?: AbortSignal): Promise<ProjectImportResult> {
    if (!request.export || typeof request.export !== "object" || request.export.kind !== "luuna-project-export") throw new Error("The document is not a Luuna project export.");
    if (!request.idempotency_key.trim()) throw new Error("An idempotency key is required.");
    if (request.repositories.length === 0) throw new Error("Give the local checkout path of each exported repository.");
    return this.request("/projects/import", { method: "POST", body: JSON.stringify(request), signal });
  }

  /** Orbit status: every registered checkout of the project, read-only. */
  orbit(projectId: string, signal?: AbortSignal): Promise<OrbitView> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/orbit`, { signal });
  }

  /** Runs one executable plus arguments in each selected repository (the developer's command, never evidence). */
  orbitExec(projectId: string, request: OrbitExecRequest, signal?: AbortSignal): Promise<OrbitExecution> {
    if (!request.executable.trim()) throw new Error("Type a command to run in each repository.");
    const timeout = request.timeout_seconds;
    if (timeout !== undefined && (!Number.isSafeInteger(timeout) || timeout < ORBIT_TIMEOUT_LIMITS[0] || timeout > ORBIT_TIMEOUT_LIMITS[1])) throw new Error(`The timeout must be ${ORBIT_TIMEOUT_LIMITS[0]} to ${ORBIT_TIMEOUT_LIMITS[1]} seconds.`);
    return this.request(`/projects/${encodeURIComponent(projectId)}/orbit/exec`, { method: "POST", body: JSON.stringify(request), signal });
  }

  /** Runs a confirmed manifest recipe in each repository that defines it; the others are skipped. */
  orbitRun(projectId: string, request: OrbitRunRequest, signal?: AbortSignal): Promise<OrbitExecution> {
    if (!request.recipe.trim()) throw new Error("Choose a recipe to run.");
    return this.request(`/projects/${encodeURIComponent(projectId)}/orbit/run`, { method: "POST", body: JSON.stringify(request), signal });
  }

  /** Clones missing checkouts from their registered remotes and fetches present ones. */
  orbitSync(projectId: string, request: OrbitSyncRequest = {}, signal?: AbortSignal): Promise<OrbitExecution> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/orbit/sync`, { method: "POST", body: JSON.stringify(request), signal });
  }

  /** The sandbox plan `luuna-sbx` applies: only this project's checkouts, a private home and the network policy. */
  orbitSandbox(projectId: string, network: SandboxNetwork = "open", signal?: AbortSignal): Promise<SandboxPlan> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/orbit/sandbox?network=${network}`, { signal });
  }

  /** A deterministic scaffold proposal; nothing is written until it is confirmed by exact hash. */
  proposeScaffold(request: ScaffoldProposalRequest, signal?: AbortSignal): Promise<ScaffoldProposal> {
    return this.request(`/projects/scaffold/propose`, { method: "POST", body: JSON.stringify(request), signal });
  }

  /** Executes a confirmed proposal: files, an initial commit, the real baseline, registration. */
  scaffold(request: ScaffoldRequest, signal?: AbortSignal): Promise<ScaffoldResult> {
    if (request.confirm !== true) throw new Error("Scaffolding requires explicit confirmation of the proposal hash.");
    return this.request(`/projects/scaffold`, { method: "POST", body: JSON.stringify(request), signal });
  }

  listPrompts(projectId: string, signal?: AbortSignal): Promise<PromptRecord[]> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/prompts`, { signal });
  }

  createPrompt(projectId: string, request: CreatePromptRequest, signal?: AbortSignal): Promise<RenderedPrompt> {
    if (!request.idempotency_key.trim()) throw new Error("An idempotency key is required.");
    return this.request(`/projects/${encodeURIComponent(projectId)}/prompts`, { method: "POST", body: JSON.stringify(request), signal });
  }

  renderPrompt(projectId: string, name: string, version: number, signal?: AbortSignal): Promise<RenderedPrompt> {
    if (!Number.isSafeInteger(version) || version < 1) throw new Error("Invalid prompt version.");
    return this.request(`/projects/${encodeURIComponent(projectId)}/prompts/${encodeURIComponent(name)}/render?version=${version}`, { signal });
  }

  showIntent(projectId: string, signal?: AbortSignal): Promise<IntentView> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/intent`, { signal });
  }
  initIntent(projectId: string, request: IntentHumanRequest, signal?: AbortSignal): Promise<IntentView> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/intent`, { method: "POST", body: JSON.stringify(intentHuman(request)), signal });
  }
  listIntentProposals(projectId: string, signal?: AbortSignal): Promise<IntentProposal[]> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/intent/proposals`, { signal });
  }
  listIntentQuestions(projectId: string, signal?: AbortSignal): Promise<IntentQuestion[]> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/intent/questions`, { signal });
  }
  listIntentSprouts(projectId: string, signal?: AbortSignal): Promise<IntentNode[]> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/intent/sprouts`, { signal });
  }
  acceptIntentProposal(projectId: string, proposalId: string, request: IntentAcceptRequest, signal?: AbortSignal): Promise<IntentDecision> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/intent/proposals/${encodeURIComponent(proposalId)}/accept`, { method: "POST", body: JSON.stringify(intentHuman(request)), signal });
  }
  rejectIntentProposal(projectId: string, proposalId: string, request: IntentRejectRequest, signal?: AbortSignal): Promise<IntentDecision> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/intent/proposals/${encodeURIComponent(proposalId)}/reject`, { method: "POST", body: JSON.stringify(intentHuman(request)), signal });
  }
  rejectIntentSprout(projectId: string, sproutId: string, request: IntentRejectRequest, signal?: AbortSignal): Promise<IntentNode> {
    return this.request(`/projects/${encodeURIComponent(projectId)}/intent/sprouts/${encodeURIComponent(sproutId)}/reject`, { method: "POST", body: JSON.stringify(intentHuman(request)), signal });
  }

  listPlans(ticketId: string, signal?: AbortSignal): Promise<PlanRevision[]> {
    return this.request(`/tickets/${encodeURIComponent(ticketId)}/plans`, { signal });
  }

  /** Records an immutable plan revision; the runtime binds ticket, manifest, base and recipes. */
  async createPlan(ticketId: string, request: CreatePlanRequest, signal?: AbortSignal): Promise<PlanOpened> {
    if (request.idempotency_key !== undefined && !validIdempotencyKey(request.idempotency_key)) throw new Error("An idempotency key is 1 to 128 visible ASCII characters.");
    const problem = request.budgets ? planBudgetProblem(request.budgets) : null;
    if (problem) throw new Error(problem);
    const path = `/tickets/${encodeURIComponent(ticketId)}/plans`;
    try {
      return await this.request(path, { method: "POST", body: JSON.stringify(request), signal });
    } catch (failure) {
      // A runtime that predates plan idempotency rejects the unknown field while decoding the
      // body (a 422 without a JSON reason), before anything is recorded; send it once without.
      if (!(failure instanceof ApiError) || failure.status !== 422 || failure.fromRuntime || request.idempotency_key === undefined) throw failure;
      const { idempotency_key: _unsupported, ...legacy } = request;
      return this.request(path, { method: "POST", body: JSON.stringify(legacy), signal });
    }
  }

  plan(id: string, signal?: AbortSignal): Promise<PlanRevision> {
    return this.request(`/plans/${encodeURIComponent(id)}`, { signal });
  }

  /** Approves exactly one plan revision by hash; permissions come from the plan, never the client. */
  approvePlan(id: string, request: ApprovePlanRequest, signal?: AbortSignal): Promise<PlanApproved> {
    if (!request.plan_hash.startsWith("sha256:")) throw new Error("Approve the plan by its exact hash.");
    return this.request(`/plans/${encodeURIComponent(id)}/approvals`, { method: "POST", body: JSON.stringify(request), signal });
  }

  /** Schedules the approved workflow's writer; the runtime revalidates everything before it starts. */
  implement(runId: string, request: ImplementRequest = {}, signal?: AbortSignal): Promise<RunRecord> {
    return this.request(`/runs/${encodeURIComponent(runId)}/implement`, { method: "POST", body: JSON.stringify(request), signal });
  }

  worktrees(runId: string, signal?: AbortSignal): Promise<WorktreeLease[]> {
    return this.request(`/runs/${encodeURIComponent(runId)}/worktrees`, { signal });
  }

  attempts(runId: string, signal?: AbortSignal): Promise<AttemptRecord[]> {
    return this.request(`/runs/${encodeURIComponent(runId)}/attempts`, { signal });
  }

  candidate(runId: string, signal?: AbortSignal): Promise<CandidateView> {
    return this.request(`/runs/${encodeURIComponent(runId)}/candidate`, { signal });
  }

  /** Publishes a reviewed candidate: pushes its exact commits and opens linked pull requests, once per idempotency key. */
  publish(runId: string, request: PublishRequest, signal?: AbortSignal): Promise<PublicationRecord> {
    if (!request.expected_hash.startsWith("sha256:")) throw new Error("Publish by the exact candidate hash.");
    return this.request(`/runs/${encodeURIComponent(runId)}/publish`, { method: "POST", body: JSON.stringify(request), signal });
  }

  /** The durable publication record with CI and merge observation refreshed; merging is never performed. */
  publication(runId: string, signal?: AbortSignal): Promise<PublicationRecord> {
    return this.request(`/runs/${encodeURIComponent(runId)}/publication`, { signal });
  }

  /** Retries only the steps a partial publication still lacks, reconciling what the remote already has. */
  reconcilePublication(runId: string, signal?: AbortSignal): Promise<PublicationRecord> {
    return this.request(`/runs/${encodeURIComponent(runId)}/publication/reconcile`, { method: "POST", signal });
  }

  /** Sends a published candidate whose pull request failed CI to a bounded repair: the runtime re-observes CI, invalidates the reviewed evidence naming the failing checks, marks the publication superseded and starts the repair writer. */
  repairPublication(runId: string, request: CandidateActionRequest, signal?: AbortSignal): Promise<PublicationRepair> {
    if (!/^sha256:[a-f0-9]{64}$/.test(request.candidate_hash)) throw new Error("Use the candidate's complete hash.");
    return this.request(`/runs/${encodeURIComponent(runId)}/publication/repair`, { method: "POST", body: JSON.stringify(request), signal });
  }

  /** The task graph of a workflow: one durable record per plan task with its state, attempt and exact candidate commit. */
  tasks(runId: string, signal?: AbortSignal): Promise<TaskRecord[]> {
    return this.request(`/runs/${encodeURIComponent(runId)}/tasks`, { signal });
  }

  candidateDiff(runId: string, signal?: AbortSignal): Promise<CandidateDiff> {
    return this.request(`/runs/${encodeURIComponent(runId)}/diff`, { signal });
  }

  /** Runs the approved recipes and an independent reviewer for this exact candidate. */
  validateCandidate(runId: string, request: ValidateCandidateRequest, signal?: AbortSignal): Promise<RunRecord> {
    return this.candidateAction(runId, "validate", request, signal);
  }

  /** Requests a bounded writer repair under the existing approval. */
  repairCandidate(runId: string, request: CandidateActionRequest, signal?: AbortSignal): Promise<RunRecord> {
    return this.candidateAction(runId, "repair", request, signal);
  }

  private candidateAction(runId: string, action: "validate" | "repair", request: CandidateActionRequest, signal?: AbortSignal): Promise<RunRecord> {
    if (!/^sha256:[a-f0-9]{64}$/.test(request.candidate_hash)) throw new Error("Use the candidate's complete hash.");
    return this.request(`/runs/${encodeURIComponent(runId)}/${action}`, { method: "POST", body: JSON.stringify(request), signal });
  }

  /** Reads a private check log by record identity (`<recipe>.stdout` or `<recipe>.stderr`). */
  async artifact(runId: string, artifactId: string, signal?: AbortSignal): Promise<string> {
    if (!/^[A-Za-z0-9._-]+\.(stdout|stderr)$/.test(artifactId) || artifactId.includes("..")) throw new Error("Invalid artifact identifier.");
    const headers = new Headers({ Authorization: `Bearer ${this.token}`, Accept: "text/plain" });
    const response = await this.fetcher.call(globalThis, `${this.baseUrl}/api/v1/runs/${encodeURIComponent(runId)}/artifacts/${encodeURIComponent(artifactId)}`, {
      headers,
      cache: "no-store",
      credentials: "omit",
      redirect: "error",
      signal,
    });
    if (!response.ok) throw new ApiError(response.status, response.status === 404 ? "The artifact is not available." : `Runtime request failed (HTTP ${response.status}).`);
    return await response.text();
  }
}

/** Extract check results from a baseline run's event stream, in execution order. */
export function checkResults(events: EventEnvelope[]): CheckResult[] {
  return events.flatMap((event) => {
    if (event.type !== "check.finished") return [];
    const result = event.payload.result;
    return result && typeof result === "object" ? [result as CheckResult] : [];
  });
}

/** The error for a non-2xx response; a raw server body is never surfaced. */
async function failureOf(response: Response): Promise<ApiError> {
  // Do not surface arbitrary server error bodies: they can contain prompts or secrets.
  // Validation and conflict responses carry short deterministic runtime reasons.
  const messages: Record<number, string> = {
    400: "The runtime rejected these settings. Check the prompt, provider and model configuration.",
    401: "The service rejected this token. Reconnect using its current token.",
    403: "The runtime denied this action.",
    409: "The run state changed or another session is active. Refresh and try again.",
  };
  const refusal = [400, 409, 413, 415, 422, 503].includes(response.status) ? await runtimeRefusal(response) : undefined;
  return new ApiError(response.status, refusal?.reason ?? messages[response.status] ?? `Runtime request failed (HTTP ${response.status}).`, refusal !== undefined, refusal?.prerequisite);
}

/**
 * The runtime's deterministic reason is at most this long; a reason that quotes an offer or an
 * override (ADR 0022, ADR 0023) stays under it, while a body of any other shape is never surfaced.
 */
export const MAX_REASON_LENGTH = 1000;

async function runtimeRefusal(response: Response): Promise<{ reason: string; prerequisite?: TestingPrerequisite } | undefined> {
  try {
    const body: unknown = await response.clone().json();
    if (body && typeof body === "object" && "error" in body && typeof body.error === "string") {
      const reason = body.error.trim();
      if (reason.length > 0 && reason.length <= MAX_REASON_LENGTH && !/[\u0000-\u001f]/.test(reason)) {
        const prerequisite = "prerequisite" in body ? testingPrerequisite(body.prerequisite) : undefined;
        return { reason, ...(prerequisite && { prerequisite }) };
      }
    }
  } catch {
    // Non-JSON bodies are never surfaced.
  }
  return undefined;
}

/** The structured offer, only when it has the recorded shape; anything else is not surfaced. */
function testingPrerequisite(value: unknown): TestingPrerequisite | undefined {
  if (!value || typeof value !== "object") return undefined;
  const record = value as Record<string, unknown>;
  const text = (field: string) => typeof record[field] === "string" && (record[field] as string).length <= 4000 && !/[\u0000-\u001f]/.test(record[field] as string);
  if (record.kind !== "testing_setup" || !["project_id", "repository_id", "baseline_id", "reason", "next"].every(text)) return undefined;
  return { kind: "testing_setup", project_id: record.project_id as string, repository_id: record.repository_id as string, baseline_id: record.baseline_id as string, reason: record.reason as string, next: record.next as string };
}

/** Fold the project event stream into the last confirmed map, or null before confirmation. */
export function confirmedManifest(events: EventEnvelope[]): { revision: number; hash: string; manifest: ProjectManifest } | null {
  let latest: { revision: number; hash: string; manifest: ProjectManifest } | null = null;
  for (const event of events) {
    if (event.type !== "manifest.confirmed") continue;
    const { revision, manifest_hash, manifest } = event.payload;
    if (typeof revision === "number" && typeof manifest_hash === "string" && manifest && typeof manifest === "object") {
      latest = { revision, hash: manifest_hash, manifest: manifest as ProjectManifest };
    }
  }
  return latest;
}

/** Merge replayed events without duplicates; a reconnect can repeat the last page. */
export function mergeEvents<T extends { sequence: number; event_id: string; run_id: string }>(previous: T[], incoming: T[]): T[] {
  const merged = new Map<number, T>();
  const identities = new Map<string, number>();
  const runId = previous[0]?.run_id ?? incoming[0]?.run_id;
  for (const event of [...previous, ...incoming]) {
    if (!Number.isSafeInteger(event.sequence) || event.sequence < 1) {
      throw new Error("Runtime returned an invalid event sequence.");
    }
    if (!event.event_id || !event.run_id || event.run_id !== runId) {
      throw new Error("Runtime returned an event for a different run or with no identity.");
    }
    const existing = merged.get(event.sequence);
    const sequenceForId = identities.get(event.event_id);
    if ((existing && existing.event_id !== event.event_id) || (sequenceForId !== undefined && sequenceForId !== event.sequence)) {
      throw new Error("Runtime returned conflicting event identities.");
    }
    // Preserve the first received record when replay overlaps earlier data.
    if (!existing) merged.set(event.sequence, event);
    identities.set(event.event_id, event.sequence);
  }
  const ordered = [...merged.values()].sort((left, right) => left.sequence - right.sequence);
  if (ordered.some((event, index) => event.sequence !== index + 1)) {
    throw new Error("Runtime event replay has a gap; retrying from the last contiguous sequence.");
  }
  return ordered;
}

/** Incremental replay for a single subscription. A rejected page never advances its cursor. */
export class EventReplay<T extends { sequence: number; event_id: string; run_id: string }> {
  private records: T[] = [];
  private identities = new Map<string, number>();

  get cursor(): number { return this.records.length; }

  append(incoming: T[]): T[] {
    const added: T[] = [];
    const identities = new Map<string, number>();
    const runId = this.records[0]?.run_id ?? incoming[0]?.run_id;
    for (const event of [...incoming].sort((a, b) => a.sequence - b.sequence)) {
      if (!Number.isSafeInteger(event.sequence) || event.sequence < 1) {
        throw new Error("Runtime returned an invalid event sequence.");
      }
      if (!event.event_id || !event.run_id || event.run_id !== runId) {
        throw new Error("Runtime returned an event for a different run or with no identity.");
      }
      const existing = this.records[event.sequence - 1] ?? added[event.sequence - this.records.length - 1];
      const sequenceForId = this.identities.get(event.event_id) ?? identities.get(event.event_id);
      if ((existing && existing.event_id !== event.event_id) || (sequenceForId !== undefined && sequenceForId !== event.sequence)) {
        throw new Error("Runtime returned conflicting event identities.");
      }
      if (existing) continue;
      if (event.sequence !== this.records.length + added.length + 1) {
        throw new Error("Runtime event replay has a gap; retrying from the last contiguous sequence.");
      }
      added.push(event);
      identities.set(event.event_id, event.sequence);
    }
    if (added.length) {
      this.records = this.records.concat(added);
      for (const [id, sequence] of identities) this.identities.set(id, sequence);
    }
    // Empty/overlapping replay is a no-op, including the React state identity.
    return this.records;
  }
}

/** Pi text deltas are display data; completion remains the runtime's RunState. */
export class EventWindow {
  cursor = 0;
  records: EventEnvelope[] = [];
  private deltas = "";
  private finalText = "";
  private runId?: string;
  private readonly textLimit = 1024 * 1024;
  get text(): string { return this.finalText || this.deltas; }

  append(incoming: EventEnvelope[]): EventEnvelope[] {
    const runId = this.runId ?? incoming[0]?.run_id;
    const ids = new Set<string>();
    // Validate the complete page before advancing any display state.
    incoming.forEach((event, index) => {
      if (!event.event_id || ids.has(event.event_id) || event.run_id !== runId || event.sequence !== this.cursor + index + 1) throw new Error("Runtime event replay has a gap or invalid identity.");
      ids.add(event.event_id);
    });
    if (!incoming.length) return this.records;
    this.runId = runId;
    for (const event of incoming) {
      if (event.type === "pi.message_update") this.deltas = this.clip(this.deltas + assistantText([event]));
      if (event.type === "pi.message_end") {
        const text = assistantText([event]);
        if (text) this.finalText = this.clip(text);
      }
    }
    this.cursor = incoming[incoming.length - 1].sequence;
    const records = this.records.concat(incoming).slice(-200);
    let bytes = 0;
    let first = records.length - 1;
    for (; first >= 0; first--) {
      bytes += JSON.stringify(records[first]).length * 2;
      if (bytes > 1024 * 1024 && first < records.length - 1) break;
    }
    this.records = records.slice(first + 1);
    return this.records;
  }

  private clip(text: string): string {
    const suffix = "\n[Display limit reached. Full output remains in durable events.]";
    return text.length > this.textLimit ? text.slice(0, this.textLimit - suffix.length) + suffix : text;
  }
}

export function assistantText(events: EventEnvelope[]): string {
  let deltas = "";
  let finalText = "";
  for (const event of events) {
    const update = event.payload.assistantMessageEvent;
    if (event.type === "pi.message_update" && update && typeof update === "object") {
      const fragment = update as Record<string, unknown>;
      if (fragment.type === "text_delta" && typeof fragment.delta === "string") deltas += fragment.delta;
    }
    const message = event.payload.message;
    if (event.type === "pi.message_end" && message && typeof message === "object") {
      const result = message as Record<string, unknown>;
      if (result.role === "assistant" && Array.isArray(result.content)) {
        finalText = result.content.flatMap((part: unknown) => {
          if (part && typeof part === "object" && "type" in part && part.type === "text" && "text" in part && typeof part.text === "string") return [part.text];
          return [];
        }).join("\n");
      }
    }
  }
  return finalText || deltas;
}

export * from "./failures.ts";
export * from "./projections.ts";
