import { useEffect, useRef, useState } from "react";
import type { ComponentContext, LuunaClient, PlanRevision, TaskRecord, TestingPrerequisite } from "@luuna/api-client";
import { retryIdentity } from "../ProjectComponents";
import { Alert, Button, EmptyState, ListGroup, Section } from "@luuna/ui";
import { advisoryTasksForTicket, rowsFromProposal, setupTicketDraft, ticketTextSizes, tooLong, triageFields } from "./model";
import { DEFAULT_RUN_SETTINGS, emptyTicketForm, usePatchState } from "./settings";
import { useWorkflowData } from "./useWorkflowData";
import { useWorkflowActions } from "./useWorkflowActions";
import { PrerequisiteOffers, TicketForm } from "./TicketForm";
import { RunSettings } from "./RunSettings";
import { TicketCard } from "./TicketCard";
import type { TicketActions } from "./TicketCard";
import { TicketList, shownTicket } from "./TicketList";

interface Props {
  client: LuunaClient | null;
  projectId: string;
  repositoryId: string;
  /** Every registered repository; the task-graph editor and ticket scope choose among them. */
  repositoryIds?: string[];
  /** The runtime's testing-setup offers from the project view (ADR 0022), one per repository without a usable testing setup. */
  prerequisites?: TestingPrerequisite[];
  manifestConfirmed: boolean;
  /** Incremented when the runtime answers again after an outage. */
  connectionEpoch?: number;
  componentContext?: ComponentContext;
  componentsEnabled?: boolean;
  componentEpoch?: number;
  /** A ticket another view asked to open (Home, the Board, the command palette). */
  focusTicket?: { ticketId?: string; nonce: number } | null;
  onChanged: () => void;
}

const SCOPE_TAIL = "You approve the exact plan; Luuna implements it in its own worktree, then checks and reviews the result.";

/**
 * A source-revisioned ticket, an exact plan approval and the candidate workflow.
 * The runtime owns all execution, validation, review and repair decisions.
 */
export function Workflows({ client, projectId, repositoryId, repositoryIds = [repositoryId], prerequisites = [], manifestConfirmed, connectionEpoch = 0, componentContext, componentsEnabled = false, componentEpoch = 0, focusTicket = null, onChanged }: Props) {
  const data = useWorkflowData(client, projectId, connectionEpoch, componentEpoch);
  const [settings, updateSettings] = usePatchState(DEFAULT_RUN_SETTINGS);
  const [form, updateForm] = usePatchState(emptyTicketForm(repositoryId));
  const ticketDelivery = useRef<{ content: string; key: string } | null>(null);
  const work = useWorkflowActions({
    client, projectId, repositoryId, settings, componentContext, componentsEnabled, reload: data.reload, onChanged,
    onSuggestedOverride: (suggested) => updateSettings((current) => ({
      overrideRows: current.overrideRows.some((row) => row.consumer === suggested.consumer && row.producer === suggested.producer) ? current.overrideRows : [...current.overrideRows, suggested],
    })),
  });

  // One ticket's detail at a time: the one asked for, the one already shown, or the newest.
  const [chosen, setChosen] = useState<string | null>(null);
  const selected = shownTicket(data.tickets.list, chosen);

  useEffect(() => {
    updateSettings({ autoRepair: false });
    ticketDelivery.current = null;
    setChosen(null);
  }, [client, projectId, updateSettings]);

  useEffect(() => {
    if (focusTicket?.ticketId) setChosen(focusTicket.ticketId);
  }, [focusTicket]);

  const scoped = form.repositories.filter((id) => repositoryIds.includes(id));
  const { purpose } = form;
  const ticketRequest = {
    title: form.title.trim(),
    intent: form.intent.trim(),
    acceptance_criteria: form.criteria.split("\n").map((line) => line.trim()).filter(Boolean),
    repository_ids: scoped.length ? scoped : [repositoryId],
    ...(purpose !== "feature" && { purpose }),
    ...triageFields(form.ticketType, form.priority, form.labels),
  };
  const sizes = ticketTextSizes(ticketRequest.title, ticketRequest.intent, ticketRequest.acceptance_criteria);
  const blocked = tooLong(sizes);
  // The offers shown: the project view's, plus the one a refused implementation carried for a repository the view does not name yet.
  const refused = work.refusedOffer;
  const offers = [...prerequisites, ...(refused && !prerequisites.some((offer) => offer.repository_id === refused.repository_id) ? [refused] : [])];

  /** Pre-fills the ticket form from the runtime's offer; the developer reviews it and creates the ticket. */
  function prefillSetupTicket(offer: TestingPrerequisite) {
    const draft = setupTicketDraft(offer);
    updateForm({ purpose: draft.purpose, title: draft.title, intent: draft.intent, criteria: draft.criteria, repositories: draft.repositoryIds });
    if (typeof document !== "undefined") document.getElementById("ticket-title")?.focus();
  }

  function createTicket() {
    if (blocked) return;
    const request = ticketRequest;
    ticketDelivery.current = retryIdentity(ticketDelivery.current, JSON.stringify(request));
    const key = ticketDelivery.current.key;
    work.guarded("Ticket creation", async (signal) => {
      if (!client) return;
      const created = await client.createTicket(projectId, { idempotency_key: key, ...request }, signal);
      if (signal.aborted) return;
      setChosen(created.id);
      updateForm({ title: "", intent: "", criteria: "", purpose: "feature" });
      ticketDelivery.current = null;
    });
  }

  /** The derive convenience (ADR 0023 point 4): the next plan's rows are the proposal's tasks, each deriving from the advisory task. */
  function derive(task: TaskRecord) {
    const rows = rowsFromProposal(task);
    if (!rows.length) return;
    updateSettings({ taskRows: rows, maxWorkers: "1" });
    if (typeof document !== "undefined") document.getElementById("plan-task-graph")?.scrollIntoView?.({ block: "start" });
  }

  const actions: TicketActions = { ...work.actions, derive };
  const runFor = (plan: PlanRevision) => data.runs.find((run) => run.metadata.plan_id === plan.id && run.metadata.plan_revision === plan.revision);
  const shownError = work.error || data.loadError;
  const detail = data.tickets.list.find((ticket) => ticket.id === selected);
  const refresh = <Button variant="text" disabled={!client || work.busy} data-capability="tickets.list tickets.show tickets.source plans.list runs.candidate runs.worktrees" onClick={data.reload}>Refresh workflows</Button>;
  const description = `${componentsEnabled ? "Tickets live in Missions; reconcile outside edits before planning again." : "Tickets are stored locally until project components are enabled."} ${SCOPE_TAIL}`;

  return <Section id="workflows-heading" title="Tickets" variant="subpanel" className="workflows" accessory={refresh} description={description}>
    {shownError && <Alert tone="danger" onDismiss={() => { work.setError(""); data.clearLoadError(); }}>{shownError}</Alert>}
    <PrerequisiteOffers offers={offers} disabled={!client || work.busy || !manifestConfirmed} prefillSetupTicket={prefillSetupTicket} />
    {data.tickets.list.length > 0
      ? <TicketList rows={data.tickets.list.map((ticket) => { const latest = data.tickets.plans[ticket.id]?.at(-1); return { ticket, run: latest && runFor(latest), planRevision: latest?.revision }; })} selected={selected} onSelect={setChosen} />
      : <EmptyState title="No tickets yet">Create the first one below.</EmptyState>}
    {detail && <ListGroup aria-label="Tickets" data-capability="tickets.list">
      <TicketCard key={detail.id} ticket={detail} source={data.tickets.sources[detail.id]} revisions={(data.tickets.plans[detail.id] ?? []).map((plan) => ({ plan, run: runFor(plan) }))}
        snapshots={data.snapshots} diffs={work.inspections.diffs} attempts={work.inspections.attempts} publications={work.inspections.publications}
        tasks={work.inspections.tasks} logs={work.inspections.logs} busy={work.busy} connected={client !== null} actions={actions} repositoryIds={repositoryIds} />
    </ListGroup>}
    <TicketForm form={form} update={updateForm} sizes={sizes} tooLong={blocked} disabled={!client || work.busy || !manifestConfirmed} manifestConfirmed={manifestConfirmed} repositoryIds={repositoryIds} scoped={scoped} onCreate={createTicket} />
    <RunSettings settings={settings} update={updateSettings} repositoryId={repositoryId} repositoryIds={repositoryIds} error={work.error} advisoryTasks={detail ? advisoryTasksForTicket((data.tickets.plans[detail.id] ?? []).map((plan) => plan.id), data.runs, work.inspections.tasks) : []} />
  </Section>;
}
