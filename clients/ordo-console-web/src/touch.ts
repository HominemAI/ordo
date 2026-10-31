import { ConsoleCallError } from './api';
import { identityMode, touch } from './identity';
import { assert } from './webauthn';

/**
 * Run `f`; if the console asks for a fresh touch, take one and run it again.
 * Every write that changes who owns the console or what runs needs a passkey
 * touch in the last 5 minutes.
 *
 * `f` runs again from the top, so it must make **one** write (or only
 * idempotent ones). The retry is safe because a refused call changed
 * nothing; a closure that made a first write and was then refused on a
 * second would repeat the first. Chain two writes as two `withTouch` calls.
 * `useDeploymentTouch`'s `atDeployment` wraps this and may run `f` up to 4
 * times, under the same rule. Every caller was checked against it on
 * 2026-10-29: each passes one call from `api.ts`'s helpers.
 */
export async function withTouch<T>(f: () => Promise<T>): Promise<T> {
  try {
    return await f();
  } catch (e) {
    if (!(e instanceof ConsoleCallError && e.code === 'step_up_required')) throw e;
  }
  // With the identity service the touch is taken there, and the token the
  // next call carries holds it; it answers the door's presence_required too.
  if (identityMode()) await touch();
  else await assert('stepup/begin', 'stepup/finish');
  return f();
}
