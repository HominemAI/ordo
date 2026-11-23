#!/usr/bin/env node
// The dev-dependency npm audit, with advisories excepted by id.
//
// `npm audit` has no way to except one advisory, so this reads its JSON and
// judges it here. It fails on any high or critical advisory that is not
// excepted below. The runtime audit (`npm audit --omit=dev`) does not go
// through this file and takes no exceptions.
//
// An exception ends by itself: once npm reports a fix that is not a breaking
// change for the excepted package, the audit fails again and says to take
// the fix and remove the entry.
//
// Usage, from a directory holding a package-lock.json:
//   node <repo>/scripts/ci-npm-audit.mjs

import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

/** Each entry is an owner ruling, recorded in docs/corpus-rulings.md. */
export const EXCEPTED = [
  {
    id: 'GHSA-fc5q-39xc-vjpf',
    package: 'picomatch',
    ruled: '2026-11-17',
    why:
      'ReDoS in a glob matcher; no patched release exists (every version up to ' +
      '4.0.7 is affected). It reaches us only through vite and vitest, on our ' +
      'own config. Board: the-audit-check-fails-on-a.',
  },
];

const SERIOUS = new Set(['high', 'critical']);

function advisoryId(via) {
  const url = typeof via.url === 'string' ? via.url : '';
  return url.slice(url.lastIndexOf('/') + 1) || String(via.source ?? via.title ?? 'unknown');
}

/**
 * Judge an `npm audit --json` report.
 * Returns { failures: string[], excepted: string[] }.
 */
export function judge(report, excepted = EXCEPTED) {
  const failures = [];
  const seen = [];
  const byId = new Map(excepted.map((e) => [e.id, e]));
  if (!report || typeof report.vulnerabilities !== 'object' || report.vulnerabilities === null) {
    return { failures: ['npm audit gave no report to judge'], excepted: seen };
  }
  for (const [name, vuln] of Object.entries(report.vulnerabilities)) {
    for (const via of vuln.via ?? []) {
      // A string names another vulnerable package; its own entry carries the advisory.
      if (typeof via !== 'object' || via === null) continue;
      if (!SERIOUS.has(via.severity)) continue;
      const id = advisoryId(via);
      const exception = byId.get(id);
      if (!exception) {
        failures.push(`${name}: ${via.severity}: ${via.title ?? id} (${via.url ?? id})`);
      } else if (vuln.fixAvailable === true) {
        failures.push(
          `${name}: ${id} is excepted, and a fix is now out. Run \`npm audit fix\` here ` +
            'and remove its entry from EXCEPTED in scripts/ci-npm-audit.mjs.',
        );
      } else {
        seen.push(`${name}: ${id} (excepted ${exception.ruled})`);
      }
    }
  }
  return { failures, excepted: seen };
}

function main() {
  const run = spawnSync('npm', ['audit', '--json'], { encoding: 'utf8', maxBuffer: 64 << 20 });
  let report;
  try {
    report = JSON.parse(run.stdout);
  } catch {
    console.error('npm audit did not print JSON:');
    console.error(run.stderr || run.stdout);
    process.exit(1);
  }
  if (report.error) {
    console.error(`npm audit failed: ${report.error.summary ?? JSON.stringify(report.error)}`);
    process.exit(1);
  }
  const { failures, excepted } = judge(report);
  for (const line of excepted) console.log(`excepted  ${line}`);
  for (const line of failures) console.error(`FAIL      ${line}`);
  if (failures.length > 0) process.exit(1);
  console.log('no high or critical advisory outside the exceptions');
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) main();
