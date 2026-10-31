#!/usr/bin/env node
// Score a batch: plan §5's measures per stack, and §1's win test.
//
//   node runner/score.mjs [runs/results.jsonl] [--include-dry] [--json]
//
// Tokens stand in for dollars (owner, 2026-10-29: the runs use the plan's
// allowance). The list-price dollars Claude Code estimates are shown beside
// them; list price decides the win (owner, 2026-10-30).

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { BENCH } from './lib/paths.mjs';

const argv = process.argv.slice(2);
const file = argv.find((a) => !a.startsWith('--')) ?? join(BENCH, 'runs', 'results.jsonl');
const includeDry = argv.includes('--include-dry');
const asJson = argv.includes('--json');

const records = readFileSync(file, 'utf8')
  .split('\n')
  .filter((l) => l.trim())
  .map((l) => JSON.parse(l))
  .filter((r) => r.schema === 'bench.token.trial/1' && (includeDry || !r.dry));

const median = (xs) => {
  if (xs.length === 0) return null;
  const s = [...xs].sort((a, b) => a - b);
  const m = Math.floor(s.length / 2);
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};
const sum = (xs) => xs.reduce((a, b) => a + b, 0);

/** Unbiased pass@k over n trials with c passes. */
function passAtK(n, c, k) {
  if (k > n) return null;
  if (n - c < k) return 1;
  let p = 1;
  for (let i = n - c + 1; i <= n; i++) p *= 1 - k / i;
  return 1 - p;
}

/** A seeded generator, so a report is the same every time it is made. */
function rng(seed) {
  let s = seed >>> 0;
  return () => ((s = (s * 1664525 + 1013904223) >>> 0) / 2 ** 32);
}

/** Bootstrap 95% interval of `stat` over trials (2,000 resamples). */
function bootstrap(trials, stat) {
  if (trials.length === 0) return null;
  const rand = rng(20261029);
  const out = [];
  for (let i = 0; i < 2000; i++) {
    const sample = trials.map(() => trials[Math.floor(rand() * trials.length)]);
    const v = stat(sample);
    if (v !== null && Number.isFinite(v)) out.push(v);
  }
  if (out.length === 0) return null;
  out.sort((a, b) => a - b);
  return [out[Math.floor(out.length * 0.025)], out[Math.floor(out.length * 0.975) - 1] ?? out[out.length - 1]];
}

/** All tokens spent in the cell, over the trials that passed. */
const perPass = (key) => (ts) => {
  const passes = ts.filter((t) => t.pass).length;
  return passes ? sum(ts.map((t) => t.tokens[key])) / passes : null;
};
const costPerPass = (ts) => {
  const passes = ts.filter((t) => t.pass).length;
  return passes ? sum(ts.map((t) => t.cost_usd_list_price ?? 0)) / passes : null;
};

const cells = new Map();
for (const r of records) {
  const key = `${r.stack} · ${r.models ? `${r.models.plan} then ${r.models.build}` : 'dry'}`;
  if (!cells.has(key)) cells.set(key, []);
  cells.get(key).push(r);
}

const rows = [];
for (const [cell, ts] of cells) {
  const n = ts.length;
  const c = ts.filter((t) => t.pass).length;
  const permPassed = sum(ts.map((t) => t.perm?.passed ?? 0));
  const permTotal = sum(ts.map((t) => t.perm?.total ?? 0));
  rows.push({
    cell,
    stack: ts[0].stack,
    trials: n,
    passes: c,
    pass_rate: c / n,
    pass_rate_95: bootstrap(ts, (s) => s.filter((t) => t.pass).length / s.length),
    perm_pass_rate: permTotal ? permPassed / permTotal : null,
    perm_all_pass_trials: ts.filter((t) => t.perm && t.perm.total > 0 && t.perm.passed === t.perm.total).length,
    tokens_per_passing_app: perPass('total')(ts),
    tokens_per_passing_app_95: bootstrap(ts, perPass('total')),
    output_tokens_per_passing_app: perPass('output')(ts),
    median_tokens_per_trial: {
      input: median(ts.map((t) => t.tokens.input)),
      output: median(ts.map((t) => t.tokens.output)),
      cache_read: median(ts.map((t) => t.tokens.cache_read)),
      cache_write: median(ts.map((t) => t.tokens.cache_write)),
      total: median(ts.map((t) => t.tokens.total)),
    },
    tokens_after_first_failure_share: sum(ts.map((t) => t.tokens.total))
      ? sum(ts.map((t) => t.tokens_after_first_failure.total)) / sum(ts.map((t) => t.tokens.total))
      : null,
    list_price_usd_per_passing_app: costPerPass(ts),
    list_price_usd_per_passing_app_95: bootstrap(ts, costPerPass),
    pass_at_1: passAtK(n, c, 1),
    pass_at_5: passAtK(n, c, 5),
    median_turns: median(ts.map((t) => t.turns)),
    mean_repair_rounds: sum(ts.map((t) => t.repair_rounds)) / n,
    median_wall_minutes: median(ts.map((t) => t.wall_ms / 60_000)),
    cap_hits: ts.filter((t) => t.cap_hit).length,
  });
}

// §1: Ordo wins only at no more than half the best baseline's cost per
// passing app, intervals apart, with a permission pass rate no lower.
function verdict(measure, interval) {
  const ordo = rows.find((r) => r.stack === 'ordo');
  const base = rows.filter((r) => r.stack !== 'ordo' && r[measure] !== null);
  if (!ordo || ordo[measure] === null || base.length === 0) return 'not enough passing trials to compare';
  const best = base.reduce((a, b) => (b[measure] < a[measure] ? b : a));
  const half = ordo[measure] <= best[measure] / 2;
  const apart = ordo[interval] && best[interval] ? ordo[interval][1] < best[interval][0] : false;
  const perm = (ordo.perm_pass_rate ?? 0) >= (best.perm_pass_rate ?? 0);
  const ratio = (ordo[measure] / best[measure]).toFixed(2);
  return `${half && apart && perm ? 'WIN' : 'NO WIN'} against ${best.cell}: ratio ${ratio} (needs 0.50 or less), intervals ${apart ? 'apart' : 'overlap'}, permission rate ${perm ? 'no lower' : 'lower'}`;
}

const report = {
  file,
  trials: records.length,
  rows,
  verdict_tokens: verdict('tokens_per_passing_app', 'tokens_per_passing_app_95'),
  verdict_list_price: verdict('list_price_usd_per_passing_app', 'list_price_usd_per_passing_app_95'),
};

if (asJson) {
  console.log(JSON.stringify(report, null, 2));
} else {
  const f = (v, d = 0) => (v === null || v === undefined ? '-' : typeof v === 'number' ? v.toLocaleString('en-US', { maximumFractionDigits: d }) : v);
  const ci = (v, d = 0) => (v ? `${f(v[0], d)} to ${f(v[1], d)}` : '-');
  console.log(`${records.length} trials from ${file}${includeDry ? ' (dry runs included)' : ''}\n`);
  for (const r of rows) {
    console.log(`## ${r.cell}`);
    console.log(`- passed ${r.passes} of ${r.trials} (${f(r.pass_rate * 100)}%, 95% ${ci(r.pass_rate_95 && r.pass_rate_95.map((x) => x * 100))}%)`);
    console.log(`- permission probes passed: ${f((r.perm_pass_rate ?? 0) * 100)}%; trials with every probe passing: ${r.perm_all_pass_trials}`);
    console.log(`- tokens per passing app: ${f(r.tokens_per_passing_app)} (95% ${ci(r.tokens_per_passing_app_95)})`);
    console.log(`- output tokens per passing app: ${f(r.output_tokens_per_passing_app)}`);
    const m = r.median_tokens_per_trial;
    console.log(`- median per trial: ${f(m.total)} total; input ${f(m.input)}, output ${f(m.output)}, cache read ${f(m.cache_read)}, cache write ${f(m.cache_write)}`);
    console.log(`- share spent after the first failed round: ${f((r.tokens_after_first_failure_share ?? 0) * 100)}%`);
    console.log(`- list price per passing app (decides the win): $${f(r.list_price_usd_per_passing_app, 2)} (95% ${ci(r.list_price_usd_per_passing_app_95, 2)})`);
    console.log(`- pass@1 ${f(r.pass_at_1, 2)}, pass@5 ${f(r.pass_at_5, 2)}; median turns ${f(r.median_turns)}, repair rounds ${f(r.mean_repair_rounds, 1)} on average, median ${f(r.median_wall_minutes, 1)} minutes; caps hit ${r.cap_hits}\n`);
  }
  console.log(`Win test, tokens: ${report.verdict_tokens}`);
  console.log(`Win test, list price: ${report.verdict_list_price}`);
}
