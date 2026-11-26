import type { ProjectRecord } from "@luuna/api-client";
import { SelectField } from "@luuna/ui";

/** The value of the "choose a project" entry, shown while no project is open. */
const NONE = "";

/** The switcher's entries: the registered projects by name, after a prompt while none is open. */
export function switcherOptions(projects: ProjectRecord[], current: string | null) {
  const listed = projects.map((project) => ({ value: project.id, label: project.name }));
  if (!projects.length) return [{ value: NONE, label: "No projects yet" }];
  return current && projects.some((project) => project.id === current) ? listed : [{ value: NONE, label: "Choose a project" }, ...listed];
}

/**
 * The project switcher in the top-left corner on every view (owner feedback D1). It shows the
 * project open in Projects and opens another one there; it lists what the Projects view read.
 */
export function ProjectSwitcher({ projects, current, disabled, onOpen }: { projects: ProjectRecord[]; current: string | null; disabled: boolean; onOpen: (projectId: string) => void }) {
  const value = current && projects.some((project) => project.id === current) ? current : NONE;
  return <SelectField label="Project" labelClassName="project-switcher" value={value} disabled={disabled || !projects.length}
    options={switcherOptions(projects, current)} onChange={(event) => { if (event.target.value) onOpen(event.target.value); }} />;
}
