import { useEffect, useRef, useState } from "react";
import { Button, Form, FormFooter, Stack, TextField, submitted } from "@luuna/ui";
import { nativeDesktop, onFoldersDropped, pickFolder } from "../native-runtime";
import type { RepositoryRow } from "./project-model";

// The id starts empty: it follows the path's folder name, or what the developer types.
const FIRST_ROW: RepositoryRow = { id: "", path: "" };

/**
 * A repository id from the checkout's folder name (ADR 0042): `C:\\src\\My API` becomes `my-api`.
 * The developer can still type their own; an id is only derived while they have not.
 */
export function repositoryIdFromPath(path: string): string {
  const folder = path.trim().replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "";
  const slug = folder.toLowerCase().replace(/[^a-z0-9-]+/g, "-").replace(/-+/g, "-").replace(/^-+|-+$/g, "");
  if (!slug) return "";
  return (/^[a-z]/.test(slug) ? slug : `repo-${slug}`).slice(0, 64).replace(/-+$/, "");
}

/**
 * A row after its path changed: an id the developer typed stays; otherwise the id follows the
 * path's folder name, and is empty (to be typed) when that name yields none, never an id derived
 * from an earlier, partly typed path.
 */
export function rowForPath(row: RepositoryRow, path: string, typed: boolean): RepositoryRow {
  return typed ? { ...row, path } : { ...row, path, id: repositoryIdFromPath(path) };
}

/**
 * Rows after folders were dropped on the window (owner feedback C1): each folder fills the next
 * row without a path, then gets a row of its own. A folder already listed is not added again, and
 * every new path's id follows its folder name like a typed path's.
 */
export function withDroppedFolders(rows: RepositoryRow[], typed: boolean[], paths: string[]): { rows: RepositoryRow[]; typed: boolean[] } {
  const next = [...rows];
  const nextTyped = [...typed];
  for (const path of paths) {
    if (next.some((row) => row.path.trim() === path)) continue;
    const empty = next.findIndex((row) => !row.path.trim());
    if (empty >= 0) next[empty] = rowForPath(next[empty]!, path, nextTyped[empty] ?? false);
    else {
      next.push(rowForPath({ id: "", path: "" }, path, false));
      nextTyped.push(false);
    }
  }
  return { rows: next, typed: nextTyped };
}

/**
 * Registers a project with every related repository at once (R05). The form clears itself once
 * the runtime has registered the project.
 */
export function RegisterForm({ disabled, onRegister }: { disabled: boolean; onRegister: (name: string, projectId: string, rows: RepositoryRow[]) => Promise<boolean> }) {
  const [name, setName] = useState("");
  const [projectId, setProjectId] = useState("");
  const [rows, setRows] = useState<RepositoryRow[]>([FIRST_ROW]);
  // Rows whose id the developer typed; the others follow their path's folder name.
  const [typed, setTyped] = useState<boolean[]>([false]);
  const rowsComplete = rows.length > 0 && rows.every((row) => row.id.trim() && row.path.trim());
  const update = (index: number, patch: Partial<RepositoryRow>) => setRows((current) => current.map((item, i) => i === index ? { ...item, ...patch } : item));
  const setPath = (index: number, path: string) => setRows((current) => current.map((item, i) => i === index ? rowForPath(item, path, typed[index] ?? false) : item));
  const setId = (index: number, id: string) => {
    setTyped((current) => current.map((value, i) => i === index ? id !== "" : value));
    update(index, { id });
  };

  const native = nativeDesktop();
  // A drop reads the rows as last rendered, so rows and typed flags change together.
  const latest = useRef({ rows, typed });
  latest.current = { rows, typed };
  useEffect(() => {
    if (!native || disabled) return;
    return onFoldersDropped((paths) => {
      const next = withDroppedFolders(latest.current.rows, latest.current.typed, paths);
      setRows(next.rows);
      setTyped(next.typed);
    });
  }, [native, disabled]);

  async function choose(index: number) {
    const path = await pickFolder();
    if (path) setPath(index, path);
  }

  async function register() {
    if (!(await onRegister(name, projectId, rows))) return;
    setName("");
    setProjectId("");
    setRows([FIRST_ROW]);
    setTyped([false]);
  }

  return <Form onSubmit={submitted(() => void register())} className="form-group" disabled={disabled}>
    <Stack gap={3}>
      <TextField label="Project name" value={name} onChange={(event) => setName(event.target.value)} required placeholder="My product" />
      <TextField label="Project ID" value={projectId} onChange={(event) => setProjectId(event.target.value)} placeholder="derived from the name" mono pattern="[a-z][a-z0-9\-]{0,63}" />
    </Stack>
    <Stack gap={3} role="group" aria-label="Repositories to register">{rows.map((row, index) => <Stack key={index} className="repository-row" gap={2}>
      <TextField label="Repository root path" value={row.path} onChange={(event) => setPath(index, event.target.value)} required mono placeholder="/absolute/path/to/checkout" />
      {native && <Button variant="text" onClick={() => void choose(index)}>Choose folder{rows.length > 1 ? ` for repository ${row.id || index + 1}` : ""}…</Button>}
      <TextField label="Repository ID" value={row.id} onChange={(event) => setId(index, event.target.value)} required mono placeholder="from the folder name" pattern="[a-z][a-z0-9\-]{0,63}" />
      {rows.length > 1 && <Button variant="text" onClick={() => { setRows((current) => current.filter((_, i) => i !== index)); setTyped((current) => current.filter((_, i) => i !== index)); }}>Remove repository {row.id || index + 1}</Button>}
    </Stack>)}</Stack>
    <Button variant="text" onClick={() => { setRows((current) => [...current, { id: "", path: "" }]); setTyped((current) => [...current, false]); }}>Add another repository</Button>
    <FormFooter hint={`Register every related repository together.${native ? " Choose a folder, type its path, or drop folders on the window." : ""} A repository ID follows its folder name until you type your own. Paths stay on this machine and never enter the manifest.`}>
      <Button variant="primary" submit disabled={!name.trim() || !rowsComplete} data-capability="projects.register">
        Register {rows.length > 1 ? `${rows.length} repositories` : "repository"}
      </Button>
    </FormFooter>
  </Form>;
}
