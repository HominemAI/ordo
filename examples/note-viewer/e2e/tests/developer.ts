/**
 * The deployment's developer, signed in through the identity service with
 * `ordo login` (examples/e2e-shared/developer.ts, shared with the notes
 * e2e). One person is an admin of the service's own deployment, `main`,
 * which both doors take their tokens for, so the same session acts on notes
 * and on note_viewer.
 *
 * The CLI reaches the service over plain http on loopback, since it trusts
 * no test certificate; the browser opens its links at the https issuer.
 */

import type { Browser } from '@playwright/test';
import { developerKit } from '../../../e2e-shared/developer.js';
import { person, stack } from './support.js';

export type { CliRun, Developer } from '../../../e2e-shared/developer.js';

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

export const { developer } = kit;

/** The door flags of a deployment command on notes or note_viewer: its own space. */
export function door(app: 'notes' | 'viewer'): string[] {
  const s = stack();
  return ['--host', app === 'notes' ? s.notesDoor : s.viewerDoor, '--space', 'system'];
}
