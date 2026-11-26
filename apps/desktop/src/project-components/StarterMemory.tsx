import { useState } from "react";
import type { StarterFact } from "@luuna/api-client";
import { Button, Hint, Inline, ListGroup, ListRow, Meta, Stack } from "@luuna/ui";
import type { ComponentsState } from "./useComponents";

/**
 * The starter set for a project without memory (ADR 0059): facts proposed from the confirmed
 * manifest and the files present in each checkout. Nothing is kept until the developer keeps it,
 * one fact at a time; a kept fact is an ordinary project memory with the fact's provenance.
 */
export function StarterMemory({ state, onReview }: { state: ComponentsState; onReview?: (reviewing: boolean) => void }) {
  const { client, projectId, busy, guarded } = state;
  const [facts, setFacts] = useState<StarterFact[] | null>(null);
  const [skipped, setSkipped] = useState<string[]>([]);
  const shown = (facts ?? []).filter((fact) => !fact.recorded && !skipped.includes(fact.key));
  const load = () => void guarded("Starter memory", async (signal) => {
    const set = await client!.starterMemory(projectId, signal);
    if (!signal.aborted) { setFacts(set.facts); setSkipped([]); onReview?.(true); }
  });
  if (facts === null) {
    return <Inline className="starter-memory" gap={2} wrap>
      <Button disabled={busy || !client} data-capability="memory.starter" onClick={load}>Build a starter set</Button>
      <Hint>Luuna proposes facts from the confirmed manifest and the files in each checkout. You keep or skip each one; nothing is saved until you keep it, and no agent runs.</Hint>
    </Inline>;
  }
  const finish = <Button onClick={() => { setFacts(null); onReview?.(false); }}>Done reviewing</Button>;
  if (!shown.length) return <Inline className="starter-memory" gap={2} wrap><Hint>No starter facts left to review.</Hint>{finish}</Inline>;
  return <Stack gap={2}><ListGroup className="starter-memory" aria-label="Starter memory to review">{shown.map((fact) => <ListRow key={fact.key}
    title={fact.content}
    detail={<Stack gap={1}>
      <Meta>{fact.provenance}</Meta>
      <Inline gap={2}>
        <Button variant="primary" disabled={busy} data-capability="memory.create" onClick={() => void guarded("Memory creation", async (signal) => {
          await client!.keepStarterFact(projectId, fact, signal);
          if (!signal.aborted) setFacts((current) => (current ?? []).map((item) => item.key === fact.key ? { ...item, recorded: true } : item));
        })}>Keep</Button>
        <Button disabled={busy} onClick={() => setSkipped((keys) => [...keys, fact.key])}>Skip</Button>
      </Inline>
    </Stack>} />)}</ListGroup>{finish}</Stack>;
}
