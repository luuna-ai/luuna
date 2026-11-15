import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { nativeExecutable, repositoryRoot, runComponent } from './components.mjs';

// Compile before entering synthetic HOME environments; commands reuse this binary.
nativeExecutable(repositoryRoot, true);

// Exercise the compiled native CLIs against fresh, test-owned stores. No user data is read.
const fixture = mkdtempSync(join(tmpdir(), 'luuna-components-'));
let checks = 0;
try {
  const home = join(fixture, 'home');
  const project = join(fixture, 'project');
  mkdirSync(home); mkdirSync(project);
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
    /^(PATH|SystemRoot|WINDIR|COMSPEC|PATHEXT|TEMP|TMP|TMPDIR|LUUNA_BUN)$/i.test(key)));
  Object.assign(env, { HOME: home, USERPROFILE: home, XDG_CONFIG_HOME: home,
    NO_COLOR: '1', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: join(home, 'gitconfig'),
    GIT_TERMINAL_PROMPT: '0' });
  execFileSync('git', ['-c', 'core.hooksPath=', 'init', '--template=', project], { env, stdio: 'pipe' });
  const json = (name, args) => {
    const result = JSON.parse(runComponent(`luuna-${name}`, [...args, '--json'], { cwd: project, env }));
    assert.equal(result.success, true, `${name} ${args[0]} failed`);
    checks++;
    return result;
  };
  json('missions', ['init']);
  const ticket = json('missions', ['create', '--title', 'Verify extracted component consumption']);
  const shown = json('missions', ['show', ticket.id]);
  assert.ok(JSON.stringify(shown).includes('Verify extracted component consumption'));
  json('missions', ['list']);
  json('atlas', ['init']);
  json('atlas', ['add', 'extraction']);
  json('atlas', ['record', 'extraction', 'Pinned components preserve the CLI contract', '--type', 'convention']);
  const records = json('atlas', ['search', 'Pinned components']);
  assert.ok(JSON.stringify(records).includes('Pinned components preserve the CLI contract'));
  json('beacon', ['init']);
  json('beacon', ['create', '--name', 'migration-check', '--section', 'purpose=Preserve Luuna runtime approval policy']);
  const rendered = json('beacon', ['render', 'migration-check']);
  assert.ok(JSON.stringify(rendered).includes('Preserve Luuna runtime approval policy'));
  json('beacon', ['list']);
  const launcher = join(repositoryRoot, 'scripts/components.mjs');
  const piped = spawnSync(process.execPath, [launcher, 'run', 'luuna-atlas', '--', 'record', 'extraction', '--stdin', '--json'], {
    // npm runs scripts from the package root, keeping the caller in INIT_CWD.
    cwd: repositoryRoot, env: { ...env, INIT_CWD: project },
    input: JSON.stringify({ type: 'convention', content: 'CLI stdin reaches the extracted Atlas' }),
    encoding: 'utf8', timeout: 30_000,
  });
  assert.equal(piped.status, 0, piped.stderr);
  assert.equal(JSON.parse(piped.stdout).success, true);
  checks++;
  const invalid = spawnSync(process.execPath, [launcher, 'run', 'luuna-missions', '--', 'deliberately-unknown-command', '--json'], {
    cwd: project, env, encoding: 'utf8', timeout: 30_000,
  });
  assert.equal(invalid.status, 1);
  assert.equal(JSON.parse(invalid.stdout).success, false);
  checks++;
  console.log(`${checks}/14 real component CLI operations passed (${process.platform}).`);
} finally {
  // Only the mkdtemp-owned fixture is removed, even when a CLI fails.
  rmSync(fixture, { recursive: true, force: true });
}
