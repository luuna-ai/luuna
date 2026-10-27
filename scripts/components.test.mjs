import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { manifest, preflightComponent, verifyComponent } from './components.mjs';

function fixture(check) {
  const root = mkdtempSync(join(tmpdir(), 'luuna-component-pins-'));
  const directory = join(root, 'components/atlas');
  const git = (cwd, ...args) => execFileSync('git', ['-c', 'core.hooksPath=', '-c', 'core.autocrlf=false',
    '-c', 'commit.gpgsign=false', '-c', 'user.name=Luuna fixture', '-c', 'user.email=fixture@example.invalid', ...args],
  { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  try {
    mkdirSync(directory, { recursive: true });
    git(root, 'init', '--template='); git(directory, 'init', '--template=');
    writeFileSync(join(directory, 'package.json'), JSON.stringify({ name: '@luuna/atlas-cli', version: '0.11.0',
      bin: { 'luuna-atlas': 'src/cli.ts', lat: 'src/cli.ts' } }));
    git(directory, 'add', 'package.json'); git(directory, 'commit', '-m', 'Synthetic pin fixture');
    const component = { brand: 'Luuna Atlas', path: 'components/atlas', sourceVersion: '0.11.0',
      package: '@luuna/atlas-cli', repository: 'https://github.com/luuna-ai/luuna-atlas.git',
      commit: git(directory, 'rev-parse', 'HEAD'), cli: { primary: 'luuna-atlas', short: 'lat',
        legacy: [], entrypoint: 'src/cli.ts' } };
    writeFileSync(join(root, '.gitmodules'), '[submodule "components/atlas"]\npath = components/atlas\nurl = https://github.com/luuna-ai/luuna-atlas.git\n');
    git(root, 'update-index', '--add', '--cacheinfo', `160000,${component.commit},components/atlas`);
    check({ root, directory, component, git });
  } finally { rmSync(root, { recursive: true, force: true }); }
}
test('accepts a clean checkout at the exact gitlink and manifest pin', () => fixture(({ root, directory, component }) => {
  assert.equal(verifyComponent(root, component), directory);
}));
test('rejects a missing checkout with setup instructions', () => fixture(({ root, directory, component }) => {
  rmSync(join(directory, '.git'), { recursive: true });
  assert.throws(() => verifyComponent(root, component), /submodule missing/);
}));
test('rejects a changed manifest pin and an unapproved checkout commit', () => fixture(({ root, directory, component, git }) => {
  assert.throws(() => verifyComponent(root, { ...component, commit: '0'.repeat(40) }), /gitlink and manifest pin differ/);
  writeFileSync(join(directory, 'new.txt'), 'manual edit');
  git(directory, 'add', 'new.txt'); git(directory, 'commit', '-m', 'Unapproved revision');
  assert.throws(() => verifyComponent(root, component), /checkout differs from pinned commit/);
}));
test('rejects local edits and mismatched repository URLs', () => fixture(({ root, directory, component }) => {
  writeFileSync(join(directory, 'untracked.txt'), 'local change');
  assert.throws(() => verifyComponent(root, component), /local changes/);
  rmSync(join(directory, 'untracked.txt'));
  writeFileSync(join(root, '.gitmodules'), '[submodule "components/atlas"]\nurl = https://github.com/other/atlas.git\n');
  assert.throws(() => verifyComponent(root, component), /URL and manifest differ/);
}));
test('rejects changed CLI contracts and missing dependencies', () => fixture(({ root, directory, component }) => {
  assert.throws(() => verifyComponent(root, component, true), /dependencies missing/);
  assert.throws(() => verifyComponent(root, { ...component, cli: { ...component.cli, primary: 'wrong-cli' } }), /incompatible package or CLI contract/);
}));
test('setup preflight refuses dirty source before changing checkout or configuration', () => fixture(({ root, directory, component, git }) => {
  const headBefore = git(directory, 'rev-parse', 'HEAD');
  writeFileSync(join(directory, 'package.json'), 'local source edit');
  assert.throws(() => preflightComponent(root, component), /local changes; preserve them before setup/);
  assert.equal(git(directory, 'rev-parse', 'HEAD'), headBefore);
}));
test('setup preflight rejects inconsistent pins even before a checkout is initialized', () => fixture(({ root, directory, component }) => {
  rmSync(join(directory, '.git'), { recursive: true });
  assert.doesNotThrow(() => preflightComponent(root, component));
  assert.throws(() => preflightComponent(root, { ...component, commit: '0'.repeat(40) }), /gitlink and manifest pin differ/);
}));
test('manifest refuses missing or duplicated components instead of a partial success', () => fixture(({ root, component }) => {
  const path = join(root, 'components/manifest.json');
  const data = { schemaVersion: 2, runtime: { name: 'bun', version: '1.4.3' }, components: [] };
  writeFileSync(path, JSON.stringify(data));
  assert.throws(() => manifest(root), /exactly Missions, Atlas and Beacon/);
  data.components = [component, component, component];
  writeFileSync(path, JSON.stringify(data));
  assert.throws(() => manifest(root), /exactly Missions, Atlas and Beacon/);
}));
