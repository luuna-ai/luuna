import { useEffect, useRef, useState } from "react";
import type { BaselineEvidence, ComponentContext, ComponentStatus, Discovery, EventEnvelope, LuunaClient, ProjectExport, ProjectRecord, ProjectView } from "@luuna/api-client";
import { isTerminal, mergeEvents, readEventPages } from "@luuna/api-client";
import { COMPLETED, useNotify } from "@luuna/ui";
import { actionFailure, describeFailure } from "../failures";
import { workflowReason } from "../workflow-state";
import { baselineRunFailure, currentProposal, requestTarget } from "./project-model";
import type { ProjectRequest } from "./project-model";

const EVENT_PAGE = 1000;

/**
 * The Projects panel's authoritative state: the project list, the selected project's view, its
 * event record and baselines, and the action guard. The runtime inspects and validates; this
 * only reads what it recorded and submits the developer's requests.
 */
export function useProjects(client: LuunaClient | null, connectionEpoch: number, requested: ProjectRequest | null) {
  const [projects, setProjects] = useState<ProjectRecord[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState<ProjectView | null>(null);
  const [events, setEvents] = useState<EventEnvelope[]>([]);
  const [draft, setDraft] = useState("");
  const [discovery, setDiscovery] = useState<Discovery | null>(null);
  const [baselines, setBaselines] = useState<BaselineEvidence[]>([]);
  // Baseline runs accepted by the runtime and not yet settled, by repository.
  const [pendingBaselineRuns, setPendingBaselineRuns] = useState<Record<string, string>>({});
  const [pollTick, setPollTick] = useState(0);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [componentContext, setComponentContext] = useState<ComponentContext>({ memory_ids: [] });
  const [componentStatus, setComponentStatus] = useState<ComponentStatus | null>(null);
  const [componentEpoch, setComponentEpoch] = useState(0);
  const [baselineLog, setBaselineLog] = useState<{ runId: string; artifactId: string; text: string } | null>(null);
  const [exported, setExported] = useState<ProjectExport | null>(null);
  const [landing, setLanding] = useState(0);
  const notify = useNotify();
  const operation = useRef<AbortController | null>(null);
  // Closures created for an earlier connection (an action's late onChanged) compare against this.
  const currentClient = useRef(client);
  currentClient.current = client;
  const projectEvents = useRef<{ id: string; events: EventEnvelope[] } | null>(null);
  // A request from Home or the Board: select the project, then land on its ticket or form once it renders.
  const pendingRequest = useRef<ProjectRequest | null>(null);

  useEffect(() => () => operation.current?.abort(), []);

  useEffect(() => {
    if (!requested || !client) return;
    pendingRequest.current = requested;
    setSelected((current) => (current === requested.projectId ? current : requested.projectId));
    setView((current) => (current?.project.id === requested.projectId ? current : null));
    // A project that is already shown keeps its view, so the landing below is asked for directly.
    setLanding((value) => value + 1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requested]);

  useEffect(() => {
    const pending = pendingRequest.current;
    if (!pending || !view || view.project.id !== pending.projectId || typeof document === "undefined") return;
    pendingRequest.current = null;
    // Tickets load after the view, so the landing is retried briefly; nothing else depends on it.
    const timers: ReturnType<typeof setTimeout>[] = [];
    const land = (attempt: number) => {
      const target = document.getElementById(requestTarget(pending));
      if (target) {
        target.scrollIntoView?.({ block: "start" });
        if (pending.ticketForm || pending.ticketId) target.focus?.();
      } else if (attempt < 6) timers.push(setTimeout(() => land(attempt + 1), 500));
    };
    land(0);
    return () => timers.forEach(clearTimeout);
  }, [view, landing]);

  /** The project's event record, read after the last sequence this panel holds. */
  async function readProjectEvents(id: string, signal: AbortSignal): Promise<EventEnvelope[]> {
    if (!client) return [];
    const known = projectEvents.current?.id === id ? projectEvents.current.events : [];
    const read = (after: number) => readEventPages((cursor, page) => client.projectEvents(id, cursor, signal, page), after, EVENT_PAGE);
    let merged: EventEnvelope[];
    try {
      merged = mergeEvents(known, await read(known.at(-1)?.sequence ?? 0));
    } catch (failure) {
      if (known.length === 0 || signal.aborted) throw failure;
      // The held record no longer lines up (another runtime, for example): replay it whole.
      merged = mergeEvents([], await read(0));
    }
    if (!signal.aborted) projectEvents.current = { id, events: merged };
    return merged;
  }

  useEffect(() => {
    // A request started against the previous runtime must not repopulate this one.
    operation.current?.abort();
    setBusy(false);
    setProjects([]);
    setSelected(null);
    setView(null);
    setDiscovery(null);
    setDraft("");
    setEvents([]);
    setBaselines([]);
    setBaselineLog(null);
    projectEvents.current = null;
    if (!client) return;
    const controller = new AbortController();
    client.listProjects(controller.signal).then((list) => {
      if (controller.signal.aborted) return;
      setProjects(list);
      setSelected((id) => id ?? list[0]?.id ?? null);
    }).catch((failure) => { if (!controller.signal.aborted) setError(describeFailure(failure)); });
    return () => controller.abort();
  }, [client]);

  useEffect(() => {
    setView(null);
    setEvents([]);
    setDiscovery(null);
    setDraft("");
    setBaselines([]);
    setPendingBaselineRuns({});
    setBaselineLog(null);
    setComponentContext({ memory_ids: [] });
    setComponentStatus(null);
    if (!client || !selected) return;
    const controller = new AbortController();
    Promise.all([client.project(selected, controller.signal), readProjectEvents(selected, controller.signal), client.listBaselines(selected, controller.signal)]).then(([next, replay, recorded]) => {
      if (controller.signal.aborted) return;
      setView(next);
      setEvents(replay);
      setBaselines(recorded);
      const proposal = currentProposal(next);
      if (proposal) {
        setDiscovery(proposal);
        setDraft(JSON.stringify(proposal.proposed_manifest, null, 2));
      }
    }).catch((failure) => { if (!controller.signal.aborted) setError(describeFailure(failure)); });
    return () => controller.abort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, selected]);

  /** Runs one action at a time; resolves true when it succeeded and reported a notice. */
  async function guarded(label: string, action: (signal: AbortSignal) => Promise<void>): Promise<boolean> {
    if (!client || client !== currentClient.current) return false;
    operation.current?.abort();
    const controller = new AbortController();
    operation.current = controller;
    setBusy(true);
    setError("");
    try {
      await action(controller.signal);
      if (controller.signal.aborted) return false;
      if (COMPLETED[label]) notify(COMPLETED[label]);
      return true;
    } catch (failure) {
      if (!controller.signal.aborted) setError(actionFailure(label, failure));
      return false;
    } finally {
      if (!controller.signal.aborted) setBusy(false);
    }
  }

  // The runtime answered again after an outage: reload authoritative state
  // (a restart marks abandoned work interrupted) instead of showing stale panels.
  useEffect(() => {
    if (!client || !selected || connectionEpoch === 0) return;
    void guarded("Reload after reconnection", (signal) => refresh(selected, signal));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectionEpoch]);

  async function refresh(id: string, signal: AbortSignal) {
    if (!client) return;
    const [next, replay, list, recorded] = await Promise.all([client.project(id, signal), readProjectEvents(id, signal), client.listProjects(signal), client.listBaselines(id, signal)]);
    if (signal.aborted) return;
    setProjects(list);
    setView(next);
    setEvents(replay);
    setBaselines(recorded);
    setSelected(id);
  }

  /** After a removal: the project is no longer listed, so select the first one that is. */
  async function afterRemoval(signal: AbortSignal) {
    if (!client) return;
    const list = await client.listProjects(signal);
    if (signal.aborted) return;
    setProjects(list);
    setView(null);
    setSelected(list[0]?.id ?? null);
  }

  // A baseline advances on the runtime: poll its durable record from the moment the
  // run is accepted (its record appears after inspection and probing) until it settles.
  const pendingList = Object.values(pendingBaselineRuns);
  const baselineInFlight = pendingList.length > 0 || baselines.some((baseline) => baseline.status === "running");
  useEffect(() => {
    if (!client || !selected || !baselineInFlight) return;
    const controller = new AbortController();
    const pending = pendingList;
    const timer = setTimeout(() => {
      // A run can settle before any baseline record exists (inspection or identity
      // failure), so each pending run itself is polled too; otherwise its button never returns.
      Promise.all([
        client.listBaselines(selected, controller.signal),
        Promise.all(pending.map((runId) => client.run(runId, controller.signal))),
      ]).then(async ([recorded, runs]) => {
        if (controller.signal.aborted) return;
        const settled: string[] = [];
        for (const run of runs) {
          const recordSettled = recorded.some((baseline) => baseline.run_id === run.id && baseline.status !== "running");
          if (recordSettled || isTerminal(run.state)) {
            if (!recordSettled) {
              // Without a record, the run's terminal event carries the reason.
              const reason = workflowReason(await client.events(run.id, 0, controller.signal));
              if (controller.signal.aborted) return;
              setError(baselineRunFailure(run.state, reason));
            }
            settled.push(run.id);
          }
        }
        if (settled.length) setPendingBaselineRuns((current) => Object.fromEntries(Object.entries(current).filter(([, runId]) => !settled.includes(runId))));
        // Updating an effect dependency aborts this request. Do it only after
        // the terminal reasons have been read, otherwise polling livelocks.
        setBaselines(recorded);
      }).catch(() => {
        // A poll that fails (the runtime restarting, for example) is retried;
        // otherwise the record would stay "running" until a manual refresh.
        if (!controller.signal.aborted) setPollTick((value) => value + 1);
      });
    }, 1500);
    return () => { controller.abort(); clearTimeout(timer); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, selected, baselines, baselineInFlight, pendingBaselineRuns, pollTick]);

  return {
    client, projects, selected, setSelected, view, events, draft, setDraft, discovery, setDiscovery, baselines,
    pendingBaselineRuns, setPendingBaselineRuns, error, setError, busy, componentContext, setComponentContext,
    componentStatus, setComponentStatus, componentEpoch, setComponentEpoch, baselineLog, setBaselineLog,
    exported, setExported, guarded, refresh, afterRemoval, requested,
  };
}

export type ProjectsState = ReturnType<typeof useProjects>;
