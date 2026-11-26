/**
 * The deployment's developer: the person who owns the deployment and runs
 * the `ordo` command against its door (plan-versions.md increment 8).
 *
 * The sign-in itself is shared with the federation e2e
 * (examples/e2e-shared/developer.ts): an invite, a passkey, `ordo login`.
 * This file hands it this stack.
 */

import type { Browser } from '@playwright/test';
import { developerKit } from '../../../e2e-shared/developer.js';
import { person, stack } from './support.js';

export type { CliRun, Developer, FreshDeveloper, TwoDeployments } from '../../../e2e-shared/developer.js';

const kit = developerKit<Browser>(
  () => {
    const s = stack();
    return {
      ordo: s.ordo,
      identity: s.identity,
      identityOrigin: s.identity,
      identityData: s.identityData,
      identityBin: s.identityBin,
      identityDb: s.identityDb,
      logs: s.logs,
    };
  },
  person,
);

/** The door flags of every deployment command: the deployment's own space. */
export function door(): string[] {
  return ['--host', stack().door, '--space', 'system'];
}

export const { developer, freshDeveloper, developerOfTwo, identityCommand } = kit;
