/**
 * The hooks can reach the program's ops, its derived admin ops, and the four
 * platform ops the platform table does not carry.
 */

import { describe, expect, it } from 'vitest';
import { INTERFACE } from '../src/interface.js';

describe('the interface the hooks read', () => {
  it('carries the program and its derived admin ops', () => {
    for (const name of ['board', 'moveTask', 'std.projects.create', 'std.team.delete']) {
      expect(INTERFACE.op(name), name).toBeDefined();
    }
  });

  it('adds the invitation and role ops as platform ops, so a stale touch is stepped up', () => {
    const wanted = {
      'std.space.invite': ['role', 'expires', 'label'],
      'std.space.revokeInvitation': ['id'],
      'std.space.setRole': ['member', 'role'],
      'std.space.invitations': [],
    };
    for (const [name, params] of Object.entries(wanted)) {
      const op = INTERFACE.op(name);
      expect(op?.platform, name).toBe(true);
      expect(op?.params.map((p) => p.name)).toEqual(params);
    }
    expect(INTERFACE.op('std.space.invitations')?.kind).toBe('read');
    expect(INTERFACE.op('std.space.invite')?.kind).toBe('write');
  });
});
