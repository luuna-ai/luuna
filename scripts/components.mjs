import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, resolve, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const componentNames = ['missions', 'atlas', 'beacon'];
export function nativeExecutable(root = repositoryRoot, rebuild = false) {
  const target = process.env.CARGO_TARGET_DIR ? resolve(root, process.env.CARGO_TARGET_DIR) : join(root, 'target');
  const binary = join(target, 'debug', process.platform === 'win32' ? 'luuna-tui.exe' : 'luuna-tui');
  if (rebuild) {
    execFileSync('cargo', ['build', '--locked', '-p', 'luuna-tui'], { cwd: root, stdio: 'inherit' });
  }
  if (!existsSync(binary)) throw new Error('Native component binary missing; run npm run components:setup first');
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
function main() {
  const [action, name, ...args] = process.argv.slice(2);
  if (action === 'setup' || action === 'verify') {
    const binary = nativeExecutable(repositoryRoot, true);
    for (const component of componentNames) {
      execFileSync(binary, [component, '--help'], { encoding: 'utf8', timeout: 30_000 });
      console.log(`${component}: native Rust`);
    }
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
