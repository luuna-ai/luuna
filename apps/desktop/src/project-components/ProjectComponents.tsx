import { Button, Disclosure, ErrorBanner, Grid, Hint, Id, Section, StatusLine, Surface } from "@luuna/ui";
import { IntentPanel } from "./Intent";
import { AtlasMemory } from "./AtlasMemory";
import { BeaconPrompts } from "./Prompts";
import { useComponents } from "./useComponents";
import type { ComponentsProps } from "./useComponents";

/** A project's components: tickets in Missions, knowledge in Atlas, prompts in Beacon. The runtime owns component storage, authority and resource identity. */
export function ProjectComponents(props: ComponentsProps) {
  const state = useComponents(props);
  const { client, projectId, status, busy, loading, selectedMemory, selectedPrompt } = state;
  const enable = () => void state.guarded("Component activation", async (signal) => {
    const next = await client!.enableComponents(projectId, signal);
    if (signal.aborted) return;
    state.setStatus(next); state.onStatusChange(next);
  });
  return <Section id="components-heading" variant="subpanel" className="project-components" title="Project components"
    description="Tickets in Missions, chosen knowledge in Atlas and versioned prompts in Beacon. What you select here is bound into the plan you approve."
    accessory={<Button variant="text" disabled={!client || busy || loading} data-capability="components.status" onClick={() => state.setRefreshTick((value) => value + 1)}>Refresh components</Button>}>
    {state.error && <ErrorBanner message={state.error} onDismiss={() => state.setError("")} />}
    {status?.enabled ? <StatusLine className="component-status">{loading ? "Refreshing component state…" : "Project components enabled"}</StatusLine>
      : <Surface level="raised" padded className="component-disabled">
        <StatusLine className="component-status">{loading ? "Refreshing component state…" : status?.reason ?? "Project components are not enabled."}</StatusLine>
        <Button variant="primary" disabled={!client || busy || loading || status === null} data-capability="components.enable" onClick={enable}>Enable project components</Button>
      </Surface>}
    <IntentPanel client={client} projectId={projectId} />
    {status?.enabled && <>
      {status.workspace_path && <Disclosure section={false} className="component-location" summary="Component data location">
        <Id>{status.workspace_path}</Id>
        <Hint>Managed local stores support external CLI edits; refresh and reconcile before re-planning a changed ticket.</Hint>
      </Disclosure>}
      <Grid cols={{ base: 2, compact: 1 }} gap={4} className="component-columns">
        <Surface level="muted" padded><AtlasMemory state={state} /></Surface>
        <Surface level="muted" padded><BeaconPrompts state={state} /></Surface>
      </Grid>
      <StatusLine className="component-context">
        Next plan: {selectedMemory.length} selected memory record(s){selectedPrompt ? ` · ${selectedPrompt.name} v${selectedPrompt.version}` : " · no prompt selected"}. Selected content and provenance appear in the plan for review and are sent to the writer and reviewer only after approval. Private user memory requires explicit selection.
      </StatusLine>
    </>}
  </Section>;
}
