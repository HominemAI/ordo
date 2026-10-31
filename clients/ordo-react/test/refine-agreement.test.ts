/**
 * The two copies of the refinement validator agree.
 *
 * `@hominemai/ordo-sdk` checks an envelope in `src/core/refine.ts`; this package keeps
 * its promise of no dependency but React and carries the same rules in
 * `src/refine.ts`. This test runs both over every read in the golden
 * descriptor, with a probe set that reaches every rule, in both modes, and
 * requires the same answer: the same envelope back, or the same refusal
 * message. A rule changed in one copy and not the other fails here.
 */

import { describe, expect, it } from 'vitest';

import { checkRefine as sdkCheck } from '../../ordo-sdk/src/index.js';
import { SPECS } from '../../ordo-sdk/src/generated/index.js';
import type { OpSpec } from '../../ordo-sdk/src/index.js';
import { checkRefine as reactCheck, type Refine } from '../src/refine.js';
import { iface } from './harness.js';

function verdict(f: () => Refine): unknown {
  try {
    return { ok: f() };
  } catch (e) {
    return { refused: (e as Error).message };
  }
}

/** Envelopes that reach every rule: each operator, each value shape, each type, the sealed rule, pick. */
function probes(fields: readonly string[], outputs: readonly string[]): Refine[] {
  const out: Refine[] = [{}, { where: [] }];
  const values: unknown[] = ['x', 'Café', 3, 2.5, '29.99', true, null, [1], { a: 1 }];
  for (const field of [...outputs, 'margin']) {
    for (const op of ['eq', 'ne', 'lt', 'gt', 'in', 'is_null', 'like'] as const) {
      if (op === 'is_null') {
        out.push({ where: [{ field, op }] }, { where: [{ field, op, value: 1 }] });
        continue;
      }
      if (op === 'in') {
        out.push(
          { where: [{ field, op, value: ['x', null] }] },
          { where: [{ field, op, value: [3, 2.5] }] },
          { where: [{ field, op, value: 'x' }] },
        );
        continue;
      }
      for (const value of values) out.push({ where: [{ field, op: op as 'eq', value }] });
    }
  }
  out.push({ pick: [] }, { pick: ['margin'] }, { pick: [...outputs] });
  if (fields.length > 0) out.push({ pick: [fields[0]!] });
  return out;
}

describe('the React validator and the SDK validator give the same verdicts', () => {
  const reads = iface().ops.filter((o) => o.kind === 'read');

  it('covers the catalog', () => {
    expect(reads.length).toBeGreaterThan(50);
  });

  it.each(reads.map((o) => [o.op, o] as const))('%s', (name, op) => {
    const spec = (SPECS as Record<string, OpSpec>)[name]!;
    expect(spec, name).toBeDefined();
    const outputs = op.result.outputs.map((o) => o.name);
    for (const mode of ['door', 'device'] as const) {
      for (const probe of probes(op.refine.fields, outputs)) {
        const sdk = verdict(() => sdkCheck(spec, probe, mode));
        const react = verdict(() => reactCheck(op, probe, mode));
        expect(react, `${mode} ${JSON.stringify(probe)}`).toEqual(sdk);
      }
    }
  });

  it('a sealed refinable field: equality only at the door, anything on the device, in both', () => {
    const op = iface().requireOp('memoDigest');
    const opened = { ...op, refine: { ...op.refine, fields: [...op.refine.fields, 'memo'] } };
    const spec = { ...SPECS.memoDigest, refine: { ...SPECS.memoDigest.refine, fields: opened.refine.fields } };
    for (const mode of ['door', 'device'] as const) {
      for (const probe of [
        { where: [{ field: 'memo', op: 'eq' as const, value: 'x' }] },
        { where: [{ field: 'memo', op: 'gt' as const, value: 'x' }] },
      ]) {
        expect(verdict(() => reactCheck(opened, probe, mode))).toEqual(verdict(() => sdkCheck(spec, probe, mode)));
      }
    }
    expect(verdict(() => reactCheck(opened, { where: [{ field: 'memo', op: 'gt', value: 'x' }] }, 'door'))).toMatchObject({
      refused: expect.stringMatching(/equality only/),
    });
  });
});
