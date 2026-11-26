import type { ReactNode } from "react";
import type { Tone } from "./tone/tone";
import { Icon } from "./Icon";
import type { IconName } from "./Icon";
import type { ViewMotion } from "./view-motion";
import { Kbd } from "./docs";

/**
 * The window (ADR 0030, ADR 0033): an optional banner across the top, a sidebar of views, and one
 * column holding the sticky top bar and `main`. The wordmark is the page's h1. The sidebar can be
 * collapsed to an icon rail above 768 px; at 768 px and below it is an off-canvas panel (`navOpen`)
 * and the rest of the window is inert while it is open.
 */
export function AppShell({ collapsed, sidebar, banner, topbar, rail, navOpen, onDismissNav, children, motion, titleBar }: {
  collapsed: boolean; sidebar: ReactNode; banner?: ReactNode; topbar?: ReactNode; rail?: ReactNode; navOpen?: boolean; onDismissNav?: () => void; children: ReactNode; motion?: ViewMotion;
  /** The top bar is the window's title bar (ADR 0038): it stays on the window's top edge, so the banner sits under it. */
  titleBar?: boolean;
}) {
  const bannerBar = banner ? <div className="shell-banner" inert={navOpen}>{banner}</div> : null;
  return <div className="app" data-sidebar={collapsed ? "collapsed" : undefined} data-nav={navOpen ? "open" : undefined} data-banner={banner && !titleBar ? "" : undefined} data-rail={rail ? "" : undefined} data-view-motion={motion} data-titlebar={titleBar ? "" : undefined}>
    <a className="skip-link" href="#main" inert={navOpen}>Skip to main content</a>
    {!titleBar && bannerBar}
    {sidebar}
    {navOpen && <button type="button" className="sidebar-scrim" aria-label="Close navigation" tabIndex={-1} onClick={onDismissNav} />}
    <div className="shell-main" inert={navOpen}>
      {topbar}
      {titleBar && bannerBar}
      <main id="main" tabIndex={-1} className="content">{children}</main>
    </div>
    {rail && <aside className="shell-rail" aria-label="Details" inert={navOpen}>{rail}</aside>}
  </div>;
}

/** The brand mark: an accent disc whose leading half is cut from the surface behind it. */
export function Mark({ className }: { className: string }) {
  return <svg className={className} viewBox="0 0 292 292" aria-hidden="true" focusable="false"><circle cx="146" cy="146" r="146" /><path d="M146 0a146 146 0 0 0 0 292z" /></svg>;
}

/** The app-wide state strip (Ledger Banner): only for state that changes what every action means, one at a time. */
export function Banner({ label, children, action, className, title }: { label: string; children: ReactNode; action?: ReactNode; className?: string; title?: string }) {
  return <div className={["banner", className].filter(Boolean).join(" ")} role="status" title={title}>
    <strong className="banner-label">{label}</strong><span className="banner-message">{children}</span>{action}
  </div>;
}

/**
 * The sticky top bar: the menu button (under 768 px), `lead` (such as the project switcher), the palette trigger shaped
 * as a Ledger search field, then `actions`.
 * With `windowControls` (ADR 0038) it is also the window's title bar: the bar and its spacer are drag regions and the
 * controls sit flush at the right edge. Buttons stay separate elements, so pressing one never starts a drag.
 */
export function TopBar({ onMenu, menuOpen, onSearch, lead, actions, windowControls }: { onMenu: () => void; menuOpen: boolean; onSearch: () => void; lead?: ReactNode; actions?: ReactNode; windowControls?: ReactNode }) {
  const drag = windowControls ? "" : undefined;
  return <header className="topbar" data-tauri-drag-region={drag}>
    <button type="button" id="nav-menu" className="topbar-menu" aria-label={menuOpen ? "Close navigation" : "Open navigation"} aria-expanded={menuOpen} aria-controls="app-sidebar" onClick={onMenu}><Icon name="sidebar" /></button>
    {lead}
    <button type="button" className="topbar-search" aria-label="Go to a view, project or ticket" aria-haspopup="dialog" aria-keyshortcuts="Control+K" title="Go to a view, project or ticket (Ctrl+K)" onClick={onSearch}>
      <Icon name="search" /><span className="topbar-search-label">Go to…</span><Kbd>Ctrl K</Kbd>
    </button>
    <span className="topbar-spacer" data-tauri-drag-region={drag} />
    {actions}
    {windowControls}
  </header>;
}

export interface Crumb { label: string; onSelect?: () => void; ellipsis?: boolean }

/** At most four crumbs: longer trails keep the first and the last two, with "…" for the middle. */
export function crumbTrail(items: ReadonlyArray<Crumb>): Crumb[] {
  if (items.length <= 4) return [...items];
  return [items[0], { label: "…", ellipsis: true }, ...items.slice(-2)];
}

/** Where a nested page sits; the last crumb is the page itself. Not wired into the top-level views. */
export function Breadcrumbs({ items }: { items: ReadonlyArray<Crumb> }) {
  const trail = crumbTrail(items);
  return <nav aria-label="Breadcrumb" className="breadcrumbs">
    <ol>{trail.map((crumb, index) => <li key={`${index}-${crumb.label}`}>
      {index === trail.length - 1 ? <span aria-current="page">{crumb.label}</span>
        : crumb.ellipsis || !crumb.onSelect ? <span aria-hidden={crumb.ellipsis ? true : undefined}>{crumb.label}</span>
          : <button type="button" onClick={crumb.onSelect}>{crumb.label}</button>}
    </li>)}</ol>
  </nav>;
}

/** `dragRegion` (ADR 0038) makes the brand row move the window; the toggle button inside it stays a button. */
export function Sidebar({ status, onToggle, collapsed, children, dragRegion }: { status: ReactNode; onToggle: () => void; collapsed: boolean; children: ReactNode; dragRegion?: boolean }) {
  const drag = dragRegion ? "" : undefined;
  return <aside id="app-sidebar" className="sidebar flex flex-col gap-5">
    <div className="sidebar-top flex items-center gap-2" data-tauri-drag-region={drag}>
      <h1 className="wordmark flex items-center gap-2" data-tauri-drag-region={drag}><Mark className="wordmark-mark" /><span className="sidebar-label" data-tauri-drag-region={drag}>luuna</span></h1>
      <button type="button" className="sidebar-toggle" aria-label={collapsed ? "Show sidebar labels" : "Collapse sidebar"} aria-expanded={!collapsed} aria-keyshortcuts="Control+\" title={`${collapsed ? "Expand" : "Collapse"} sidebar (Ctrl+\\)`} onClick={onToggle}><Icon name="sidebar" /></button>
    </div>
    <nav className="sidebar-nav" aria-label="Views">{children}</nav>
    <div className="sidebar-footer">{status}</div>
  </aside>;
}

/** One labelled group of views; the label stays announced in the rail. */
export function NavSection({ id, label, children }: { id: string; label: string; children: ReactNode }) {
  return <div className="nav-section">
    <span className="nav-section-label sidebar-label" id={`nav-${id}`}>{label}</span>
    <ul className="flex flex-col gap-0.5" aria-labelledby={`nav-${id}`}>{children}</ul>
  </div>;
}

/** The connection status in the sidebar footer (Ledger StatusIndicator look): the shared tone mark, then the label. */
export function ConnectionStatus({ tone, label, title }: { tone: Tone; label: string; title?: string }) {
  return <span className="connection" data-tone={tone} role="status" aria-live="polite" title={title}><i className="tone-mark" aria-hidden="true" data-tone={tone} /><span className="sidebar-label">{label}</span></span>;
}

export interface NavItemProps {
  icon: IconName;
  label: string;
  current: boolean;
  shortcut: string;
  count?: number;
  onSelect: () => void;
}

/** One view in the sidebar; the current one carries `aria-current="page"`. */
export function NavItem({ icon, label, current, shortcut, count, onSelect }: NavItemProps) {
  return <li>
    <button type="button" className="nav-item" aria-current={current ? "page" : undefined} aria-keyshortcuts={shortcut.replace("Ctrl", "Control")} title={`${label} (${shortcut})`} onClick={onSelect}>
      <Icon name={icon} /><span className="sidebar-label">{label}</span>
      {count !== undefined && count > 0 && <span className="nav-count" aria-label={`${count} active`}>{count}</span>}
    </button>
  </li>;
}

/** One view's content; every view stays mounted and only the current one is shown. */
export function ViewSlot({ hidden, entering, children }: { hidden: boolean; entering?: boolean; children: ReactNode }) {
  return <div className="view" hidden={hidden} data-entering={entering ? "" : undefined}>{children}</div>;
}
