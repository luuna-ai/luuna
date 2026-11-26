import type { MemoryScope } from "@luuna/api-client";
import { Actions, Button, Checkbox, CodeBlock, Disclosure, DownloadLink, EmptyState, Form, Hint, Inline, Meta, Section, SearchField, SelectField, submitted, TextArea, TextField } from "@luuna/ui";
import { jsonDataUrl } from "../projects/project-model";
import { MemorySelection } from "./MemoryList";
import { MEMORY_SCOPES } from "./model";
import { MemoryRetention } from "./Retention";
import { StarterMemory } from "./StarterMemory";
import type { ComponentsState } from "./useComponents";

/** Atlas memory: search, save, select, correct, promote, delete and export knowledge, and its retention. */
export function AtlasMemory({ state }: { state: ComponentsState }) {
  const { client, projectId, busy, loading, guarded } = state;
  const drop = (id: string) => state.setSelectedMemory((ids) => ids.filter((item) => item !== id));
  return <Section id="memory-heading" level={4} variant="plain" title="Atlas memory">
    <Checkbox className="checkbox" label="Include private user memory" checked={state.includeUser} disabled={busy} onChange={(event) => state.toggleUserMemory(event.target.checked)} />
    <Form onSubmit={submitted(() => state.setAppliedQuery(state.query.trim()))} className="component-search">
      <Inline gap={2} align="end" wrap>
        <SearchField label="Search memory" clearable={false} value={state.query} onChange={state.setQuery} />
        <Button submit disabled={busy || loading} data-capability="memory.list">Search memory</Button>
        {state.appliedQuery && <Button disabled={busy} onClick={() => { state.setQuery(""); state.setAppliedQuery(""); }}>Clear search</Button>}
      </Inline>
    </Form>
    <Form onSubmit={submitted(state.saveMemory)} disabled={busy || !client}>
      <SelectField label="Memory scope" aria-label="Memory scope" value={state.scope} onChange={(event) => state.setScope(event.target.value as MemoryScope)} options={MEMORY_SCOPES} />
      <TextArea label="Memory content" value={state.content} onChange={(event) => state.setContent(event.target.value)} required rows={3} maxLength={16000} />
      <TextField label="Memory provenance" value={state.provenance} onChange={(event) => state.setProvenance(event.target.value)} required maxLength={1000} placeholder="Where this knowledge came from" />
      <Button submit disabled={!state.content.trim() || !state.provenance.trim()} data-capability="memory.create">Save memory</Button>
    </Form>
    <MemorySelection records={state.records} selected={state.selectedMemory} busy={busy || loading}
      onToggle={(record) => state.setSelectedMemory((ids) => ids.includes(record.id) ? ids.filter((id) => id !== record.id) : [...ids, record.id])}
      onDelete={(record) => void guarded("Memory deletion", async (signal) => {
        await client!.deleteMemory(projectId, record.id, record.hash, signal);
        drop(record.id);
      })}
      onPromote={(record, toScope) => void guarded("Memory promotion", async (signal) => {
        await client!.promoteMemory(projectId, record.id, { idempotency_key: `promote-${record.id}-${record.hash.slice(7, 19)}-${toScope}`, expected_hash: record.hash, to_scope: toScope }, signal);
        drop(record.id);
      })}
      onCorrect={(record, content) => void guarded("Memory correction", async (signal) => {
        // The key binds the correction to the displayed hash: a retry after a lost response replays it.
        await client!.updateMemory(projectId, record.id, { idempotency_key: `correct-${record.id}-${record.hash.slice(7, 19)}`, expected_hash: record.hash, content }, signal);
        drop(record.id);
      })} />
    {!state.records.length && <EmptyState>No memory records in this view.</EmptyState>}
    {!state.records.length && !state.appliedQuery && <StarterMemory state={state} />}
    <Actions className="user-memory-export">
      <Button disabled={busy} data-capability="memory.user.export" onClick={() => void guarded("User memory export", async (signal) => {
        const exported = await client!.exportUserMemory(signal);
        if (!signal.aborted) state.setUserExport(exported);
      })}>Export private user memory</Button>
      <Hint>The personal export: your user-scope records with provenance, which the project export never carries. The runtime refuses one that would include a credential value or a path of this computer.</Hint>
    </Actions>
    {state.userExport && <Disclosure open className="user-memory-export-result" summary={`Private user memory export · ${state.userExport.memory.length} record(s) · omits ${state.userExport.omitted.join(", ")}`}>
      <Meta block><DownloadLink href={jsonDataUrl(state.userExport)} file="luuna-user-memory-export.json">Save user memory JSON</DownloadLink></Meta>
      <CodeBlock label="User memory export">{JSON.stringify(state.userExport, null, 2)}</CodeBlock>
    </Disclosure>}
    <MemoryRetention state={state} />
  </Section>;
}
