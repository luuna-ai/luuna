#!/usr/bin/env node
// Native terminal evidence against a disposable synthetic runtime, never user state.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";
import { AuditRuntime, waitUntil } from "./tui-audit-runtime.mjs";
import { ConptyTerminal, terminalSnapshotHtml } from "./tui-terminal.mjs";
import { TmuxTerminal } from "./tui-tmux.mjs";
import { clientBinary } from "./tui-client.mjs";
import { nativeProcessEvidence } from "./native-process-evidence.mjs";
import { auditWorkflows } from "./tui-audit-workflows.mjs";
import { auditLaunchRoutes, auditShutdownAndPowerShell } from "./tui-audit-launch.mjs";
import { auditRefusals } from "./tui-audit-refusals.mjs";

const options = Object.fromEntries(process.argv.slice(2).flatMap((value, index, all) => value.startsWith("--") ? [[value.slice(2), all[index + 1]?.startsWith("--") || !all[index + 1] ? true : all[index + 1]]] : []));
const driver = process.platform === "win32" ? "conpty" : "tmux";
if (options.terminal && options.terminal !== driver) throw new Error(`This native host requires --terminal ${driver}; requested ${options.terminal}`);
const root = fileURLToPath(new URL("../", import.meta.url));
const clientRoot = path.resolve(options["client-root"] || root);
const out = path.resolve(options.out || path.join(tmpdir(), `ltui-${Date.now()}`));
if (existsSync(out)) throw new Error("Refusing to reuse an audit output directory");
const Terminal = process.platform === "win32" ? ConptyTerminal : TmuxTerminal;
if (!options.runtime) throw new Error("Pass --runtime <built native runtime>");
mkdirSync(path.join(out, "captures"), { recursive: true });
const fixture = new AuditRuntime({ root, clientRoot, runtime: options.runtime, out, componentsRoot: path.resolve(options["components-root"] || root) });
const checks = []; const captures = []; const terminals = [];
const required = [
  "Home and Projects use the real confirmed project", "Atlas and exact Beacon preview can select unsent plan context", "all eight Missions columns are keyboard reachable",
  "ticket create/edit forms persist through the real runtime", "Atlas invalid/create/correct/promote/delete forms enforce runtime identity", "user-memory inclusion is an explicit UI choice",
  "Beacon multiline create/update/exact preview use recorded versions", "explicit plan records selected memory and exact prompt context", "empty hash does not approve the plan",
  "typed approval after the displayed plan persists against the same immutable plan", "Runs exposes all eight real evidence tabs", "Help, searchable command catalog and doctor settings remain reachable",
  "wrong nonempty full hash does not approve the plan", "all catalog actions and projections are searchable",
  "long Unicode multiline input remains visible in the native form", "stale Beacon version cannot overwrite a concurrent prompt update",
  "40x12: visible header/footer and bounded cursor", "60x24: visible header/footer and bounded cursor", "80x24: visible header/footer and bounded cursor", "120x40: visible header/footer and bounded cursor",
  "runtime reconnect reloads the client", "normal client quit leaves the runtime alive", "no-color: native screen retains words without color", "dumb: native screen retains words without color",
  "npm run tui enters native workbench", "staged native launch from unrelated cwd ignores development overrides",
  "missing token prompts without echo", "missing token can be cancelled", "hidden token entry connects through the in-process service",
  "busy Ctrl-C restores the console", "shutdown request restores the console",
  "native event follow resumes replay after runtime reconnect", "PowerShell launcher routes to staged native workbench", "stale revision and hash refusals remain visible"
];
const record = (name, ok, detail = "") => { checks.push({ name, status: ok ? "pass" : "fail", detail }); console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`); };
async function capture(terminal, label) {
  const state = await terminal.capture();
  if (state.lines.join("\n").includes(fixture.token) || terminal.raw.includes(Buffer.from(fixture.token))) throw new Error("Credential appeared in terminal capture; refusing to persist it");
  const name = `${String(captures.length + 1).padStart(3, "0")}-${label.replace(/[^a-z0-9-]/gi, "-")}`;
  writeFileSync(path.join(out, "captures", `${name}.json`), JSON.stringify(state, null, 2));
  writeFileSync(path.join(out, "captures", `${name}.html`), terminalSnapshotHtml(state, `Luuna ${label} — reconstructed terminal capture`));
  captures.push({ name, columns: state.columns, rows: state.rows }); return state;
}
async function screenContains(terminal, pattern, timeout = 15000) {
  return waitUntil(async () => { const state = await terminal.capture(); return pattern.test(state.lines.join("\n")) && state; }, { timeout, message: `Native screen did not show ${pattern}` });
}
async function open(label, { columns = 120, rows = 35, env = {}, argv = [fixture.entry], cwd = out, expected = /\bconnected\b/ } = {}) {
  const terminal = new Terminal({ env: fixture.environment(env), ...(options.tmux ? { tmux: options.tmux } : {}) }); terminals.push(terminal);
  terminal.resultPath = path.join(out, `${label}-console.json`);
  await terminal.open({ argv, cwd, columns, rows, result: terminal.resultPath });
  await screenContains(terminal, expected);
  terminal.ownedProcesses = await terminal.processes();
  return terminal;
}
async function quit(terminal, label, key = "q", exit = 0) {
  if (key !== null) await terminal.send(key);
  const state = await waitUntil(async () => { const snapshot = await terminal.capture(); return snapshot.exit !== null && snapshot; }, { message: "Native terminal did not exit" });
  const consoleState = JSON.parse(readFileSync(terminal.resultPath, "utf8"));
  const expectedModes = terminal.expectedConsoleModes || consoleState.before;
  record(`${label}: native quit exit and terminal restoration`, state.exit === exit && consoleState.exit === exit && consoleState.after?.every((mode, index) => mode === expectedModes[index]) && !state.alternate && state.cursorVisible, JSON.stringify({ exit: state.exit, modes: [consoleState.before, consoleState.after], externalLauncherBaseline: terminal.expectedConsoleModes, alternate: state.alternate, cursorVisible: state.cursorVisible }));
  let childExists = false; try { process.kill(consoleState.child_pid, 0); childExists = true; } catch { /* terminated owned entry */ }
  record(`${label}: public entry process exited`, !childExists);
  const alive = terminal.ownedProcesses.filter((row) => { try { process.kill(row.pid, 0); return true; } catch { return false; } });
  record(`${label}: native and bridge descendants exited`, alive.length === 0, alive.map((row) => row.name).join(", "));
  await terminal.close(); terminals.splice(terminals.indexOf(terminal), 1);
}

let error = null;
try {
  if (options["shutdown-launch-only"]) {
    await fixture.start(); await auditShutdownAndPowerShell({ fixture, clientRoot, out, open, quit, screenContains, capture, record });
  } else {
  await fixture.start(); await fixture.seed(); record("synthetic runtime and all three components seeded", true);
  const terminal = await open("default");
  let screen = await capture(terminal, "default-connected");
  record("default public CLI enters real native alternate screen", screen.alternate && screen.lines.join("\n").includes("LUUNA"));
  const processEvidence = nativeProcessEvidence(terminal.ownedProcesses, { platform: process.platform, expectedClient: clientBinary(clientRoot), supervisorPid: terminal.pid, expectedSupervisor: process.execPath });
  record("public entry runs the resolved native executable without a Node process", processEvidence.ok, processEvidence.reason);
  writeFileSync(path.join(out, "native-process-evidence.json"), JSON.stringify(processEvidence, null, 2));
  writeFileSync(path.join(out, "native-processes.json"), JSON.stringify(terminal.ownedProcesses, null, 2));
  record("VT model understands the native output", screen.unknown.length === 0, screen.unknown.join(", "));
  record("DOS blue background is present in native cells", screen.colors.some((row) => row.some((sgr) => /48;2;0;0;170/.test(sgr))));
  if (!options.foundation) {
    try { await auditWorkflows({ terminal, fixture, capture, screenContains, record }); await auditRefusals({ terminal, fixture, capture, screenContains, record }); }
    catch (failure) { record("component UI workflows", false, failure.message); await capture(terminal, "workflow-failure"); await terminal.send("\x1b"); await sleep(150); await terminal.send("\x1bOP"); }
  }
  for (const [columns, rows] of [[40, 12], [60, 24], [80, 24], [120, 40]]) {
    await terminal.resize(columns, rows); await sleep(300);
    screen = await capture(terminal, `resize-${columns}x${rows}`);
    record(`${columns}x${rows}: visible header/footer and bounded cursor`, screen.lines.some((line) => line.includes("LUUNA")) && screen.lines.at(-1).trim().length > 0 && screen.cursor[0] <= columns && screen.cursor[1] < rows && screen.unknown.length === 0);
  }
  await fixture.kill(); await screenContains(terminal, /offline|reconnecting|disconnected|outage/i, 15000); await capture(terminal, "runtime-outage");
  record("lost runtime is visible", true);
  await fixture.start(); await screenContains(terminal, /\bconnected\b/, 15000); await capture(terminal, "runtime-reconnected"); record("runtime reconnect reloads the client", true);
  await quit(terminal, "default");
  record("normal client quit leaves the runtime alive", (await fixture.api("GET", "/projects")).status === 200);
  for (const [name, env] of [["no-color", { NO_COLOR: "1" }], ["dumb", { TERM: "dumb" }]]) {
    const plain = await open(name, { env, columns: 80, rows: 24 });
    const state = await capture(plain, name);
    record(`${name}: native screen retains words without color`, !state.colors.some((row) => row.some((style) => /(?:^|;)(?:3[0-8]|4[0-8]|9[0-7]|10[0-7])(?:;|$)/.test(style))) && state.lines.join("\n").includes("LUUNA"));
    await quit(plain, name, "\x03");
  }
  await auditLaunchRoutes({ fixture, clientRoot, out, open, quit, screenContains, capture, record });
  }
} catch (failure) { error = String(failure.message).replaceAll(fixture.token, "[redacted]"); record("native audit completed", false, error); for (const terminal of terminals) { try { await capture(terminal, "failure"); } catch { /* preserve original failure */ } } }
finally {
  for (const terminal of terminals) { try { await terminal.close(); } catch (failure) { record("owned terminal cleanup", false, failure.message); } }
  await fixture.stop();
}
const clientPath = clientBinary(clientRoot);
const binary = existsSync(clientPath) ? { path: clientPath, sha256: createHash("sha256").update(readFileSync(clientPath)).digest("hex") } : { path: clientPath, missing: true };
const coverage = required.filter((name) => process.platform === "win32" || name !== "PowerShell launcher routes to staged native workbench").map((name) => checks.find((check) => check.name === name) || { name, status: "not-run" });
const uiComplete = [...checks, ...coverage].every((check) => check.status === "pass") && !options.foundation && !options["shutdown-launch-only"];
const validationGaps = fixture.validationGaps || [];
const report = { ui_complete: uiComplete, validation_gaps: validationGaps, driver, synthetic: true, scope: options.foundation ? "native foundation only; component view acceptance pending" : "native component UI", platform: process.platform, node: process.version, complete: uiComplete && validationGaps.length === 0, checks, coverage, captures, identities: fixture.identities(), runtime: fixture.binaryIdentity(), terminal: binary, error };
writeFileSync(path.join(out, "report.json"), JSON.stringify(report, null, 2));
writeFileSync(path.join(out, "report.md"), `# Native terminal audit\n\nSynthetic runtime; ${report.scope}. Driver: ${driver}.\nUI coverage complete: ${report.ui_complete}. All acceptance complete: ${report.complete}.\n\n${checks.map((check) => `- ${check.status}: ${check.name}${check.detail ? ` — ${check.detail}` : ""}`).join("\n")}\n\n## Required coverage\n\n${coverage.map((check) => `- ${check.status}: ${check.name}`).join("\n")}\n\n## External validation gaps\n\n${validationGaps.length ? validationGaps.map((gap) => `- ${gap.status}: ${gap.name}. ${gap.reason}`).join("\n") : "None."}\n`);
console.log(`Evidence: ${out}`);
process.exitCode = report.complete ? 0 : 1;
