import { describe, expect, it } from 'vitest';

import { entity, op, src, synth, t } from '../src/index.js';

describe('download', () => {
  it('prints the op flag last', () => {
    const notes = entity('notes', { title: t.text() });
    const o = op('allNotes', {}, (b) => b.source(src(notes).pick('id', 'title')))
      .unbounded()
      .download();
    expect(synth(o)).toMatch(/^op allNotes unbounded download =/);
  });

  it('prints nothing for an op that is not marked', () => {
    const notes = entity('notes', { title: t.text() });
    const o = op('allNotes', {}, (b) => b.source(src(notes).pick('id', 'title'))).unbounded();
    expect(synth(o)).not.toContain('download');
  });
});
