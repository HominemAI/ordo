/**
 * `useResumedSession` (owner ruling 2026-10-21): a provider starts signed in
 * when the browser still holds a sign-in, and the sign-in screen shows only
 * when there is nothing to resume.
 */

import { describe, expect, it } from 'vitest';
import { act, create } from 'react-test-renderer';
import { createElement } from 'react';
import { useResumedSession, type ResumedSession } from '../src/hooks/useResumedSession.js';

const settle = async () => {
  for (let i = 0; i < 5; i++) await act(async () => {});
};

function mount<S>(resume: () => Promise<S | null>) {
  const seen: Array<ResumedSession<S>> = [];
  function Probe() {
    seen.push(useResumedSession(resume));
    return null;
  }
  act(() => {
    create(createElement(Probe));
  });
  return { seen, last: () => seen[seen.length - 1]! };
}

describe('useResumedSession', () => {
  it('starts checking, then signed in with the resumed session', async () => {
    const m = mount(async () => ({ token: 'tok' }));
    expect(m.seen[0]?.phase).toBe('checking');
    await settle();
    expect(m.last().phase).toBe('signed-in');
    expect(m.last().session).toEqual({ token: 'tok' });
  });

  it('is signed out when there is nothing to resume', async () => {
    const m = mount(async () => null);
    await settle();
    expect(m.last().phase).toBe('signed-out');
    expect(m.last().session).toBeNull();
  });

  it('fails without signing out when the door cannot be asked, and retries', async () => {
    let n = 0;
    const m = mount(async () => {
      n++;
      if (n === 1) throw new Error('unreachable');
      return { token: 'tok' };
    });
    await settle();
    expect(m.last().phase).toBe('failed');
    await act(async () => m.last().retry());
    await settle();
    expect(m.last().phase).toBe('signed-in');
    expect(n).toBe(2);
  });

  it('asks the door once per mount, whatever the renders', async () => {
    let n = 0;
    const m = mount<{ token: string }>(async () => {
      n++;
      return null;
    });
    await settle();
    await act(async () => m.last().set({ token: 'after sign-in' }));
    await settle();
    expect(n).toBe(1);
    expect(m.last().phase).toBe('signed-in');
    await act(async () => m.last().set(null));
    expect(m.last().phase).toBe('signed-out');
  });
});
