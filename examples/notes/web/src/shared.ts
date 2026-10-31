/**
 * Notes other people shared with this person: the guest's half.
 *
 * A share is a window into the sharer's tenant. The page asks the door for a
 * window (`windowFeed`), which lets it call `myNotes` there and nothing else,
 * and the door can close it at any call. A share link is the same with no
 * account (`linkFeed`). Either gives a tenant and a function that answers a
 * token the SDK renews; the page builds clients from that function and never
 * holds the token itself:
 *
 * - the hooks' client (`guestClient`), which a guest's `<OrdoProvider>` takes;
 * - for a window, an SDK client too, which the sync manager of the guest's
 *   device needs.
 */

import { DeviceRegistry, guestClient, type DescriptorClient as HooksClient } from '@hominemai/ordo-react';
import {
  acceptShare,
  leaveShare,
  linkFeed,
  listSpaces,
  peekShare,
  windowFeed,
  type AcceptedShare,
  type GuestFeed,
  type ListedSpace,
  type PeekedShare,
} from '@hominemai/ordo-sdk/auth/share.js';
import type { PasskeyEndpoint } from '@hominemai/ordo-sdk/auth/passkey.js';
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { INTERFACE } from './interface.js';
import type { OrdoDevice } from '@hominemai/ordo-sdk/replica/device.js';
import { openNotesDevice } from './device.js';
import { namedFetchOf, type Session } from './session.js';

/** What accepting a link tells the person before they agree. */
export function peek(session: { endpoint: Session['endpoint'] }, ticket: string): Promise<PeekedShare> {
  return peekShare(session.endpoint, ticket);
}

/** Agree: the name typed is shown to the sharer. */
export async function accept(session: Session, ticket: string, displayName: string): Promise<AcceptedShare> {
  return acceptShare(session.endpoint, await session.token(), ticket, displayName);
}

/** A guest's way into one tenant: the hooks' client, and the SDK's for the device that syncs it. */
export interface GuestClients {
  readonly hooks: HooksClient;
  readonly sdk: OrdoClient;
}

function clientsOf(endpoint: PasskeyEndpoint, feed: GuestFeed): GuestClients {
  const fetch = endpoint.fetch ? { fetch: endpoint.fetch } : {};
  return {
    hooks: guestClient({ baseUrl: endpoint.baseUrl, iface: INTERFACE, feed, fetch: namedFetchOf(endpoint) }),
    sdk: new OrdoClient({
      baseUrl: endpoint.baseUrl,
      space: feed.space,
      token: feed.token,
      // A refused window token is minted again before the device gives up:
      // a second accept replaces the grant it names.
      ...(feed.renew ? { renewToken: feed.renew } : {}),
      ...fetch,
      ...(endpoint.network ? { network: endpoint.network } : {}),
    }),
  };
}

/** Clients into one shared tenant, their token renewed as it ends. */
export async function windowInto(session: Session, space: string): Promise<GuestClients> {
  return clientsOf(session.endpoint, await windowFeed(session.endpoint, () => session.token(), space));
}

/**
 * Clients for an open share link. No session is involved: the secret goes to
 * `POST /auth/link`, and a link that is unknown, expired or revoked is refused
 * here as a `PasskeyError` with `failure: 'refused'`.
 */
export async function linkInto(endpoint: PasskeyEndpoint, secret: string): Promise<GuestClients> {
  return clientsOf(endpoint, await linkFeed(endpoint, secret));
}

/** A guest's device for one shared tenant, with the hooks' client that reads it. */
export interface SharedDevice {
  readonly device: OrdoDevice;
  readonly hooks: HooksClient;
}

/**
 * One device per shared tenant and person, opened when a screen first shows
 * it and closed when none does (`DeviceRegistry`). `purge` erases every one
 * this person opened since signing in, closed or not, which sign-out does.
 */
export function sharedDevices(session: Session): {
  readonly registry: DeviceRegistry<SharedDevice>;
  readonly purge: () => Promise<void>;
} {
  const registry = new DeviceRegistry<SharedDevice>({
    open: async (space, person) => {
      const clients = await windowInto(session, space);
      // The device syncs through the SDK's client; the hooks read through theirs.
      const device = await openNotesDevice(clients.sdk, space, person);
      return { device, hooks: clients.hooks };
    },
    close: (shared) => shared.device.close(),
    purge: (shared) => shared.device.purge(),
  });
  return { registry, purge: () => registry.purgeAll(session.principal) };
}

/** The tenants this person is a guest in. */
export async function guestSpaces(session: Session): Promise<ListedSpace[]> {
  const all = await listSpaces(session.endpoint, await session.token());
  return all.filter((s) => s.via === 'guest');
}

/** Stop being a guest in a tenant. The sharer's notes leave this device. */
export async function leave(session: Session, space: string): Promise<void> {
  await leaveShare(session.endpoint, await session.token(), space);
}
