import { useEffect } from "react";
import type { LuunaClient, ProjectRecord } from "@luuna/api-client";
import { Breadcrumbs, ErrorBanner, Page, Split } from "@luuna/ui";
import { viewInfo } from "../content/views";
import { ProjectDetail } from "./ProjectDetail";
import { ProjectRail } from "./ProjectRail";
import { projectActions } from "./project-actions";
import type { ProjectRequest } from "./project-model";
import { useProjects } from "./useProjects";

/**
 * The Projects view: register an existing repository, review the read-only discovery proposal,
 * confirm an exact manifest revision, run baselines, and work its tickets. The runtime inspects
 * and validates; this view only displays authoritative state and submits the developer's requests.
 */
export function Projects({ client, connectionEpoch = 0, requested = null, onSelection }: { client: LuunaClient | null; connectionEpoch?: number; requested?: ProjectRequest | null; onSelection?: (projects: ProjectRecord[], selected: string | null) => void }) {
  const state = useProjects(client, connectionEpoch, requested);
  // The window's project switcher shows what this view lists and has open.
  useEffect(() => { onSelection?.(state.projects, state.selected); }, [onSelection, state.projects, state.selected]);
  const actions = projectActions(state);
  const info = viewInfo("projects");
  return <Page id="projects" className="projects-panel" title={info.label} description={info.description} breadcrumbs={state.view && <Breadcrumbs items={[{ label: info.label }, { label: state.view.project.name }]} />}>
    {state.error && <ErrorBanner message={state.error} onDismiss={() => state.setError("")} />}
    <Split className="projects-layout" stickyRail rail={<ProjectRail projects={state.projects} selected={state.selected} disabled={!client || state.busy} busy={state.busy} actions={actions} />}>
      <ProjectDetail client={client} connectionEpoch={connectionEpoch} state={state} actions={actions} />
    </Split>
  </Page>;
}
