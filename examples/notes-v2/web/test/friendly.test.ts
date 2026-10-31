/**
 * What the page says when the door cannot be reached. The SDK names the
 * failure (`OrdoNetworkError`: blocked, unreachable or timeout), so the page
 * reads its kind and guesses nothing.
 */

import { beforeEach, describe, expect, it } from 'vitest';
import { OrdoNetworkError, resetNetworkProbes } from '@hominemai/ordo-sdk/core/network.js';
import { friendly, refusalText, returnedErrorText } from '../src/errors.js';
import { listSpaces } from '@hominemai/ordo-sdk/auth/share.js';
import { NoTenantError, PasskeyError, Session } from '../src/session.js';
import { credentials } from './support.js';

const ORIGIN = 'https://notes.inf.smg.icu';
const DOOR = 'https://ordo.inf.smg.icu';
const network = { pageOrigin: ORIGIN };

beforeEach(() => resetNetworkProbes());

describe('a page the door does not answer', () => {
  it('names the two settings when the door is up and refuses this page', async () => {
    // What a browser does with a preflight the door refuses: the fetch fails,
    // and a no-cors request to a health path still gets an (opaque) answer.
    const blocked: typeof globalThis.fetch = async (_url, init) => {
      if (init?.mode === 'no-cors') return new Response(null);
      throw new TypeError('Failed to fetch');
    };
    const e = await Session.signUp({ baseUrl: DOOR, fetch: blocked, credentials, network }, 'Ada').catch(
      (x: unknown) => x,
    );
    expect(e).toBeInstanceOf(OrdoNetworkError);
    expect((e as OrdoNetworkError).kind).toBe('blocked');
    const said = friendly(e, DOOR);
    expect(said).toContain(DOOR);
    expect(said).toContain(`ORDO_CORS_ORIGINS=${ORIGIN}`);
    expect(said).toContain(`ORDO_AUTH_ORIGINS=${ORIGIN}`);
  });

  it('says the door could not be reached, and names no setting, when it is down', async () => {
    const down: typeof globalThis.fetch = async () => {
      throw new TypeError('Failed to fetch');
    };
    const e = await Session.signUp({ baseUrl: DOOR, fetch: down, credentials, network }, 'Ada').catch(
      (x: unknown) => x,
    );
    expect((e as OrdoNetworkError).kind).toBe('unreachable');
    const said = friendly(e, DOOR);
    expect(said).toContain('could not be reached');
    expect(said).not.toContain('ORDO_');
  });

  it('names only the call setting when a call after sign-in is blocked', async () => {
    const blocked: typeof globalThis.fetch = async (_url, init) => {
      if (init?.mode === 'no-cors') return new Response(null);
      throw new TypeError('Failed to fetch');
    };
    const e = await listSpaces({ baseUrl: DOOR, fetch: blocked, network }, 'tok').catch((x: unknown) => x);
    expect(e).toBeInstanceOf(OrdoNetworkError);
    expect(friendly(e)).toContain(`ORDO_CORS_ORIGINS=${ORIGIN}`);
  });
});

describe('the other failures', () => {
  it('does not blame the deployment for a dismissed prompt', () => {
    const said = friendly(new PasskeyError('cancelled', 'x'), DOOR);
    expect(said).toMatch(/prompt was closed/);
    expect(said).not.toContain('ORDO_');
  });

  it('calls a slow door slow, and does not blame the page origin for it', async () => {
    const never: typeof globalThis.fetch = () => new Promise<Response>(() => {});
    const e = await Session.signUp({ baseUrl: DOOR, fetch: never, credentials, callTimeoutMs: 20 }, 'Ada').catch(
      (x: unknown) => x,
    );
    expect(e).toBeInstanceOf(OrdoNetworkError);
    expect((e as OrdoNetworkError).kind).toBe('timeout');
    const said = friendly(e, DOOR);
    expect(said).toMatch(/took too long to answer/);
    expect((e as OrdoNetworkError).option).toBe('callTimeoutMs');
    expect(said).not.toContain('ORDO_');
  });

  it('says a 503 from the door is the door being busy, not an origin problem', () => {
    const said = friendly(new PasskeyError('unavailable', 'x'), DOOR);
    expect(said).toMatch(/cannot answer that right now/);
    expect(said).not.toContain('ORDO_');
  });

  it('points a person with no account at the sign up button', () => {
    expect(friendly(new PasskeyError('refused', 'x'), DOOR)).toMatch(/sign up button/);
  });

  it('tells a deployment owner the account id when no tenant was made', () => {
    const said = friendly(new NoTenantError('failed', 'usr_owner'), DOOR);
    expect(said).toContain('usr_owner');
  });
});

describe('the new ways in', () => {
  it('names the setting when guests are switched off', () => {
    const e = new PasskeyError('guests_off', 'x', { setting: 'ORDO_AUTH_GUEST', value: 'on' });
    expect(friendly(e, DOOR, 'guest')).toContain('ORDO_AUTH_GUEST=on');
  });

  it('asks for a new code when the email way is refused', () => {
    expect(friendly(new PasskeyError('refused', 'x'), DOOR, 'email')).toMatch(/Ask for a new code/);
  });

  it('says what went wrong when an outside provider sends the person back with an error', () => {
    expect(returnedErrorText('identity_provider_unavailable')).toMatch(/could not be reached/);
    expect(returnedErrorText('authentication_failed')).toMatch(/did not work/);
  });

  it('says a paused space is not lost and a quota is a limit', () => {
    const r = (c: string) => ({ class: c, message: 'm' }) as unknown as Parameters<typeof refusalText>[0];
    expect(refusalText(r('space_paused'))).toMatch(/paused/);
    expect(refusalText(r('quota_exceeded'))).toMatch(/limit/);
  });
});
