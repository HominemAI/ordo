#!/usr/bin/env node
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { pathToFileURL } from 'node:url';
import { hash, inventory } from './web-coverage-inventory.mjs';
import { mergeLcov } from './coverage-crates.mjs';
const insist = (ok, message) => { if (!ok) throw new Error(message); };
const text = value => typeof value === 'string' && value.trim().length > 0;
export function validateScope(scope, source) {
  insist(scope.version === 1 && scope.sourceDigest === source.sourceDigest, 'Scope source digest is stale');
  insist(Array.isArray(scope.ranges) && Array.isArray(scope.entries), 'Scope requires ranges and entries');
  const files = new Map(source.files.map(f => [f.file, f.sha256]));
  const ids = new Set(source.entries.map(e => e.id)), seen = new Set();
  for (const entry of scope.entries) {
    insist(ids.has(entry.id) && !seen.has(entry.id), `Unknown or duplicate reviewed entry: ${entry.id}`);
    seen.add(entry.id);
    for (const field of ['registration', 'actors', 'contract', 'journey', 'profile'])
      insist(text(entry[field]), `${entry.id}: missing ${field}`);
    insist(Array.isArray(entry.downstream) && entry.downstream.length > 0 && entry.downstream.every(f => files.has(f)), `${entry.id}: invalid downstream paths`);
    insist(entry.status === 'source-reviewed', 'Runtime observations belong in run receipts, not source reviews');
  }
  const occupied = new Set();
  for (const r of scope.ranges) {
    insist(files.get(r.file) === r.sha256, `Stale or unknown range: ${r.file}`);
    insist(Number.isInteger(r.start) && Number.isInteger(r.end) && r.start > 0 && r.end >= r.start && r.end <= source.files.find(f => f.file === r.file).lines, 'Invalid source range');
    insist(['reachable', 'non-web', 'invariant'].includes(r.classification), 'Invalid range classification');
    insist(text(r.evidence) && text(r.reviewedBy), 'Range needs reachability/exclusion evidence and reviewer');
    for (let n = r.start; n <= r.end; n++) {
      const key = `${r.file}:${n}`;
      insist(!occupied.has(key), `Overlapping scope ranges: ${key}`); occupied.add(key);
    }
  }
}
export function lcovLines(reports) {
  const files = new Map();
  for (const report of reports) {
    let lines;
    for (const row of report.split(/\r?\n/)) {
      if (row.startsWith('SF:')) {
        const match = row.slice(3).replaceAll('\\', '/').match(/(?:^|\/)(crates\/[^/]+\/src\/.+)$/);
        lines = undefined;
        if (match) { if (!files.has(match[1])) files.set(match[1], new Map()); lines = files.get(match[1]); }
      } else if (row.startsWith('DA:') && lines) {
        const m = row.match(/^DA:(\d+),(\d+)(?:,.*)?$/);
        insist(m && Number(m[1]) > 0, `Invalid LCOV: ${row}`);
        lines.set(Number(m[1]), (lines.get(Number(m[1])) ?? false) || Number(m[2]) > 0);
      } else if (row === 'end_of_record') lines = undefined;
    }
  }
  insist(files.size > 0, 'No crate source coverage');
  return files;
}
export function summarize(source, scope, reports) {
  validateScope(scope, source);
  const files = lcovLines(reports), known = new Set(source.files.map(f => f.file));
  const groups = { reachable: { lines: 0, hit: 0 }, unresolved: { lines: 0, hit: 0 }, excluded: { lines: 0, hit: 0 } };
  const gaps = [];
  for (const [file, lines] of files) {
    insist(known.has(file), `Coverage contains unknown source file: ${file}`);
    const ranges = scope.ranges.filter(r => r.file === file);
    const maxLine = source.files.find(f => f.file === file).lines;
    let missing = 0, unresolved = 0;
    for (const [n, hit] of lines) {
      insist(n <= maxLine, `Coverage line outside source: ${file}:${n}`);
      const r = ranges.find(r => r.start <= n && n <= r.end);
      const group = !r ? 'unresolved' : r.classification === 'reachable' ? 'reachable' : 'excluded';
      groups[group].lines++; groups[group].hit += Number(hit);
      if (group !== 'excluded' && !hit) missing++;
      if (group === 'unresolved') unresolved++;
      insist(!(hit && group === 'excluded'), `Observed web hit contradicts exclusion: ${file}:${n}`);
    }
    if (missing || unresolved) gaps.push({ file, uncovered: missing, unresolved });
  }
  const unmeasured = source.files.filter(f => !files.has(f.file)).map(f => f.file);
  const pending = source.entries.filter(e => !scope.entries.some(r => r.id === e.id)).map(e => e.id);
  const denominator = groups.reachable.lines + groups.unresolved.lines;
  const hits = groups.reachable.hit + groups.unresolved.hit;
  const complete = !groups.unresolved.lines && !unmeasured.length && !pending.length;
  return {
    sourceDigest: source.sourceDigest, raw: mergeLcov(reports), groups,
    // Never call this web coverage while the source/measurement inventory is incomplete.
    conservativeMeasured: { lines: denominator, hit: hits, percent: denominator ? hits * 100 / denominator : null },
    webPercent: null, // Requires runtime registration and outcome reconciliation, not yet implemented.
    runtimeReconciliation: 'pending',
    scopeComplete: complete, unmeasuredFiles: unmeasured, unreviewedEntries: pending,
    gaps: gaps.sort((a, b) => b.uncovered - a.uncovered || a.file.localeCompare(b.file)),
    branchCoverage: { status: 'not-measured', percent: null },
  };
}
export function readRun(path, source) {
  const run = JSON.parse(readFileSync(path, 'utf8'));
  insist(run.version === 1 && run.sourceDigest === source.sourceDigest, 'Run source digest is stale');
  insist(run.kind === 'web-journey' && run.phase === 'web-only', 'Only web-only journey profiles are accepted; setup and unit reports are not');
  insist(['passed', 'failed'].includes(run.result) && text(run.scenario) && text(run.profile), 'Run needs scenario, configuration profile and result');
  insist(text(run.separationEvidence), 'Run needs evidence that setup profiles were separated');
  insist(Array.isArray(run.artifacts) && run.artifacts.some(a => a.kind === 'lcov') && run.artifacts.some(a => a.kind === 'runtime-catalog'), 'Run needs LCOV and a runtime catalogue artifact');
  const reports = [];
  for (const artifact of run.artifacts) {
    insist(text(artifact.path) && /^[a-f0-9]{64}$/.test(artifact.sha256), 'Artifact needs path and SHA-256');
    const bytes = readFileSync(resolve(dirname(path), artifact.path));
    insist(hash(bytes) === artifact.sha256, `Artifact checksum mismatch: ${artifact.path}`);
    if (artifact.kind === 'lcov') reports.push(bytes.toString('utf8'));
  }
  // Presence of a catalogue is evidence retention, not automatic registration reconciliation.
  return { run, reports };
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [scopePath, out, ...runs] = process.argv.slice(2);
    insist(scopePath && out && runs.length, 'Usage: web-coverage-report.mjs scope.json output.json run-receipt.json ...');
    const source = inventory(process.cwd());
    const scope = JSON.parse(readFileSync(scopePath, 'utf8'));
    const loaded = runs.map(path => readRun(path, source));
    const report = summarize(source, scope, loaded.flatMap(r => r.reports));
    report.runs = loaded.map(({ run }) => ({ scenario: run.scenario, profile: run.profile, result: run.result }));
    report.acceptance = 'incomplete'; // Outcome and catalogue review are separate, required work.
    writeFileSync(out, JSON.stringify(report, null, 2) + '\n');
    console.log(`Measured ${report.conservativeMeasured.hit}/${report.conservativeMeasured.lines}; unresolved ${report.groups.unresolved.lines}; unmeasured files ${report.unmeasuredFiles.length}.`);
    console.log('This is not a release-acceptance result. See the scope and outcome gaps.');
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
