import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { accessSync, constants, readFileSync, readlinkSync, realpathSync, statSync } from "node:fs";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { TerminalScreen } from "./tui-terminal.mjs";

// Only fixed supervisor code reaches a shell. Client argv and credentials travel
// through the private server's inherited environment, never command flags/files.
export const shellQuote = (value) => {
  if (String(value).includes("\0")) throw new Error("Shell argument contains NUL");
  return `'${String(value).replaceAll("'", "'\\''")}'`;
};

export function resolveExecutable(name, env = process.env, cwd = process.cwd()) {
  const candidates = name.includes("/") || path.isAbsolute(name)
    ? [path.resolve(cwd, name)]
    : (env.PATH ?? "").split(path.delimiter).map((dir) => path.resolve(cwd, dir || ".", name));
  for (const candidate of candidates) {
    try {
      accessSync(candidate, constants.X_OK);
      if (statSync(candidate).isFile()) return realpathSync(candidate);
    } catch { /* next explicit PATH entry */ }
  }
  throw new Error("Required terminal executable is unavailable");
}

const supervisor = String.raw`
const { spawn, execFileSync } = require('node:child_process');
const { writeFileSync, renameSync } = require('node:fs');
const { constants } = require('node:os');
const options = JSON.parse(process.env.LUUNA_TMUX_SUPERVISOR);
delete process.env.LUUNA_TMUX_SUPERVISOR;
if (options.term !== undefined) process.env.TERM = options.term;
const mode = () => [execFileSync(options.stty, ['-g'], { encoding:'utf8', stdio:[0,'pipe','pipe'], timeout:5000 }).trim()];
const report = { before: mode() };
const save = () => { const temporary = options.result + '.' + process.pid + '.tmp'; writeFileSync(temporary, JSON.stringify(report), {mode:0o600}); renameSync(temporary, options.result); };
const child = spawn(options.argv[0], options.argv.slice(1), {cwd:options.cwd, env:process.env, stdio:'inherit'});
child.once('spawn', () => { report.child_pid = child.pid; save(); });
for (const signal of ['SIGINT','SIGTERM','SIGHUP']) process.on(signal, () => { if (child.exitCode === null && child.signalCode === null) child.kill(signal); });
child.once('error', () => { report.exit = 127; report.error = 'Client spawn failed'; try { report.after = mode(); } catch { report.mode_error = 'Terminal mode unavailable'; } save(); process.exit(127); });
child.once('exit', async (code, signal) => {
  report.exit = code ?? 128 + (constants.signals[signal] ?? 0); if (signal) report.signal = signal;
  // tmux hides the cursor of a dead pane. Observe the real restored state
  // while this supervisor still owns the live terminal, after pending VT output.
  await new Promise(resolve => setTimeout(resolve, 50));
  try {
    report.after = mode();
    const state = execFileSync(options.tmux, ['-L', options.socket, '-f', '/dev/null', 'display-message', '-p', '-t', 'audit:0.0', '#{cursor_flag}|#{alternate_on}'], {encoding:'utf8', timeout:5000}).trim().split('|');
    if (state.length !== 2 || state.some(value => !/^[01]$/.test(value))) throw new Error('Invalid terminal metadata');
    report.after_terminal = {cursorVisible:state[0] === '1', alternate:state[1] === '1'};
  } catch { report.mode_error = 'Terminal state unavailable'; }
  save(); process.exit(report.exit);
});
`;

function dimensions(columns, rows) {
  if (!Number.isInteger(columns) || !Number.isInteger(rows) || columns < 1 || columns > 500 || rows < 1 || rows > 200) throw new Error("Invalid terminal dimensions");
}

/** Native Unix PTY adapter. Every instance owns a fresh tmux server/socket. */
export class TmuxTerminal {
  constructor({ env = process.env, tmux = "tmux" } = {}) {
    if (process.platform === "win32") throw new Error("TmuxTerminal requires native Unix execution");
    this.env = { ...env }; delete this.env.TMUX;
    this.env.SHELL = resolveExecutable("sh", this.env);
    this.tmux = resolveExecutable(tmux, this.env);
    this.ps = resolveExecutable("ps", this.env);
    this.stty = resolveExecutable("stty", this.env);
    this.socket = `luuna-native-${process.pid}-${randomBytes(12).toString("hex")}`;
    this.raw = Buffer.alloc(0); this.exit = null; this.opened = false; this.closed = false;
    this.events = []; this.lastAlternate = false; this.owned = new Map();
  }

  command(args, input) {
    try {
      return execFileSync(this.tmux, ["-L", this.socket, "-f", "/dev/null", ...args], {
        env: this.env, encoding: "utf8", input, timeout: 15000, maxBuffer: 8 * 1024 * 1024,
        stdio: ["pipe", "pipe", "pipe"],
      });
    } catch { throw new Error(`tmux ${args[0]} failed or timed out`); }
  }

  async open({ argv, cwd = process.cwd(), columns = 80, rows = 24, result }) {
    if (this.opened || this.closed) throw new Error("Terminal cannot be opened twice");
    dimensions(columns, rows);
    if (!Array.isArray(argv) || !argv.length || argv.some((arg) => typeof arg !== "string" || arg.includes("\0"))) throw new Error("Expected executable argument array");
    if (typeof result !== "string" || !result) throw new Error("A console result path is required");
    this.resultPath = path.resolve(result);
    const executable = resolveExecutable(argv[0], this.env, cwd);
    this.env.LUUNA_TMUX_SUPERVISOR = JSON.stringify({ argv: [executable, ...argv.slice(1)], cwd: path.resolve(cwd), result: this.resultPath, stty: this.stty, tmux: this.tmux, socket: this.socket, term: this.env.TERM });
    const node = shellQuote(process.execPath);
    // Configure retention before launching the subject, even if it exits instantly.
    this.opened = true;
    try {
      this.command(["new-session", "-d", "-s", "audit", "-x", String(columns), "-y", String(rows), "-c", path.resolve(cwd), `exec ${node} -e ${shellQuote("setInterval(() => {}, 1000)")}`]);
      this.command(["set-option", "-t", "audit", "status", "off"]);
      this.command(["set-option", "-w", "-t", "audit", "remain-on-exit", "on"]);
      this.command(["set-option", "-w", "-t", "audit", "remain-on-exit-format", ""]);
      this.command(["respawn-pane", "-k", "-t", "audit:0.0", "-c", path.resolve(cwd), `exec ${node} -e ${shellQuote(supervisor)}`]);
      await this.resize(columns, rows);
      this.pid = Number(this.command(["display-message", "-p", "-t", "audit:0.0", "#{pane_pid}"]).trim());
      return { pid: this.pid };
    } catch (error) { try { await this.close(); } catch { /* preserve launch failure */ } throw error; }
  }

  async send(text) {
    // Input may be a hidden token: feed stdin, not send-keys argv or a disk file.
    const bytes = Buffer.from(text);
    for (let offset = 0; offset < bytes.length; offset += 4096) {
      this.command(["load-buffer", "-b", "luuna-input", "-"], bytes.subarray(offset, offset + 4096));
      this.command(["paste-buffer", "-d", "-r", "-b", "luuna-input", "-t", "audit:0.0"]);
    }
  }

  async resize(columns, rows) {
    dimensions(columns, rows);
    this.command(["resize-window", "-t", "audit:0", "-x", String(columns), "-y", String(rows)]);
    return {};
  }

  async capture() {
    const values = this.command(["display-message", "-p", "-t", "audit:0.0", "#{pane_width}|#{pane_height}|#{cursor_x}|#{cursor_y}|#{alternate_on}|#{cursor_flag}|#{pane_dead}|#{pane_dead_status}"]).trim().split("|");
    const [columns, rows, x, y, alternate, cursorVisible, dead] = values.map(Number);
    const data = this.command(["capture-pane", "-p", "-e", "-N", "-t", "audit:0.0"]);
    this.raw = Buffer.concat([this.raw, Buffer.from(data)]).subarray(-8 * 1024 * 1024);
    // Position each captured row explicitly; a trailing full-width newline must
    // not scroll the reconstructed grid or inherit the previous row's rendition.
    const screen = new TerminalScreen(columns, rows);
    for (const [index, line] of data.split("\n").slice(0, rows).entries()) screen.feed(`\x1b[${index + 1};1H\x1b[0m${line}`);
    this.exit = dead && values[7] !== "" ? Number(values[7]) : null;
    let restored;
    if (dead) {
      try { const report = JSON.parse(readFileSync(this.resultPath, "utf8")); this.exit = report.exit ?? this.exit; restored = report.after_terminal; } catch { /* caller can report missing supervisor evidence */ }
    }
    const observedAlternate = dead ? restored?.alternate !== false : Boolean(alternate);
    if (observedAlternate !== this.lastAlternate) this.events.push(observedAlternate ? "alternate-enter" : "alternate-leave");
    this.lastAlternate = observedAlternate;
    return { ...screen.snapshot(), cursor: [x, y], alternate: observedAlternate, cursorVisible: dead ? restored?.cursorVisible === true : Boolean(cursorVisible), deadPaneCursorVisible: dead ? Boolean(cursorVisible) : undefined, deadPaneAlternate: dead ? Boolean(alternate) : undefined, restoredTerminal: restored, events: [...this.events], exit: this.exit, pid: this.pid };
  }

  processTable() {
    let output;
    try { output = execFileSync(this.ps, ["-axo", "pid=,ppid=,lstart=,comm="], { env: { ...this.env, LC_ALL: "C" }, encoding: "utf8", timeout: 10000, maxBuffer: 8 * 1024 * 1024 }); }
    catch { throw new Error("Owned terminal process enumeration failed"); }
    return output.split("\n").flatMap((line) => {
      const match = /^\s*(\d+)\s+(\d+)\s+(\S+\s+\S+\s+\d+\s+\S+\s+\d+)\s+(.+)$/.exec(line);
      if (!match) return [];
      const pid = Number(match[1]); let executable = match[4].trim();
      if (process.platform === "linux") { try { executable = readlinkSync(`/proc/${pid}/exe`); } catch { executable = ""; } }
      // Never resolve a different PATH candidate and claim it was this process.
      if (!path.isAbsolute(executable)) executable = "";
      return [{ pid, parent: Number(match[2]), started: match[3], name: path.basename(executable || match[4].trim()), executable }];
    });
  }

  async processes() {
    const rows = this.processTable(); const owned = new Set([this.pid]);
    for (let count = -1; count !== owned.size;) {
      count = owned.size;
      for (const row of rows) if (owned.has(row.parent)) owned.add(row.pid);
    }
    const result = rows.filter((row) => owned.has(row.pid));
    for (const row of result) this.owned.set(row.pid, row);
    return result;
  }

  async close() {
    if (this.closed) return;
    this.closed = true;
    if (!this.opened) return;
    try { await this.processes(); } catch { /* previously observed ownership remains usable */ }
    const owned = [...this.owned.values()];
    const matches = (current, row) => current.some((now) => now.pid === row.pid && now.started === row.started && now.executable === row.executable);
    const signalOwned = (signal) => {
      const current = this.processTable();
      for (const row of [...owned].reverse()) {
        if (!matches(current, row)) continue;
        try { process.kill(row.pid, signal); } catch (error) { if (error.code !== "ESRCH") throw error; }
      }
    };
    try {
      if (owned.length) {
        signalOwned("SIGTERM");
        const deadline = Date.now() + 3000;
        while (Date.now() < deadline) {
          const current = this.processTable();
          if (!owned.some((row) => matches(current, row))) break;
          await sleep(100);
        }
        signalOwned("SIGKILL");
      }
    } finally {
      this.command(["kill-server"]);
      delete this.env.LUUNA_TMUX_SUPERVISOR;
    }
  }
}
