/**
 * The descriptor the React hooks read at run time: `interface.json`, written
 * by `ordo interface` (`npm run gen`) and committed beside the generated client.
 * The hooks take their methods, versions, id types and form fields from it.
 *
 * The sharing ops are the platform's, not the program's, so the descriptor
 * does not carry them. The page names the five it uses; nothing else of the
 * platform is reachable from a hook (`withPlatformOps`).
 */

import { parseInterface, withPlatformOps } from '@hominemai/ordo-react';
import descriptor from './interface.json';

export const INTERFACE = withPlatformOps(
  parseInterface(descriptor),
  'std.space.share',
  'std.space.revokeShare',
  'std.space.shareLink',
  'std.space.revokeLink',
  'std.space.links',
);
