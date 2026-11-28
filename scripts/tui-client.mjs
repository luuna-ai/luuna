// Where the Rust terminal client is, and its command table, for the smoke and audit harnesses.
// The client is one executable (crates/luuna-tui); its command table is the data it embeds.
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repository = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** The development build of the client (honouring CARGO_TARGET_DIR), or the packaged executable beside `root`. */
export function clientBinary(root = repository, env = process.env) {
  const exe = process.platform === "win32" ? "luuna-tui.exe" : "luuna-tui";
  const target = path.resolve(root, env.CARGO_TARGET_DIR || "target");
  return path.join(target, "debug", exe);
}

export const tuiTable = JSON.parse(readFileSync(path.join(repository, "crates/luuna-tui/commands.json"), "utf8"));
/** Every command the command line reaches: the cataloged actions and the projections. */
export const commandsByName = new Map([...tuiTable.commands, ...tuiTable.projections].map((command) => [command.name, command]));
// The form's field order, as crates/luuna-tui/src/form.rs builds it: arguments, then flags in
// commands.json order, with the rarely set flags (form::ADVANCED) last. The prompt form moves
// its body argument to the end and adds the @document field before the advanced ones; the
// memory confirmations add @confirm last.
const ADVANCED = ["key", "id", "revision", "expected-version", "derived-from", "override", "task", "workers", "attempts", "timeout", "cost-usd", "check", "checks", "clear-labels"];
export function formFields(name, extras = []) {
  const command = commandsByName.get(name);
  const flags = Object.keys(command.flags ?? {}).filter((key) => key !== "acknowledge-hash");
  let fields = [...command.args.map((arg) => arg.name), ...flags];
  fields = [...fields.filter((f) => !ADVANCED.includes(f)), ...fields.filter((f) => ADVANCED.includes(f))];
  if (extras.includes("@document")) {
    fields = fields.filter((f) => f !== "body");
    const at = fields.findIndex((f) => ADVANCED.includes(f));
    fields.splice(at < 0 ? fields.length : at, 0, "@document");
    fields.push("body");
  }
  return [...fields, ...extras.filter((e) => e !== "@document")];
}
// A form's title is its catalog action ("Ticket creation").
export function formTitle(name) {
  const action = commandsByName.get(name).action;
  return new RegExp(`[╔═] ${action.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")} ═`);
}
export const commandsById = new Map(tuiTable.commands.map((command) => [command.id, command]));
