import { setTimeout as sleep } from "node:timers/promises";
import { commandsByName, formFields, formTitle } from "./tui-client.mjs";
import { waitUntil } from "./tui-audit-runtime.mjs";

export const keys = { F1: "\x1bOP", F2: "\x1bOQ", F3: "\x1bOR", F4: "\x1bOS", F5: "\x1b[15~", F6: "\x1b[17~", F7: "\x1b[18~", F8: "\x1b[19~", F10: "\x1b[21~", Right: "\x1b[C", Left: "\x1b[D", Down: "\x1b[B", Up: "\x1b[A", Home: "\x1b[H", End: "\x1b[F", PageDown: "\x1b[6~", Escape: "\x1b", Enter: "\r", Tab: "\t" };

export async function auditWorkflows({ terminal, fixture, capture, screenContains, record }) {
  const send = async (key) => {
    if (/^(F\d+|[epnsduiga])$/.test(key)) await waitUntil(async () => !/Runtime \/ working|waiting for the runtime/.test((await terminal.capture()).lines.join("\n")), { timeout: 25000, message: "Native operation remained busy before keyboard input" });
    await terminal.send(keys[key] ?? key); await sleep(120);
  };
  const view = async (key, expected, label) => { await send(key); await screenContains(terminal, expected); return capture(terminal, label); };
  const apiRows = async (route) => (await fixture.api("GET", route)).body;
  const changed = (read, message) => waitUntil(read, { timeout: 90000, delay: 500, message });
  async function fill(name, values, extras = []) {
    await screenContains(terminal, formTitle(name));
    for (const field of formFields(name, extras)) {
      if (Object.hasOwn(values, field)) { await terminal.send("\x15"); await terminal.send(String(values[field]).replaceAll("\n", "\x1b\r")); }
      await terminal.send("\t");
    }
    await capture(terminal, `form-${name}`);
  }
  const submit = () => send("\x13");
  async function missionTicket(title) {
    await send("F4"); await screenContains(terminal, /> Missions ═/); await sleep(300);
    // Walk the actual selected column; Enter opens the selected record, and the
    // API-backed detail identifies it. Every column remains keyboard-reachable.
    for (let column = 0; column < 8; column++) {
      await send("Home");
      for (let row = 0; row < 8; row++) {
        await send("Enter"); const state = await terminal.capture();
        if (state.lines.join("\n").includes("Missions / ticket")) {
          await sleep(250); const detail = await terminal.capture();
          if (detail.lines.join("\n").includes(title)) return;
          await send("Escape"); await sleep(150);
        }
        await send("Down");
      }
      await send("Right");
    }
    throw new Error(`Could not open ticket ${title} through Missions`);
  }

  await screenContains(terminal, /> Home ═/); await capture(terminal, "home-populated");
  await view("F3", /Native Audit/, "projects-populated"); await send("Enter");
  await screenContains(terminal, /> Project ═/); await capture(terminal, "project-detail");
  record("Home and Projects use the real confirmed project", true);

  await view("F6", /Native fixture knowledge/, "atlas-populated"); await send("Enter");
  await screenContains(terminal, /> Memory ═/); await send("s");
  await screenContains(terminal, /uses it \(s removes\)/); await capture(terminal, "atlas-context-selected");
  await view("F7", /native-guidance/, "beacon-populated"); await send("Enter");
  await screenContains(terminal, /Preserve exact approval and candidate identity/); await send("s");
  await screenContains(terminal, /uses it \(s removes\)/); await capture(terminal, "beacon-context-selected");
  record("Atlas and exact Beacon preview can select unsent plan context", true);

  await send("F4"); await screenContains(terminal, /> Missions ═/);
  const columnNames = ["Backlog", "Testing prerequisite", "Discovery / plan", "Approval", "Implementation", "Checks", "Review", "PRs"];
  for (const [index, name] of columnNames.entries()) { await screenContains(terminal, new RegExp(`> ${name.replaceAll("/", "\\/")}`)); await capture(terminal, `missions-column-${index + 1}`); await send("Right"); }
  record("all eight Missions columns are keyboard reachable", true);
  await send("n"); await fill("create-ticket", { title: "UI native ticket", intent: "Created through a real terminal form", criterion: "native form evidence", repository: "app" }); await submit();
  const created = await changed(async () => (await apiRows(`/projects/${fixture.projectId}/tickets`))?.find((ticket) => ticket.title === "UI native ticket"), "UI ticket was not created");
  await screenContains(terminal, /> Missions ═/);
  await missionTicket("UI native ticket"); await capture(terminal, "ticket-detail-created");
  await send("e"); await fill("edit-ticket", { title: "UI native ticket edited" }); await submit();
  await changed(async () => (await apiRows(`/tickets/${created.id}`))?.title === "UI native ticket edited", "UI ticket edit not recorded");
  record("ticket create/edit forms persist through the real runtime", true);

  await send("F6"); await screenContains(terminal, /> Atlas ═/); await send("n");
  await submit(); await screenContains(terminal, /required/); await capture(terminal, "memory-invalid-input");
  await fill("remember", { scope: "process", content: "UI native knowledge", provenance: "ConPTY form evidence" });
  // Printable q stays inside the form; resize while the unsent value exists.
  await terminal.resize(40, 12); await sleep(200); await capture(terminal, "memory-form-narrow"); await terminal.resize(120, 40); await sleep(200);
  await submit();
  const memory = await changed(async () => (await apiRows(`/projects/${fixture.projectId}/memory`))?.find((row) => row.content === "UI native knowledge"), "UI memory not recorded");
  await screenContains(terminal, /UI native knowledge/); await send("Enter"); await screenContains(terminal, /> Memory ═/);
  await send("e"); await fill("correct-memory", { content: "UI native knowledge corrected" }); await submit();
  await changed(async () => (await apiRows(`/projects/${fixture.projectId}/memory`))?.some((row) => row.id === memory.id && row.content === "UI native knowledge corrected"), "Memory correction not recorded");
  await screenContains(terminal, /UI native knowledge corrected/); await send("Enter"); await screenContains(terminal, /> Memory ═/);
  await send("p"); await fill("promote", { "to-scope": "project", "@confirm": "yes" }, ["@confirm"]); await submit();
  const promoted = await changed(async () => (await apiRows(`/projects/${fixture.projectId}/memory`))?.find((row) => row.content === "UI native knowledge corrected" && row.scope === "project"), "Memory promotion not recorded");
  await screenContains(terminal, /UI native knowledge corrected/); await send("Enter"); await screenContains(terminal, /> Memory ═/);
  await send("d"); await fill("forget", { "@confirm": "yes" }, ["@confirm"]); await submit();
  await changed(async () => !(await apiRows(`/projects/${fixture.projectId}/memory`))?.some((row) => row.id === promoted.id), "Exact-hash memory deletion not recorded");
  record("Atlas invalid/create/correct/promote/delete forms enforce runtime identity", true);
  await send("F6"); await screenContains(terminal, /User memory: excluded/); await send("u"); await screenContains(terminal, /User memory: included/); await capture(terminal, "atlas-explicit-user-memory"); await send("u");
  record("user-memory inclusion is an explicit UI choice", true);

  await send("F7"); await screenContains(terminal, /> Beacon ═/); await send("n");
  await fill("save-prompt", { name: "ui-native-prompt", "@document": "Keep exact identities.\nq remains prompt text." }, ["@document"]); await submit();
  const prompt = await changed(async () => (await apiRows(`/projects/${fixture.projectId}/prompts`))?.find((row) => row.name === "ui-native-prompt"), "UI prompt not saved");
  await screenContains(terminal, /ui-native-prompt/); await send("Enter"); await screenContains(terminal, /> Prompt ═/);
  await send("e"); await fill("save-prompt", { "@document": "Updated exact prompt body.\nSecond line." }, ["@document"]); await submit();
  const updated = await changed(async () => (await apiRows(`/projects/${fixture.projectId}/prompts`))?.find((row) => row.name === prompt.name && row.version > prompt.version), "Versioned prompt update not recorded");
  await screenContains(terminal, /ui-native-prompt/); await send("Enter"); await screenContains(terminal, /Updated exact prompt body/); await capture(terminal, "beacon-updated-preview");
  record("Beacon multiline create/update/exact preview use recorded versions", updated.version === prompt.version + 1);

  await missionTicket("UI native ticket edited"); await send("p"); await fill("plan", { repository: "app", path: "src" }); await submit();
  const plans = await changed(async () => { const values = await apiRows(`/tickets/${created.id}/plans`); return values?.length && values; }, "UI plan not recorded");
  const plan = plans.at(-1); await screenContains(terminal, /> Plan ═/); await capture(terminal, "plan-full-context");
  const serialized = JSON.stringify(plan);
  record("explicit plan records selected memory and exact prompt context", serialized.includes(fixture.memory.id) && serialized.includes("native-guidance"));
  await send("a"); await fill("approve", {}); await submit(); await screenContains(terminal, /Type approve \(or 8\+ hash digits\) to approve plan/); await capture(terminal, "approval-blank-hash");
  await send("Enter"); await screenContains(terminal, /does not match|complete|hash/i);
  const run = (await apiRows("/runs")).find((row) => row.metadata?.plan_id === plan.id);
  record("empty hash does not approve the plan", (await apiRows(`/runs/${run.id}`))?.state === "plan_ready");
  await send("Escape"); await send("Escape");
  await send("F2"); await send("/"); await send("approve"); await send("Enter"); await send("Enter");
  await fill("approve", { plan: plan.id }); await submit(); await screenContains(terminal, /Type approve \(or 8\+ hash digits\) to approve plan/);
  await terminal.send(`sha256:${"0".repeat(64)}`); await send("Enter"); await screenContains(terminal, /acknowledged hash does not match/);
  await capture(terminal, "approval-wrong-full-hash-refused");
  record("wrong nonempty full hash does not approve the plan", (await apiRows(`/runs/${run.id}`))?.state === "plan_ready");
  await send("Escape"); await send("F2"); await send("/"); await send("approve"); await send("Enter"); await send("Enter");
  await fill("approve", { plan: plan.id }); await submit(); await screenContains(terminal, /Type approve \(or 8\+ hash digits\) to approve plan/);
  // ADR 0042: the typed word stands for the complete hash displayed above it; the client sends that hash.
  await terminal.send("approve"); await send("Enter");
  await changed(async () => (await apiRows(`/runs/${run.id}`))?.state === "approved", "Typed approval did not persist");
  await capture(terminal, "approval-typed-complete");
  record("typed approval after the displayed plan persists against the same immutable plan", true);
  // Return through the plan route rather than assuming a cancelled prompt's mode.
  await send("F8"); await screenContains(terminal, /> Runs ═/); await capture(terminal, "runs-list");
  await send("Enter"); await screenContains(terminal, /> Run ═/);
  for (const [index, name] of ["Overview", "Candidate", "Diff", "Tasks", "Worktrees", "Attempts", "Events", "Publication"].entries()) {
    await screenContains(terminal, new RegExp(`${index + 1} / 8: ${name}`));
    await waitUntil(async () => !/waiting for the runtime/.test((await terminal.capture()).lines.join("\n")), { timeout: 60000 });
    await capture(terminal, `run-evidence-${index + 1}`); await send("Right");
  }
  record("Runs exposes all eight real evidence tabs", true);
  await view("F1", /> Help ═/, "help");
  await view("F2", /Actions \/|Commands/, "commands-catalog");
  await send("/"); await send("doctor"); await send("Enter"); await capture(terminal, "commands-doctor-search"); await send("Enter");
  await screenContains(terminal, /doctor|Diagnostics/i); await capture(terminal, "settings-doctor");
  record("Help, searchable command catalog and doctor settings remain reachable", true);
  await send("Escape");
  const searched = [];
  for (const name of commandsByName.keys()) {
    await send("F2"); await send("/"); await send(name); await send("Enter");
    const state = await screenContains(terminal, /Actions \/ all contexts/);
    if (!state.lines.slice(3, -2).some((line) => line.slice(1, -1).trim().replace(/^▸\s*/, "").split(/\s+/)[0] === name)) throw new Error(`Catalog action ${name} is not visible after native search`);
    searched.push(name); await capture(terminal, `catalog-search-${name}`);
  }
  record("all catalog actions and projections are searchable", searched.length === commandsByName.size, `${searched.length} commands: ${searched.join(", ")}`);
  await send("F1");
}
