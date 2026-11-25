/**
 * The tenant's program version on every answer (`plan-versions.md` §1.9):
 * the client remembers the last `x-ordo-tenant-version` each tenant's
 * answers named, and raises "version changed" when one names another.
 *
 * A regression looks like a move nobody hears (no event, so a device copy of
 * the old shape stays), an event on the first answer, or an event for a
 * refusal that came before the door and names no version.
 */

import { describe, expect, it } from 'vitest';

import { OrdoClient, TENANT_VERSION_HEADER, TenantVersions } from '../src/index.js';
import type { OpSpec, VersionChange } from '../src/index.js';
import { MockDoor } from './harness.js';

const read = {
  op: 'allNotes',
  version: '0123456789abcdef',
  method: 'GET',
  kind: 'read',
  terminal: ['fetch'],
  paged: false,
  singleRow: false,
  casParam: null,
  params: [],
  refusals: [],
  standingError: null,
  sync: null,
  refine: { tier: 'none', fields: [] },
  order: { keys: [], stated: false },
} as unknown as OpSpec;

const under = (version: string) => ({ json: { rows: [] }, headers: { [TENANT_VERSION_HEADER]: version } });

describe('TenantVersions', () => {
  it('remembers the first version, raises a change on another, and ignores an answer with none', () => {
    const v = new TenantVersions();
    const heard: VersionChange[] = [];
    v.onChange((c) => heard.push(c));
    expect(v.saw('spc_a', '0.2.0')).toBeNull();
    expect(v.saw('spc_a', '0.2.0')).toBeNull();
    expect(v.saw('spc_a', null)).toBeNull();
    expect(v.last('spc_a')).toBe('0.2.0');
    // Another tenant's first answer is its own baseline.
    expect(v.saw('spc_b', '0.3.0')).toBeNull();
    expect(v.saw('spc_a', '0.3.0')).toEqual({ space: 'spc_a', from: '0.2.0', to: '0.3.0' });
    expect(heard).toEqual([{ space: 'spc_a', from: '0.2.0', to: '0.3.0' }]);
  });
});

describe('OrdoClient.onVersionChanged', () => {
  it("raises the event when an op's answer names another version for the tenant", async () => {
    const door = new MockDoor().reply(under('0.2.0'), under('0.2.0'), under('0.3.0'));
    const client = door.client();
    const heard: VersionChange[] = [];
    const stop = client.onVersionChanged((c) => heard.push(c));
    await client.request(read, {});
    expect(client.tenantVersion()).toBe('0.2.0');
    await client.request(read, {});
    expect(heard).toEqual([]);
    await client.request(read, {});
    expect(heard).toEqual([{ space: 'spc_test', from: '0.2.0', to: '0.3.0' }]);
    expect(client.tenantVersion()).toBe('0.3.0');
    stop();
  });

  it('a refusal names the version too, and one with no header changes nothing', async () => {
    const refused = {
      status: 409,
      json: { error: { code: 'versions_disagree', message: 'the two disagree on `notes`' } },
      headers: { [TENANT_VERSION_HEADER]: '0.3.1' },
    };
    const door = new MockDoor().reply(under('0.3.0'), { status: 503, json: { error: { code: 'server_busy', message: 'x' } } }, refused);
    const client = new OrdoClient({
      baseUrl: 'https://door.example/api',
      space: 'spc_test',
      token: 'tok',
      fetch: door.fetch,
      busyRetries: 0,
    });
    const heard: VersionChange[] = [];
    client.onVersionChanged((c) => heard.push(c));
    await client.request(read, {});
    await client.request(read, {}).catch(() => undefined);
    expect(heard).toEqual([]);
    const e = await client.request(read, {}).catch((x: unknown) => x);
    expect((e as { detail: { class: string } }).detail.class).toBe('versions_disagree');
    expect(heard).toEqual([{ space: 'spc_test', from: '0.3.0', to: '0.3.1' }]);
  });

  it('is shared by every client withSpace makes, each tenant apart', async () => {
    const door = new MockDoor().reply(under('0.2.0'), under('0.3.0'), under('0.2.0'));
    const a = door.client('spc_a');
    const b = a.withSpace('spc_b');
    const heard: VersionChange[] = [];
    a.onVersionChanged((c) => heard.push(c));
    await a.request(read, {});
    await b.request(read, {});
    // spc_b's first answer is its baseline, not a change of spc_a's.
    expect(heard).toEqual([]);
    await b.request(read, {});
    expect(heard).toEqual([{ space: 'spc_b', from: '0.3.0', to: '0.2.0' }]);
    expect(a.tenantVersion()).toBe('0.2.0');
    expect(b.tenantVersion()).toBe('0.2.0');
  });
});
