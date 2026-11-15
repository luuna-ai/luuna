// Disposable synthetic-runtime fixture for native TUI audits. No live user state.
import { execFile, execFileSync, spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import path from "node:path";
import { clientBinary } from "./tui-client.mjs";
import { setTimeout as sleep } from "node:timers/promises";

export const waitUntil = async (read, { timeout = 15000, delay = 100, message = "Condition timed out" } = {}) => {
  const until = Date.now() + timeout;
  while (Date.now() < until) { const value = await read(); if (value) return value; await sleep(delay); }
  throw new Error(message);
};

export function toolEnvironment(env = process.env) {
  const names = new Set(["PATH", "Path", "HOME", "USERPROFILE", "LANG", "LC_ALL", "TMPDIR", "TEMP", "TMP", "SystemRoot", "SYSTEMROOT", "PATHEXT", "COMSPEC", "PYTHONIOENCODING", "CARGO_TARGET_DIR", "CARGO_HOME", "RUSTUP_HOME", "LUUNA_CONFIG_DIR", "XDG_CONFIG_HOME"]);
  return Object.fromEntries(Object.entries(env).filter(([name]) => names.has(name)));
}

export class AuditRuntime {
  constructor({ root, clientRoot = root, runtime, out, componentsRoot = root, bun = process.env.LUUNA_BUN }) {
    Object.assign(this, { root, clientRoot, binary: path.resolve(runtime), out, componentsRoot, bun });
    this.token = randomBytes(32).toString("hex"); this.child = null; this.exited = null;
    this.python = process.platform === "win32" ? "python" : "python3";
    this.projectId = "native-audit";
    // The Rust terminal client (crates/luuna-tui): one executable for commands and the workbench.
    this.entry = clientBinary(clientRoot);
    mkdirSync(out, { recursive: true });
  }
  async start() {
    if (!this.port) this.port = await new Promise((resolve, reject) => { const server = createServer(); server.on("error", reject); server.listen(0, "127.0.0.1", () => { const port = server.address().port; server.close(() => resolve(port)); }); });
    this.base = `http://127.0.0.1:${this.port}`;
    const env = { ...toolEnvironment(), LUUNA_API_TOKEN: this.token, LUUNA_DATA_DIR: path.join(this.out, "runtime-data"), LUUNA_PORT: String(this.port), LUUNA_COMPONENTS_ROOT: this.componentsRoot,
      LUUNA_PI_COMMAND: this.python, LUUNA_PI_ARGS_JSON: JSON.stringify([path.join(this.root, "fixtures/pi-rpc/mock_pi.py")]), LUUNA_PI_PROVIDER: "test", LUUNA_PI_MODEL: "synthetic", LUUNA_RUN_TIMEOUT_SECONDS: "600", ...(this.bun ? { LUUNA_BUN: this.bun } : {}) };
    this.child = spawn(this.binary, [], { cwd: this.root, env, windowsHide: true, stdio: ["ignore", "ignore", "pipe"] });
    this.child.stderr.on("data", (chunk) => writeFileSync(path.join(this.out, "runtime.log"), String(chunk).replaceAll(this.token, "[redacted]"), { flag: "a" }));
    this.exited = new Promise((resolve) => { this.child.once("exit", (code, signal) => resolve({ code, signal })); this.child.once("error", () => resolve({ code: -1 })); });
    await waitUntil(() => fetch(`${this.base}/health`, { signal: AbortSignal.timeout(1000) }).then((response) => response.ok).catch(() => false), { timeout: 30000, message: "Disposable runtime did not become healthy" });
  }
  // The workbench reads its defaults, never the developer's own settings file (ADR 0040).
  environment(extra = {}) { return { ...toolEnvironment(), LUUNA_URL: this.base, LUUNA_API_TOKEN: this.token, LUUNA_TUI_CONFIG: path.join(this.out, "tui-settings-absent.json"), ...extra }; }
  async api(method, route, body) {
    const response = await fetch(`${this.base}/api/v1${route}`, { method, signal: AbortSignal.timeout(60000), headers: { Authorization: `Bearer ${this.token}`, ...(body === undefined ? {} : { "Content-Type": "application/json" }) }, body: body === undefined ? undefined : JSON.stringify(body) });
    return { status: response.status, body: await response.json().catch(() => null) };
  }
  command(args, { requireSuccess = true } = {}) {
    return new Promise((resolve, reject) => {
      execFile(this.entry, [...args, "--json"], { cwd: this.out, env: this.environment(), timeout: 60000, windowsHide: true, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 }, (error, stdout, stderr) => {
        const code = error ? typeof error.code === "number" ? error.code : 1 : 0;
        let value; try { value = JSON.parse(stdout.trim().split("\n").at(-1)); } catch { value = null; }
        const result = { code, value, stdout, stderr: stderr.replaceAll(this.token, "[redacted]") };
        if (requireSuccess && code !== 0) reject(new Error(`${args[0]} failed: ${result.stderr}`)); else resolve(result);
      });
    });
  }
  createRepository(id = this.projectId) {
    const directory = path.join(this.out, "fixture", id); mkdirSync(path.join(directory, "src"), { recursive: true });
    const files = { ".gitignore": "__pycache__/\n", "README.md": "# Synthetic native terminal audit\n", "pyproject.toml": `[project]\nname = "${id}"\n`, "src/main.py": "print('fixture')\n", "test_main.py": "import unittest\nclass TestFixture(unittest.TestCase):\n    def test_fixture(self): self.assertTrue(True)\n" };
    for (const [file, value] of Object.entries(files)) writeFileSync(path.join(directory, file), value);
    const noHooks = path.join(this.out, "no-hooks"); mkdirSync(noHooks, { recursive: true });
    const env = { ...toolEnvironment(), GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: process.platform === "win32" ? "NUL" : "/dev/null", GIT_TEMPLATE_DIR: noHooks, GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" };
    const git = (...args) => execFileSync("git", ["-C", directory, "-c", `core.hooksPath=${noHooks}`, "-c", "core.fsmonitor=false", "-c", "commit.gpgsign=false", ...args], { env, encoding: "utf8", windowsHide: true, timeout: 30000 }).trim();
    git("init", "-q", "-b", "main"); git("remote", "add", "origin", `https://example.invalid/${id}.git`); git("add", "-A"); git("commit", "-q", "-m", "synthetic fixture");
    return { directory, head: git("rev-parse", "HEAD") };
  }
  async seed() {
    this.repository = this.createRepository();
    await this.command(["register", "Native Audit", "app", this.repository.directory, "--id", this.projectId]);
    await this.command(["discover", this.projectId]);
    const manifest = { schema_version: "0.1", id: this.projectId, name: "Native Audit", revision: 1, repositories: [{ id: "app", remote: `https://example.invalid/${this.projectId}.git`, default_branch: "main", profiles: ["python"], execution: "native", checks: [{ id: "unit", executable: this.python, args: ["-m", "unittest", "-v"], required: true, timeout_seconds: 120, capabilities: ["python"], cwd: "." }] }], relationships: [], policy: { approval: "plan", testing_prerequisite: true, publish: "automatic_pr", merge: "human", max_explorers: 1, max_implementers: 1, max_task_repairs: 2 } };
    this.manifestPath = path.join(this.out, "manifest.json"); writeFileSync(this.manifestPath, JSON.stringify(manifest));
    await this.command(["confirm", this.projectId, this.manifestPath]);
    const baseline = (await this.command(["baseline", this.projectId, "app"])).value;
    await waitUntil(async () => {
      const rows = (await this.api("GET", `/projects/${this.projectId}/baselines`)).body;
      const record = rows?.find((item) => item.run_id === baseline.id);
      if (record?.status && record.status !== "running" && record.status !== "passed") throw new Error(`Fixture baseline failed: ${record.status}`);
      return record?.status === "passed";
    }, { timeout: 120000, delay: 300, message: "Fixture baseline did not pass" });
    const components = (await this.command(["enable-components", this.projectId])).value;
    if (!components.enabled) throw new Error("Component setup did not enable all runtime components; native audit cannot skip their UI");
    this.ticket = (await this.command(["create-ticket", this.projectId, "--title", "Native fixture ticket", "--intent", "Exercise native terminal views", "--criterion", "fixture:write_in_scope"])).value;
    this.memory = (await this.command(["remember", this.projectId, "project", "Native fixture knowledge", "Synthetic ConPTY audit"])).value;
    this.promptPath = path.join(this.out, "prompt.md"); writeFileSync(this.promptPath, "Preserve exact approval and candidate identity.\n");
    this.prompt = (await this.command(["save-prompt", this.projectId, "native-guidance", this.promptPath])).value;
    const planned = (await this.command(["plan", this.ticket.id, "--repository", "app", "--path", "src"])).value;
    this.plan = planned.plan; this.run = planned.run;
    return this.identities();
  }
  identities() { return { project: this.projectId, ticket: this.ticket?.id, memory: this.memory?.id, prompt: this.prompt?.name, plan: this.plan?.id, run: this.run?.id, repository_head: this.repository?.head }; }
  binaryIdentity() { return { path: this.binary, sha256: createHash("sha256").update(readFileSync(this.binary)).digest("hex") }; }
  async kill() { if (!this.child) return; this.child.kill("SIGKILL"); await this.exited; this.child = null; }
  async stop() {
    if (!this.child) return;
    try { await this.api("POST", "/shutdown"); } catch { /* bounded fallback only for this owned process */ }
    const exited = await Promise.race([this.exited, sleep(10000).then(() => null)]);
    if (!exited) await this.kill(); else this.child = null;
  }
}
