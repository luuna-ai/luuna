import assert from "node:assert/strict";
import { test } from "node:test";
import { argumentsFor, credentialStore, validOpenaiOAuth } from "../oauth-launch.mjs";

const oauth = { type: "oauth", access: "fixture-access", refresh: "fixture-refresh", expires: 1000,
  clientId: "fixture-client", scopes: ["chatgpt.tokens.use.direct"] };
function memory(data) {
  return {
    async read(provider) { return data[provider]; },
    async list() { return Object.entries(data).map(([providerId, c]) => ({ providerId, type: c.type })); },
    async modify(provider, fn) { return data[provider] = await fn(data[provider]); },
  };
}

test("OAuth is explicit and requires the direct-plan grant, not an API key", () => {
  assert.ok(validOpenaiOAuth(oauth));
  for (const credential of [undefined, { ...oauth, type: "api_key" }, { ...oauth, scopes: [] }, { ...oauth, refresh: "" }]) {
    assert.equal(Boolean(validOpenaiOAuth(credential)), false);
  }
  assert.deepEqual(argumentsFor(["--luuna-auth-file", "/private/auth.json", "--luuna-pi-root", "/private/pi", "--version"]),
    { authFile: "/private/auth.json", piRoot: "/private/pi", args: ["--version"] });
  assert.throws(() => argumentsFor(["--luuna-auth-file", "relative"]));
});

test("only OpenAI OAuth is shared; refresh preserves other providers and local state", async () => {
  const source = { openai: { ...oauth }, anthropic: { type: "api_key", key: "private-unrelated" } };
  const local = { groq: { type: "api_key", key: "fixture-local" } };
  const store = credentialStore(memory(source), memory(local));
  assert.deepEqual(await store.list(), [{ providerId: "groq", type: "api_key" }, { providerId: "openai", type: "oauth" }]);
  assert.equal(await store.read("anthropic"), undefined);
  assert.equal(await store.read("openai"), source.openai);
  await store.modify("openai", async current => ({ ...current, access: "refreshed-access", refresh: "refreshed-refresh" }));
  assert.equal(source.openai.access, "refreshed-access");
  assert.equal(source.anthropic.key, "private-unrelated");
  assert.equal(local.openai, undefined);
  await store.modify("groq", async current => ({ ...current, key: "updated-local" }));
  assert.equal(local.groq.key, "updated-local");
  assert.equal(source.groq, undefined);
  await assert.rejects(store.delete("openai"));
});

test("missing shared OAuth cannot fall back to a session API key", async () => {
  const store = credentialStore(memory({}), memory({ openai: { type: "api_key", key: "fixture-local" } }));
  await assert.rejects(store.read("openai"));
  assert.deepEqual(await store.list(), []);
});
