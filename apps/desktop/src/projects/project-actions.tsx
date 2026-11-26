import type { LuunaClient, ProjectImportResult, ScaffoldProposal, ScaffoldResult } from "@luuna/api-client";
import { ApiError } from "@luuna/api-client";
import { parseManifestDraft, registrationRequest } from "./project-model";
import type { RepositoryRow } from "./project-model";
import type { ImportRequest } from "./ImportForm";
import type { ScaffoldActions } from "./ScaffoldForm";
import type { ProjectsState } from "./useProjects";

/**
 * The Projects panel's requests to the runtime, each through the panel's action guard, so one
 * runs at a time, a refusal is reported with the runtime's reason, and a completed one is
 * confirmed. Forms keep their own fields and clear them when a request resolves true.
 */
export function projectActions(state: ProjectsState) {
  const { client, selected, view, guarded, refresh } = state;

  /** Baselines are per repository: each registered repository runs its own confirmed recipes. */
  function runBaseline(repositoryId: string) {
    if (!selected || !view) return;
    if (!view.repositories.some((repository) => repository.repository_id === repositoryId)) return;
    void guarded("Baseline", async (signal) => {
      if (!client) return;
      const run = await client.startBaseline(selected, { repository_id: repositoryId }, signal);
      if (signal.aborted) return;
      state.setPendingBaselineRuns((current) => ({ ...current, [repositoryId]: run.id }));
      await refresh(selected, signal);
    });
  }

  function cancelBaseline(runId: string) {
    void guarded("Baseline cancellation", async (signal) => {
      if (!client || !selected) return;
      await client.cancelRun(runId, signal);
      if (signal.aborted) return;
      await refresh(selected, signal);
    });
  }

  function viewBaselineLog(runId: string, artifactId: string) {
    void guarded("Log inspection", async (signal) => {
      if (!client) return;
      const text = await client.artifact(runId, artifactId, signal);
      if (!signal.aborted) state.setBaselineLog({ runId, artifactId, text });
    });
  }

  function rebind(repositoryId: string, path: string) {
    if (!selected) return Promise.resolve(false);
    return guarded("Rebind", async (signal) => {
      if (!client) return;
      await client.rebindRepository(selected, repositoryId, path ? { local_path: path } : {}, signal);
      if (signal.aborted) return;
      await refresh(selected, signal);
    });
  }

  /** ADR 0063: the project leaves every list; its records stay and its folders bring it back. */
  function remove() {
    if (!selected) return Promise.resolve(false);
    return guarded("Project removal", async (signal) => {
      if (!client) return;
      await client.removeProject(selected, signal);
      if (signal.aborted) return;
      await state.afterRemoval(signal);
    });
  }

  function register(name: string, projectId: string, rows: RepositoryRow[]) {
    return guarded("Registration", async (signal) => {
      if (!client) return;
      // Every row is sent; the shared builder refuses an incomplete or repeated row before any request.
      const created = await client.registerProject(registrationRequest(name, projectId, rows), signal);
      if (signal.aborted) return;
      await refresh(created.project.id, signal);
    });
  }

  function exportProject() {
    if (!selected) return;
    void guarded("Project export", async (signal) => {
      if (!client) return;
      const value = await client.exportProject(selected, signal);
      if (!signal.aborted) state.setExported(value);
    });
  }

  async function importProject({ draft, repositories }: ImportRequest): Promise<ProjectImportResult | null> {
    let result: ProjectImportResult | null = null;
    await guarded("Project import", async (signal) => {
      if (!client) return;
      const imported = await client.importProject({ export: draft, repositories, idempotency_key: `import-${(draft.export_hash ?? "").slice(7, 31) || draft.project.id}` }, signal);
      if (signal.aborted) return;
      result = imported;
      await refresh(imported.project.project.id, signal);
    });
    return result;
  }

  const scaffold: ScaffoldActions = {
    async propose(request) {
      let proposal: ScaffoldProposal | null = null;
      await guarded("Scaffold proposal", async (signal) => {
        if (!client) return;
        const value = await client.proposeScaffold(request, signal);
        if (!signal.aborted) proposal = value;
      });
      return proposal;
    },
    async confirm(proposal) {
      let result: ScaffoldResult | null = null;
      const hash = proposal.hash;
      const network = proposal.network_steps.length > 0;
      await guarded("Scaffolding", async (signal) => {
        if (!client) return;
        const created = await client.scaffold({ proposal_hash: hash, idempotency_key: `scaffold-${hash.slice(7, 31)}`, confirm: true, ...(network && { confirm_network: true }) }, signal);
        if (signal.aborted) return;
        result = created;
        await refresh(created.project.project.id, signal);
      });
      return result;
    },
  };

  function discover() {
    if (!selected) return;
    void guarded("Discovery", async (signal) => {
      if (!client) return;
      const proposal = await client.discover(selected, signal);
      if (signal.aborted) return;
      state.setDiscovery(proposal);
      state.setDraft(JSON.stringify(proposal.proposed_manifest, null, 2));
      await refresh(selected, signal);
    });
  }

  function confirm() {
    if (!selected || !view) return;
    const draft = state.draft;
    void guarded("Manifest confirmation", async (signal) => {
      if (!client) return;
      const manifest = parseManifestDraft(draft);
      try {
        await client.confirmManifest(selected, view.project.revision, manifest as Parameters<LuunaClient["confirmManifest"]>[2], signal);
      } catch (failure) {
        // A runtime that answers a shape error without a JSON reason still gets a useful sentence.
        if (failure instanceof ApiError && failure.status === 422 && !failure.fromRuntime) throw new Error("The runtime could not read the manifest draft: a field is missing, misspelled or of the wrong type (for example a quoted true). Compare the draft with schemas/project.schema.json, then confirm again.");
        throw failure;
      }
      if (signal.aborted) return;
      await refresh(selected, signal);
      // The confirmed proposal is spent; a new discovery produces the next revision's draft.
      state.setDiscovery(null);
      state.setDraft("");
    });
  }

  /** Selecting another project clears what the panel held for the previous one. */
  function selectProject(id: string) {
    state.setComponentContext({ memory_ids: [] });
    state.setComponentStatus(null);
    state.setSelected(id);
  }

  return { runBaseline, cancelBaseline, viewBaselineLog, rebind, remove, register, exportProject, importProject, scaffold, discover, confirm, selectProject };
}

export type ProjectActions = ReturnType<typeof projectActions>;
