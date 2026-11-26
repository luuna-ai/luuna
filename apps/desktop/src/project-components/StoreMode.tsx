import { useEffect, useState } from "react";
import { Button, Disclosure, ErrorBanner, FormFooter, Hint, RadioGroup, StatusLine } from "@luuna/ui";
import type { LuunaClient, StoreModeChoice, StoreModeStatus } from "@luuna/api-client";
import { actionFailure } from "../failures";

const choiceValue = (choice: StoreModeChoice) => choice.mode === "repository" ? `repository:${choice.repository}` : "runtime";
const choiceLabel = (choice: StoreModeChoice) => choice.mode === "repository"
  ? `Repository ${choice.repository}: its own .luuna stores, as you set them up`
  : "Runtime: Luuna's private store in its data folder";

/** Where the project's component data lives (ADR 0062). Luuna never moves or merges records, so a change is refused once tickets or memory exist. */
export function StoreMode({ client, projectId, onChanged }: { client: LuunaClient | null; projectId: string; onChanged: () => void }) {
  const [status, setStatus] = useState<StoreModeStatus | null>(null);
  const [selected, setSelected] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const show = (next: StoreModeStatus) => {
    setStatus(next);
    setSelected(next.mode === "repository" ? `repository:${next.repository}` : next.mode === "runtime" ? "runtime" : "");
  };
  const run = async (label: string, action: (signal: AbortSignal) => Promise<StoreModeStatus>, controller = new AbortController()) => {
    setBusy(true);
    try {
      const next = await action(controller.signal);
      if (!controller.signal.aborted) { show(next); setError(""); }
      return next;
    } catch (failure) {
      if (!controller.signal.aborted) setError(actionFailure(label, failure));
      return null;
    } finally {
      if (!controller.signal.aborted) setBusy(false);
    }
  };
  useEffect(() => {
    if (!client) return;
    const controller = new AbortController();
    void run("Store mode inspection", (signal) => client.storeMode(projectId, signal), controller);
    return () => controller.abort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, projectId]);
  const choice = status?.choices.find((candidate) => choiceValue(candidate) === selected);
  const unchanged = !!status && selected === (status.mode === "repository" ? `repository:${status.repository}` : status.mode ?? "");
  const save = () => void run("Store mode change", (signal) => client!.setStoreMode(projectId, choice!, signal)).then((next) => { if (next) onChanged(); });
  const summary = `Where component data lives${status?.mode ? ` · ${status.mode}${status.repository ? ` ${status.repository}` : ""}` : ""}`;
  return <Disclosure className="store-mode" summary={summary}>
    {error && <ErrorBanner message={error} onDismiss={() => setError("")} />}
    {status && !status.usable && status.problem && <StatusLine>{status.problem}</StatusLine>}
    {status && <RadioGroup legend="Store mode" name={`store-mode-${projectId}`} value={selected}
      options={status.choices.map((candidate) => ({ value: choiceValue(candidate), label: choiceLabel(candidate), disabled: busy }))}
      onChange={(value) => setSelected(value)} />}
    <Hint>A repository's stores must be initialised there first and ignored or committed by you; Luuna never initialises, stages or commits anything in a checkout. A change is refused once this project keeps tickets or memory, or when the chosen side already holds records.</Hint>
    <FormFooter>
      <Button variant="text" disabled={!client || busy} data-capability="projects.storemode.show" onClick={() => void run("Store mode inspection", (signal) => client!.storeMode(projectId, signal))}>Reload</Button>
      <Button variant="primary" disabled={!client || busy || !choice || unchanged} data-capability="projects.storemode.set" onClick={save}>Use this store</Button>
    </FormFooter>
  </Disclosure>;
}
