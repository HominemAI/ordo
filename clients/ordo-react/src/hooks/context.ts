/**
 * The one piece of React wiring: a client in context.
 *
 * Written with `createElement` rather than JSX so the package ships no JSX
 * runtime dependency and no `.tsx` in a library that renders nothing. The
 * provider is the only component in `@hominemai/ordo-react`, and it renders its
 * children — headless means behaviour, and a `<Provider>` is plumbing.
 */

import { createContext, createElement, useContext, useMemo, type ReactNode } from 'react';
import type { DescriptorClient } from '../client.js';
import { asDeviceCopy, type DeviceCopy, type DeviceSource } from '../device.js';
import { DEFAULT_LIVE_PACE_MS, LiveHub, type LiveHubOptions } from '../live.js';
import type { OrdoInterface } from '../load.js';

/** What the provider turns on. Everything defaults to off. */
export interface OrdoRuntime {
  readonly client: DescriptorClient;
  /** Read hooks follow the live feed by default. A hook's own `live` overrides. */
  readonly live: boolean;
  /**
   * Minimum milliseconds between live refetches of one hook: the app's
   * `live.paceMs`, or `DEFAULT_LIVE_PACE_MS`. A floor: a hook's `livePaceMs`
   * may lengthen it, never shorten it.
   */
  readonly paceMs: number;
  readonly hub: LiveHub;
  /** The device copy, or `null` when reads always go to the network. */
  readonly device: DeviceCopy | null;
  /** Asks the person for a fresh touch before a platform write is tried once more, or `null`. */
  readonly stepUp: (() => Promise<void>) | null;
}

const Ctx = createContext<OrdoRuntime | null>(null);

export interface LiveConfig extends LiveHubOptions {
  /**
   * Minimum milliseconds between refetches after an invalidation. A burst of
   * invalidations is one refetch, and refetches are at least this far apart.
   * Unset, `DEFAULT_LIVE_PACE_MS`: a quarter second (ruled 2026-10-01, 27n).
   */
  readonly paceMs?: number;
}

export interface OrdoProviderProps {
  readonly client: DescriptorClient;
  /**
   * Turn live on for every read hook that can follow it. `true` or a config.
   * Off by default: a subscription is an open connection held at the door for
   * as long as the hook is mounted, so an app chooses it. A hook can still
   * set its own `live: true` or `live: false`.
   */
  readonly live?: boolean | LiveConfig;
  /**
   * The device: what the SDK's `openDevice` answers, or a copy from
   * `deviceCopy({ manager, replica, ... })`. The first kind is wrapped once.
   * When set, a read hook answers from the device for every op the
   * descriptor marks syncable and goes to the network for the rest. A hook
   * can set `device: false` to ask the network.
   */
  readonly device?: DeviceSource | undefined;
  /**
   * How the app asks the person for a fresh touch (a passkey step-up).
   * A platform write (`std.space.share` and the rest, see `platform.ts`)
   * wants a recent one, and the door answers `access_denied` both for "too
   * old" and for "not allowed". With this set, `useCommit` on a platform op
   * that is refused `access_denied` calls it once and tries the write again;
   * a second refusal is believed. Program ops never use it.
   */
  readonly stepUp?: () => Promise<void>;
  readonly children?: ReactNode;
}

export function OrdoProvider({ client, live, device, stepUp, children }: OrdoProviderProps): ReactNode {
  const config: LiveConfig = typeof live === 'object' ? live : {};
  const on = live !== undefined && live !== false;
  const hub = useMemo(
    () => new LiveHub(client, config),
    // The hub is one per client. Its backoff and wait are read once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [client],
  );
  const runtime = useMemo<OrdoRuntime>(
    () => ({
      client,
      live: on,
      paceMs: config.paceMs ?? DEFAULT_LIVE_PACE_MS,
      hub,
      device: device ? asDeviceCopy(device) : null,
      stepUp: stepUp ?? null,
    }),
    [client, on, config.paceMs, hub, device, stepUp],
  );
  return createElement(Ctx.Provider, { value: runtime }, children);
}

/** The provider's settings, for the hooks in this package. */
export function useOrdoRuntime(): OrdoRuntime {
  const runtime = useContext(Ctx);
  if (!runtime) {
    throw new Error('useOrdo() outside <OrdoProvider> — every hook in @hominemai/ordo-react needs a client in context');
  }
  return runtime;
}

export function useOrdo(): DescriptorClient {
  return useOrdoRuntime().client;
}

export function useInterface(): OrdoInterface {
  return useOrdo().iface;
}
