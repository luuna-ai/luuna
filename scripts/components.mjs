import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync, existsSync } from 'node:fs';
import { delimiter, dirname, isAbsolute, resolve, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const componentNames = ['missions', 'atlas', 'beacon'];
const git = (cwd, args) => execFileSync('git', args, { cwd, encoding: 'utf8', timeout: 30_000 }).trim();
export function manifest(root = repositoryRoot) {
  const data = JSON.parse(readFileSync(join(root, 'components/manifest.json'), 'utf8'));
  const paths = data.components?.map(component => component.path).sort();
  if (data.schemaVersion !== 2 || data.runtime?.name !== 'bun' ||
      !/^\d+\.\d+\.\d+$/.test(data.runtime?.version ?? '') ||
      JSON.stringify(paths) !== JSON.stringify(componentNames.map(name => `components/${name}`).sort())) {
    throw new Error('Manifest must pin exactly Missions, Atlas and Beacon with a versioned Bun runtime');
  }
  return data;
}
function verifyPinMetadata(root, component) {
  const expectedPath = `components/${component.brand.replace('Luuna ', '').toLowerCase()}`;
  if (!componentNames.includes(expectedPath.split('/')[1]) || component.path !== expectedPath ||
      !/^[a-f0-9]{40}$/.test(component.commit) ||
      component.repository !== `https://github.com/luuna-ai/luuna-${expectedPath.split('/')[1]}.git`) {
    throw new Error('Invalid component identity or immutable commit in manifest');
  }
  const entry = git(root, ['ls-files', '--stage', '--', component.path]);
  if (entry !== `160000 ${component.commit} 0\t${component.path}`) throw new Error(`${component.brand}: gitlink and manifest pin differ`);
  const url = git(root, ['config', '-f', '.gitmodules', '--get', `submodule.${component.path}.url`]);
  if (url !== component.repository) throw new Error(`${component.brand}: submodule URL and manifest differ`);
}
export function preflightComponent(root, component) {
  verifyPinMetadata(root, component);
  const directory = resolve(root, component.path);
  if (existsSync(join(directory, '.git')) && git(directory, ['status', '--porcelain', '--untracked-files=all'])) {
    throw new Error(`${component.brand}: component checkout has local changes; preserve them before setup`);
  }
}
export function verifyComponent(root, component, installed = false) {
  const directory = resolve(root, component.path);
  if (!existsSync(join(directory, '.git'))) throw new Error(`${component.brand}: submodule missing; run npm run components:setup`);
  verifyPinMetadata(root, component);
  if (git(directory, ['rev-parse', 'HEAD']) !== component.commit) throw new Error(`${component.brand}: checkout differs from pinned commit`);
  if (git(directory, ['status', '--porcelain', '--untracked-files=all'])) throw new Error(`${component.brand}: component checkout has local changes`);
  const pkg = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'));
  if (pkg.name !== component.package || pkg.version !== component.sourceVersion ||
      pkg.bin[component.cli.primary] !== component.cli.entrypoint ||
      pkg.bin[component.cli.short] !== component.cli.entrypoint ||
      component.cli.legacy.some(alias => pkg.bin[alias] !== component.cli.entrypoint)) {
    throw new Error(`${component.brand}: incompatible package or CLI contract`);
  }
  if (installed && !existsSync(join(directory, 'node_modules'))) throw new Error(`${component.brand}: dependencies missing; run npm run components:setup`);
  return directory;
}
export function bunExecutable(root = repositoryRoot) {
  const platform = process.platform === 'win32' ? 'windows' : process.platform;
  const local = join(root, '.tools/bun/node_modules/@oven', `bun-${platform}-${process.arch}`,
    'bin', process.platform === 'win32' ? 'bun.exe' : 'bun');
  let bun = process.env.LUUNA_BUN ?? 'bun';
  let version;
  try {
    version = execFileSync(bun, ['--version'], { encoding: 'utf8', timeout: 10_000 }).trim();
  } catch (error) {
    if (process.env.LUUNA_BUN || error.code !== 'ENOENT' || !existsSync(local)) throw error;
    bun = local;
    version = execFileSync(bun, ['--version'], { encoding: 'utf8', timeout: 10_000 }).trim();
  }
  if (version !== manifest(root).runtime.version) throw new Error(`Use Bun ${manifest(root).runtime.version}; found ${version}`);
  // The runtime resolves a bare name on PATH without PATHEXT, so hand it the absolute executable.
  if (!isAbsolute(bun)) {
    const extensions = process.platform === 'win32' ? (process.env.PATHEXT ?? '.EXE;.CMD;.BAT').split(';').map((e) => e.toLowerCase()) : [''];
    for (const dir of (process.env.PATH ?? '').split(delimiter)) {
      for (const ext of [''].concat(extensions)) {
        const candidate = join(dir, bun + ext);
        if (dir && existsSync(candidate)) return candidate;
      }
    }
  }
  return bun;
}
export function nativeExecutable(root = repositoryRoot, rebuild = false) {
  const binary = join(root, 'target', 'debug', process.platform === 'win32' ? 'luuna-tui.exe' : 'luuna-tui');
  if (rebuild || !existsSync(binary)) {
    execFileSync('cargo', ['build', '--locked', '-p', 'luuna-tui'], { cwd: root, stdio: 'inherit' });
  }
  return binary;
}
function nativeName(name) {
  const normalized = name?.replace(/^luuna-/, '');
  if (!componentNames.includes(normalized)) throw new Error(`Unknown component: ${name}`);
  return normalized;
}
export function runComponent(name, args, options = {}) {
  const binary = nativeExecutable(options.root ?? repositoryRoot);
  return execFileSync(binary, [nativeName(name), ...args], {
    cwd: options.cwd ?? process.cwd(), env: options.env ?? process.env,
    encoding: 'utf8', timeout: 30_000, maxBuffer: 4 * 1024 * 1024,
  });
}
export function setupComponents(root = repositoryRoot) {
  const data = manifest(root);
  // Validate every pin, URL and existing worktree before any checkout or install.
  // This also stops Git from carrying a dirty tree to a different pinned revision.
  for (const component of data.components) preflightComponent(root, component);
  const bun = bunExecutable(root);
  const paths = data.components.map(component => component.path);
  execFileSync('git', ['submodule', 'sync', '--', ...paths], { cwd: root, stdio: 'inherit' });
  execFileSync('git', ['submodule', 'update', '--init', '--checkout', '--', ...paths], { cwd: root, stdio: 'inherit' });
  for (const component of data.components) {
    const directory = verifyComponent(root, component);
    const hooksBefore = spawnSync('git', ['config', '--get', 'core.hooksPath'], { cwd: root, encoding: 'utf8' }).stdout;
    execFileSync(bun, ['install', '--frozen-lockfile', '--ignore-scripts'], { cwd: directory, stdio: 'inherit' });
    const hooksAfter = spawnSync('git', ['config', '--get', 'core.hooksPath'], { cwd: root, encoding: 'utf8' }).stdout;
    if (hooksBefore !== hooksAfter) throw new Error('Component installation changed root Git hooks');
    verifyComponent(root, component, true);
  }
}
function main() {
  const [action, name, ...args] = process.argv.slice(2);
  if (action === 'setup' || action === 'verify') {
    const binary = nativeExecutable(repositoryRoot, true);
    for (const component of componentNames) {
      execFileSync(binary, [component, '--help'], { encoding: 'utf8', timeout: 30_000 });
      console.log(`${component}: native Rust`);
    }
  } else if (action === 'audit-legacy') {
    for (const component of manifest().components) verifyComponent(repositoryRoot, component, true);
  } else if (action === 'setup-legacy') {
    setupComponents();
  } else if (action === 'run') {
    const child = spawnSync(nativeExecutable(), [nativeName(name),
      ...(args[0] === '--' ? args.slice(1) : args)], {
      cwd: process.env.INIT_CWD ?? process.cwd(), env: process.env, stdio: 'inherit',
    });
    if (child.error) throw child.error;
    if (child.signal) process.kill(process.pid, child.signal);
    process.exitCode = child.status ?? 1;
  } else {
    throw new Error('Usage: node scripts/components.mjs setup|verify|run <luuna-missions|luuna-atlas|luuna-beacon> -- <CLI arguments>');
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
