import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";
import { AppShell, Banner, Breadcrumbs, ConnectionStatus, NavItem, NavSection, Sidebar, TopBar, ViewSlot, crumbTrail } from "./shell";

const noop = () => undefined;
const html = (element: Parameters<typeof renderToStaticMarkup>[0]) => renderToStaticMarkup(element);

describe("Sidebar", () => {
  const item = (label: string, current: boolean) => createElement(NavItem, { key: label, icon: "home", label, current, shortcut: "Ctrl+1", onSelect: noop });
  const markup = html(createElement(Sidebar, {
    status: createElement(ConnectionStatus, { tone: "success", label: "Local runtime connected" }),
    collapsed: false, onToggle: noop,
    children: [
      createElement(NavSection, { key: "a", id: "workspace", label: "Workspace", children: [item("Home", true), item("Board", false)] }),
      createElement(NavSection, { key: "b", id: "general", label: "General", children: [item("Help", false)] }),
    ],
  }));
  test("one labelled nav, no pill, one current item, the off-canvas id", () => {
    expect(markup).toContain('<nav class="sidebar-nav" aria-label="Views">');
    expect(markup.match(/<nav /g)).toHaveLength(1);
    expect(markup).not.toContain("nav-pill");
    expect(markup).not.toContain("data-pill");
    expect(markup.match(/aria-current="page"/g)).toHaveLength(1);
    expect(markup).toMatch(/^<aside id="app-sidebar" class="sidebar/);
  });
  test("each section list points at an existing label", () => {
    for (const [, id] of markup.matchAll(/<ul[^>]*aria-labelledby="([^"]+)"/g)) expect(markup).toContain(`id="${id}"`);
    expect(markup.match(/<ul[^>]*aria-labelledby/g)).toHaveLength(2);
  });
  test("the footer holds the one status; the palette trigger has moved to the top bar", () => {
    expect(markup).toMatch(/<div class="sidebar-footer"><span class="connection"[^>]*role="status"/);
    expect(markup.match(/class="connection"/g)).toHaveLength(1);
    expect(markup).toContain("Local runtime connected");
    expect(markup).not.toContain("data-live");
    expect(markup).not.toContain("aria-haspopup");
  });
});

describe("TopBar", () => {
  test("has the palette trigger with its key cap and the menu button", () => {
    const markup = html(createElement(TopBar, { onMenu: noop, menuOpen: false, onSearch: noop }));
    expect(markup).toMatch(/^<header class="topbar">/);
    expect(markup).toContain('aria-label="Go to a view, project or ticket"');
    expect(markup).toContain('aria-haspopup="dialog"');
    expect(markup).toContain("<kbd");
    expect(markup).toContain("Ctrl K");
    expect(markup).toContain('aria-controls="app-sidebar"');
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).toContain('aria-label="Open navigation"');
  });
  test("puts the lead slot between the menu button and the palette trigger", () => {
    const markup = html(createElement(TopBar, { onMenu: noop, menuOpen: false, onSearch: noop, lead: createElement("span", { className: "lead-probe" }) }));
    expect(markup.indexOf("topbar-menu")).toBeLessThan(markup.indexOf("lead-probe"));
    expect(markup.indexOf("lead-probe")).toBeLessThan(markup.indexOf("topbar-search"));
  });
  test("an open panel reads as expanded and offers to close", () => {
    const markup = html(createElement(TopBar, { onMenu: noop, menuOpen: true, onSearch: noop }));
    expect(markup).toContain('aria-expanded="true"');
    expect(markup).toContain('aria-label="Close navigation"');
  });
});

describe("Banner", () => {
  const markup = html(createElement(Banner, { label: "Reconnecting", className: "outage", children: "Runtime unreachable." }));
  test("is a status with its class, label first and then the message", () => {
    expect(markup).toBe('<div class="banner outage" role="status"><strong class="banner-label">Reconnecting</strong><span class="banner-message">Runtime unreachable.</span></div>');
  });
  test("is never the connection element the smokes read", () => {
    expect(markup).not.toContain("connection");
  });
});

describe("ConnectionStatus", () => {
  test("shows the tone mark, the label and the tone, without a pulse hook", () => {
    const markup = html(createElement(ConnectionStatus, { tone: "success", label: "Local runtime connected" }));
    expect(markup).toContain('data-tone="success"');
    expect(markup).toContain('<i class="tone-mark"');
    expect(markup).not.toContain("data-live");
  });
});

describe("ViewSlot and AppShell", () => {
  test("a hidden entering slot carries both attributes", () => {
    const markup = html(createElement(ViewSlot, { hidden: true, entering: true, children: "x" }));
    expect(markup).toContain('hidden=""');
    expect(markup).toContain("data-entering");
    expect(html(createElement(ViewSlot, { hidden: false, children: "x" }))).not.toContain("data-entering");
  });
  test("the skip link comes first, ahead of the banner, and main is the focus target", () => {
    const markup = html(createElement(AppShell, { collapsed: false, sidebar: null, banner: createElement("p", null, "b"), motion: "css", children: "x" }));
    expect(markup).toContain('data-view-motion="css"');
    expect(markup).toMatch(/^<div class="app"[^>]*><a class="skip-link" href="#main">/);
    expect(markup.indexOf("skip-link")).toBeLessThan(markup.indexOf("shell-banner"));
    expect(markup).toContain('<main id="main" tabindex="-1" class="content">');
    expect(markup).toContain("data-banner");
  });
  test("without a banner, a rail or an open panel, none of their markers appear", () => {
    const markup = html(createElement(AppShell, { collapsed: false, sidebar: null, children: "x" }));
    for (const absent of ["data-banner", "data-rail", "data-nav", "sidebar-scrim", "shell-rail", "inert", "shell-banner"]) expect(markup).not.toContain(absent);
  });
  test("an open panel adds the scrim, the marker and inert content", () => {
    const markup = html(createElement(AppShell, { collapsed: false, sidebar: null, navOpen: true, onDismissNav: noop, rail: "r", children: "x" }));
    expect(markup).toContain('data-nav="open"');
    expect(markup).toContain('class="sidebar-scrim" aria-label="Close navigation"');
    expect(markup).toMatch(/<a class="skip-link" href="#main" inert="">/);
    expect(markup).toMatch(/<div class="shell-main" inert=""/);
    expect(markup).toMatch(/<aside class="shell-rail" aria-label="Details" inert=""/);
  });
  test("a rail renders beside main only when given", () => {
    expect(html(createElement(AppShell, { collapsed: false, sidebar: null, rail: "r", children: "x" }))).toContain('<aside class="shell-rail" aria-label="Details">r</aside>');
  });
  test("the collapsed rail marks the shell", () => {
    expect(html(createElement(AppShell, { collapsed: true, sidebar: null, children: "x" }))).toContain('data-sidebar="collapsed"');
  });
});

describe("Breadcrumbs and crumbTrail", () => {
  const crumbs = (count: number) => Array.from({ length: count }, (_, index) => ({ label: `c${index}`, onSelect: noop }));
  test("a short trail is unchanged", () => {
    expect(crumbTrail(crumbs(4)).map((crumb) => crumb.label)).toEqual(["c0", "c1", "c2", "c3"]);
  });
  test("a long trail keeps the first and the last two around an ellipsis", () => {
    expect(crumbTrail(crumbs(6)).map((crumb) => crumb.label)).toEqual(["c0", "…", "c4", "c5"]);
  });
  test("renders a labelled list whose last item is the current page", () => {
    const markup = html(createElement(Breadcrumbs, { items: crumbs(3) }));
    expect(markup).toMatch(/^<nav aria-label="Breadcrumb" class="breadcrumbs"><ol>/);
    expect(markup).toContain('<span aria-current="page">c2</span>');
    expect(markup.match(/<button/g)).toHaveLength(2);
    expect(markup.match(/aria-current="page"/g)).toHaveLength(1);
  });
});
