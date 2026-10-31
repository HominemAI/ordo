/**
 * `TenantGate`: render the app once the person's own tenant is ready, and the
 * app developer's own content until then (owner, 2026-10-21: what to show
 * while a tenant is made is "up to the developer").
 *
 * Three slots, each a node or a function of the hook's result:
 *
 * | Slot | Shown while | Default |
 * |---|---|---|
 * | `settingUp` | the status is being read, or the tenant is being made | "Setting up your account." as a polite status |
 * | `stuck` | the setup has taken longer than `ORDO_PROVISION_STUCK_SECS` | the door's reason for the person, as a polite status |
 * | `failed` | it cannot be made now, or the session was refused | the reason as an alert, and a "Try again" button where trying again can help |
 *
 * The children render once it is ready. A function child is given the
 * tenant's space id, which is what a `DescriptorClient` needs.
 *
 * The defaults are plain elements with no styling, so a page that does not
 * set a slot still tells the person something. Written with `createElement`,
 * like the provider, so the package ships no JSX runtime.
 */

import { createElement, Fragment, type ReactNode } from 'react';
import { useTenantStatus, type TenantStatusResult, type UseTenantStatusOptions } from './hooks/useTenantStatus.js';

/** A slot: a node, or a function of where the setup stands. */
export type TenantSlot = ReactNode | ((tenant: TenantStatusResult) => ReactNode);

export interface TenantGateProps extends UseTenantStatusOptions {
  /** What the app shows once the tenant is ready. A function gets its space id. */
  readonly children: ReactNode | ((space: string) => ReactNode);
  /** Shown while the tenant is being made. */
  readonly settingUp?: TenantSlot;
  /** Shown while the setup is reported stuck. The server is still trying. */
  readonly stuck?: TenantSlot;
  /** Shown when it cannot be made now. */
  readonly failed?: TenantSlot;
}

/** The default words while setting up. */
export const SETTING_UP_TEXT = 'Setting up your account.';
/** The default words while stuck, when the door gave no reason. */
export const STUCK_TEXT = 'Setting up your account is taking longer than it should. It keeps trying on its own.';

function fill(slot: TenantSlot, tenant: TenantStatusResult): ReactNode {
  return typeof slot === 'function' ? slot(tenant) : slot;
}

function defaultFailed(tenant: TenantStatusResult): ReactNode {
  const status = tenant.status;
  const reason =
    tenant.problem?.message ??
    status?.reason ??
    (status?.state === 'off'
      ? 'This app does not set up an account of your own.'
      : 'Setting up your account did not start.');
  // Trying again cannot help a deployment that makes no tenants, or a
  // session that was refused (that needs a new sign-in).
  const canRetry = status?.state !== 'off' && tenant.problem?.problem !== 'refused';
  return createElement(
    'div',
    { role: 'alert' },
    createElement('p', null, reason),
    canRetry ? createElement('button', { type: 'button', onClick: () => void tenant.retry() }, 'Try again') : null,
  );
}

export function TenantGate(props: TenantGateProps): ReactNode {
  const { children, settingUp, stuck, failed, ...options } = props;
  const tenant = useTenantStatus(options);
  let shown: ReactNode;
  switch (tenant.phase) {
    case 'ready':
      shown = typeof children === 'function' ? children(tenant.space as string) : children;
      break;
    case 'stuck':
      shown =
        stuck !== undefined
          ? fill(stuck, tenant)
          : createElement('p', { role: 'status' }, tenant.status?.reason ?? STUCK_TEXT);
      break;
    case 'failed':
      shown = failed !== undefined ? fill(failed, tenant) : defaultFailed(tenant);
      break;
    default:
      shown = settingUp !== undefined ? fill(settingUp, tenant) : createElement('p', { role: 'status' }, SETTING_UP_TEXT);
  }
  return createElement(Fragment, null, shown);
}
