#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const directory = dirname(fileURLToPath(import.meta.url));
const formula = readFileSync(resolve(directory, '../Formula/luuna.rb'), 'utf8');
const doctor = readFileSync(resolve(directory, 'luuna-doctor'), 'utf8');

assert.match(formula, /url "https:\/\/github\.com\/luuna-ai\/luuna\.git", revision: "[0-9a-f]{40}"/, 'the source must be pinned to one commit');
assert.ok(!/resource "/.test(formula), 'components are native; the formula fetches no other repository');
const code = formula.split('\n').filter((line) => !line.trim().startsWith('#')).join('\n');
assert.ok(!/\bbun\b|depends_on "go"/i.test(code), 'the native build needs neither Bun nor Go');
for (const alias of ['lu', 'luuna', 'luuna-runtime', 'luuna-missions', 'lms', 'lm', 'luuna-atlas', 'lat', 'la', 'luuna-beacon', 'lbc', 'lb']) {
	assert.ok(doctor.includes(alias), `installation doctor must check ${alias}`);
	assert.ok(formula.includes(`"${alias}"`) || formula.includes(` ${alias} `) || formula.includes(`[${alias} `) || formula.includes(` ${alias}]`), `formula must install ${alias}`);
}
assert.ok(formula.includes('--locked'), 'the Cargo build must use the lockfile');
assert.ok(!formula.includes('TODO') && !formula.match(/sha256\s+"0{64}"/), 'formula cannot contain placeholder hashes');
console.log('Homebrew native formula: one pinned source, no Bun or component resources, every command installed and checked.');
