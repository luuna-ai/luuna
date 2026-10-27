import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  DIST_KIND, DIST_MANIFEST, DIST_SCHEMA_VERSION, copyTree, scriptPath, stageComponents, treeDigest, verifyStaged,
} from './stage-components.mjs';

function scratch(check) {
  const root = mkdtempSync(join(tmpdir(), 'luuna-stage-components-'));
  try { return check(root); } finally { rmSync(root, { recursive: true, force: true }); }
}

test('tree digests are deterministic, order-independent and sensitive to every byte and link', () => scratch((root) => {
  const a = join(root, 'a');
  const b = join(root, 'b');
  for (const dir of [a, b]) {
    mkdirSync(join(dir, 'node_modules/dep'), { recursive: true });
    mkdirSync(join(dir, 'empty'), { recursive: true });
    writeFileSync(join(dir, 'package.json'), '{"name":"x"}');
    writeFileSync(join(dir, 'node_modules/dep/index.js'), 'module.exports = 1;\n');
  }
  // Written in another order and with an extra empty directory: the same digest.
  mkdirSync(join(b, 'another-empty'));
  const first = treeDigest(a);
  assert.equal(first.digest, treeDigest(b).digest);
  assert.match(first.digest, /^sha256:[0-9a-f]{64}$/);
  assert.equal(first.files, 2);
  assert.equal(first.entries, 2);
  assert.equal(first.bytes, 12 + 20);
  writeFileSync(join(b, 'node_modules/dep/index.js'), 'module.exports = 2;\n');
  assert.notEqual(treeDigest(b).digest, first.digest);
  writeFileSync(join(b, 'node_modules/dep/index.js'), 'module.exports = 1;\n');
  assert.equal(treeDigest(b).digest, first.digest);
  if (process.platform !== 'win32') {
    symlinkSync('dep/index.js', join(b, 'node_modules/shortcut'));
    const linked = treeDigest(b);
    assert.notEqual(linked.digest, first.digest);
    assert.equal(linked.entries, 3);
    assert.equal(linked.files, 2);
    rmSync(join(b, 'node_modules/shortcut'));
    symlinkSync('dep/other.js', join(b, 'node_modules/shortcut'));
    assert.notEqual(treeDigest(b).digest, linked.digest, 'the link target is part of the digest');
  }
  // The command-line form prints exactly the digest.
  assert.equal(execFileSync(process.execPath, [scriptPath, 'digest', a], { encoding: 'utf8' }).trim(), first.digest);
}));

test('copying a checkout leaves its Git link behind and nothing else', () => scratch((root) => {
  const source = join(root, 'source');
  mkdirSync(join(source, 'src'), { recursive: true });
  mkdirSync(join(source, 'node_modules/.bin'), { recursive: true });
  writeFileSync(join(source, '.git'), 'gitdir: ../../.git/modules/components/atlas\n');
  writeFileSync(join(source, '.gitignore'), 'node_modules\n');
  writeFileSync(join(source, 'src/.git'), 'a file that merely shares the name stays');
  writeFileSync(join(source, 'node_modules/.bin/tool'), '#!/bin/sh\n');
  const destination = join(root, 'staged');
  copyTree(source, destination);
  assert.deepEqual(readFileSync(join(destination, '.gitignore'), 'utf8'), 'node_modules\n');
  assert.ok(!readFileSync(join(destination, 'src/.git'), 'utf8').includes('gitdir'));
  assert.throws(() => readFileSync(join(destination, '.git')), /ENOENT/);
  assert.equal(readFileSync(join(destination, 'node_modules/.bin/tool'), 'utf8'), '#!/bin/sh\n');
}));

const pins = {
  missions: { brand: 'Luuna Missions', version: '0.6.0', entrypoint: 'src/index.ts', short: 'lms', legacy: [] },
  atlas: { brand: 'Luuna Atlas', version: '0.11.0', entrypoint: 'src/cli.ts', short: 'lat', legacy: [] },
  beacon: { brand: 'Luuna Beacon', version: '0.2.6', entrypoint: 'src/index.ts', short: 'lbc', legacy: [] },
};

/** A synthetic Luuna checkout: three pinned component submodules and a Bun stand-in. */
function checkout(root) {
  const git = (cwd, ...args) => execFileSync('git', ['-c', 'core.hooksPath=', '-c', 'core.autocrlf=false',
    '-c', 'commit.gpgsign=false', '-c', 'user.name=Luuna fixture', '-c', 'user.email=fixture@example.invalid', ...args],
  { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  git(root, 'init', '--template=');
  const components = [];
  let modules = '';
  for (const [name, pin] of Object.entries(pins)) {
    const directory = join(root, 'components', name);
    mkdirSync(join(directory, 'src'), { recursive: true });
    git(directory, 'init', '--template=');
    const bin = Object.fromEntries([`luuna-${name}`, pin.short, ...pin.legacy].map((alias) => [alias, pin.entrypoint]));
    writeFileSync(join(directory, 'package.json'), JSON.stringify({ name: `@luuna/${name}-cli`, version: pin.version, bin }));
    writeFileSync(join(directory, pin.entrypoint), `console.log(JSON.stringify({ success: true, component: "${name}" }));\n`);
    writeFileSync(join(directory, '.gitignore'), 'node_modules\n');
    git(directory, 'add', '.');
    git(directory, 'commit', '-m', 'Synthetic component');
    const commit = git(directory, 'rev-parse', 'HEAD');
    modules += `[submodule "components/${name}"]\n\tpath = components/${name}\n\turl = https://github.com/luuna-ai/luuna-${name}.git\n`;
    git(root, 'update-index', '--add', '--cacheinfo', `160000,${commit},components/${name}`);
    components.push({
      brand: pin.brand, path: `components/${name}`, sourceVersion: pin.version, package: `@luuna/${name}-cli`,
      cli: { primary: `luuna-${name}`, short: pin.short, legacy: pin.legacy, entrypoint: pin.entrypoint },
      license: 'MIT', published: false, repository: `https://github.com/luuna-ai/luuna-${name}.git`, commit,
    });
  }
  writeFileSync(join(root, '.gitmodules'), modules);
  writeFileSync(join(root, 'components/manifest.json'), `${JSON.stringify({ schemaVersion: 2, components, runtime: { name: 'bun', version: '1.4.3' } }, null, 2)}\n`);
  git(root, 'add', '.gitmodules', 'components/manifest.json');
  git(root, 'commit', '-m', 'Pin synthetic components');
  // The Bun stand-in answers --version with the pinned version and "installs" a dependency tree.
  const bun = join(root, 'fake-bun');
  writeFileSync(bun, '#!/bin/sh\nif [ "$1" = "--version" ]; then echo 1.4.3; exit 0; fi\nmkdir -p node_modules/dep && echo "$@" > node_modules/dep/installed.txt\n');
  chmodSync(bun, 0o755);
  return { bun, components, git };
}

test('staging installs with the frozen script-free command, copies every pin and writes a verifiable manifest', { skip: process.platform === 'win32' && 'the Bun stand-in is a POSIX shell script' }, () => scratch((root) => {
  const source = join(root, 'checkout');
  mkdirSync(source);
  const { bun, components } = checkout(source);
  const out = join(root, 'stage');
  const dist = stageComponents({ out, root: source, bun });
  assert.equal(dist.schemaVersion, DIST_SCHEMA_VERSION);
  assert.equal(dist.kind, DIST_KIND);
  assert.equal(dist.runtime.version, '1.4.3');
  assert.equal(dist.runtime.executable, 'bun/bun');
  assert.match(dist.runtime.sha256, /^[0-9a-f]{64}$/);
  assert.match(dist.source.luunaCommit, /^[0-9a-f]{40}$/);
  assert.deepEqual(dist.components.map((c) => c.name), ['missions', 'atlas', 'beacon']);
  for (const [index, component] of components.entries()) {
    const staged = dist.components[index];
    assert.equal(staged.commit, component.commit);
    assert.equal(staged.path, component.path);
    assert.equal(staged.package, component.package);
    // The install ran in the source checkout with exactly the setup arguments, then the tree was copied.
    const installed = join(out, component.path, 'node_modules/dep/installed.txt');
    assert.equal(readFileSync(installed, 'utf8'), 'install --frozen-lockfile --ignore-scripts\n');
    assert.throws(() => readFileSync(join(out, component.path, '.git')), /ENOENT/);
    assert.equal(treeDigest(join(out, component.path)).digest, staged.digest);
  }
  const written = JSON.parse(readFileSync(join(out, DIST_MANIFEST), 'utf8'));
  assert.deepEqual(written, dist);
  assert.equal(readFileSync(join(out, 'components/manifest.json'), 'utf8'), readFileSync(join(source, 'components/manifest.json'), 'utf8'));
  assert.deepEqual(verifyStaged(out), written);
  assert.match(execFileSync(process.execPath, [scriptPath, 'verify', out], { encoding: 'utf8' }), /atlas: [0-9a-f]{40} sha256:[0-9a-f]{64} verified/);

  // Tampering with a staged file, a pin or the Bun executable is refused by name.
  writeFileSync(join(out, 'components/atlas/src/cli.ts'), 'console.log("changed");\n');
  assert.throws(() => verifyStaged(out), /Luuna Atlas: staged tree digest/);
  copyTree(join(source, 'components/atlas'), join(out, 'components/atlas'));
  assert.deepEqual(verifyStaged(out), written);
  rmSync(join(out, 'components/beacon'), { recursive: true });
  assert.throws(() => verifyStaged(out), /Luuna Beacon: staged tree missing/);
  copyTree(join(source, 'components/beacon'), join(out, 'components/beacon'));
  writeFileSync(join(out, 'bun/bun'), '#!/bin/sh\necho 1.4.3\n');
  assert.throws(() => verifyStaged(out), /Bun executable differs/);
}));

test('staging refuses a checkout whose pins and gitlinks disagree before copying anything', { skip: process.platform === 'win32' && 'the Bun stand-in is a POSIX shell script' }, () => scratch((root) => {
  const source = join(root, 'checkout');
  mkdirSync(source);
  const { bun, components } = checkout(source);
  const manifestPath = join(source, 'components/manifest.json');
  const data = JSON.parse(readFileSync(manifestPath, 'utf8'));
  data.components[1].commit = '0'.repeat(40);
  writeFileSync(manifestPath, JSON.stringify(data));
  const out = join(root, 'stage');
  assert.throws(() => stageComponents({ out, root: source, bun }), /gitlink and manifest pin differ/);
  assert.throws(() => readFileSync(join(out, DIST_MANIFEST)), /ENOENT/);
  assert.ok(components.length === 3);
}));
