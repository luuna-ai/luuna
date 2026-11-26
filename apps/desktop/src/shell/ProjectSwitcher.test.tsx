import { expect, test } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { ProjectRecord } from "@luuna/api-client";
import { ProjectSwitcher, switcherOptions } from "./ProjectSwitcher";

const record = (id: string, name: string) => ({ id, name, revision: 1, manifest_hash: null, created_at: "", updated_at: "" }) as ProjectRecord;
const projects = [record("api", "API"), record("web", "Web shop")];

test("the switcher prompts for a project until one is open, then lists only the projects", () => {
  expect(switcherOptions([], null)).toEqual([{ value: "", label: "No projects yet" }]);
  expect(switcherOptions(projects, null).map((option) => option.label)).toEqual(["Choose a project", "API", "Web shop"]);
  expect(switcherOptions(projects, "web").map((option) => option.value)).toEqual(["api", "web"]);
  // A project that is no longer listed (for example, removed) counts as none open.
  expect(switcherOptions(projects, "gone")[0]!.label).toBe("Choose a project");
});

test("the switcher shows the open project by name and is disabled with nothing to choose", () => {
  const open = renderToStaticMarkup(<ProjectSwitcher projects={projects} current="web" disabled={false} onOpen={() => {}} />);
  expect(open).toContain('class="project-switcher"');
  expect(open).toMatch(/<option value="web" selected="">Web shop<\/option>/);
  const empty = renderToStaticMarkup(<ProjectSwitcher projects={[]} current={null} disabled={false} onOpen={() => {}} />);
  expect(empty).toContain("disabled");
  expect(empty).toContain("No projects yet");
});
