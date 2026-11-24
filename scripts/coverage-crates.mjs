#!/usr/bin/env node
// Merge executable source lines, not rounded percentages or LCOV summaries.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import { pathToFileURL } from 'node:url';

export function mergeLcov(reports) {
  const files = new Map();
  for (const report of reports) {
    let lines;
    for (const row of report.split(/\r?\n/)) {
      if (row.startsWith('SF:')) {
        const match = row.slice(3).replaceAll('\\', '/').match(/(?:^|\/)(crates\/([^/]+)\/src\/.+)$/);
        lines = undefined;
        if (match) {
          if (!files.has(match[1])) files.set(match[1], { crate: match[2], lines: new Map() });
          lines = files.get(match[1]).lines;
        }
      } else if (row.startsWith('DA:') && lines) {
        const match = row.match(/^DA:(\d+),(\d+)(?:,.*)?$/);
        if (!match || Number(match[1]) < 1) throw new Error(`Invalid LCOV line: ${row}`);
        const number = Number(match[1]);
        lines.set(number, (lines.get(number) ?? false) || Number(match[2]) > 0);
      } else if (row === 'end_of_record') lines = undefined;
    }
  }
  const crates = {};
  for (const { crate, lines } of files.values()) {
    const count = crates[crate] ??= { lines: 0, hit: 0 };
    count.lines += lines.size;
    count.hit += [...lines.values()].filter(Boolean).length;
  }
  if (!Object.values(crates).some(c => c.lines)) throw new Error('No crate source lines found in LCOV input');
  for (const count of Object.values(crates)) count.percent = count.lines ? 100 * count.hit / count.lines : 0;
  const total = Object.values(crates).reduce((sum, c) => ({ lines: sum.lines + c.lines, hit: sum.hit + c.hit }), { lines: 0, hit: 0 });
  total.percent = 100 * total.hit / total.lines;
  return { total, crates };
}

export function checkFloor(report, floor, label) {
  if (!Number.isFinite(floor.tolerance) || floor.tolerance < 0 || !floor[label] || typeof floor[label] !== 'object') {
    throw new Error(`Invalid coverage floor for ${label}`);
  }
  const failures = [], raises = [];
  for (const [crate, minimum] of Object.entries(floor[label])) {
    if (!Number.isFinite(minimum) || minimum < 0 || minimum > 100) throw new Error(`Invalid floor: ${crate}`);
    const measured = report.crates[crate];
    if (!measured) failures.push(`${crate} ${label}: missing from report (floor ${minimum}%)`);
    else if (minimum - measured.percent > floor.tolerance + 1e-9) {
      failures.push(`${crate} ${label}: ${measured.percent.toFixed(2)}% below ${minimum}% (tolerance ${floor.tolerance})`);
    }
  }
  for (const [crate, count] of Object.entries(report.crates)) {
    if (count.percent > (floor[label][crate] ?? 0)) raises.push(`raise ${crate} ${label} to ${count.percent.toFixed(2)}`);
  }
  return { failures, raises };
}

function main(args) {
  const files = [], options = {};
  while (args.length) {
    const arg = args.shift();
    if (['--label', '--out', '--check'].includes(arg)) {
      if (!args.length || args[0].startsWith('--')) throw new Error(`Missing value for ${arg}`);
      options[arg] = args.shift();
    } else if (arg.startsWith('-')) throw new Error(`Unknown option ${arg}`);
    else files.push(arg);
  }
  const label = options['--label'];
  if (!['unit', 'examples'].includes(label) || !files.length) {
    throw new Error('Usage: coverage-crates.mjs --label unit|examples [--out report.json] [--check floor.json] file.info ...');
  }
  const report = { label, ...mergeLcov(files.map(file => readFileSync(file, 'utf8'))) };
  console.log(`### ${label}\n\n| Crate | Lines | Hit | Percent |\n| --- | ---: | ---: | ---: |`);
  for (const [crate, c] of Object.entries(report.crates).sort(([a], [b]) => a.localeCompare(b))) {
    console.log(`| ${crate} | ${c.lines} | ${c.hit} | ${c.percent.toFixed(2)}% |`);
  }
  const t = report.total;
  console.log(`| **Total** | ${t.lines} | ${t.hit} | ${t.percent.toFixed(2)}% |\n`);
  if (options['--out']) {
    mkdirSync(dirname(options['--out']), { recursive: true });
    writeFileSync(options['--out'], JSON.stringify(report, null, 2) + '\n');
  }
  if (options['--check']) {
    const { failures, raises } = checkFloor(report, JSON.parse(readFileSync(options['--check'], 'utf8')), label);
    for (const raise of raises) console.log(raise);
    for (const failure of failures) console.error(failure);
    if (failures.length) process.exitCode = 1;
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try { main(process.argv.slice(2)); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
