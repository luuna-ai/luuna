#!/usr/bin/env node
// Real app -> isolated runtime -> pinned Missions/Atlas/Beacon smoke.
// Pi writer/reviewer sessions use mock_pi.py and are synthetic, never provider evidence.
// Browser mode uses a test proxy; --native <desktop.exe> uses the bundled WebView and production CORS.
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { randomBytes, createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";
import { chromium } from "playwright-core";
import { runComponent } from "./components.mjs";

const repository = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const args = process.argv.slice(2);
const option = (name) => { const i = args.indexOf(`--${name}`); return i < 0 ? undefined : args[i + 1]; };
const nativeBinary = option("native") ? path.resolve(option("native")) : null;
if (nativeBinary && process.platform !== "win32") throw new Error("Native component verification requires Windows WebView2");
const out = option("out") ? path.resolve(option("out")) : mkdtempSync(path.join(tmpdir(), "luuna-m6-ui-"));
if (option("out")) {
  if (existsSync(out)) throw new Error("Refusing to reuse a smoke output directory");
  mkdirSync(out, { recursive: true });
}
const runtimeBinary = path.resolve(option("runtime") ?? path.join(repository, "target/debug", process.platform === "win32" ? "luuna-runtime.exe" : "luuna-runtime"));
if (!existsSync(runtimeBinary)) throw new Error("Build the current runtime with cargo build --locked -p luuna-runtime, or pass --runtime");
const python = process.platform === "win32" ? "python" : "python3";
const token = randomBytes(32).toString("hex");
const checks = [];
let runtime, runtimeExit, runtimeResult, vite, viteExit, browser, browserInfo, page, nativeClient, nativeExit;
let fixtureView = false;

function env() {
  return Object.fromEntries(Object.entries(process.env).filter(([key]) => /^(PATH|SystemRoot|WINDIR|COMSPEC|PATHEXT|TEMP|TMP|TMPDIR|LANG|LC_ALL|CARGO_TARGET_DIR)$/i.test(key)));
}
async function port(requested) {
  const number = requested === undefined ? 0 : Number(requested);
  if (!Number.isInteger(number) || number < 0 || number > 65535) throw new Error("Invalid smoke port");
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(number, "127.0.0.1", () => { const selected = server.address().port; server.close(() => resolve(selected)); });
  });
}
async function waitFor(label, predicate, timeout = 90_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (runtimeResult) throw new Error(`Isolated runtime exited during ${label}: ${JSON.stringify(runtimeResult)}`);
    try { const value = await predicate(); if (value) return value; }
    catch (error) { if (error instanceof Error && error.message.includes("strict mode violation")) throw error; /* allow a settling request to finish */ }
    await sleep(300);
  }
  throw new Error(`Timed out: ${label}`);
}
function record(name, condition) {
  assert.ok(condition, name);
  checks.push({ name, ok: true });
  console.log(`PASS ${name}`);
}
function git(cwd, ...gitArgs) {
  return execFileSync("git", ["-C", cwd, "-c", "core.hooksPath=", "-c", "core.fsmonitor=false", "-c", "commit.gpgsign=false", ...gitArgs], {
    encoding: "utf8", timeout: 30_000, env: { ...env(), GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: process.platform === "win32" ? "NUL" : "/dev/null", GIT_AUTHOR_NAME: "Luuna fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Luuna fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" },
  }).trim();
}
function fixture(name) {
  const directory = path.join(out, name);
  mkdirSync(path.join(directory, "src"), { recursive: true });
  writeFileSync(path.join(directory, ".gitignore"), "__pycache__/\n");
  writeFileSync(path.join(directory, "pyproject.toml"), `[project]\nname = "${name}"\n`);
  writeFileSync(path.join(directory, "src/main.py"), "print('fixture')\n");
  writeFileSync(path.join(directory, "test_main.py"), "import subprocess\nimport sys\nimport unittest\n\nclass TestMain(unittest.TestCase):\n    def test_main(self):\n        result = subprocess.run([sys.executable, 'src/main.py'], capture_output=True, text=True, check=True)\n        self.assertTrue(result.stdout.strip())\n");
  git(directory, "init", "--initial-branch=main", "--template=");
  git(directory, "add", "."); git(directory, "commit", "-m", "Synthetic component workflow fixture");
  return directory;
}
const runtimePort = await port(option("runtime-port"));
let uiPort = await port(option("ui-port"));
while (uiPort === runtimePort) uiPort = await port();
const runtimeUrl = `http://127.0.0.1:${runtimePort}`;
const uiUrl = nativeBinary ? runtimeUrl : `http://127.0.0.1:${uiPort}`;
async function api(method, route, body) {
  const response = await fetch(`${runtimeUrl}/api/v1${route}`, {
    method, headers: { Authorization: `Bearer ${token}`, ...(body === undefined ? {} : { "Content-Type": "application/json" }) },
    body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(30_000), redirect: "error",
  });
  return { status: response.status, body: await response.json().catch(() => null) };
}
async function successful(method, route, body) {
  const response = await api(method, route, body);
  assert.ok(response.status >= 200 && response.status < 300, `${method} ${route} returned ${response.status}`);
  return response.body;
}
function ticketItem(title) { return page.locator('ul[aria-label="Tickets"] > li').filter({ has: page.locator(".ticket-head strong", { hasText: title }) }); }
function memoryItem(content) { return page.locator('ul[aria-label="Atlas memory records"] > li').filter({ hasText: content }); }
// The desktop shows one view at a time (ADR 0030); the sidebar selects it.
async function show(view, on = page) { await on.getByRole("navigation", { name: "Views" }).getByRole("button", { name: new RegExp(`^${view}`) }).click(); }
async function connectFixtureView() {
  fixtureView = false;
  if (nativeBinary) {
    await page.locator('.connection[data-tone="success"], [role="alert"]').first().waitFor({ timeout: 90_000 });
    if (await page.locator('.connection[data-tone="success"]').count()) await show("Settings");
    const disconnect = page.getByRole("button", { name: "Disconnect", exact: true });
    if (await disconnect.count()) await disconnect.click();
    const dismiss = page.locator('[role="alert"] button:has-text("Dismiss")');
    if (await dismiss.count()) await dismiss.click();
  }
  await page.getByLabel("Service URL", { exact: true }).fill(uiUrl);
  await page.getByLabel("Service token", { exact: true }).fill(token);
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  await show("Settings");
  await page.getByRole("button", { name: "Disconnect", exact: true }).waitFor();
  await show("Projects");
  fixtureView = true;
}

async function register(id, name, directory) {
  const add = page.locator("details.add-project");
  if (!(await add.evaluate((element) => element.open))) await add.locator("summary").click();
  await page.getByLabel("Project name", { exact: true }).fill(name);
  await page.getByLabel("Project ID", { exact: true }).fill(id);
  await page.getByLabel("Repository ID", { exact: true }).fill("app");
  await page.getByLabel("Repository root path", { exact: true }).fill(directory);
  await page.getByRole("button", { name: "Register repository", exact: true }).click();
  await waitFor("registered project", async () => (await api("GET", `/projects/${id}`)).status === 200 && await page.locator(".facts").count());
}
async function confirmManifest() {
  await page.getByRole("button", { name: "Discover proposed map", exact: true }).click();
  const editor = page.getByLabel(/Manifest to confirm/);
  await editor.waitFor();
  const manifest = JSON.parse(await editor.inputValue());
  manifest.repositories[0].checks = [{ id: "unit", executable: python, args: ["-m", "unittest", "-v"], cwd: ".", required: true, timeout_seconds: 30, capabilities: [] }];
  await editor.fill(JSON.stringify(manifest, null, 2));
  await page.getByRole("button", { name: "Confirm manifest", exact: true }).click();
  await waitFor("confirmed manifest", async () => (await page.locator(".facts").innerText()).includes("revision 1"));
}
// The Tickets section lists every ticket and shows one ticket's detail (ADR 0030): select it first.
async function showTicket(title) {
  const row = page.locator('ul[aria-label="Ticket list"] button').filter({ hasText: title }).first();
  await row.waitFor({ timeout: 20_000 });
  if ((await row.getAttribute("aria-pressed")) !== "true") await row.click();
}
async function createTicket(title, criteria = "Keep existing behavior") {
  await page.getByLabel("Ticket title", { exact: true }).fill(title);
  await page.getByLabel("Intent", { exact: true }).fill("Exercise the isolated component workflow.");
  await page.getByLabel("Acceptance criteria (one per line)", { exact: true }).fill(criteria);
  await page.getByRole("button", { name: "Create ticket", exact: true }).click();
  await showTicket(title);
  await ticketItem(title).waitFor();
}
async function createMemory(scope, content) {
  await page.getByLabel("Memory scope", { exact: true }).selectOption(scope);
  await page.getByLabel("Memory content", { exact: true }).fill(content);
  await page.getByLabel("Memory provenance", { exact: true }).fill("Synthetic m6 smoke fixture");
  await page.getByRole("button", { name: "Save memory", exact: true }).click();
  await memoryItem(content).waitFor();
}
async function retryUncertainWrite(resource, buttonName) {
  const url = `${uiUrl}/api/v1/projects/m6-primary/${resource}`;
  const deliveries = [];
  await page.route(url, async (route) => {
    if (route.request().method() !== "POST") return route.continue();
    deliveries.push(route.request().postDataJSON());
    if (deliveries.length === 1) {
      await route.fetch(); // Commit the genuine CLI write, then lose its response.
      return route.abort("failed");
    }
    return route.continue();
  });
  try {
    const button = page.getByRole("button", { name: buttonName, exact: true });
    await button.click();
    await page.locator(".project-components .error").waitFor();
    await waitFor("uncertain write may be retried", async () => !(await button.isDisabled()));
    await button.click();
    await waitFor("same write retried", () => deliveries.length === 2);
    record(`${resource} retry preserves its idempotency key and exact payload after a committed response is lost`, deliveries[0].idempotency_key === deliveries[1].idempotency_key && JSON.stringify(deliveries[0]) === JSON.stringify(deliveries[1]));
  } finally { await page.unroute(url); }
}
async function componentRefresh() {
  const button = page.getByRole("button", { name: "Refresh components", exact: true });
  await waitFor("component refresh available", async () => !(await button.isDisabled()));
  await button.click();
  await waitFor("component refresh settled", async () => !(await button.isDisabled()));
}
async function plan(title) {
  const item = ticketItem(title);
  const pre = item.getByLabel("Complete immutable plan", { exact: true });
  const previous = await pre.count() ? JSON.parse(await pre.innerText()).revision : 0;
  const supersede = item.locator(".plan-supersede input[type=checkbox]");
  if (await supersede.count()) await supersede.check();
  await item.getByRole("button", { name: /^Plan(?: r\d+)?$/ }).click();
  return waitFor("new immutable plan revision", async () => {
    if (!(await pre.count())) return null;
    const next = JSON.parse(await pre.innerText());
    return next.revision > previous ? next : null;
  });
}
async function approve(title) {
  const item = ticketItem(title);
  await item.getByLabel("I reviewed this complete plan and hash.", { exact: true }).check();
  await item.getByRole("button", { name: /^Approve r/ }).click();
  await item.getByRole("button", { name: "Implement", exact: true }).waitFor();
}

try {
  const dataDir = path.join(out, "runtime-data");
  runtime = spawn(runtimeBinary, [], { cwd: repository, env: { ...env(), LUUNA_API_TOKEN: token, LUUNA_DATA_DIR: dataDir, LUUNA_PORT: String(runtimePort), LUUNA_PI_COMMAND: python, LUUNA_PI_ARGS_JSON: JSON.stringify([path.join(repository, "fixtures/pi-rpc/mock_pi.py")]), LUUNA_PI_PROVIDER: "test", LUUNA_PI_MODEL: "synthetic", LUUNA_RUN_TIMEOUT_SECONDS: "120" }, stdio: ["ignore", "ignore", "pipe"] });
  runtimeExit = new Promise((resolve) => { runtime.once("exit", (code, signal) => { runtimeResult = { code, signal }; resolve(runtimeResult); }); runtime.once("error", () => { runtimeResult = { error: true }; resolve(runtimeResult); }); });
  runtime.stderr.on("data", (chunk) => writeFileSync(path.join(out, "runtime.log"), chunk, { flag: "a" }));
  await waitFor("isolated runtime health", () => fetch(`${runtimeUrl}/health`).then((r) => r.ok).catch(() => false), 45_000);
  if (nativeBinary) {
    if (!existsSync(nativeBinary)) throw new Error("The bundled native app must be built first");
    const debugPort = await port();
    const nativeEnv = { ...env() };
    for (const name of ["USERPROFILE", "HOME", "APPDATA", "LOCALAPPDATA"]) if (process.env[name]) nativeEnv[name] = process.env[name];
    nativeClient = spawn(nativeBinary, [], { cwd: repository, env: { ...nativeEnv, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort}`, WEBVIEW2_USER_DATA_FOLDER: path.join(out, "webview2-profile") }, windowsHide: true, stdio: ["ignore", "ignore", "pipe"] });
    nativeExit = new Promise((resolve) => { nativeClient.once("exit", (code, signal) => resolve({ code, signal })); nativeClient.once("error", () => resolve({ error: true })); });
    nativeClient.stderr.on("data", (chunk) => writeFileSync(path.join(out, "native.log"), chunk, { flag: "a" }));
    const endpoint = `http://127.0.0.1:${debugPort}`;
    await waitFor("native WebView debugging endpoint", () => fetch(`${endpoint}/json/version`).then((r) => r.ok).catch(() => false), 30_000);
    browser = await chromium.connectOverCDP(endpoint);
    page = await waitFor("native bundled application page", () => browser.contexts().flatMap((context) => context.pages()).find((candidate) => /^https?:\/\/tauri\.localhost\//.test(candidate.url())));
    browserInfo = { version: browser.version() };
    record("native Tauri app loads its bundled frontend in actual WebView2", true);
    await page.getByText("Local runtime connected", { exact: true }).waitFor({ timeout: 90_000 });
    record("native app authenticates automatic startup without token entry", (await page.getByLabel("Service token", { exact: true }).count()) === 0);
  } else {
    const configFile = path.join(out, "vite.config.mjs");
    const react = pathToFileURL(require.resolve("@vitejs/plugin-react")).href;
    // The desktop's stylesheet compiles Tailwind utilities at build time (ADR 0030).
    const tailwind = pathToFileURL(require.resolve("@tailwindcss/vite")).href;
    const viteConfig = { root: path.join(repository, "apps/desktop"), envDir: path.join(out, "no-env"), server: { host: "127.0.0.1", port: uiPort, strictPort: true, watch: { ignored: ["**/src-tauri/**"] }, proxy: { "/api/v1": runtimeUrl, "/health": runtimeUrl } } };
    writeFileSync(configFile, `import react from ${JSON.stringify(react)}; import tailwindcss from ${JSON.stringify(tailwind)}; export default { ...${JSON.stringify(viteConfig)}, plugins: [react(), tailwindcss()] };`);
    vite = spawn(process.execPath, [path.join(repository, "node_modules/vite/bin/vite.js"), "--config", configFile], { cwd: repository, env: { ...env(), NODE_ENV: "development" }, stdio: ["ignore", "pipe", "pipe"] });
    viteExit = new Promise((resolve) => { vite.once("exit", resolve); vite.once("error", resolve); });
    for (const stream of [vite.stdout, vite.stderr]) stream.on("data", (chunk) => writeFileSync(path.join(out, "vite.log"), chunk, { flag: "a" }));
    await waitFor("test Vite proxy", () => fetch(uiUrl).then((r) => r.ok).catch(() => false), 45_000);
    const candidates = [option("browser"), option("chromium"), process.env.LUUNA_CHROMIUM, "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe", "C:/Program Files/Google/Chrome/Application/chrome.exe", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
    const executablePath = candidates.find((candidate) => candidate && existsSync(candidate));
    browser = await chromium.launch({ headless: true, ...(executablePath && { executablePath }) });
    browserInfo = { executable_path: executablePath ?? chromium.executablePath(), version: browser.version() };
    page = await browser.newPage({ viewport: { width: 1440, height: 1080 } });
  }
  page.setDefaultTimeout(60_000);
  if (!nativeBinary) await page.goto(uiUrl);
  await connectFixtureView();
  record(nativeBinary ? "native app authenticates directly to the isolated runtime under production CORS" : "app authenticates to the isolated loopback runtime through the test proxy", true);

  const first = fixture("m6-primary");
  await register("m6-primary", "M6 primary", first);
  await confirmManifest();
  await createTicket("Existing local ticket");
  const legacy = (await successful("GET", "/projects/m6-primary/tickets"))[0];
  record("app creates a legacy ticket before explicit component activation", legacy.source === "luuna_sqlite_temporary");
  await page.getByRole("button", { name: "Enable project components", exact: true }).click();
  await waitFor("component activation", async () => (await page.locator(".project-components").innerText()).includes("Project components enabled"));
  const migrated = (await successful("GET", "/projects/m6-primary/tickets")).find((ticket) => ticket.id === legacy.id);
  record("activation migrates existing ticket identity and revision to genuine Missions", migrated?.source === "missions" && migrated.revision === legacy.revision);

  await createMemory("project", "Prefer exact candidate checks.");
  await createMemory("project", "Preserve compatibility aliases.");
  await createMemory("process", "Process workflow fixture.");
  record("app creates process-scoped Atlas memory with displayed provenance", (await memoryItem("Process workflow fixture.").innerText()).includes("process memory") && (await successful("GET", "/projects/m6-primary/memory")).some((record) => record.scope === "process"));
  await page.getByLabel("Memory scope", { exact: true }).selectOption("project");
  await page.getByLabel("Memory content", { exact: true }).fill("Disposable knowledge for deletion.");
  await page.getByLabel("Memory provenance", { exact: true }).fill("Synthetic m6 smoke fixture");
  await retryUncertainWrite("memory", "Save memory");
  await memoryItem("Disposable knowledge for deletion.").waitFor();
  record("uncertain memory delivery creates one actual Atlas record", (await successful("GET", "/projects/m6-primary/memory")).filter((record) => record.content === "Disposable knowledge for deletion.").length === 1);
  await memoryItem("Disposable knowledge for deletion.").getByRole("button", { name: "Delete memory", exact: true }).click();
  await waitFor("memory deletion", async () => !(await memoryItem("Disposable knowledge for deletion.").count()));
  record("app writes and hash-deletes actual Atlas memory", !(await successful("GET", "/projects/m6-primary/memory")).some((record) => record.content.includes("Disposable")));
  await page.getByLabel("Search memory", { exact: true }).fill("exact candidate");
  await page.getByRole("button", { name: "Search memory", exact: true }).click();
  await memoryItem("Prefer exact candidate checks.").waitFor();
  await waitFor("memory search excludes nonmatches", async () => !(await memoryItem("Preserve compatibility aliases.").count()));
  record("app retrieves matching Atlas knowledge with displayed provenance", (await memoryItem("Prefer exact candidate checks.").innerText()).includes("Synthetic m6 smoke fixture") && !(await memoryItem("Preserve compatibility aliases.").count()));
  await page.getByRole("button", { name: "Clear search", exact: true }).click();
  await createMemory("user", "Private user fixture: never attach automatically.");
  const userRecord = (await successful("GET", "/projects/m6-primary/memory?include_user=true")).find((record) => record.scope === "user");
  await memoryItem("Private user fixture: never attach automatically.").getByRole("checkbox").check();
  await page.getByLabel("Include private user memory", { exact: true }).uncheck();
  record("turning off private user memory removes it from the view and pending plan", !(await memoryItem("Private user fixture: never attach automatically.").count()) && (await page.locator(".component-context").innerText()).includes("0 selected"));
  record("project memory API omits user scope without explicit opt-in", !(await successful("GET", "/projects/m6-primary/memory")).some((record) => record.id === userRecord.id));

  const second = fixture("m6-other");
  await register("m6-other", "M6 other", second);
  await page.getByRole("button", { name: "Enable project components", exact: true }).click();
  await waitFor("second component activation", async () => (await successful("GET", "/projects/m6-other/components")).enabled);
  record("second project neither imports project knowledge nor exposes user knowledge by default", (await successful("GET", "/projects/m6-other/memory")).length === 0 && !(await page.getByLabel("Include private user memory", { exact: true }).isChecked()));
  await page.getByRole("button", { name: /M6 primary/ }).click();
  await waitFor("primary component panel restored", async () => (await memoryItem("Prefer exact candidate checks.").count()) > 0);
  await memoryItem("Prefer exact candidate checks.").getByRole("checkbox").check();
  await page.getByLabel("Prompt name", { exact: true }).fill("writer-guidance");
  await page.getByLabel("Prompt body", { exact: true }).fill("Use the exact approved task scope.");
  await retryUncertainWrite("prompts", "Save prompt");
  await waitFor("prompt version one", async () => (await successful("GET", "/projects/m6-primary/prompts")).some((prompt) => prompt.name === "writer-guidance" && prompt.version === 1) && (await page.locator('ul[aria-label="Beacon prompts"]').innerText()).includes("version 1"));
  await componentRefresh();
  const firstPrompt = await successful("GET", "/projects/m6-primary/prompts/writer-guidance/render?version=1");
  await page.getByRole("button", { name: "Use this prompt in next plan", exact: true }).click();
  await page.getByLabel("Prompt body", { exact: true }).fill("Use the exact approved task scope and minimal edits.");
  await page.getByRole("button", { name: "Save prompt", exact: true }).click();
  await waitFor("prompt version two", async () => (await page.locator(".prompt-preview").innerText()).includes("version 2"));
  const rendered = await successful("GET", "/projects/m6-primary/prompts/writer-guidance/render?version=2");
  record("app creates and updates a real versioned Beacon prompt with exact rendered hash", rendered.version === 2 && rendered.body.includes("minimal edits") && /^sha256:[a-f0-9]{64}$/.test(rendered.hash));
  const stalePromptPlan = await api("POST", `/tickets/${legacy.id}/plans`, { task: { repository_id: "app", paths: ["src"], operations: ["read", "write"] }, component_context: { memory_ids: [], prompt: { name: firstPrompt.name, version: firstPrompt.version, hash: firstPrompt.hash } } });
  record("a changed Beacon prompt clears UI selection and rejects an old prompt pin", stalePromptPlan.status === 409 && (await page.locator(".component-context").innerText()).includes("no prompt selected"));
  await page.getByRole("button", { name: "Use this prompt in next plan", exact: true }).click();

  await page.getByRole("button", { name: /^Run baseline\b/ }).click();
  await waitFor("genuine unittest baseline", async () => (await successful("GET", "/projects/m6-primary/baselines")).some((baseline) => baseline.status === "passed"), 120_000);
  const auxiliaryTicket = async (title) => successful("POST", "/projects/m6-primary/tickets", { idempotency_key: randomBytes(16).toString("hex"), title, intent: "Check source invalidation without starting a writer.", acceptance_criteria: ["Keep existing behavior"], repository_ids: ["app"] });
  const approvalPrompt = await successful("POST", "/projects/m6-primary/prompts", { idempotency_key: randomBytes(16).toString("hex"), name: "approval-fence", body: "Initial explicit guidance.", expected_version: 0 });
  const approvalTicket = await auxiliaryTicket("Approval source invalidation");
  const approvalPlan = await successful("POST", `/tickets/${approvalTicket.id}/plans`, { task: { repository_id: "app", paths: ["src"], operations: ["read", "write"] }, component_context: { memory_ids: [], prompt: { name: approvalPrompt.name, version: approvalPrompt.version, hash: approvalPrompt.hash } } });
  await successful("POST", "/projects/m6-primary/prompts", { idempotency_key: randomBytes(16).toString("hex"), name: "approval-fence", body: "Changed after this plan was built.", expected_version: 1 });
  const refusedApproval = await api("POST", `/plans/${approvalPlan.plan.id}/approvals`, { plan_revision: approvalPlan.plan.revision, plan_hash: approvalPlan.plan.hash, approved_by: "synthetic developer" });
  record("runtime refuses plan approval after its selected prompt source changes", refusedApproval.status === 409);
  const doomedMemory = await successful("POST", "/projects/m6-primary/memory", { idempotency_key: randomBytes(16).toString("hex"), scope: "project", content: "Delete after exact plan approval.", provenance: "Synthetic invalidation fixture" });
  const executionTicket = await auxiliaryTicket("Approved memory invalidation");
  const executionPlan = await successful("POST", `/tickets/${executionTicket.id}/plans`, { task: { repository_id: "app", paths: ["src"], operations: ["read", "write"] }, component_context: { memory_ids: [doomedMemory.id] } });
  await successful("POST", `/plans/${executionPlan.plan.id}/approvals`, { plan_revision: executionPlan.plan.revision, plan_hash: executionPlan.plan.hash, approved_by: "synthetic developer" });
  await successful("DELETE", `/projects/m6-primary/memory/${doomedMemory.id}?expected_hash=${encodeURIComponent(doomedMemory.hash)}`);
  const refusedExecution = await api("POST", `/runs/${executionPlan.run.id}/implement`, {});
  record("runtime refuses implementation after an approved memory source is deleted", refusedExecution.status === 409);
  await createTicket("Component-guided writer", "fixture:component_context\nmain prints a greeting");
  const sealed = await plan("Component-guided writer");
  record("new Missions ticket plan seals exact selected Atlas content/provenance and Beacon body", sealed.component_context?.memory?.length === 1 && sealed.component_context.memory[0].content === "Prefer exact candidate checks." && sealed.component_context.memory[0].provenance === "Synthetic m6 smoke fixture" && sealed.component_context.prompt?.hash === rendered.hash && sealed.component_context.prompt.body === rendered.body && !sealed.component_context.memory.some((record) => record.id === userRecord.id));
  await page.locator(".project-components").screenshot({ path: path.join(out, "component-panels.png") });
  await ticketItem("Component-guided writer").locator(".plan-review").screenshot({ path: path.join(out, "approved-resources-plan.png") });
  await approve("Component-guided writer");
  const tickets = await successful("GET", "/projects/m6-primary/tickets");
  const ticket = tickets.find((item) => item.title === "Component-guided writer");
  const source = await successful("GET", `/tickets/${ticket.id}/source`);
  runComponent("missions", ["update", source.source_id, "--title", "External Missions edit", "--json"], { root: repository, cwd: source.workspace, env: env() });
  await page.getByRole("button", { name: "Refresh workflows", exact: true }).click();
  await waitFor("external ticket drift shown", async () => (await ticketItem("Component-guided writer").innerText()).includes("changed outside Luuna"));
  const plans = await successful("GET", `/tickets/${ticket.id}/plans`);
  const runs = await successful("GET", "/runs");
  const oldRun = runs.find((run) => run.metadata.plan_id === plans.at(-1).id && run.metadata.plan_revision === plans.at(-1).revision);
  const refused = await api("POST", `/runs/${oldRun.id}/implement`, {});
  record("external Missions edit disables app planning and runtime rejects stale approved work", (await ticketItem("Component-guided writer").getByRole("button", { name: /^Plan r/ }).isDisabled()) && refused.status === 409);
  await ticketItem("Component-guided writer").getByRole("button", { name: "Reconcile external changes", exact: true }).click();
  await showTicket("External Missions edit");
  await ticketItem("External Missions edit").waitFor();
  const reconciled = await successful("GET", `/tickets/${ticket.id}`);
  record("app reconciliation preserves ticket identity and advances the source revision", reconciled.id === ticket.id && reconciled.revision > ticket.revision && reconciled.title === "External Missions edit");
  const resealed = await plan("External Missions edit");
  record("re-planning produces a new exact hash for the reconciled ticket revision", resealed.hash !== sealed.hash && resealed.ticket_revision === reconciled.revision);
  await approve("External Missions edit");
  await ticketItem("External Missions edit").getByRole("button", { name: "Implement", exact: true }).click();
  await waitFor("synthetic scoped writer settles", async () => (await ticketItem("External Missions edit").locator(".ticket-actions .state").filter({ hasText: /^validating$/ }).count()) > 0, 120_000);
  record("synthetic writer asserts approved memory and prompt content with unselected private memory absent", true);
  await ticketItem("External Missions edit").getByRole("button", { name: "Validate and review", exact: true }).click();
  await waitFor("real checks and synthetic independent review settle", async () => (await ticketItem("External Missions edit").locator(".ticket-actions .state").filter({ hasText: /^reviewed$/ }).count()) > 0, 120_000);
  record("app runs real candidate checks and a fresh synthetic reviewer that asserts the same explicit component context", true);
  fixtureView = false;
  await page.reload();
  await connectFixtureView();
  await page.getByRole("button", { name: /M6 primary/ }).waitFor();
  await page.getByRole("button", { name: /M6 primary/ }).click();
  await waitFor("reloaded authoritative tickets", async () => (await ticketItem("External Missions edit").count()) > 0);
  record("client reload restores authoritative Missions tickets without auto-attaching resources", (await page.locator(".component-context").innerText()).includes("0 selected") && !(await page.getByLabel("Include private user memory", { exact: true }).isChecked()));
  await page.screenshot({ path: path.join(out, "component-workflow.png"), fullPage: true });
} catch (error) {
  checks.push({ name: error instanceof Error ? error.message : "Component UI smoke failed", ok: false });
  console.error(error instanceof Error ? error.message : "Component UI smoke failed");
  if (page && fixtureView) await page.screenshot({ path: path.join(out, "failure.png"), fullPage: true }).catch(() => {});
  process.exitCode = 1;
} finally {
  if (nativeClient && nativeClient.exitCode === null && nativeClient.signalCode === null) {
    try {
      // Only this test-owned window is closed; its independent services stay alive.
      const quote = (value) => `'${value.replaceAll("'", "''")}'`;
      execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", `$p=Get-Process -Id ${nativeClient.pid} -ErrorAction Stop; if ($p.Path -ne ${quote(nativeBinary)}) { throw 'Native process identity changed' }; if (!$p.CloseMainWindow() -or !$p.WaitForExit(10000)) { throw 'Native window did not close normally' }`], { windowsHide: true, timeout: 15_000, stdio: "ignore" });
      record("native test window closes normally", true);
    } catch {
      nativeClient.kill();
      checks.push({ name: "native test window required forced cleanup", ok: false }); process.exitCode = 1;
    }
    await Promise.race([nativeExit, sleep(10_000)]);
    const healthy = await fetch(`${runtimeUrl}/health`).then((response) => response.ok).catch(() => false);
    checks.push({ name: "native window closure leaves the independent fixture runtime healthy", ok: healthy });
    if (!healthy) process.exitCode = 1;
  }
  if (browser) await browser.close().catch(() => {});
  if (vite) { vite.kill(); await Promise.race([viteExit, sleep(10_000)]); }
  if (runtime) {
    await api("POST", "/shutdown").catch(() => {});
    const result = await Promise.race([runtimeExit, sleep(15_000).then(() => null)]);
    if (!result) { runtime.kill("SIGKILL"); await runtimeExit; checks.push({ name: "runtime required forced cleanup", ok: false }); process.exitCode = 1; }
  }
  const report = { surface: nativeBinary ? "native Tauri WebView2" : "Chromium browser", native_binary: nativeBinary, native_sha256: nativeBinary ? createHash("sha256").update(readFileSync(nativeBinary)).digest("hex") : null, runtime_label: option("runtime-label") ?? "supplied build", complete: checks.length > 0 && checks.every((check) => check.ok), platform: process.platform, runtime_binary: runtimeBinary, runtime_sha256: createHash("sha256").update(readFileSync(runtimeBinary)).digest("hex"), runtime_exit: runtimeResult, browser: browserInfo, provider_evidence: false, component_evidence: "real pinned CLI processes", transport: nativeBinary ? "actual native WebView origin to authenticated loopback API with production CORS" : "test-only same-origin Vite proxy to authenticated runtime", checks };
  writeFileSync(path.join(out, "report.json"), JSON.stringify(report, null, 2));
  console.log(`${checks.filter((check) => check.ok).length}/${checks.length} component app smoke checks passed. Report: ${path.join(out, "report.json")}`);
}
