/**
 * A backend function call: the client's one verb and the hook that holds its
 * state. The wire shape is the SDK's (`ordo-sdk/test/function.test.ts`).
 */

import { describe, expect, it } from 'vitest';
import { act, create } from 'react-test-renderer';
import { createElement, type ReactNode } from 'react';
import { OrdoProvider } from '../src/hooks/context.js';
import { useFunction, type UseFunction } from '../src/hooks/useFunction.js';
import { OrdoRefusal } from '../src/refusal.js';
import { MockDoor, client, refusal } from './harness.js';

describe('callFunction', () => {
  it('posts to /fn/<name> with the token and the space', async () => {
    const door = new MockDoor().program({ status: 200, body: { result: { title: 'Groceries' } } });
    const out = await client(door).callFunction('suggestTitle', { text: 'milk' });
    expect(out.result).toEqual({ title: 'Groceries' });
    expect(door.last.url).toBe('http://door.test/fn/suggestTitle');
    expect(door.last.method).toBe('POST');
    expect(door.last.headers['Authorization']).toBe('Bearer tok-abc');
    expect(door.last.headers['X-Ordo-Space']).toBe('spc_test');
    expect(door.last.body).toEqual({ text: 'milk' });
  });

  it('throws a refusal as OrdoRefusal', async () => {
    const door = new MockDoor().program({ status: 400, body: refusal('egress_refused', 'run `ordo secret set llmKey`') });
    const err = await client(door).callFunction('suggestTitle', {}).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoRefusal);
    expect((err as OrdoRefusal).class).toBe('egress_refused');
  });
});

describe('useFunction', () => {
  it('answers the result, or null with the refusal in error', async () => {
    const door = new MockDoor()
      .program({ status: 200, body: { result: { title: 'T' } } })
      .program({ status: 400, body: refusal('function_error', 'the code threw') });
    let hook!: UseFunction<{ title: string }>;
    function Probe(): ReactNode {
      hook = useFunction<{ title: string }>('suggestTitle');
      return null;
    }
    await act(async () => {
      create(createElement(OrdoProvider, { client: client(door) }, createElement(Probe)));
    });
    let first: unknown;
    await act(async () => {
      first = await hook.call({ text: 'x' });
    });
    expect(first).toEqual({ title: 'T' });
    expect(hook.error).toBeNull();
    let second: unknown = 'unset';
    await act(async () => {
      second = await hook.call({ text: 'x' });
    });
    expect(second).toBeNull();
    expect(hook.error).toBeInstanceOf(OrdoRefusal);
    expect(hook.busy).toBe(false);
  });
});
