import { expect, test } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { TicketPreview } from "./TicketForm";
import { emptyTicketForm } from "./settings";

test("the ticket preview shows the ticket as the runtime will receive it", () => {
  const form = { ...emptyTicketForm("app-web"), title: "  Fix the greeting  ", intent: "Say hello\nto everyone", criteria: "Greets by name\n\n  Handles empty names ", ticketType: "bug" as const, priority: 1, labels: "UI, docs, ui" };
  const html = renderToStaticMarkup(<TicketPreview form={form} scoped={["app-web", "api"]} />);
  expect(html).toContain('aria-label="Ticket preview"');
  expect(html).toContain(">Fix the greeting<");
  expect(html).toContain("Say hello\nto everyone");
  expect(html).toContain("<li>Greets by name</li><li>Handles empty names</li>");
  for (const tag of ["bug", "P1 high", "ui", "docs"]) expect(html).toContain(`<span class="tag">${tag}</span>`);
  expect(html.match(/<span class="tag">ui<\/span>/g)).toHaveLength(1);
  expect(html.replace(/<\/?span>/g, "")).toContain("Scoped to <code>app-web</code>, <code>api</code>");
});

test("an empty form previews what is still missing", () => {
  const html = renderToStaticMarkup(<TicketPreview form={emptyTicketForm("app-web")} scoped={["app-web"]} />);
  for (const missing of ["No title yet", "No intent yet", "No acceptance criteria yet"]) expect(html).toContain(missing);
  expect(html).toContain('<span class="tag">task</span>');
  expect(html).not.toContain("testing setup");
});
