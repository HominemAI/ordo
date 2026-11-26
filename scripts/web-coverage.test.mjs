import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { discover, hash, inventory } from './web-coverage-inventory.mjs';
import { summarize, validateScope, readRun } from './web-coverage-report.mjs';
const file = 'crates/ordo-host/src/serve.rs';
const source = { sourceDigest: 'revision-a', files: [{ file, sha256: 'abc', lines: 10 }, { file: 'crates/other/src/lib.rs', sha256: 'def', lines: 5 }], entries: [{ id: 'route-a' }] };
const scope = () => ({ version: 1, sourceDigest: source.sourceDigest, entries: [], ranges: [] });
const lcov = (rows, path = file) => `SF:/runner/${path}\n${rows.map(([n, h]) => `DA:${n},${h}`).join('\n')}\nend_of_record\n`;
const range = (classification, start = 1, end = 1) => ({ file, sha256: 'abc', start, end, classification, evidence: 'reviewed caller chain', reviewedBy: 'reviewer' });
test('discovery handles nested calls, chained verbs, dynamic paths, comments, raw strings and Unicode offsets', () => {
  const code = `// 🚀 .route("fake", get(fake))\n/* nested /* .route("bad", get(x)) */ */
const DOC: &str = r###".route("fake", get(x))"###;
const CHARACTER: char = '"';
router.route(format!("/{}/go", prefix), get(a).post(b)).route("/real", post(c));
pub const OP: &str = "std.space.create";`;
  const entries = discover(file, code);
  assert.equal(entries.length, 3);
  assert.equal(entries[0].path, 'format!("/{}/go", prefix)');
  assert.equal(entries[0].handler, 'get(a).post(b)');
  assert.equal(entries[1].path, '"/real"');
  assert.equal(entries[2].expression, 'std.space.create');
});
test('union counts a line once and leaves unreviewed and unmeasured code visible', () => {
  const result = summarize(source, scope(), [lcov([[1, 1], [2, 0]]), lcov([[1, 0], [2, 1], [3, 0]])]);
  assert.deepEqual(result.conservativeMeasured, { lines: 3, hit: 2, percent: 200 / 3 });
  assert.equal(result.groups.unresolved.lines, 3);
  assert.equal(result.webPercent, null);
  assert.deepEqual(result.unmeasuredFiles, ['crates/other/src/lib.rs']);
  assert.deepEqual(result.unreviewedEntries, ['route-a']);
  assert.equal(result.branchCoverage.percent, null);
});
test('reviewed exclusions remain explicit and observed hits contradict them', () => {
  const s = scope(); s.ranges = [range('non-web')];
  const result = summarize(source, s, [lcov([[1, 0], [2, 1]])]);
  assert.equal(result.groups.excluded.lines, 1);
  assert.equal(result.conservativeMeasured.lines, 1);
  assert.throws(() => summarize(source, s, [lcov([[1, 1]])]), /contradicts exclusion/);
});
test('stale, overlapping, unsubstantiated and out-of-bounds classifications fail', () => {
  for (const mutate of [
    s => { s.sourceDigest = 'old'; },
    s => { s.ranges = [{ ...range('non-web'), sha256: 'old' }]; },
    s => { s.ranges = [range('reachable', 1, 2), range('non-web', 2, 3)]; },
    s => { s.ranges = [{ ...range('non-web'), evidence: '' }]; },
    s => { s.ranges = [range('reachable', 1, 11)]; },
  ]) { const s = scope(); mutate(s); assert.throws(() => validateScope(s, source)); }
});
test('unreviewed entries cannot be marked as reviewed without contracts and journeys', () => {
  const s = scope(); s.entries = [{ id: 'route-a' }];
  assert.throws(() => validateScope(s, source), /missing registration/);
});
test('coverage cannot silently introduce unknown source or impossible line numbers', () => {
  assert.throws(() => summarize(source, scope(), [lcov([[1, 1]], 'crates/unknown/src/a.rs')]), /unknown source/);
  assert.throws(() => summarize(source, scope(), [lcov([[11, 1]])]), /outside source/);
  assert.throws(() => summarize(source, scope(), ['SF:crates/ordo-host/src/serve.rs\nDA:1,nope']), /Invalid LCOV/);
});
test('run provenance rejects unit/setup/stale reports and changed artifacts, retaining failed runs', () => {
  const dir = mkdtempSync(join(tmpdir(), 'ordo-web-report-'));
  try {
    const report = lcov([[1, 1]]), catalog = '{"ops":[]}';
    writeFileSync(join(dir, 'web.info'), report); writeFileSync(join(dir, 'catalog.json'), catalog);
    const receipt = { version: 1, sourceDigest: source.sourceDigest, kind: 'web-journey', phase: 'web-only', result: 'failed', scenario: 'user edits a row', profile: 'two-hosts', separationEvidence: 'Fresh host processes after setup; disjoint profile directory', artifacts: [
      {kind:'lcov',path:'web.info',sha256:hash(report)}, {kind:'runtime-catalog',path:'catalog.json',sha256:hash(catalog)},
    ] };
    const path = join(dir, 'receipt.json');
    const save = value => writeFileSync(path, JSON.stringify(value));
    save(receipt); assert.equal(readRun(path, source).run.result, 'failed');
    for (const patch of [{kind:'unit'}, {phase:'setup'}, {sourceDigest:'old'}, {separationEvidence:''}, {artifacts:receipt.artifacts.slice(0,1)}]) {
      save({...receipt,...patch}); assert.throws(() => readRun(path, source));
    }
    save(receipt); writeFileSync(join(dir, 'web.info'), report + '\n');
    assert.throws(() => readRun(path, source), /checksum mismatch/);
  } finally { rmSync(dir, {recursive:true,force:true}); }
});
test('checked-in inventory and review ledger match current source', () => {
  const found = inventory(process.cwd());
  assert.deepEqual(found, JSON.parse(readFileSync('coverage/web-inventory.json')));
  validateScope(JSON.parse(readFileSync('coverage/web-scope.json')), found);
});


test('zero-hit ranges preserve uncovered islands, source hashes and unresolved scope', () => {
  const s = scope(); s.ranges = [range('reachable', 5, 6)];
  const result = summarize(source, s, [lcov([[8, 0], [1, 0], [2, 0], [3, 1], [5, 0], [6, 0], [7, 0]])]);
  assert.deepEqual(result.uncoveredRanges, [
    { file, sha256: 'abc', start: 1, end: 2, classification: 'unresolved', outcomeReview: 'pending' },
    { file, sha256: 'abc', start: 5, end: 6, classification: 'reachable', outcomeReview: 'pending' },
    { file, sha256: 'abc', start: 7, end: 8, classification: 'unresolved', outcomeReview: 'pending' },
  ]);
  assert.equal(result.webPercent, null);
});
