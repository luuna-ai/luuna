#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const directory = dirname(fileURLToPath(import.meta.url));
const root = resolve(directory, '../../..');
const formula = readFileSync(resolve(directory, '../Formula/luuna.rb'), 'utf8');
const doctor = readFileSync(resolve(directory, 'luuna-doctor'), 'utf8');
const staging = readFileSync(resolve(root, 'scripts/stage-components.mjs'), 'utf8');
const manifest = JSON.parse(readFileSync(resolve(root, 'components/manifest.json'), 'utf8'));
const pins = [
	['luuna', 'b25f3e5d6a5f1baff9897a73ff03df3ac00b8f84'],
	...manifest.components.map((component) => [component.path.split('/')[1], component.commit]),
];

for (const [name, commit] of pins) {
	assert.match(formula, new RegExp(`${name === 'luuna' ? 'url' : `resource "${name}" do[\\s\\S]*?url`} "https://github\\.com/luuna-ai/luuna${name === 'luuna' ? '' : `-${name}`}\\.git", revision: "${commit}"`), `${name} source must be pinned to manifest commit`);
}

const bunAssets = new Map([
	['bun-darwin-aarch64.zip', '80afd4c069b46a6fa8f3ec520bebc750a7843e4a8d5b9583d29ad436c00a3403'],
	['bun-darwin-x64.zip', '62660e6438eb872278877acca77e1152d5d031324df368d4a81b428211489d2d'],
	['bun-linux-aarch64.zip', 'efa9813da5ed72423bf847f916e8d2c47c0d776add972354026a75e10da9aa21'],
	['bun-linux-x64.zip', '3e730528d89775f03f87f05c06787c4df4099d0a37f3f252e7b829cb23816e18'],
]);
for (const [asset, digest] of bunAssets) {
	assert.ok(formula.includes(asset), `missing platform asset ${asset}`);
	assert.ok(formula.includes(digest), `missing verified SHA-256 for ${asset}`);
}

for (const alias of ['lu', 'luuna', 'luuna-runtime', 'luuna-missions', 'lms', 'lm', 'luuna-atlas', 'lat', 'la', 'luuna-beacon', 'lbc', 'lb']) {
	assert.ok(doctor.includes(alias), `installation doctor must check ${alias}`);
}
assert.ok(formula.includes('components/dist-manifest.json'), 'formula test must verify portable component layout');
assert.ok(formula.includes('stage-homebrew'), 'formula must use the shared portable-layout stager');
assert.ok(staging.includes('sourceDirectories') && staging.includes('treeDigest(destination)'), 'source staging must use the runtime-tested digest implementation');
assert.ok(formula.includes('GOTOOLCHAIN') && formula.includes('GOSUMDB'), 'Go toolchain and module verification must fail closed');
assert.ok(formula.includes('bun-v1.4.3'), 'Bun runtime must be version pinned');
assert.ok(formula.includes('opt_prefix'), 'runtime wrappers must survive Homebrew upgrades');
assert.ok(!formula.includes('TODO') && !formula.match(/sha256\s+"0{64}"/), 'formula cannot contain placeholder hashes');
console.log(`Homebrew source pins, Bun checksums, aliases, and portable-layout checks passed for ${pins.length} repositories.`);
