/**
 * The person a session token names.
 *
 * The token is `<base64url payload>.<signature>` and the payload is
 * `v<n>|<principal>|<expiry>|...`. The door moved to v4 with the second step
 * (2026-09-29), to v5 with the way in, `via` (2026-10-08), and to v6 with an
 * outside provider's two-step claim (2026-10-09). The principal has stayed
 * the second field throughout, so any version from v4 on is read. How the
 * session began is read from the sign-in answer's `via`, not from the token.
 * The page cannot verify the signature and
 * does not try: the door does that on every call. This only reads the
 * principal id, which names a device replica (`LocalReplica` is per space and
 * per person), so a wrong answer costs an empty replica and nothing else.
 */
export function principalOf(token: string): string {
  const payload = token.split('.')[0] ?? '';
  const text = decode(payload);
  const fields = text.split('|');
  const principal = fields[1] ?? '';
  const version = /^v(\d+)$/.exec(fields[0] ?? '');
  if (!version || Number(version[1]) < 4 || principal === '') {
    throw new Error('the door sent a token this page cannot read');
  }
  return principal;
}

function decode(base64url: string): string {
  const padded = base64url.replace(/-/g, '+').replace(/_/g, '/');
  const binary = atob(padded + '='.repeat((4 - (padded.length % 4)) % 4));
  const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}
