#!/usr/bin/env node
// Opt-in OpenAI OAuth for otherwise isolated Pi RPC workers. No token copies,
// shell launcher, extensions, settings or history are imported from the user.
import { readFileSync, statSync } from "node:fs";
import { isAbsolute, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

export function argumentsFor(argv) {
  const options = {};
  const args = [...argv];
  for (const [flag, name] of [["--luuna-auth-file", "authFile"], ["--luuna-pi-root", "piRoot"]]) {
    if (args.shift() !== flag) throw new Error("Missing OAuth launcher option");
    options[name] = args.shift();
    if (!options[name] || !isAbsolute(options[name])) throw new Error("OAuth launcher paths must be absolute");
  }
  return { ...options, args };
}

export function validOpenaiOAuth(credential) {
  return credential?.type === "oauth" &&
    typeof credential.access === "string" && credential.access.length > 0 &&
    typeof credential.refresh === "string" && credential.refresh.length > 0 &&
    Number.isFinite(credential.expires) &&
    typeof credential.clientId === "string" && credential.clientId.length > 0 &&
    Array.isArray(credential.scopes) && credential.scopes.includes("chatgpt.tokens.use.direct");
}

export function credentialStore(shared, isolated) {
  // Pi's own AuthStorage.modify owns its cross-process refresh lock and merges
  // only this provider into the latest file. Other providers stay session-local.
  const store = provider => provider === "openai" ? shared : isolated;
  return {
    async read(provider, options) {
      const credential = await store(provider).read(provider, options);
      if (provider === "openai" && !validOpenaiOAuth(credential)) throw new Error("OpenAI OAuth login is unavailable");
      return credential;
    },
    async list(options) {
      const local = (await isolated.list(options)).filter(entry => entry.providerId !== "openai");
      const remote = (await shared.list(options)).filter(entry => entry.providerId === "openai" && entry.type === "oauth");
      return [...local, ...remote];
    },
    modify(provider, fn, options) {
      return store(provider).modify(provider, fn, options);
    },
    async delete() {
      // A supervised worker may refresh a login, never log the developer out.
      throw new Error("OAuth logout is unavailable in a supervised worker");
    },
  };
}

export async function launch(argv) {
  const { authFile, piRoot, args } = argumentsFor(argv);
  const pkg = JSON.parse(readFileSync(join(piRoot, "package.json"), "utf8"));
  if (pkg.name !== "@earendil-works/pi-coding-agent" || pkg.version !== "1.3.0") throw new Error("Unsupported Pi package");
  if (args.length === 1 && args[0] === "--version") {
    console.log(pkg.version);
    return;
  }
  const agentDir = process.env.PI_CODING_AGENT_DIR;
  if (!agentDir || !isAbsolute(agentDir) || resolve(agentDir) === resolve(authFile, "..")) throw new Error("Isolated worker directory is required");
  const info = statSync(authFile);
  if (!info.isFile() || info.size > 1024 * 1024 || (process.platform !== "win32" && (info.mode & 0o077))) throw new Error("OAuth file must be private");
  const auth = JSON.parse(readFileSync(authFile, "utf8"));
  if (!validOpenaiOAuth(auth.openai)) throw new Error("OpenAI OAuth login is unavailable");
  if (!args.includes("--offline") || !args.includes("--no-context-files") || !args.includes("--no-extensions")) throw new Error("Isolated Luuna worker flags are required");
  const moduleAt = path => import(pathToFileURL(join(piRoot, "dist", path)).href);
  const { AuthStorage } = await moduleAt("core/auth-storage.js");
  const { ModelRuntime } = await moduleAt("core/model-runtime.js");
  const shared = AuthStorage.create(authFile);
  const create = ModelRuntime.create.bind(ModelRuntime);
  // Pi's CLI does not expose a credential-store argument. Its pinned SDK
  // factory does; this narrow factory adapter keeps the normal CLI/RPC flow.
  ModelRuntime.create = options => create({ ...options,
    credentials: credentialStore(shared, AuthStorage.create(options?.authPath ?? join(agentDir, "auth.json"))),
  });
  // OAuth must never silently fall back to a separately billed API key.
  delete process.env.OPENAI_API_KEY;
  delete process.env.LUUNA_PI_OPENAI_API_KEY;
  process.argv = [process.execPath, join(piRoot, "dist/cli.js"), ...args];
  const { setupCli } = await moduleAt("cli/setup.js");
  const { main } = await moduleAt("main.js");
  setupCli();
  await main(args);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    await launch(process.argv.slice(2));
  } catch {
    // OAuth/network exceptions can contain token responses. Never print them.
    console.error("Luuna OAuth worker unavailable; verify the pinned Pi installation and your OpenAI login with pi.");
    process.exitCode = 1;
  }
}
