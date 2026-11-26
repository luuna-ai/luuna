import { useState } from "react";
import type { ProjectView } from "@luuna/api-client";
import { Button, Checkbox, Disclosure, Form, FormFooter, submitted } from "@luuna/ui";

/**
 * Takes the selected project off every list and frees its folders (ADR 0063). Its records stay
 * on disk, and registering the same repositories at the same folders brings it back. Rendered
 * per selected project (keyed by it), so the confirmation starts unchecked for each project.
 */
export function RemoveProject({ view, busy, onRemove }: { view: ProjectView; busy: boolean; onRemove: () => Promise<boolean> }) {
  const [confirmed, setConfirmed] = useState(false);

  async function remove() {
    if (!confirmed) return;
    if (!(await onRemove())) return;
    setConfirmed(false);
  }

  return <Disclosure className="remove-project" summary="Remove project">
    <Form onSubmit={submitted(() => void remove())} className="remove-project-form" disabled={busy}>
      <Checkbox className="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} label={`Remove ${view.project.name} from every list and free its folders.`} />
      <FormFooter hint="Nothing is deleted: its tickets, runs, evidence and history stay on disk, and adding the same folders again brings the project back. Removal waits while a run is working in one of its checkouts.">
        <Button variant="danger" submit disabled={!confirmed} data-capability="projects.remove">Remove project</Button>
      </FormFooter>
    </Form>
  </Disclosure>;
}
