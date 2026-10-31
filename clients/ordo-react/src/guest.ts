/**
 * A client for a guest: someone who reads through a share window or an open
 * link rather than a session of their own.
 *
 * The SDK's `openWindow` and `openLink` return an SDK client whose token is
 * private to it, so a guest's screen could not be given to the hooks, which
 * take this package's {@link DescriptorClient}. The SDK now also offers
 * `windowFeed` and `linkFeed`, which return the tenant and a function that
 * answers the current token (minted again before it ends). That function is
 * exactly what a `DescriptorClient` takes as its `token`, so this builds the
 * client from it and the page never holds the token string:
 *
 * ```ts
 * const feed = await windowFeed(endpoint, () => session.token(), space);
 * const client = guestClient({ baseUrl: endpoint.baseUrl, iface, feed });
 * <OrdoProvider client={client} live device={deviceCopy}>...</OrdoProvider>
 * ```
 *
 * {@link GuestFeed} is the SDK's own type, re-exported.
 */

import type { GuestFeed } from '@hominemai/ordo-sdk/auth/share.js';
import { DescriptorClient, type DescriptorClientOptions } from './client.js';

export type { GuestFeed };

export interface GuestClientOptions extends Omit<DescriptorClientOptions, 'space' | 'token'> {
  readonly feed: GuestFeed;
}

/** A {@link DescriptorClient} bound to the feed's tenant, asking the feed for its token on every call. */
export function guestClient(options: GuestClientOptions): DescriptorClient {
  const { feed, ...rest } = options;
  return new DescriptorClient({ ...rest, space: feed.space, token: feed.token, ...(feed.renew ? { renewToken: feed.renew } : {}) });
}
