import { setTimeout as sleep } from "node:timers/promises";
import { writeFileSync } from "node:fs";
import path from "node:path";
import { commandsByName, formFields, formTitle } from "./tui-client.mjs";
import { keys } from "./tui-audit-workflows.mjs";
import { waitUntil } from "./tui-audit-runtime.mjs";

// Race the real UI's recorded identity against a second client. The records are
// dedicated audit fixtures; neither the selected plan nor its context is edited.
export async function auditRefusals({ terminal, fixture, capture, screenContains, record }) {
  const ready = () => waitUntil(async () => !/Runtime \/ working|waiting for the runtime/.test((await terminal.capture()).lines.join("\n")), { timeout: 60000, message: "Refusal audit client stayed busy" });
  async function form(name, values) {
    await ready();
    await terminal.send(keys.F2); await sleep(100);
    await terminal.send("/"); await terminal.send(name); await terminal.send(keys.Enter); await sleep(100);
    await terminal.send(keys.Enter);
    await screenContains(terminal, formTitle(name));
    for (const field of formFields(name)) {
      if (Object.hasOwn(values, field)) { await terminal.send("\x15"); await terminal.send(String(values[field]).replaceAll("\n", "\x1b\r")); }
      await terminal.send(keys.Tab);
    }
  }
  async function refused(label) {
    await terminal.send("\x13");
    await screenContains(terminal, /refused|conflict/i, 60000);
    await ready(); await capture(terminal, label);
    await terminal.send(keys.Escape); await sleep(100);
  }

  const ticket = (await fixture.command(["create-ticket", fixture.projectId, "--title", "Stale revision fixture", "--intent", "Verify a concurrent edit cannot be overwritten", "--criterion", "preserve the newer revision"])).value;
  await form("edit-ticket", { ticket: ticket.id, revision: ticket.revision, title: "Rejected stale UI title", intent: "", criterion: "", repository: "" });
  const winner = (await fixture.command(["edit-ticket", ticket.id, "--revision", String(ticket.revision), "--title", "Concurrent ticket winner"])).value;
  await refused("stale-ticket-revision-refused");
  const currentTicket = (await fixture.api("GET", `/tickets/${ticket.id}`)).body;
  const ticketSafe = currentTicket?.title === "Concurrent ticket winner" && currentTicket.revision === winner.revision;
  record("stale ticket revision cannot overwrite a concurrent edit", ticketSafe);

  const memory = (await fixture.command(["remember", fixture.projectId, "project", "Stale hash fixture", "Synthetic concurrent-edit audit"])).value;
  await form("correct-memory", { project: fixture.projectId, "memory-id": memory.id, hash: memory.hash, content: "Rejected stale UI content" });
  const changedMemory = (await fixture.command(["correct-memory", fixture.projectId, memory.id, memory.hash, "Concurrent memory winner"])).value;
  await refused("stale-memory-hash-refused");
  const memories = (await fixture.api("GET", `/projects/${fixture.projectId}/memory`)).body;
  const currentMemory = memories?.find((row) => row.id === memory.id);
  const memorySafe = currentMemory?.content === "Concurrent memory winner" && currentMemory.hash === changedMemory.hash;
  record("stale memory hash cannot overwrite a concurrent correction", memorySafe);
  record("stale revision and hash refusals remain visible", ticketSafe && memorySafe);

  const promptFile = path.join(fixture.out, "concurrent-prompt.md");
  writeFileSync(promptFile, "Original concurrent prompt\n");
  const prompt = (await fixture.command(["save-prompt", fixture.projectId, "stale-version-fixture", promptFile])).value;
  const unicode = "café 界 q remains text; keep the newer version.\n".repeat(24) + "UTF8-END café 界";
  await form("save-prompt", { project: fixture.projectId, name: prompt.name, body: "-", "expected-version": prompt.version });
  writeFileSync(promptFile, "Concurrent prompt winner\n");
  const promptWinner = (await fixture.command(["save-prompt", fixture.projectId, prompt.name, promptFile, "--expected-version", String(prompt.version)])).value;
  await terminal.send("\x13");
  await screenContains(terminal, /Enter the complete document/);
  await terminal.send(unicode.replaceAll("\n", "\r"));
  await screenContains(terminal, /UTF8-END/, 60000);
  await capture(terminal, "beacon-long-unicode-input");
  record("long Unicode multiline input remains visible in the native form", true);
  await refused("stale-beacon-version-refused");
  const prompts = (await fixture.api("GET", `/projects/${fixture.projectId}/prompts`)).body;
  const currentPrompt = prompts?.find((row) => row.name === prompt.name);
  const preview = currentPrompt && (await fixture.api("GET", `/projects/${fixture.projectId}/prompts/${encodeURIComponent(prompt.name)}/render?version=${currentPrompt.version}`)).body;
  record("stale Beacon version cannot overwrite a concurrent prompt update", currentPrompt?.version === promptWinner.version && preview?.hash === promptWinner.hash && preview.body === promptWinner.body);
  await terminal.send(keys.F1);
}
