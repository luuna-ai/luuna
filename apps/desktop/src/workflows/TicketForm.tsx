import type { TestingPrerequisite, TicketPurpose, TicketType } from "@luuna/api-client";
import { Button, Form, FormFooter, Hint, Id, Inline, ListGroup, ListRow, ReasonNote, SelectField, Stack, StatusBadge, submitted, TextArea, TextField } from "@luuna/ui";
import { TEXT_LIMITS, TICKET_PRIORITIES, TICKET_TYPES } from "./model";
import type { ticketTextSizes } from "./model";
import { ByteCount } from "./PlanReview";
import { RepositoryScope } from "./RepositoryScope";
import type { Patch, TicketFormState } from "./settings";

const PURPOSES = [
  { value: "feature", label: "feature" },
  { value: "testing_setup", label: "testing setup (the prerequisite a repository without a usable testing setup is offered)" },
];

/** The runtime's testing-setup offers (ADR 0022): the reason ruled, the runtime's next step, and the control that pre-fills the ticket. */
export function PrerequisiteOffers({ offers, disabled, prefillSetupTicket }: { offers: TestingPrerequisite[]; disabled: boolean; prefillSetupTicket: (offer: TestingPrerequisite) => void }) {
  if (offers.length === 0) return null;
  return <ListGroup className="prerequisite-offers" aria-label="Testing-setup prerequisites">{offers.map((offer) => <ListRow key={offer.repository_id} className="prerequisite-offer" data-tone="attention"
    leading={<StatusBadge tone="attention">testing setup needed</StatusBadge>}
    title={<strong>Repository <Id>{offer.repository_id}</Id> has no usable testing setup; feature plans for it are refused until the prerequisite is established.</strong>}
    detail={<Stack gap={2}>
      <ReasonNote tone="attention" reason={offer.reason} next={offer.next} />
      <Inline wrap gap={2}>
        <Button disabled={disabled} data-capability="tickets.create" onClick={() => prefillSetupTicket(offer)}>Create the testing-setup ticket for {offer.repository_id}</Button>
        <Hint>Fills in the ticket form below for you to review. Its plan proposes the checks that become the baseline.</Hint>
      </Inline>
    </Stack>} />)}</ListGroup>;
}

interface Props {
  form: TicketFormState;
  update: (patch: Patch<TicketFormState>) => void;
  sizes: ReturnType<typeof ticketTextSizes>;
  tooLong: boolean;
  disabled: boolean;
  manifestConfirmed: boolean;
  repositoryIds: string[];
  scoped: string[];
  onCreate: () => void;
}

/** The hint under the ticket form: what blocks it, or how the ticket is scoped. */
function ticketHint(manifestConfirmed: boolean, tooLong: boolean, multi: boolean): string {
  if (!manifestConfirmed) return "Confirm a manifest before creating tickets.";
  if (tooLong) return "Shorten the field marked too long: the runtime counts UTF-8 bytes, so accented letters, CJK text and emoji take more than one.";
  return multi ? "Tickets are scoped to the registered repositories you tick; a plan's tasks stay within them." : "Tickets are scoped to this project's registered repository.";
}

/** A new ticket: title, intent and acceptance criteria, each counted in UTF-8 bytes as the runtime counts them. */
export function TicketForm({ form, update, sizes, tooLong, disabled, manifestConfirmed, repositoryIds, scoped, onCreate }: Props) {
  const empty = !form.title.trim() || !form.intent.trim() || !form.criteria.trim();
  return <Form onSubmit={submitted(onCreate)} className="ticket-form form-group" id="ticket-form" disabled={disabled}>
    <Stack gap={1}>
      <TextField label="Ticket title" id="ticket-title" value={form.title} onChange={(event) => update({ title: event.target.value })} required aria-describedby="ticket-title-bytes" placeholder="Implement the greeting" />
      <ByteCount id="ticket-title-bytes" bytes={sizes.title} limit={TEXT_LIMITS.title} />
    </Stack>
    <Stack gap={1}>
      <TextArea label="Intent" value={form.intent} onChange={(event) => update({ intent: event.target.value })} required rows={3} aria-describedby="ticket-intent-bytes" placeholder="What should change and why" />
      <ByteCount id="ticket-intent-bytes" bytes={sizes.intent} limit={TEXT_LIMITS.intent} />
    </Stack>
    <Stack gap={1}>
      <TextArea label="Acceptance criteria (one per line)" value={form.criteria} onChange={(event) => update({ criteria: event.target.value })} required rows={2} aria-describedby="ticket-criteria-bytes" />
      <ByteCount id="ticket-criteria-bytes" prefix="Longest criterion: " bytes={sizes.criterion} limit={TEXT_LIMITS.criterion} />
    </Stack>
    <SelectField label="Purpose" aria-label="Ticket purpose" value={form.purpose} onChange={(event) => update({ purpose: event.target.value as TicketPurpose })} options={PURPOSES} />
    <Inline wrap gap={2}>
      <SelectField label="Type" aria-label="Ticket type" value={form.ticketType} onChange={(event) => update({ ticketType: event.target.value as TicketType })} options={TICKET_TYPES} />
      <SelectField label="Priority" aria-label="Ticket priority" value={String(form.priority)} onChange={(event) => update({ priority: Number(event.target.value) })} options={TICKET_PRIORITIES} />
      <TextField label="Labels (comma-separated)" aria-label="Ticket labels" value={form.labels} onChange={(event) => update({ labels: event.target.value })} placeholder="ui, docs" />
    </Inline>
    {repositoryIds.length > 1 && <RepositoryScope choices={repositoryIds} selected={scoped} onChange={(repositories) => update({ repositories })} />}
    <FormFooter hint={ticketHint(manifestConfirmed, tooLong, repositoryIds.length > 1)}>
      <Button variant="primary" submit disabled={empty || tooLong} data-capability="tickets.create">Create ticket</Button>
    </FormFooter>
  </Form>;
}
