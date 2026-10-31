/**
 * A signed-in person's own email address (owner ruling 2026-10-22).
 *
 * A person who signs in by passkey or an outside connection has no address
 * Ordo knows, so the app's mail (a reminder, a routine's `notify`) cannot
 * reach them. These calls add one, change it or take it off. An address
 * counts only once the person types the code mailed to it. A change, and a
 * removal, mail the old address to say so.
 *
 * | Function | Route |
 * |---|---|
 * | `readAccountEmail` | `GET /auth/email` |
 * | `beginAccountEmail` | `POST /auth/email/begin` |
 * | `confirmAccountEmail` | `POST /auth/email/complete` |
 * | `removeAccountEmail` | `POST /auth/email/remove` |
 *
 * The calls are `@hominemai/ordo-sdk`'s (`src/auth/account-email.ts`),
 * re-exported (owner ruling, 2026-10-23; audit-service-interfaces.md, fix
 * order step 7). Each takes the SDK's form, `(endpoint, token, ...)`, and
 * this package's 0.3 form, the token inside the endpoint, which throws
 * {@link AccountEmailError}; the 0.3 form is deprecated. `useAccountEmail`
 * is built on it.
 */

export {
  AccountEmailError,
  accountEmail,
  beginAccountEmail,
  confirmAccountEmail,
  isEmailRequired,
  needsAccountEmail,
  readAccountEmail,
  removeAccountEmail,
  type AccountEmail,
  type AccountEmailEndpoint,
  type AccountEmailProblem,
  type ConfirmedEmail,
} from '@hominemai/ordo-sdk/auth/account-email.js';
export type { EmailCodeSent } from '@hominemai/ordo-sdk/auth/email.js';
