/**
 * Orphans and pruning (plan-versions.md increment 14, §2.7).
 *
 * notes v3 stops declaring `attachments.size`. The column stays, with what
 * v2 wrote in it, and nothing writes it again: it is an orphan. What a
 * person wrote there is still theirs:
 *
 * 1. Ada adds a file under v2, so her file's row holds a size. v3 is
 *    applied; the apply names the orphan, and `ordo evolve --orphans` lists
 *    it.
 * 2. Ada exports her notes (`std.space.export`): the bundle carries her
 *    orphaned size under `orphans/`.
 * 3. Ada erases her notes (`std.space.delete`): the answer names the orphan
 *    it reached, and the rows holding a size drop by hers.
 * 4. The prune is refused while 0.2.0, which declares the size, is a
 *    version a restore may still need (increment 17): the restore window
 *    is unset, and the refusal names `ordo versions set restore-window`.
 *    The developer sets it to `0`.
 * 5. The developer prunes the column. `ordo evolve --prune` alone prunes
 *    nothing and says what would go; the name typed again without the
 *    deployment's name is refused in production; asked with both, the
 *    column is gone and `--orphans` shows its tombstone.
 *
 * A regression looks like an orphan the apply does not name, an export
 * without the orphaned size, an erasure that leaves it, or a prune on one
 * ask.
 *
 * It applies v2 then v3 again and prunes a column for the whole stack, so
 * it runs after `evolve-checks`, last. Chromium only.
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import { door, developer } from '../developer.js';
import { owner } from '../people.js';
import { addNote, applyProgram, noteInList, signIn, signedIn, stack, unique } from '../support.js';

test.describe.configure({ mode: 'serial' });

async function openV2(page: Page): Promise<void> {
  await page.goto(stack().pageV2);
  const notes = page.getByLabel('New note', { exact: true });
  const settingUp = page.getByRole('heading', { name: 'Setting up your account' });
  const passkey = page.getByRole('button', { name: 'Sign in with a passkey' });
  await expect(notes.or(settingUp).or(passkey)).toBeVisible({ timeout: 30_000 });
  if (await passkey.isVisible()) await signIn(page);
  await signedIn(page);
}

/** The token and tenant the page sends to its door, as last seen. */
function watchSession(page: Page): () => { token: string; space: string } {
  let seen = { token: '', space: '' };
  page.on('request', (r) => {
    const h = r.headers();
    const auth = h['authorization'] ?? '';
    const space = h['x-ordo-space'] ?? '';
    // The page reaches the door by `localhost`, the test by `127.0.0.1`.
    const toDoor = new URL(r.url()).port === new URL(stack().door).port;
    if (toDoor && auth.startsWith('Bearer ') && space && space !== 'system') {
      seen = { token: auth.slice('Bearer '.length), space };
    }
  });
  return () => {
    expect(seen.token, 'the page has called its door').not.toBe('');
    return seen;
  };
}

/**
 * A passkey touch in the page, as the SDK's `stepUp` makes it: the door
 * re-mints the session with presence from now. Answers the new token.
 * Export and erasure ask for a touch, and the page has no button for them.
 */
async function touch(page: Page, token: string): Promise<string> {
  const door = new URL(page.url()).origin.replace(/:\d+$/, `:${new URL(stack().door).port}`);
  return page.evaluate(
    async ({ door, token }) => {
      const b64u = (b: ArrayBuffer) =>
        btoa(String.fromCharCode(...new Uint8Array(b)))
          .replace(/\+/g, '-')
          .replace(/\//g, '_')
          .replace(/=+$/, '');
      const bytes = (s: string) =>
        Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((s.length + 3) % 4)), (c) =>
          c.charCodeAt(0),
        );
      const post = async (path: string, body: unknown) => {
        const r = await fetch(`${door}${path}`, {
          method: 'POST',
          headers: { Accept: 'application/json', 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
          credentials: 'include',
          body: body === null ? undefined : JSON.stringify(body),
        });
        if (!r.ok) throw new Error(`${path}: ${r.status} ${await r.text()}`);
        return (await r.json()) as Record<string, unknown>;
      };
      const o = await post('/auth/step-up/begin', null);
      const challenge = String(o['challenge']);
      const pk = (await navigator.credentials.get({
        publicKey: {
          challenge: bytes(challenge),
          rpId: String(o['rp_id']),
          userVerification: String(o['user_verification']) as UserVerificationRequirement,
          allowCredentials: ((o['allow_credentials'] as string[]) ?? []).map((id) => ({
            type: 'public-key' as const,
            id: bytes(id),
          })),
        },
      })) as PublicKeyCredential;
      const a = pk.response as AuthenticatorAssertionResponse;
      const done = await post('/auth/step-up', {
        method: 'passkey',
        kind: 'passkey_assertion',
        challenge,
        credential_id: b64u(pk.rawId),
        client_data_json: b64u(a.clientDataJSON),
        authenticator_data: b64u(a.authenticatorData),
        signature: b64u(a.signature),
      });
      return String(done['token']);
    },
    { door, token },
  );
}

/** An op in the person's own tenant, as the page calls it. */
async function op(
  request: APIRequestContext,
  who: { token: string; space: string },
  name: string,
  body: object,
  method: 'POST' | 'DELETE' = 'POST',
) {
  // A delete op takes its parameters in the query, as a read does.
  const query = method === 'DELETE' ? `?${new URLSearchParams(body as Record<string, string>)}` : '';
  const r = await request.fetch(`${stack().door}/op/${name}${query}`, {
    method,
    data: method === 'DELETE' ? undefined : body,
    headers: {
      'X-Ordo-Space': who.space,
      Authorization: `Bearer ${who.token}`,
      Origin: stack().page,
      'Idempotency-Key': unique('e2e-'),
    },
  });
  const text = await r.text();
  expect(r.status(), text).toBe(200);
  return (JSON.parse(text) as { rows: Record<string, unknown>[] }).rows[0]!;
}

/**
 * How many rows hold a value in the orphan, in every database, from the
 * prune's first ask. Each person's tenant has a database of its own here.
 */
function holding(out: string): number {
  const counts = [...out.matchAll(/^WOULD PRUNE {2}attachments\.size {2}database `[^`]+` {2}(\d+) row\(s\) hold a value$/gm)];
  expect(counts.length, out).toBeGreaterThan(0);
  return counts.reduce((sum, m) => sum + Number(m[1]), 0);
}

test('attachments.size is orphaned by v3: exported and erased with Ada, then pruned when asked twice', async ({
  browser,
  request,
}, testInfo) => {
  testInfo.setTimeout(420_000);
  applyProgram(stack().programV2);
  const ada = await owner('Ada');
  const dev = await developer(browser);
  try {
    const page = ada.page;
    const session = watchSession(page);
    await openV2(page);

    const withFile = unique('orphan-file-');
    await test.step('under v2, Ada adds a file: its row holds a size', async () => {
      await addNote(page, withFile, 'A note with a file.');
      const file = `${unique('list-')}.txt`;
      await noteInList(page, withFile).click();
      await page.getByLabel('Attach images or files').setInputFiles([
        { name: file, mimeType: 'text/plain', buffer: Buffer.from('milk, eggs\n') },
      ]);
      await expect(page.getByRole('button', { name: `Download ${file}` })).toBeVisible({ timeout: 30_000 });
      await page.getByRole('button', { name: 'Close', exact: true }).click();
    });

    await test.step('v3 is applied: the apply names the orphan, and --orphans lists it', () => {
      const applied = applyProgram(stack().programV3);
      expect(applied).toContain('orphaned    `attachments.size`');
      const listed = dev.ordo(['evolve', '--orphans', ...door()]);
      expect(listed.code, `${listed.out}\n${listed.err}`).toBe(0);
      expect(listed.out).toMatch(/^ORPHAN {5}attachments\.size {2}database `default` {2}left by 0\.3\.0 /m);
    });

    const turnedOn = dev.ordo(['environments', 'set', 'prune', 'on', '--reason', 'the e2e prunes', ...door()]);
    expect(turnedOn.code, `${turnedOn.out}\n${turnedOn.err}`).toBe(0);

    await test.step('a version a restore may need holds the column until the restore window is set', () => {
      // 0.2.0 declares the size and served a moment ago. With
      // `versions.restore_window` unset, a backup or bundle at it could
      // still need the column, so the prune names the setting.
      const held = dev.ordo(['evolve', '--prune', 'attachments.size', ...door()]);
      expect(held.code, held.out).not.toBe(0);
      expect(`${held.out}${held.err}`).toContain('prune_restorable');
      expect(`${held.out}${held.err}`).toContain('ordo versions set restore-window');
      const set = dev.ordo(['versions', 'set', 'restore-window', '0', '--reason', 'the e2e keeps no backups', ...door()]);
      expect(set.code, `${set.out}\n${set.err}`).toBe(0);
      expect(set.out).toContain('restore window 0');
    });

    const before = dev.ordo(['evolve', '--prune', 'attachments.size', ...door()]);
    expect(before.code, `${before.out}\n${before.err}`).toBe(0);
    expect(before.out).toContain('nothing was pruned');
    const held = holding(before.out);
    expect(held).toBeGreaterThanOrEqual(1);

    // Export and erasure ask for a fresh passkey touch.
    const seen = session();
    const fresh = { token: await touch(page, seen.token), space: seen.space };

    await test.step("Ada's export carries her orphaned size", async () => {
      const done = await op(request, fresh, 'std.space.export', {});
      const bundle = String(done['bundle']);
      const size = readFileSync(join(bundle, 'orphans', 'attachments.size.jsonl'), 'utf8');
      expect(size).toContain('"size": 11');
    });

    await test.step("Ada's erasure removes her orphaned size", async () => {
      const done = await op(request, fresh, 'std.space.delete', { confirm: fresh.space }, 'DELETE');
      expect(done['orphans']).toContain('attachments.size');
      const after = dev.ordo(['evolve', '--prune', 'attachments.size', ...door()]);
      expect(after.code, `${after.out}\n${after.err}`).toBe(0);
      expect(holding(after.out)).toBe(held - 1);
    });

    await test.step('the prune is refused without the deployment name, then runs asked twice', () => {
      const once = dev.ordo(['evolve', '--prune', 'attachments.size', '--confirm', 'attachments.size', ...door()]);
      expect(once.code, once.out).not.toBe(0);
      expect(`${once.out}${once.err}`).toContain('--confirm-deployment notes');
      const twice = dev.ordo([
        'evolve',
        '--prune',
        'attachments.size',
        '--confirm',
        'attachments.size',
        '--confirm-deployment',
        'notes',
        ...door(),
      ]);
      expect(twice.code, `${twice.out}\n${twice.err}`).toBe(0);
      expect(twice.out).toContain('pruned     attachments.size');
      const listed = dev.ordo(['evolve', '--orphans', ...door()]);
      expect(listed.out).toMatch(/^PRUNED {5}attachments\.size {2}database `default`/m);
      expect(listed.out).not.toMatch(/^ORPHAN {5}attachments\.size/m);
    });
  } finally {
    await dev.close();
    await ada.close();
  }
});
