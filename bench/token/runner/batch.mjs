#!/usr/bin/env node
// Several trials, one after another, stacks interleaved so a slow afternoon
// or a usage limit hits both alike.
//
//   node runner/batch.mjs --stacks ordo,supabase --trials 3 [--dry] [--pause 0]
//
// Every other option goes through to trial.mjs. Trials never overlap: they
// share ports, and one plan's usage allowance.

import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
const take = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  if (i < 0) return fallback;
  const v = args[i + 1];
  args.splice(i, 2);
  return v;
};
const stacks = take('stacks', 'ordo,supabase').split(',').filter(Boolean);
const trials = Number(take('trials', '1'));
const pause = Number(take('pause', '0'));
const trial = join(dirname(fileURLToPath(import.meta.url)), 'trial.mjs');

let failed = 0;
for (let i = 1; i <= trials; i++) {
  for (const stack of stacks) {
    console.log(`\n=== trial ${i} of ${trials}, ${stack} ===`);
    const r = spawnSync(process.execPath, [trial, '--stack', stack, ...args], { stdio: 'inherit' });
    if (r.status !== 0) failed++;
    if (pause > 0) await new Promise((res) => setTimeout(res, pause * 60_000));
  }
}
console.log(`\nbatch done: ${trials * stacks.length} trials, ${failed} runner errors. Score: node runner/score.mjs`);
