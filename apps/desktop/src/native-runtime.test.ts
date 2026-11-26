import { beforeEach, expect, it, vi } from "vitest";

const core = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => core);
import { ensureLocalRuntime } from "./native-runtime";

beforeEach(() => { core.invoke.mockReset(); core.isTauri.mockReturnValue(true); });

it("never invokes host startup in a browser", async () => {
  core.isTauri.mockReturnValue(false);
  await expect(ensureLocalRuntime()).rejects.toThrow("native desktop");
  expect(core.invoke).not.toHaveBeenCalled();
});

it("shares concurrent startup requests without retaining the connection for future reloads", async () => {
  const connection = { url: "http://127.0.0.1:4242", token: "fixture" };
  core.invoke.mockResolvedValue(connection);
  const first = ensureLocalRuntime();
  expect(ensureLocalRuntime()).toBe(first);
  await expect(first).resolves.toEqual(connection);
  await ensureLocalRuntime();
  expect(core.invoke).toHaveBeenCalledTimes(2);
  expect(core.invoke).toHaveBeenCalledWith("ensure_local_runtime");
});

it("allows retry after a failed startup", async () => {
  core.invoke.mockRejectedValueOnce("Port occupied");
  await expect(ensureLocalRuntime()).rejects.toThrow("Port occupied");
  core.invoke.mockResolvedValue({ url: "http://127.0.0.1:4242", token: "fixture" });
  await ensureLocalRuntime();
  expect(core.invoke).toHaveBeenCalledTimes(2);
});

import { onFoldersDropped, pickFolder } from "./native-runtime";

it("opens the folder dialog only in the native app", async () => {
  core.isTauri.mockReturnValue(false);
  await expect(pickFolder()).resolves.toBeNull();
  expect(core.invoke).not.toHaveBeenCalled();
  core.isTauri.mockReturnValue(true);
  core.invoke.mockResolvedValue("/srv/app");
  await expect(pickFolder()).resolves.toBe("/srv/app");
  expect(core.invoke).toHaveBeenCalledWith("pick_folder");
  core.invoke.mockResolvedValue(null);
  await expect(pickFolder()).resolves.toBeNull();
});

it("passes on only the path strings of dropped folders, until unsubscribed", () => {
  const seen: string[][] = [];
  const window = new EventTarget();
  const stop = onFoldersDropped((paths) => seen.push(paths), window);
  window.dispatchEvent(new CustomEvent("luuna:folders-dropped", { detail: ["/srv/app", 3, "", "/srv/web"] }));
  window.dispatchEvent(new CustomEvent("luuna:folders-dropped", { detail: "not a list" }));
  stop();
  window.dispatchEvent(new CustomEvent("luuna:folders-dropped", { detail: ["/srv/late"] }));
  expect(seen).toEqual([["/srv/app", "/srv/web"]]);
});
