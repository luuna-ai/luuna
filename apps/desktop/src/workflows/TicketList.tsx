import type { RunRecord, Ticket } from "@luuna/api-client";
import { runTone, ListGroup, ListRow, StatusBadge, copyText, useNotify } from "@luuna/ui";
import { stateText } from "./model";

export interface TicketRow { ticket: Ticket; run?: RunRecord; planRevision?: number }

/**
 * The project's tickets as one selectable row each: title, the latest plan's run state and the
 * ticket revision. Selecting a row shows that ticket's detail below, so the page holds one
 * ticket's plan, candidate and evidence at a time (HIG › Split views: "persistently highlight
 * the current selection"). Each row's context menu opens it or copies its id.
 */
export function TicketList({ rows, selected, onSelect }: { rows: readonly TicketRow[]; selected: string | null; onSelect: (id: string) => void }) {
  const notify = useNotify();
  return <ListGroup aria-label="Ticket list">{rows.map(({ ticket, run, planRevision }) => <ListRow key={ticket.id} onOpen={() => onSelect(ticket.id)} selected={selected === ticket.id}
    buttonLabel={`${ticket.title}, ${run ? stateText(run.state) : "no plan"}`} title={ticket.title}
    meta={<>
      {run ? <StatusBadge tone={runTone[run.state]}>{stateText(run.state)}</StatusBadge> : <StatusBadge>no plan</StatusBadge>}
      <span>ticket r{ticket.revision}</span>
      {planRevision !== undefined && <span>plan r{planRevision}</span>}
      {ticket.purpose === "testing_setup" && <span>testing setup</span>}
      {ticket.priority !== undefined && <span>P{ticket.priority}</span>}
      {ticket.type && <span>{ticket.type}</span>}
      {ticket.labels?.length ? <span>{ticket.labels.join(", ")}</span> : null}
    </>}
    menuLabel={`Actions for ${ticket.title}`} commands={() => [
      { label: "Open ticket", onSelect: () => onSelect(ticket.id) },
      { label: "Copy ticket ID", group: true, onSelect: () => void copyText(ticket.id).then(() => notify("Ticket ID copied")) },
    ]} />)}</ListGroup>;
}

/** Which ticket the detail shows: the one asked for, the one already shown, or the newest. */
export function shownTicket(tickets: readonly Ticket[], current: string | null): string | null {
  if (current && tickets.some((ticket) => ticket.id === current)) return current;
  return tickets.at(-1)?.id ?? null;
}
