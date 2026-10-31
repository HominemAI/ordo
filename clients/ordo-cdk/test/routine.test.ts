/**
 * The routine builder prints the text the parser reads (checklist 12a).
 *
 * The corpus covers the clock forms and `debounce`; this file covers what the
 * docs' routines do not: flags, `coalesce`, `let_`, a `del`, an `each` body,
 * and the refusals.
 */

import { describe, expect, it } from 'vitest';

import { daily, each, every, f, lit, path, routine, src, synth, t, when } from '../src/index.js';

describe('routine', () => {
  it('prints a listener routine with flags, coalesce and several writes', () => {
    const r = routine(
      'motionSeen',
      { trigger: when('estate.events.created', { debounce: '30s', coalesce: true }) },
      (b) => [
        b.set('events', { kind: lit('motion-seen') }).where(f.kind.eq(lit('motion'))),
        b.del('events', f.kind.eq(lit('noise'))),
      ],
    )
      .unguarded()
      .nopause();
    expect(synth(r)).toBe(
      [
        'routine motionSeen {',
        '  trigger when estate.events.created debounce 30s coalesce',
        '  run unguarded nopause:',
        '    set events(kind == "motion") { kind: "motion-seen" }',
        '    del events(kind == "noise")',
        '}',
        '',
      ].join('\n'),
    );
  });

  it('prints a daily routine with a domain and a parameter', () => {
    const r = routine(
      'sweep',
      { in: 'estate', trigger: daily('09:00Z'), params: { keep: t.int() } },
      (b) => b.del('alerts', f.status.eq(lit('dismissed'))),
    );
    expect(synth(r)).toBe(
      [
        'routine sweep in estate {',
        '  trigger every day 09:00Z',
        '  run($keep: int):',
        '    del alerts(status == "dismissed")',
        '}',
        '',
      ].join('\n'),
    );
  });

  it('prints an each body: the pipeline, then the writes for each row', () => {
    const r = routine(
      'leaseExpiry',
      { in: 'estate', trigger: daily('08:00Z'), params: { today: t.date() } },
      (b, p) =>
        each(b.source(src('leases').where(f.end.le(p.today)).pick('id', 'unit.name')), [
          b.set('alerts', {
            key: lit('leaseExpiry:').add(f.id),
            title: path('unit', 'name'),
          }),
          b.del('drafts', f.id.eq(f.id)),
        ]),
    );
    expect(synth(r)).toBe(
      [
        'routine leaseExpiry in estate {',
        '  trigger every day 08:00Z',
        '  run($today: date):',
        '    leases(end <= $today)[id, unit.name]',
        '    each ->',
        '      set alerts { key: "leaseExpiry:" + id, title: unit.name }',
        '      del drafts { id: id }',
        '}',
        '',
      ].join('\n'),
    );
  });

  it('refuses an each with no writes', () => {
    expect(() =>
      routine('x', { trigger: every('5m') }, (b) => each(b.source('leases'), [])),
    ).toThrow(/at least one write/);
  });

  it('refuses a malformed trigger and an empty body', () => {
    expect(() => every('soon')).toThrow(/expected/);
    expect(() => daily('09:00')).toThrow(/UTC/);
    expect(() => routine('x', { trigger: every('5m') }, () => [])).toThrow(/at least one/);
  });
});
