/**
 * The door at request level, with no browser: what the page relies on before
 * it draws anything, and two ways in walked as raw requests.
 */

import { expect, test, type APIRequestContext } from '@playwright/test';
import { codeFor, stack, unique } from '../support.js';

interface Signed {
  readonly token: string;
  readonly space: string;
}

async function auth(request: APIRequestContext, path: string, body: object, token?: string) {
  return request.post(`${stack().door}${path}`, {
    data: body,
    headers: { Origin: stack().page, ...(token ? { Authorization: `Bearer ${token}` } : {}) },
  });
}

async function op(request: APIRequestContext, who: Signed, name: string, body?: object) {
  const headers = {
    'X-Ordo-Space': who.space,
    Authorization: `Bearer ${who.token}`,
    Origin: stack().page,
  };
  return body === undefined
    ? request.get(`${stack().door}/op/${name}`, { headers })
    : request.post(`${stack().door}/op/${name}`, {
        data: body,
        headers: { ...headers, 'Idempotency-Key': unique('e2e-') },
      });
}

/**
 * The person's own tenant, once it is ready. The sign-in answers before the
 * tenant is made (`provisioning: "in progress"`), and `GET /auth/provisioning`
 * says when it is, as the SDK's `waitForTenant` reads it.
 */
async function ready(request: APIRequestContext, done: { token: string; space?: string; provisioning?: string }) {
  if (done.space) return done.space;
  expect(done.provisioning).toBe('in progress');
  let space = '';
  await expect
    .poll(
      async () => {
        const r = await request.get(`${stack().door}/auth/provisioning`, {
          headers: { Origin: stack().page, Authorization: `Bearer ${done.token}` },
        });
        const s = (await r.json()) as { state: string; space?: string };
        expect(s.state, JSON.stringify(s)).not.toBe('failed');
        space = s.space ?? '';
        return s.state;
      },
      { message: 'the tenant is ready', timeout: 120_000 },
    )
    .toBe('ready');
  return space;
}

/** Wait until the door serves the new tenant, as the page's `untilServed` does. */
async function served(request: APIRequestContext, who: Signed) {
  await expect.poll(async () => (await op(request, who, 'myNotes')).status(), { timeout: 10_000 }).toBe(200);
}

const stamp = () => new Date().toISOString().replace(/\.\d+Z$/, 'Z');

test('the door lists the ways in this deployment serves', async ({ request }) => {
  const res = await request.get(`${stack().door}/auth/methods`);
  expect(res.status()).toBe(200);
  const m = await res.json();
  expect(m).toMatchObject({ passkey: true, email: true, guest: true });
  expect(m.connections).toEqual([expect.objectContaining({ name: 'discord', label: 'Discord' })]);
});

test("the door answers a preflight from the page's origin and refuses any other", async ({ request }) => {
  const preflight = (origin: string) =>
    request.fetch(`${stack().door}/op/myNotes`, {
      method: 'OPTIONS',
      headers: {
        Origin: origin,
        'Access-Control-Request-Method': 'GET',
        'Access-Control-Request-Headers': 'authorization,x-ordo-space',
      },
    });
  const ok = await preflight(stack().page);
  expect(ok.status()).toBe(204);
  expect(ok.headers()['access-control-allow-origin']).toBe(stack().page);
  expect((await preflight('http://localhost:1')).status()).toBe(403);
});

test('an emailed code signs a person in to a tenant of their own, and their note is theirs alone', async ({
  request,
}) => {
  const signInByEmail = async (address: string, n: number): Promise<Signed> => {
    const begun = await auth(request, '/auth/begin', { method: 'email', identifier: address });
    expect(begun.status()).toBe(200);
    const { challenge, binder } = await begun.json();
    const done = await auth(request, '/auth/complete', {
      method: 'email',
      kind: 'email_code',
      challenge,
      binder,
      code: await codeFor(address, n),
    });
    expect(done.status(), await done.text()).toBe(200);
    const s = await done.json();
    return { token: s.token, space: await ready(request, s) };
  };

  const kim = await signInByEmail(`${unique('kim')}@example.org`, 0);
  await served(request, kim);
  const title = unique('api-');
  const made = await op(request, kim, 'createNote', { title, body: '', tags: ['api'], created: stamp() });
  expect(made.status(), await made.text()).toBe(200);
  const rows = (await (await op(request, kim, 'myNotes')).json()).rows as { title: string }[];
  expect(rows.map((r) => r.title)).toContain(title);

  const lee = await signInByEmail(`${unique('lee')}@example.org`, 0);
  await served(request, lee);
  expect(lee.space).not.toEqual(kim.space);
  const theirs = (await (await op(request, lee, 'myNotes')).json()).rows as { title: string }[];
  expect(theirs.map((r) => r.title)).not.toContain(title);
  // Lee's token in Kim's tenant is refused.
  expect((await op(request, { token: lee.token, space: kim.space }, 'myNotes')).status()).toBe(403);
});

test('the Discord stand-in: begin, consent, callback, and the hand-off become a session', async ({ request }) => {
  const begun = await auth(request, '/auth/begin', { method: 'discord', return_to: `${stack().page}/` });
  expect(begun.status()).toBe(200);
  const { redirect, binder } = await begun.json();
  expect(redirect).toContain(`${stack().provider}/oauth2/authorize?`);
  expect(redirect).toContain('code_challenge_method=S256');

  const consent = new URL(redirect);
  consent.pathname = '/oauth2/decide';
  consent.searchParams.set('username', unique('ray'));
  consent.searchParams.set('decision', 'allow');
  const toCallback = await request.get(consent.toString(), { maxRedirects: 0 });
  expect(toCallback.status()).toBe(302);
  const callback = toCallback.headers()['location'] ?? '';
  expect(callback).toContain(`${stack().provider}/auth/callback/discord?`);

  // The door exchanges the code with the stand-in over TLS it trusts through
  // ORDO_AUTH_PROVIDER_CA_FILE, then sends the browser back to the page.
  const back = await request.get(callback, { maxRedirects: 0 });
  expect(back.status()).toBe(303);
  const handoff = new URL(back.headers()['location'] ?? '').searchParams.get('handoff');
  expect(handoff).toBeTruthy();

  const done = await auth(request, '/auth/complete', { method: 'discord', kind: 'handoff', handoff, binder });
  expect(done.status(), await done.text()).toBe(200);
  const s = await done.json();
  await served(request, { token: s.token, space: await ready(request, s) });
});

test('a tenant made after the door booted gets live changes on its feed, with no reload', async ({ request }) => {
  // The person is new, so their tenant did not exist when the door booted.
  // The door listens on a tenant's database when a feed opens, so a change
  // written after the feed opened reaches it. The page once polled for this.
  const address = `${unique('new')}@example.org`;
  const begun = await auth(request, '/auth/begin', { method: 'email', identifier: address });
  const { challenge, binder } = await begun.json();
  const done = await auth(request, '/auth/complete', {
    method: 'email',
    kind: 'email_code',
    challenge,
    binder,
    code: await codeFor(address, 0),
  });
  expect(done.status(), await done.text()).toBe(200);
  const session = await done.json();
  const who: Signed = { token: session.token, space: await ready(request, session) };
  await served(request, who);

  const controller = new AbortController();
  const feed = await fetch(`${stack().door}/op/myNotes?live=1&sync=1`, {
    headers: {
      Accept: 'text/event-stream',
      'X-Ordo-Space': who.space,
      Authorization: `Bearer ${who.token}`,
      Origin: stack().page,
    },
    cache: 'no-store',
    signal: controller.signal,
  });
  expect(feed.status).toBe(200);
  const reader = feed.body!.getReader();
  const decoder = new TextDecoder();
  let seen = '';
  /** Read the stream until `text` has arrived, or fail after `ms`. */
  const until = async (text: string, ms: number): Promise<void> => {
    const stop = Date.now() + ms;
    while (!seen.includes(text)) {
      const left = stop - Date.now();
      if (left <= 0) throw new Error(`the feed did not carry ${text} within ${ms} ms; it carried: ${seen.slice(-500)}`);
      const next = await Promise.race([
        reader.read(),
        new Promise<null>((resolve) => setTimeout(() => resolve(null), left)),
      ]);
      if (next === null || next.done) continue;
      seen += decoder.decode(next.value, { stream: true });
    }
  };
  try {
    // The first load has arrived: the feed is open and the door is listening.
    const first = await reader.read();
    seen += decoder.decode(first.value, { stream: true });
    expect(seen).toContain('load');
    const title = unique('after-');
    const made = await op(request, who, 'createNote', { title, body: '', tags: [], created: stamp() });
    expect(made.status(), await made.text()).toBe(200);
    await until(title, 10_000);
  } finally {
    controller.abort();
  }
});
