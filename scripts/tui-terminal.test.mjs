import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { test } from "node:test";
import { ConptyTerminal, TerminalScreen, terminalSnapshotHtml } from "./tui-terminal.mjs";
import { stageTerminal } from "./stage-terminal.mjs";

test("VT captures current cursor-addressed cells, colors and split UTF-8/control sequences", () => {
  const screen = new TerminalScreen(20, 5);
  screen.feed("obsolete\x1b[2J\x1b[H\x1b[32mReady\x1b[0m\x1b[2;");
  screen.feed("3H✓界\x1b[?25l");
  const snapshot = screen.snapshot();
  assert.equal(snapshot.lines[0].trim(), "Ready");
  assert.equal(snapshot.lines[1].trim(), "✓界");
  assert.match(snapshot.colors[0][0], /32/);
  assert.equal(snapshot.cursorVisible, false);
  const bytes = Buffer.from("\x1b[3;1Hé"); screen.feed(bytes.subarray(0, -1)); screen.feed(bytes.subarray(-1));
  assert.equal(screen.snapshot().lines[2].trim(), "é");
  assert.deepEqual(snapshot.unknown, []);
});

test("VT alternate screen restoration and resize discard stale screen contents", () => {
  const screen = new TerminalScreen(10, 3);
  screen.feed("main\x1b[?1049h\x1b[Htemporary");
  assert.equal(screen.snapshot().lines[0].trim(), "temporary");
  screen.resize(8, 4); screen.feed("\x1b[?1049l\x1b[?25h");
  assert.equal(screen.snapshot().lines[0].trim(), "main");
  assert.equal(screen.snapshot().alternate, false);
  assert.deepEqual(screen.snapshot().events, ["alternate-enter", "alternate-leave"]);
});

test("zero RGB/indexed channels preserve existing rendition and reconstructed HTML colors", () => {
  const screen = new TerminalScreen(12, 2);
  screen.feed("\x1b[1;48;2;0;0;170m\x1b[38;2;0;170;0mA\x1b[38;5;0mB\x1b[0mC");
  const result = screen.snapshot();
  assert.equal(result.colors[0][0], "0;1;48;2;0;0;170;38;2;0;170;0");
  assert.equal(result.colors[0][1], "0;1;48;2;0;0;170;38;5;0");
  assert.equal(result.colors[0][2], "0");
  const html = terminalSnapshotHtml(result, "<capture>");
  assert.match(html, /color:rgb\(0,170,0\);background:rgb\(0,0,170\);font-weight:bold/);
  assert.match(html, /color:#000000;background:rgb\(0,0,170\)/);
  assert.ok(html.includes("&lt;capture&gt;"));
});

test("VT erasures, inserted cells and scrolling retain the current background", () => {
  for (const erase of ["\x1b[2J", "\x1b[2K", "\x1b[8X", "\x1b[8P", "\x1b[8@", "\x1b[2L", "\x1b[2M", "\x1b[2S"]) {
    const screen = new TerminalScreen(8, 2);
    screen.feed(`\x1b[48;2;0;0;170m\x1b[2Jtext\x1b[H${erase}`);
    const result = screen.snapshot();
    assert.ok(result.colors.flat().every((sgr) => sgr.includes("48;2;0;0;170")), erase);
    assert.ok(!terminalSnapshotHtml(result).includes("background:#000000;"), erase);
  }
});

test("staging refuses incomplete payload before copying", (t) => {
  const root = mkdtempSync(path.join(tmpdir(), "luuna-stage-test-")); t.after(() => rmSync(root, { recursive: true, force: true }));
  assert.throws(() => stageTerminal({ root, out: path.join(root, "stage"), binary: path.join(root, "missing.exe") }), /prerequisite missing/);
  assert.throws(() => stageTerminal({ root, out: root, binary: path.join(root, "missing.exe") }), /separate output/);
});

test("Windows ConPTY runs real Node terminal input, resize, screen rendering and cleanup", { skip: process.platform !== "win32", timeout: 30000 }, async (t) => {
  const dir = mkdtempSync(path.join(tmpdir(), "luuna-conpty-test-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const program = path.join(dir, "probe.mjs"); const result = path.join(dir, "console-result.json");
  writeFileSync(program, `
    process.stdout.write('main\\r\\n\\x1b[?1049h\\x1b[2J\\x1b[H\\x1b[?25l\\x1b[32mREADY '+process.stdin.isTTY+' '+process.stdout.isTTY);
    process.stdin.setRawMode(true);
    process.stdin.on('data', data => {
      for (const char of data.toString()) {
        if (char === 'x') process.stdout.write('\\x1b[2;1HINPUT x');
        if (char === 'r') process.stdout.write('\\x1b[3;1HSIZE '+process.stdout.getWindowSize().join('x'));
        if (char === 'q') { process.stdin.setRawMode(false); process.stdout.write('\\x1b[0m\\x1b[?25h\\x1b[?1049l'); process.stdin.pause(); process.exitCode=7; }
      }
    });
  `);
  const terminal = new ConptyTerminal();
  try {
    await terminal.open({ argv: [process.execPath, program], cwd: dir, result });
    const wait = async (predicate) => { for (let i = 0; i < 100; i++) { const state = await terminal.capture(); if (predicate(state)) return state; await sleep(50); } assert.fail("Timed out waiting for real terminal state"); };
    const ready = await wait((state) => state.lines.join("\n").includes("READY true true"));
    assert.equal(ready.alternate, true); assert.equal(ready.cursorVisible, false);
    const processes = await terminal.processes();
    assert.ok(processes.some((row) => row.name.toLowerCase() === "node.exe" && row.executable.toLowerCase() === process.execPath.toLowerCase()));
    await terminal.send("x"); await wait((state) => state.lines[1].includes("INPUT x"));
    await terminal.resize(40, 12); await sleep(250); await terminal.send("r");
    const resized = await wait((state) => state.lines.join("\n").includes("SIZE 40x12"));
    assert.equal(resized.columns, 40); assert.equal(resized.rows, 12);
    await terminal.send("q"); const closed = await wait((state) => state.exit !== null);
    assert.equal(closed.exit, 7, `${readFileSync(result, "utf8")} ${terminal.raw.toString().slice(-1000)}`); assert.equal(closed.alternate, false); assert.equal(closed.cursorVisible, true);
    assert.equal(closed.lines[0].trim(), "main"); assert.deepEqual(closed.unknown, []);
    const consoleResult = JSON.parse(readFileSync(result, "utf8"));
    assert.equal(consoleResult.exit, 7);
    assert.ok(consoleResult.after, JSON.stringify(consoleResult));
    assert.equal(consoleResult.after[0], consoleResult.before[0], "input console mode restored");
    assert.equal(consoleResult.after[1], consoleResult.before[1], "output console mode restored");
    assert.throws(() => process.kill(consoleResult.child_pid, 0), { code: "ESRCH" });
  } finally { await terminal.close(); }
});

test("POSIX terminal evidence preserves the actual restored cursor and detects a missing restore", { skip: process.platform === "win32", timeout: 20000 }, async (t) => {
  const { spawnSync } = await import("node:child_process");
  if (spawnSync("tmux", ["-V"], { stdio: "ignore" }).status !== 0) { t.skip("Requires tmux for native POSIX terminal evidence"); return; }
  const { TmuxTerminal } = await import("./tui-tmux.mjs");
  const dir = mkdtempSync(path.join(tmpdir(), "luuna-tmux-cursor-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const [visible, leave] of [[true, true], [false, true], [true, false]]) {
    const terminal = new TmuxTerminal();
    const result = path.join(dir, `${visible}-${leave}.json`);
    const program = `process.stdin.setRawMode(true);process.stdout.write('\\x1b[?1049h\\x1b[?25lREADY');process.stdin.once('data',()=>{process.stdin.setRawMode(false);process.stdout.write('${visible ? "\\x1b[?25h" : ""}${leave ? "\\x1b[?1049l" : ""}');process.exitCode=7;process.stdin.pause();});`;
    try {
      await terminal.open({ argv: [process.execPath, "-e", program], cwd: dir, result });
      const wait = async (predicate) => { for (let i = 0; i < 100; i++) { const state = await terminal.capture(); if (predicate(state)) return state; await sleep(30); } assert.fail("Timed out waiting for POSIX terminal state"); };
      const ready = await wait((state) => state.lines.join("\n").includes("READY") && state.alternate && !state.cursorVisible);
      assert.equal(ready.alternate, true, JSON.stringify(ready)); assert.equal(ready.cursorVisible, false);
      await terminal.send("q"); const closed = await wait((state) => state.exit !== null);
      assert.equal(closed.exit, 7); assert.equal(closed.alternate, !leave); assert.equal(closed.cursorVisible, visible);
      const evidence = JSON.parse(readFileSync(result, "utf8"));
      assert.deepEqual(evidence.after, evidence.before);
    } finally { await terminal.close(); }
  }
});

test("native process evidence permits only the identified POSIX supervisor and rejects Node wrappers or descendants", async () => {
  const { nativeProcessEvidence } = await import("./native-process-evidence.mjs");
  const supervisor = { pid: 10, parent: 1, name: "node", executable: "/tools/node" };
  const native = { pid: 11, parent: 10, name: "luuna-tui", executable: "/app/luuna-tui" };
  const options = { platform: "linux", expectedClient: native.executable, supervisorPid: 10, expectedSupervisor: supervisor.executable };
  assert.equal(nativeProcessEvidence([supervisor, native], options).ok, true);
  assert.equal(nativeProcessEvidence([supervisor, { ...native, parent: 12 }, { pid: 12, parent: 10, name: "node", executable: "/tools/node" }], options).ok, false);
  assert.equal(nativeProcessEvidence([supervisor, native, { pid: 12, parent: 11, name: "node", executable: "/tools/node" }], options).ok, false);
  assert.equal(nativeProcessEvidence([{ ...supervisor, executable: "/other/node" }, native], options).ok, false);
  assert.equal(nativeProcessEvidence([supervisor, { ...native, executable: "/other/luuna-tui" }], options).ok, false);
  assert.equal(nativeProcessEvidence([native], options).ok, false);
  const win = { pid: 11, parent: 1, name: "luuna-tui.exe", executable: "C:\\app\\luuna-tui.exe" };
  const windows = { platform: "win32", expectedClient: win.executable.toUpperCase() };
  assert.equal(nativeProcessEvidence([win], windows).ok, true);
  assert.equal(nativeProcessEvidence([win, { pid: 12, parent: 11, name: "node.exe", executable: "C:\\tools\\node.exe" }], windows).ok, false);
});

test("audit isolation carries explicit private config and build-cache paths without provider credentials", async () => {
  const { toolEnvironment } = await import("./tui-audit-runtime.mjs");
  assert.deepEqual(toolEnvironment({ PATH: "/tools", HOME: "/private/home", CARGO_HOME: "/build/cargo", RUSTUP_HOME: "/build/rustup", LUUNA_CONFIG_DIR: "/private/config", XDG_CONFIG_HOME: "/private/xdg", OPENAI_API_KEY: "synthetic-secret", LUUNA_API_TOKEN: "synthetic-secret", UNREVIEWED_OPTION: "no" }), { PATH: "/tools", HOME: "/private/home", CARGO_HOME: "/build/cargo", RUSTUP_HOME: "/build/rustup", LUUNA_CONFIG_DIR: "/private/config", XDG_CONFIG_HOME: "/private/xdg" });
});
