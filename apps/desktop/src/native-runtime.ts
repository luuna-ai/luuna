import { invoke, isTauri } from "@tauri-apps/api/core";

export const nativeDesktop = () => isTauri();
export interface LocalConnection { url: string; token: string }
let pending: Promise<LocalConnection> | undefined;

/** Share concurrent StrictMode startup calls; retain no token after settlement. */
export function ensureLocalRuntime(): Promise<LocalConnection> {
  if (!nativeDesktop()) return Promise.reject(new Error("Open the native desktop app for automatic startup."));
  if (!pending) {
    pending = invoke<LocalConnection>("ensure_local_runtime")
      .catch((failure: unknown) => {
        throw failure instanceof Error ? failure : new Error(typeof failure === "string" ? failure : "Local runtime startup failed.");
      })
      .finally(() => { pending = undefined; });
  }
  return pending;
}

/** The native folder dialog (owner feedback C1). `null` when cancelled; never called in a browser. */
export async function pickFolder(): Promise<string | null> {
  if (!nativeDesktop()) return null;
  return (await invoke<string | null>("pick_folder")) ?? null;
}

/**
 * Folders dropped on the native window: the shell dispatches `luuna:folders-dropped` with their
 * paths (dropped files are left out). Returns the unsubscribe function.
 */
export function onFoldersDropped(handler: (paths: string[]) => void, target: EventTarget = window): () => void {
  const listener = (event: Event) => {
    const detail = (event as CustomEvent<unknown>).detail;
    if (Array.isArray(detail)) handler(detail.filter((path): path is string => typeof path === "string" && path.length > 0));
  };
  target.addEventListener("luuna:folders-dropped", listener);
  return () => target.removeEventListener("luuna:folders-dropped", listener);
}
