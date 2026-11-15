import { useEffect, useRef, useState } from "react";
import { ApiError } from "@luuna/api-client";
import type { LuunaClient, IntentView, IntentProposal, IntentQuestion, IntentNode } from "@luuna/api-client";
import { Button, ErrorBanner, StatusLine, Hint, Inline, ListGroup, ListRow, Section, Stack, TextField } from "@luuna/ui";

/** Accepted intent and pending suggestions. The runtime makes every decision. */
export function IntentPanel({ client, projectId }: { client: LuunaClient | null; projectId: string }) {
  const [view, setView] = useState<IntentView | null>(null);
  const [proposals, setProposals] = useState<IntentProposal[]>([]);
  const [questions, setQuestions] = useState<IntentQuestion[]>([]);
  const [sprouts, setSprouts] = useState<IntentNode[]>([]);
  const [by, setBy] = useState("developer");
  const [reason, setReason] = useState("");
  const [rel, setRel] = useState("");
  const [keep, setKeep] = useState("");
  const [busy, setBusy] = useState(false);
  const [needsInit, setNeedsInit] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState("");
  const operation = useRef<AbortController | null>(null);
  useEffect(() => {
    setView(null); setProposals([]); setQuestions([]); setSprouts([]); setNeedsInit(false); setError(""); setResult(""); setBusy(false);
    return () => operation.current?.abort();
  }, [client, projectId]);
  async function perform(work: (signal: AbortSignal) => Promise<void>) {
    if (!client) return;
    operation.current?.abort(); const controller = new AbortController(); operation.current = controller;
    setBusy(true); setError("");
    try { await work(controller.signal); }
    catch (failure) { if (!controller.signal.aborted) {
      setError(failure instanceof Error ? failure.message : "Intent request failed.");
      setNeedsInit(failure instanceof ApiError && failure.status === 409 && failure.message === "Roots is not set up for this project");
    } }
    finally { if (!controller.signal.aborted) setBusy(false); }
  }
  async function refresh(signal: AbortSignal) {
    const [next, pending, asked, open] = await Promise.all([client!.showIntent(projectId, signal), client!.listIntentProposals(projectId, signal), client!.listIntentQuestions(projectId, signal), client!.listIntentSprouts(projectId, signal)]);
    if (signal.aborted) return;
    setView(next); setProposals(pending); setQuestions(asked); setSprouts(open); setNeedsInit(false);
  }
  const human = { by, ...(reason.trim() ? { reason: reason.trim() } : {}) };
  const disabled = !client || busy;
  return <Section id="intent-heading" level={4} variant="plain" title="Intent (Roots)">
    <Stack gap={3}>
      <Hint>Review accepted ideas and suggestions. Your name is the label recorded for your decisions.</Hint>
      <TextField label="Your name" value={by} onChange={(event) => setBy(event.target.value)} required maxLength={200} />
      <Hint>Use letters, digits, dots, underscores or hyphens. Agent and Roots identities cannot make human decisions.</Hint>
      <Inline gap={2} wrap>
        <Button disabled={disabled} data-capability="intent.show intent.proposals intent.questions intent.sprouts" onClick={() => void perform(refresh)}>Refresh intent</Button>
        {needsInit && <Button disabled={disabled || !by.trim()} data-capability="intent.init" onClick={() => void perform(async (signal) => {
          const next = await client!.initIntent(projectId, { by }, signal); if (signal.aborted) return; setView(next); await refresh(signal);
        })}>Initialize intent</Button>}
      </Inline>
      {error && <ErrorBanner message={error} onDismiss={() => setError("")} />}
      {result && <StatusLine>{result}</StatusLine>}
      {view && <>
        <Hint>{view.settings.project} · tier {view.settings.tier} · {view.nodes.length} ideas and sprouts · {view.edges.length} relationships</Hint>
        <ListGroup aria-label="Accepted intent">{view.nodes.map((node) => <ListRow key={node.id} title={<strong>{node.slug}</strong>} detail={`${node.id} · ${node.kind} · ${node.status} · ${node.author}`} />)}</ListGroup>
        <ListGroup aria-label="Intent relationships">{view.edges.map((edge) => <ListRow key={edge.id} title={`${edge.from} → ${edge.to}`} detail={`${edge.rel} · ${edge.by}`} />)}</ListGroup>
        <TextField label="Decision reason (optional)" value={reason} onChange={(event) => setReason(event.target.value)} />
        <TextField label="Relation for an edge without one (optional)" value={rel} onChange={(event) => setRel(event.target.value)} />
        <TextField label="Idea id to keep for a merge (optional)" value={keep} onChange={(event) => setKeep(event.target.value)} />
        <ListGroup aria-label="Pending intent proposals">{proposals.map((proposal) => <ListRow key={proposal.id}
          title={<strong>{proposal.id} · {proposal.kind} · {proposal.from} {proposal.to ? `→ ${proposal.to}` : ""} {proposal.rel ?? ""}</strong>}
          detail={<Stack gap={2}><span>{proposal.reason} · {proposal.by}</span>{proposal.cites.map((cite, index) => <span key={index}>{cite.node}: {cite.quote}</span>)}<Inline gap={2} wrap>
            <Button disabled={disabled || !by.trim()} data-capability="intent.proposals.accept" onClick={() => void perform(async (signal) => {
              const decision = await client!.acceptIntentProposal(projectId, proposal.id, { ...human, ...(rel.trim() ? { rel: rel.trim() } : {}), ...(keep.trim() ? { keep: keep.trim() } : {}) }, signal);
              if (signal.aborted) return; setResult(`${decision.id}: ${decision.action}`); await refresh(signal);
            })}>Accept proposal</Button>
            <Button disabled={disabled || !by.trim()} data-capability="intent.proposals.reject" onClick={() => void perform(async (signal) => {
              const decision = await client!.rejectIntentProposal(projectId, proposal.id, human, signal); if (signal.aborted) return; setResult(`${decision.id}: ${decision.action}`); await refresh(signal);
            })}>Reject proposal</Button>
          </Inline></Stack>} />)}</ListGroup>
        <ListGroup aria-label="Intent questions">{questions.map((question) => <ListRow key={question.id} title={question.text} detail={`${question.node} · ${question.status} · ${question.by}`} />)}</ListGroup>
        <ListGroup aria-label="Open sprouts">{sprouts.map((sprout) => <ListRow key={sprout.id} title={sprout.slug} detail={<Inline gap={2} wrap><span>{sprout.id} · {sprout.status} · {sprout.author}</span>
          <Button disabled={disabled || !by.trim()} data-capability="intent.sprouts.reject" onClick={() => void perform(async (signal) => {
            const next = await client!.rejectIntentSprout(projectId, sprout.id, human, signal); if (signal.aborted) return; setResult(`${next.id}: ${next.status}`); await refresh(signal);
          })}>Reject sprout</Button>
        </Inline>} />)}</ListGroup>
      </>}
    </Stack>
  </Section>;
}
