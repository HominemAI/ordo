import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

/** bench/token */
export const BENCH = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
/** The repository root. */
export const REPO = join(BENCH, '..', '..');
