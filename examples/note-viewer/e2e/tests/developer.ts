/**
 * The deployments' developer, signed in through the identity service with
 * `ordo login` (examples/e2e-shared/developer.ts, shared with the notes
 * e2e). Each door is its own deployment at the service (`notes`,
 * `viewer`), with its own `ORDO_IDENTITY_DEPLOYMENT`, so a call from one to
 * the other is addressed to that deployment. The developer is a member of
 * both and holds a session for each.
 *
 * The CLI reaches the service over plain http on loopback, since it trusts
 * no test certificate; the browser opens its links at the https issuer.
 */

import type { Browser } from '@playwright/test';
import { developerKit, type CliRun, type ManyDeployments } from '../../../e2e-shared/developer.js';
import { person, stack } from './support.js';

export type { CliRun };

const kit = developerKit<Browser>(
  () => {
    const s = stack();
    return {
      ordo: s.ordo,
      identity: s.identity,
      identityPage: s.issuer,
      identityOrigin: s.issuer,
      identityData: s.identityData,
      identityBin: s.identityBin,
      identityDb: s.identityDb,
      logs: s.logs,
    };
  },
  person,
);

export type App = 'notes' | 'viewer';

/** The developer of both apps: `ordo <args>` with the session for `app`. */
export interface Developer {
  readonly ordo: (app: App, args: readonly string[]) => CliRun;
  readonly token: (app: App) => string;
  readonly close: () => Promise<void>;
}

/** The one developer of the stack, made on first use and found after. */
export async function developer(browser: Browser): Promise<Developer> {
  const d: ManyDeployments = await kit.developerOf(browser, 'Dev', ['notes', 'viewer']);
  return { ordo: d.ordo, token: d.token, close: d.close };
}

/** The door flags of a deployment command on notes or note_viewer: its own space. */
export function door(app: App): string[] {
  const s = stack();
  return ['--host', app === 'notes' ? s.notesDoor : s.viewerDoor, '--space', 'system'];
}
