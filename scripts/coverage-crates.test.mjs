import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { mergeLcov, checkFloor } from './coverage-crates.mjs';
const record = (path, lines) => `SF:${path}\n${lines.map(([n, hits]) => `DA:${n},${hits}`).join('\n')}\nend_of_record\n`;
const sample = record('/runner/ordo/crates/ordo-host/src/serve.rs', [[1, 1], [2, 0]]);
test('merge unions line hits across runners and repeated records without counting tests or dependencies', () => {
  const result = mergeLcov([sample + sample + record('/registry/serde/src/lib.rs', [[1, 9]]) + record('/repo/crates/ordo-host/tests/a.rs', [[1, 9]]),
    record('C:\\checkout\\crates\\ordo-host\\src\\serve.rs', [[1, 0], [2, 1], [3, 0]])]);
  assert.deepEqual(result.crates['ordo-host'], { lines: 3, hit: 2, percent: 200 / 3 });
  assert.deepEqual(result.total, result.crates['ordo-host']);
});
test('total is weighted by executable lines', () => {
  const result = mergeLcov([sample + record('crates/ordo-cli/src/main.rs', [[1, 1]])]);
  assert.equal(result.total.percent, 200 / 3);
});
test('ratchet fails above tolerance, passes within it and fails a missing crate', () => {
  const report = mergeLcov([sample]);
  const floor = n => ({ tolerance: 0.1, unit: { 'ordo-host': n } });
  assert.equal(checkFloor(report, floor(50.2), 'unit').failures.length, 1);
  assert.equal(checkFloor(report, floor(50.05), 'unit').failures.length, 0);
  assert.equal(checkFloor(report, floor(50.1), 'unit').failures.length, 0);
  assert.match(checkFloor(report, floor(49), 'unit').raises[0], /raise ordo-host unit to 50.00/);
  assert.match(checkFloor(report, { tolerance: 0.1, unit: { missing: 0 } }, 'unit').failures[0], /missing from report/);
});
test('empty reports and invalid floors fail closed', () => {
  assert.throws(() => mergeLcov(['']), /No crate/);
  assert.throws(() => mergeLcov(['SF:crates/a/src/lib.rs\nDA:1,nope\n']), /Invalid LCOV/);
  assert.throws(() => checkFloor(mergeLcov([sample]), { tolerance: -1, unit: {} }, 'unit'), /Invalid/);
});
test('CLI returns failure for a regression and success inside tolerance', () => {
  const dir = mkdtempSync(join(tmpdir(), 'ordo-coverage-'));
  try {
    const input = join(dir, 'sample.info'), floor = join(dir, 'floor.json');
    writeFileSync(input, sample);
    for (const [minimum, status] of [[50.2, 1], [50.05, 0]]) {
      writeFileSync(floor, JSON.stringify({ tolerance: 0.1, unit: { 'ordo-host': minimum } }));
      const result = spawnSync(process.execPath, ['scripts/coverage-crates.mjs', '--label', 'unit', '--check', floor, input], { encoding: 'utf8' });
      assert.equal(result.status, status, result.stderr);
      if (status) assert.match(result.stderr, /ordo-host unit/);
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
