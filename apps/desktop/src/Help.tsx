import type React from "react";
import { ExternalLink, Id, Kbd, ListGroup, ListRow, Page, PropertyList, Section, Stack, StatusBadge, Text, TONES, candidateTone, runTone, type Tone } from "@luuna/ui";
import { VIEWS, viewInfo, viewShortcut } from "./content/views";

/** The repository the desktop is built from; its documents are linked from the Help panel. */
export const REPOSITORY_URL = "https://github.com/luuna-ai/luuna/blob/main";

export const ONBOARDING_STEPS: readonly { title: string; detail: string }[] = [
  { title: "Connect to the runtime", detail: "The desktop app starts the local runtime and connects; the web build takes the loopback address and the service token, which stays in memory. The sidebar's connection status is the only green that is not evidence." },
  { title: "Register a project", detail: "Register every related repository by its absolute root path. The runtime observes identity, HEAD and dirty state read-only; paths stay in this machine's registry." },
  { title: "Discover and confirm the manifest", detail: "Discovery proposes check recipes and relationships from manifests and lockfiles without running anything. Confirming records an immutable manifest revision bound to the exact base commits." },
  { title: "Establish the baseline", detail: "Run a baseline per repository: its confirmed required recipes on the developer checkout, recorded as exact evidence. A repository with no required check, or whose checks execute no test, is offered the testing-setup prerequisite: a testing_setup ticket whose plan proposes the recipes, approved and implemented like any plan; once its candidate is reviewed you merge it, confirm the recipes and run a passing baseline." },
  { title: "Create and plan a ticket", detail: "A ticket names the intent and acceptance criteria. A plan revision is immutable: its tasks (with roles, dependencies and any justified dependency override), permissions, checks and budgets hash together." },
  { title: "Approve the exact plan", detail: "Review the complete plan and acknowledge its hash. Approval binds to exactly that revision; an edited ticket or a changed manifest stales it." },
  { title: "Implement in a managed worktree", detail: "The approved writer runs in a Luuna-owned worktree with the approved paths; the developer checkout is never reset. A failed, blocked or interrupted task of a settled graph can be resumed under the same approval." },
  { title: "Validate and review", detail: "The required checks run in the candidate worktree and a fresh independent reviewer records structured findings; both bind to the exact candidate hash. Repairs are bounded by the approved budgets." },
  { title: "Publish", detail: "A reviewed candidate pushes its exact commits and opens linked pull requests. Merging stays with you; Luuna never merges." },
];

const TONE_MEANING: Record<Tone, string> = {
  success: "Passed durable evidence: a passed check, baseline or review verdict, a reviewed candidate, a live connection. Green is earned; a completed session is not a validation result.",
  active: "The runtime is working (a writer, checks, a reviewer, a baseline). Cancellation is available while it is.",
  danger: "Failed or blocked evidence: a failed check or run, a blocked baseline, a verdict that requires changes, a blocking finding.",
  attention: "Waiting on you: a plan to approve, an approved plan to implement, a paused candidate, an unconfirmed project, an inferred recipe, a testing-setup offer.",
  neutral: "Recorded, not evidence: queued, completed, cancelled, discovered; a pending candidate; a released lease.",
  interrupted: "Stopped by an outage or a restart, recoverable: the work is durable and its worktree is kept.",
};

/** Every run and candidate value a tone carries, from the same maps the badges use. */
export function statesForTone(tone: Tone): string[] {
  const values = new Set<string>();
  for (const [state, mapped] of Object.entries(runTone)) if (mapped === tone) values.add(`run ${state.replace(/_/g, " ")}`);
  for (const [state, mapped] of Object.entries(candidateTone)) if (mapped === tone) values.add(`candidate ${state.replace(/_/g, " ")}`);
  return [...values];
}

export const EVIDENCE_LOCATIONS: readonly { where: string; what: string }[] = [
  { where: "Projects › Recorded baselines", what: "Each baseline with its manifest revision, exact commit, per-recipe result and the stdout/stderr log of every recipe; a blocked baseline carries the testing-setup offer." },
  { where: "Projects › Project event record", what: "The durable, sequenced project events: registrations, discoveries, manifest confirmations, baselines, offers." },
  { where: "Ticket › Complete immutable plan", what: "The sealed plan revision with its hash, tasks, roles, dependency overrides, required recipes, budgets and selected component resources." },
  { where: "Ticket › Candidate", what: "The candidate hash, per-repository commits and trees, each check's command, exit code, environment fingerprint and logs, the independent review with its findings, workflow usage, and for a setup workflow the handoff." },
  { where: "Ticket › Task graph", what: "Per-task state, exact candidate commit, who assigned it (the approval of the exact plan revision), what it derives from, the overrides that name it, and an advisory task's findings, proposed tasks and risks." },
  { where: "Ticket › Worktrees and the editor handoff", what: "The Luuna-owned worktree path of each lease; the candidate view shows the path to open in your editor and copies it. Opening it transfers no permission." },
  { where: "Session events", what: "The run's durable event record with cursor replay after any interruption." },
  { where: "Project components", what: "Tickets in Missions, scoped memory with provenance in Atlas, versioned prompts in Beacon; retention and the personal export." },
];

export const DOCUMENTS: readonly { path: string; title: string }[] = [
  { path: "AI-START-HERE.md", title: "Start here: how the repository is organised" },
  { path: "docs/MVP-WALKTHROUGH.md", title: "The walkthrough: import, baseline, ticket, plan, writer, validation, review" },
  { path: "docs/09-experience.md", title: "User experience: views, parity, lifecycle" },
  { path: "docs/08-validation.md", title: "Validation: baselines, the testing prerequisite, checks and review" },
  { path: "docs/CURRENT-ARCHITECTURE.md", title: "Current architecture and what is authoritative" },
  { path: "docs/DESIGN-SYSTEM.md", title: "The desktop design system: tones, components, accessibility" },
  { path: "docs/decisions/0022-testing-setup-prerequisite.md", title: "ADR 0022: the testing-setup prerequisite workflow" },
  { path: "docs/decisions/0023-delegated-roles-and-dependency-overrides.md", title: "ADR 0023: roles, assignment traces and dependency overrides" },
  { path: "docs/decisions/0027-v1-advertised-scope.md", title: "ADR 0027: the advertised v1 scope, including these surfaces" },
  { path: "crates/luuna-tui/README.md", title: "The terminal client: the same actions and projections" },
];

/** The keyboard shortcuts: one per view (derived from the view list, Settings is Ctrl+,) and the global ones. */
export const SHORTCUTS: readonly { keys: string; action: string }[] = [
  ...VIEWS.map((view) => ({ keys: viewShortcut(view.id), action: view.label })),
  { keys: "Ctrl+K", action: "Go to a view, project or ticket" },
  { keys: "Ctrl+\\", action: "Collapse sidebar" },
];

/** One Help topic: a titled section of the page. */
function Topic({ id, title, children }: { id: string; title: string; children: React.ReactNode }) {
  return <Section id={id} title={title} variant="plain" className="help-section">{children}</Section>;
}

/** The Help panel of docs/09-experience.md (ADR 0027 point 4): onboarding, what each state means, where evidence lives, and the documents. Static; it decides nothing. */
export function Help() {
  const info = viewInfo("help");
  return <Page id="help" title={info.label} description={info.description} className="help-panel" data-surface="help">
    <Stack gap={6} className="help-sections">
      <Topic id="help-onboarding" title="Onboarding">
        <Stack as="ol" className="help-steps">{ONBOARDING_STEPS.map((step, index) => <ListRow key={step.title} leading={<span className="help-step-number">{index + 1}</span>} title={`${step.title}.`} meta={step.detail} />)}</Stack>
      </Topic>
      <Topic id="help-states" title="What each state means">
        <PropertyList className="help-states" items={TONES.map((tone) => ({
          key: tone,
          term: <StatusBadge tone={tone}>{tone}</StatusBadge>,
          value: <Stack gap="0.5"><Text className="help-state-values">{statesForTone(tone).join(", ") || "connection and chips only"}</Text><Text className="help-state-meaning">{TONE_MEANING[tone]}</Text></Stack>,
        }))} />
      </Topic>
      <Topic id="help-evidence" title="Where evidence lives">
        <ListGroup aria-label="Where evidence lives">{EVIDENCE_LOCATIONS.map((location) => <ListRow key={location.where} title={`${location.where}.`} meta={location.what} />)}</ListGroup>
      </Topic>
      <Topic id="help-shortcuts" title="Keyboard shortcuts">
        <ListGroup aria-label="Keyboard shortcuts" className="help-shortcuts">{SHORTCUTS.map((shortcut) => <ListRow key={shortcut.keys} title={shortcut.action} trailing={<Kbd>{shortcut.keys}</Kbd>} />)}</ListGroup>
      </Topic>
      <Topic id="help-documents" title="Documents">
        <ListGroup aria-label="Documents" className="help-documents">{DOCUMENTS.map((document) => <ListRow key={document.path} title={<ExternalLink href={`${REPOSITORY_URL}/${document.path}`}>{document.title}</ExternalLink>} trailing={<Id>{document.path}</Id>} />)}</ListGroup>
      </Topic>
    </Stack>
  </Page>;
}
