// Keep supervisor ownership separate from the application under test. Never
// remove a process from cleanup evidence to make an application check pass.
export function nativeProcessEvidence(rows, { platform, expectedClient, supervisorPid, expectedSupervisor }) {
  const windows = platform === "win32";
  const same = (a, b) => typeof a === "string" && typeof b === "string" && (windows ? a.toLowerCase() === b.toLowerCase() : a === b);
  const isNode = (row) => /^(node|node.exe)$/i.test(row.name);
  const native = rows.find((row) => row.name.toLowerCase() === (windows ? "luuna-tui.exe" : "luuna-tui") && same(row.executable, expectedClient));
  if (!native) return { ok: false, reason: "The expected native client executable was not observed" };
  const supervisor = windows ? null : rows.find((row) => row.pid === supervisorPid);
  if (!windows && (!supervisor || !isNode(supervisor) || !same(supervisor.executable, expectedSupervisor) || native.parent !== supervisor.pid)) return { ok: false, reason: "The native client is not the direct child of the identified terminal supervisor" };
  if (rows.some((row) => isNode(row) && row.pid !== supervisor?.pid)) return { ok: false, reason: "A Node process was observed in the application tree" };
  return { ok: true, nativePid: native.pid, supervisorPid: supervisor?.pid ?? null, reason: windows ? "Native application tree contains no Node process" : "Only the identified terminal supervisor uses Node; its direct native client and descendants do not" };
}
