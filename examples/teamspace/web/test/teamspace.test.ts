/**
 * The rules the screens share, with no render: the invitation link, who sees
 * the admin screens, and the rows the report reads.
 */

import { describe, expect, it } from 'vitest';
import {
  inviteLink,
  invitableRoles,
  isAdmin,
  joinRoute,
  neighbour,
  personReportOf,
  projectReportOf,
  taskOf,
  overdue,
} from '../src/teamspace.js';

describe('the join route', () => {
  it('reads the team and the ticket', () => {
    expect(joinRoute('#/join/spc_abc123/tkt_Xy-9._~')).toEqual({ space: 'spc_abc123', ticket: 'tkt_Xy-9._~' });
  });

  it('is null for anything else', () => {
    expect(joinRoute('')).toBeNull();
    expect(joinRoute('#/join/spc_abc')).toBeNull();
    expect(joinRoute('#/join/spc_abc/')).toBeNull();
    expect(joinRoute('#/join/spc_abc/t/extra')).toBeNull();
    expect(joinRoute('#/accept/abc')).toBeNull();
  });
});

describe('the invitation link', () => {
  it('keeps the ticket in the fragment, after the page path', () => {
    const link = inviteLink({ origin: 'http://localhost:5174', pathname: '/' }, 'spc_abc', 'tkt_1');
    expect(link).toBe('http://localhost:5174/#/join/spc_abc/tkt_1');
    expect(joinRoute(link.slice(link.indexOf('#')))).toEqual({ space: 'spc_abc', ticket: 'tkt_1' });
  });
});

describe('who sees what', () => {
  it('shows the admin screens to an owner or a manager only', () => {
    expect(isAdmin('owner')).toBe(true);
    expect(isAdmin('manager')).toBe(true);
    expect(isAdmin('editor')).toBe(false);
    expect(isAdmin('viewer')).toBe(false);
    expect(isAdmin(undefined)).toBe(false);
  });

  it('offers the owner role in an invitation to an owner alone', () => {
    expect(invitableRoles('owner')).toContain('owner');
    expect(invitableRoles('manager')).not.toContain('owner');
    expect(invitableRoles('manager')).toEqual(['editor', 'manager', 'viewer']);
  });
});

describe('the board', () => {
  it('moves a task to the next column and stops at the edges', () => {
    expect(neighbour('todo', 1)).toBe('doing');
    expect(neighbour('todo', -1)).toBeNull();
    expect(neighbour('done', 1)).toBeNull();
    expect(neighbour('review', -1)).toBe('doing');
  });

  it('calls a task late when it is past due and not done', () => {
    const t = taskOf({ id: 't', rev: 2, title: 'x', status: 'doing', due: '2026-10-01' });
    expect(t.rev).toBe(2);
    expect(overdue(t, '2026-10-10')).toBe(true);
    expect(overdue({ ...t, status: 'done' }, '2026-10-10')).toBe(false);
    expect(overdue({ ...t, due: null }, '2026-10-10')).toBe(false);
  });
});

describe('the reports', () => {
  it('reads a missing count as 0', () => {
    // A project or a person with no tasks comes back with no count keys.
    expect(projectReportOf({ id: 'p', name: 'Empty' })).toMatchObject({ total: 0, todo: 0, done: 0, overdue: 0 });
    expect(personReportOf({ id: 'u', displayName: 'Ada', role: 'editor' })).toMatchObject({ total: 0, open: 0, overdue: 0 });
  });
});
