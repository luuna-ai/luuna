import { expect, test, vi } from "vitest";
import { LuunaClient } from "../src/index";

test("intent requests preserve authorization, cancellation and encoded resource identities", async () => {
  const fetcher = vi.fn<typeof fetch>().mockImplementation(() => Promise.resolve(new Response("{}")));
  const client = new LuunaClient("http://localhost:4242", "synthetic-token", fetcher);
  const signal = new AbortController().signal;
  await client.showIntent("project/one", signal);
  await client.initIntent("project/one", { by: " alice " }, signal);
  await client.listIntentProposals("project/one", signal);
  await client.listIntentQuestions("project/one", signal);
  await client.listIntentSprouts("project/one", signal);
  await client.acceptIntentProposal("project/one", "p/one", { by: "alice", rel: "serves", keep: "r-one", reason: "yes" }, signal);
  await client.rejectIntentProposal("project/one", "p/one", { by: "bob", reason: "no" }, signal);
  await client.rejectIntentSprout("project/one", "s/one", { by: "bob" }, signal);
  expect(fetcher.mock.calls.map(([url, init]) => [String(url), init?.method ?? "GET"])).toEqual([
    ["http://localhost:4242/api/v1/projects/project%2Fone/intent", "GET"],
    ["http://localhost:4242/api/v1/projects/project%2Fone/intent", "POST"],
    ["http://localhost:4242/api/v1/projects/project%2Fone/intent/proposals", "GET"],
    ["http://localhost:4242/api/v1/projects/project%2Fone/intent/questions", "GET"],
    ["http://localhost:4242/api/v1/projects/project%2Fone/intent/sprouts", "GET"],
    ["http://localhost:4242/api/v1/projects/project%2Fone/intent/proposals/p%2Fone/accept", "POST"],
    ["http://localhost:4242/api/v1/projects/project%2Fone/intent/proposals/p%2Fone/reject", "POST"],
    ["http://localhost:4242/api/v1/projects/project%2Fone/intent/sprouts/s%2Fone/reject", "POST"],
  ]);
  expect(JSON.parse(String(fetcher.mock.calls[1][1]?.body))).toEqual({ by: "alice" });
  expect(JSON.parse(String(fetcher.mock.calls[5][1]?.body))).toEqual({ by: "alice", rel: "serves", keep: "r-one", reason: "yes" });
  for (const [, init] of fetcher.mock.calls) {
    expect(new Headers(init?.headers).get("Authorization")).toBe("Bearer synthetic-token");
    expect(init?.signal).toBe(signal);
    expect(init?.redirect).toBe("error");
  }
});

test("every human intent method refuses missing, reserved or oversized names before any request", async () => {
  const fetcher = vi.fn<typeof fetch>().mockImplementation(() => Promise.resolve(new Response("{}")));
  const client = new LuunaClient("http://localhost:4242", "synthetic-token", fetcher);
  const methods = [
    (by: string) => client.initIntent("app", { by }),
    (by: string) => client.acceptIntentProposal("app", "p-one", { by }),
    (by: string) => client.rejectIntentProposal("app", "p-one", { by }),
    (by: string) => client.rejectIntentSprout("app", "s-one", { by }),
  ];
  for (const method of methods) for (const by of ["", "  ", "agent:x", " agent:run-1 ", "roots", "roots:rule", "a".repeat(201), "é".repeat(101)]) expect(() => method(by)).toThrow("explicit human name");
  expect(fetcher).not.toHaveBeenCalled();
  for (const method of methods) await method("a".repeat(200));
  expect(fetcher).toHaveBeenCalledTimes(4);
});
