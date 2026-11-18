// Tests for the dev audit's judgement. Run: node scripts/ci-npm-audit.test.mjs
import assert from 'node:assert/strict';
import { judge } from './ci-npm-audit.mjs';

const EX = [{ id: 'GHSA-aaaa-bbbb-cccc', package: 'globber', ruled: '2026-11-17', why: 'test' }];
const adv = (id, severity = 'high') => ({
  title: `advisory ${id}`,
  url: `https://github.com/advisories/${id}`,
  severity,
});
const report = (vulnerabilities) => ({ auditReportVersion: 2, vulnerabilities });

// An excepted advisory passes, and so do the packages that only depend on it.
{
  const r = judge(
    report({
      globber: { severity: 'high', via: [adv('GHSA-aaaa-bbbb-cccc')], fixAvailable: { isSemVerMajor: true } },
      bundler: { severity: 'high', via: ['globber'], fixAvailable: { isSemVerMajor: true } },
    }),
    EX,
  );
  assert.deepEqual(r.failures, []);
  assert.equal(r.excepted.length, 1);
}

// Another high advisory fails, on the same package or elsewhere.
{
  const r = judge(
    report({
      globber: { severity: 'high', via: [adv('GHSA-aaaa-bbbb-cccc'), adv('GHSA-zzzz-zzzz-zzzz')], fixAvailable: false },
      other: { severity: 'critical', via: [adv('GHSA-yyyy-yyyy-yyyy', 'critical')], fixAvailable: true },
    }),
    EX,
  );
  assert.equal(r.failures.length, 2);
}

// Moderate and low do not fail, as with `--audit-level=high`.
{
  const r = judge(report({ other: { severity: 'moderate', via: [adv('GHSA-mmmm-mmmm-mmmm', 'moderate')] } }), EX);
  assert.deepEqual(r.failures, []);
}

// The exception ends when a fix that is not a breaking change is out.
{
  const r = judge(report({ globber: { severity: 'high', via: [adv('GHSA-aaaa-bbbb-cccc')], fixAvailable: true } }), EX);
  assert.equal(r.failures.length, 1);
  assert.match(r.failures[0], /a fix is now out/);
}

// No report is a failure, never a pass.
assert.equal(judge(null, EX).failures.length, 1);
assert.equal(judge({}, EX).failures.length, 1);
assert.deepEqual(judge(report({}), EX).failures, []);

console.log('ci-npm-audit: all passed');
