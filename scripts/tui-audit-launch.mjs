import { cpSync, existsSync, mkdirSync, readFileSync, realpathSync, renameSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { createServer } from "node:net";

import path from "node:path";
import { stageTerminal } from "./stage-terminal.mjs";
import { clientBinary, formTitle } from "./tui-client.mjs";
import { waitUntil } from "./tui-audit-runtime.mjs";
const sameExecutable = (left, right) => { try { return realpathSync.native(left).toLowerCase() === realpathSync.native(right).toLowerCase(); } catch { return false; } };

export async function auditLaunchRoutes({ fixture, clientRoot, out, open, quit, screenContains, capture, record }) {
  const npm = [path.join(path.dirname(process.execPath), "node_modules/npm/bin/npm-cli.js"), path.resolve(path.dirname(process.execPath), "../lib/node_modules/npm/bin/npm-cli.js")].find(existsSync);
  if (!npm) throw new Error("Pinned Node installation lacks npm-cli.js for real npm launcher acceptance");
  const npmBaseline = path.join(out, "npm-console-baseline"); mkdirSync(npmBaseline);
  writeFileSync(path.join(npmBaseline, "package.json"), JSON.stringify({ scripts: { probe: "node probe.mjs" } }));
  writeFileSync(path.join(npmBaseline, "probe.mjs"), "console.log('npm console baseline');setTimeout(()=>{},150);");
  const noop = await open("npm-baseline", { argv: [process.execPath, npm, "run", "probe"], cwd: npmBaseline, expected: /npm console baseline/ });
  await waitUntil(async () => (await noop.capture()).exit !== null);
  const baselineModes = JSON.parse(readFileSync(noop.resultPath, "utf8"));
  noop.expectedConsoleModes = baselineModes.after;
  record("npm console-mode baseline measured independently", Boolean(baselineModes.after), JSON.stringify(baselineModes));
  await quit(noop, "npm-baseline", null);
  const rootNpm = await open("npm-root", { argv: [process.execPath, npm, "run", "tui"], cwd: clientRoot });
  rootNpm.expectedConsoleModes = baselineModes.after;
  await capture(rootNpm, "npm-root-connected"); await quit(rootNpm, "npm-root");
  record("npm run tui enters native workbench", true);

  const binary = clientBinary(clientRoot);
  const stage = path.join(out, "staged");
  const staged = stageTerminal({ root: clientRoot, out: stage, binary });
  writeFileSync(path.join(out, "staged-identity.json"), JSON.stringify(staged, null, 2));
  const stagedEnv = { CARGO_TARGET_DIR: path.join(out, "deliberately-missing-development-target") };
  const terminal = await open("staged", { argv: [staged.binary], env: stagedEnv });
  record("staged native launch from unrelated cwd ignores development overrides", terminal.ownedProcesses.some((row) => row.executable.toLowerCase() === staged.binary.toLowerCase()));
  await capture(terminal, "staged-connected"); await quit(terminal, "staged");

  const tokenCancelled = await open("token-cancel", { env: { LUUNA_API_TOKEN: "" }, expected: /Service token \(hidden\)/ });
  await tokenCancelled.send("private-disposable-input");
  const hidden = await capture(tokenCancelled, "missing-token-hidden");
  record("missing token prompts without echo", !hidden.lines.join("\n").includes("private-disposable-input"));
  await quit(tokenCancelled, "token-cancel", "\x1b");
  record("missing token can be cancelled", true);
  const tokenEntered = await open("token-enter", { env: { LUUNA_API_TOKEN: "" }, expected: /Service token \(hidden\)/ });
  await tokenEntered.send(fixture.token + "\r"); await screenContains(tokenEntered, /\bconnected\b/);
  await capture(tokenEntered, "hidden-token-connected"); await quit(tokenEntered, "token-enter");
  record("hidden token entry connects through the in-process service", true);


  const following = await open("event-follow");
  await following.send("\x1b[19~"); await screenContains(following, /> Runs ═/);
  await waitUntil(async () => !(await following.capture()).lines.join("\n").includes("waiting for the runtime"));
  await following.send("\r"); await screenContains(following, /> Run ═/);
  await waitUntil(async () => !(await following.capture()).lines.join("\n").includes("waiting for the runtime"));
  await following.send("f"); await screenContains(following, formTitle("follow")); await following.send("\x15" + fixture.run.id); await following.send("\x13");
  await screenContains(following, /sequence|Runtime \/ working/); await capture(following, "event-follow-live");
  await fixture.kill(); await screenContains(following, /replay resumes after sequence/, 20000); await capture(following, "event-follow-outage");
  await fixture.start();
  await fixture.command(["approve", fixture.plan.id, "--acknowledge-hash", fixture.plan.hash]);
  await screenContains(following, /Reconnected to the runtime/, 20000);
  await screenContains(following, /plan.approved/, 20000);
  const replay = await capture(following, "event-follow-resumed");
  const replayText = replay.lines.slice(3, -3).map((line) => line.slice(1, -1).trimEnd()).join("");
  const ids = [...replayText.matchAll(/"event_id":"([^"]+)"/g)].map((match) => match[1]);
  const authoritative = (await fixture.api("GET", `/runs/${fixture.run.id}/events?after=0`)).body;
  record("native event follow resumes replay after runtime reconnect", ids.length >= 4 && ids.length === new Set(ids).size && authoritative.every((event) => ids.includes(event.event_id)), JSON.stringify({ displayed_event_ids: ids, expected_event_ids: authoritative.map((event) => event.event_id) }));
  await quit(following, "busy-follow", "\x03");
  record("busy Ctrl-C restores the console", true);

  await auditShutdownAndPowerShell({ fixture, clientRoot, out, open, quit, screenContains, capture, record });
}

export async function auditShutdownAndPowerShell({ fixture, clientRoot, out, open, quit, screenContains, capture, record }) {
  // An orderly shutdown request (the marker file the workbench polls; Windows has no
  // graceful SIGTERM) must restore the console like a keyed quit.
  const marker = path.join(out, "shutdown-request");
  const signalled = await open("shutdown-request", { env: { LUUNA_TUI_SHUTDOWN_FILE: marker } });
  await capture(signalled, "before-shutdown-request"); writeFileSync(marker, ""); await quit(signalled, "shutdown-request", null);
  record("shutdown request restores the console", true, "LUUNA_TUI_SHUTDOWN_FILE marker observed by the running workbench.");

  if (process.platform !== "win32") return;
  const stage = path.join(out, "shutdown-staged");
  const staged = stageTerminal({ root: clientRoot, out: stage, binary: clientBinary(clientRoot) });
  // Preserve an existing fixed-port service. A controlled helper may supply
  // only the disposable connection to exercise unchanged launcher routing; the
  // production bootstrap gap remains explicit in the report.
  const portFree = await new Promise((resolve) => { const server = createServer(); server.on("error", () => resolve(false)); server.listen(4242, "127.0.0.1", () => server.close(() => resolve(true))); });
  const originalBinary = fixture.binary; const originalPort = fixture.port;
  mkdirSync(path.join(stage, "scripts"));
  for (const name of ["start-terminal.ps1", "ensure-windows-runtime.ps1", "windows-common.ps1"]) cpSync(path.join(clientRoot, "scripts", name), path.join(stage, "scripts", name));
  mkdirSync(path.join(stage, "bin")); cpSync(path.join(clientRoot, "bin/luuna.cmd"), path.join(stage, "bin/luuna.cmd"));
  const aliasHelp = execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", "& $env:AUDIT_ALIAS help; exit $LASTEXITCODE"], { env: fixture.environment({ AUDIT_ALIAS: path.join(stage, "bin/luuna.cmd") }), windowsHide: true, timeout: 30000, encoding: "utf8" });
  record("Windows luuna.cmd alias preserves credential-free help", /usage|commands/i.test(aliasHelp));
  const profile = path.join(out, "disposable-profile"); mkdirSync(profile);
  const profileEnv = { USERPROFILE: profile, HOME: profile, LOCALAPPDATA: path.join(profile, "local"), APPDATA: path.join(profile, "roaming") };
  if (portFree) {
    await fixture.stop(); cpSync(originalBinary, path.join(stage, "luuna-runtime.exe"));
    fixture.binary = path.join(stage, "luuna-runtime.exe"); fixture.port = 4242; await fixture.start();
    const credentials = path.join(profile, ".luuna", "credentials"); mkdirSync(credentials, { recursive: true });
    const setup = path.join(out, "save-fixture-connection.ps1");
    writeFileSync(setup, `$ErrorActionPreference='Stop'\nImport-Module Microsoft.PowerShell.Security\n$record=@{runtime_pid=[int]$env:AUDIT_PID;runtime_path=$env:AUDIT_BINARY;data_directory=$env:AUDIT_DATA;runtime_url='http://127.0.0.1:4242';encrypted_token=(ConvertFrom-SecureString (ConvertTo-SecureString $env:LUUNA_API_TOKEN -AsPlainText -Force))}\n$record | ConvertTo-Json | Set-Content -LiteralPath $env:AUDIT_RECORD\n`);
    execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", setup], { env: fixture.environment({ ...profileEnv, AUDIT_PID: String(fixture.child.pid), AUDIT_BINARY: fixture.binary, AUDIT_DATA: path.join(out, "runtime-data"), AUDIT_RECORD: path.join(credentials, "connection.json") }), windowsHide: true, timeout: 30000, stdio: "pipe" });
  } else {
    (fixture.validationGaps ??= []).push({ name: "production PowerShell bootstrap warm reuse", status: "blocked", reason: "Existing loopback port 4242 service preserved. Unchanged start-terminal.ps1 is tested with a controlled fixture connection helper; production bootstrap is not claimed." });
    renameSync(path.join(stage, "scripts/ensure-windows-runtime.ps1"), path.join(stage, "scripts/ensure-windows-runtime.production.ps1"));
    writeFileSync(path.join(stage, "scripts/ensure-windows-runtime.ps1"), "param([string]$DataDirectory)\n@{url=$env:LUUNA_URL;token=$env:LUUNA_API_TOKEN}\n");
  }
  const psNoop = await open("powershell-baseline", { argv: ["powershell.exe", "-NoProfile", "-Command", "Write-Output 'powershell console baseline'; Start-Sleep -Milliseconds 150"], env: profileEnv, expected: /powershell console baseline/ });
  await waitUntil(async () => (await psNoop.capture()).exit !== null);
  const psModes = JSON.parse(readFileSync(psNoop.resultPath, "utf8")); psNoop.expectedConsoleModes = psModes.after;
  record("PowerShell console-mode baseline measured independently", Boolean(psModes.after), JSON.stringify(psModes));
  await quit(psNoop, "powershell-baseline", null);
  const powershell = await open("powershell", { argv: ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", path.join(stage, "scripts/start-terminal.ps1")], env: profileEnv });
  powershell.expectedConsoleModes = psModes.after;
  writeFileSync(path.join(out, "powershell-native-processes.json"), JSON.stringify(powershell.ownedProcesses, null, 2));
  await capture(powershell, "powershell-staged-connected"); await quit(powershell, "powershell");
  record("PowerShell launcher routes to staged native workbench", powershell.ownedProcesses.some((row) => sameExecutable(row.executable, staged.binary)) && (await fixture.api("GET", "/projects")).status === 200, portFree ? "Production bootstrap warm reuse against the staged disposable runtime" : "Routing only: controlled connection helper; production bootstrap blocked by preserved existing service");
  if (portFree) { await fixture.stop(); fixture.binary = originalBinary; fixture.port = originalPort; await fixture.start(); }
}
