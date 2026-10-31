// The stand-ins the throwaway stack needs, in one Node process:
//
// 1. **A stand-in Discord.** An OAuth2 provider over HTTPS, shaped like
//    Discord (`/oauth2/authorize`, `/api/oauth2/token`, `/api/users/@me`), so
//    the door's `discord` preset runs unchanged with only its three addresses
//    pointed here. The authorize page is a small form: type a user name and
//    press Allow, or press Deny. The token endpoint checks the client id, the
//    client secret, the redirect URI and the PKCE verifier (S256), so a door
//    that sends any of them wrong fails here as it would at Discord.
//
// 2. **The door's public address.** An outside provider sends the browser
//    back to `<ORDO_PUBLIC_URL>/auth/callback/<name>`, and the door requires
//    that address to be https. The door itself serves plain http on loopback,
//    so this listener also forwards `/auth/callback/*` to it.
//
// 3. **A mail catcher.** An SMTP listener that keeps every message, and an
//    HTTP route that lists them, so a test can read the emailed code.
//
// The TLS certificate is the test fixture `localhost` leaf, signed by the
// test CA in `crates/ordo-host/tests/fixture/ca.pem`. The door trusts that
// CA through `ORDO_AUTH_PROVIDER_CA_FILE`; the browser is told to ignore the
// certificate. Nothing here is fit for anything but a test.
//
// Settings, all from the environment:
//   E2E_PROVIDER_PORT   https port for the provider and the callback (default 4643)
//   E2E_MAIL_SMTP_PORT  SMTP port (default 4625)
//   E2E_MAIL_HTTP_PORT  HTTP port for GET /messages (default 4626)
//   E2E_DOOR            the door's own address, for the callback (required)
//   E2E_CLIENT_ID, E2E_CLIENT_SECRET  what the door must present (required)

import { createHash, randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createServer as createHttp, request as httpRequest } from 'node:http';
import { createServer as createHttps } from 'node:https';
import { createServer as createTcp } from 'node:net';
import { fileURLToPath } from 'node:url';

const fixture = (name) =>
  readFileSync(fileURLToPath(new URL(`../../../crates/ordo-host/tests/fixture/${name}`, import.meta.url)));

const env = (k, d) => {
  const v = process.env[k];
  if (v !== undefined && v !== '') return v;
  if (d === undefined) throw new Error(`${k} is required`);
  return d;
};

const providerPort = Number(env('E2E_PROVIDER_PORT', '4643'));
const smtpPort = Number(env('E2E_MAIL_SMTP_PORT', '4625'));
const mailHttpPort = Number(env('E2E_MAIL_HTTP_PORT', '4626'));
const door = new URL(env('E2E_DOOR'));
const clientId = env('E2E_CLIENT_ID');
const clientSecret = env('E2E_CLIENT_SECRET');

const log = (...a) => console.log('[stand-ins]', ...a);

// ── The stand-in Discord ──────────────────────────────────────────────────

/** code -> what was authorized. Each is good once. */
const codes = new Map();
/** access token -> the user. */
const tokens = new Map();
/** What the token endpoint saw, for a test that wants to look. */
const exchanges = [];

/** A Discord id is a snowflake: digits. Derived from the name so it is stable. */
const snowflake = (name) => BigInt('0x' + createHash('sha256').update(name).digest('hex').slice(0, 15)).toString();

const userOf = (name) => ({
  id: snowflake(name),
  username: name,
  global_name: name.charAt(0).toUpperCase() + name.slice(1),
  email: `${name}@example.org`,
  verified: true,
  avatar: null,
});

const escape = (s) => String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
const b64url = (buf) => buf.toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');

function send(res, status, body, headers = {}) {
  const text = typeof body === 'string' ? body : JSON.stringify(body);
  res.writeHead(status, {
    'content-type': typeof body === 'string' ? 'text/html; charset=utf-8' : 'application/json',
    'cache-control': 'no-store',
    ...headers,
  });
  res.end(text);
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const parts = [];
    req.on('data', (c) => parts.push(c));
    req.on('end', () => resolve(Buffer.concat(parts)));
    req.on('error', reject);
  });
}

function authorizePage(q) {
  const keep = ['client_id', 'redirect_uri', 'state', 'code_challenge', 'code_challenge_method', 'scope', 'response_type'];
  const hidden = keep
    .map((k) => `<input type="hidden" name="${k}" value="${escape(q.get(k) ?? '')}">`)
    .join('');
  return `<!doctype html><html><head><meta charset="utf-8"><title>Stand-in Discord</title></head>
<body><h1>Stand-in Discord</h1>
<p>An app asks to know who you are (${escape(q.get('scope') ?? '')}).</p>
<form method="get" action="/oauth2/decide">${hidden}
<label for="username">User name</label> <input id="username" name="username" value="">
<button type="submit" name="decision" value="allow">Allow</button>
<button type="submit" name="decision" value="deny">Deny</button>
</form></body></html>`;
}

function decide(q, res) {
  const redirect = q.get('redirect_uri') ?? '';
  const state = q.get('state') ?? '';
  if (q.get('client_id') !== clientId) return send(res, 400, '<p>Unknown client.</p>');
  if (!redirect.startsWith(`https://localhost:${providerPort}/auth/callback/`)) {
    return send(res, 400, '<p>That redirect URI is not registered.</p>');
  }
  const back = new URL(redirect);
  back.searchParams.set('state', state);
  if (q.get('decision') !== 'allow') {
    back.searchParams.set('error', 'access_denied');
  } else {
    const username = (q.get('username') ?? '').trim().toLowerCase();
    if (!/^[a-z0-9_.-]{1,32}$/.test(username)) return send(res, 400, '<p>Type a user name.</p>');
    if (q.get('code_challenge_method') !== 'S256' || !q.get('code_challenge')) {
      return send(res, 400, '<p>PKCE (S256) is required.</p>');
    }
    const code = b64url(randomBytes(18));
    codes.set(code, { username, challenge: q.get('code_challenge'), redirect });
    back.searchParams.set('code', code);
  }
  res.writeHead(302, { location: back.toString(), 'cache-control': 'no-store' });
  res.end();
}

async function token(req, res) {
  const form = new URLSearchParams((await readBody(req)).toString('utf8'));
  let id = form.get('client_id');
  let secret = form.get('client_secret');
  const basic = /^Basic (.+)$/i.exec(req.headers.authorization ?? '');
  if (basic) {
    const [u, p] = Buffer.from(basic[1], 'base64').toString('utf8').split(':');
    id = decodeURIComponent(u ?? '');
    secret = decodeURIComponent(p ?? '');
  }
  const code = form.get('code') ?? '';
  const held = codes.get(code);
  codes.delete(code);
  const verifier = form.get('code_verifier') ?? '';
  const fail = (why) => {
    exchanges.push({ ok: false, why });
    log('token refused:', why);
    send(res, 400, { error: 'invalid_grant', error_description: why });
  };
  if (form.get('grant_type') !== 'authorization_code') return fail('grant_type');
  if (id !== clientId || secret !== clientSecret) return fail('client');
  if (!held) return fail('code unknown or used');
  if (form.get('redirect_uri') !== held.redirect) return fail('redirect_uri');
  if (b64url(createHash('sha256').update(verifier).digest()) !== held.challenge) return fail('pkce');
  const access = b64url(randomBytes(24));
  tokens.set(access, held.username);
  exchanges.push({ ok: true, username: held.username });
  send(res, 200, { access_token: access, token_type: 'Bearer', expires_in: 600, scope: 'identify email' });
}

function me(req, res) {
  const bearer = /^Bearer (.+)$/i.exec(req.headers.authorization ?? '');
  const username = bearer ? tokens.get(bearer[1]) : undefined;
  if (!username) return send(res, 401, { message: '401: Unauthorized', code: 0 });
  send(res, 200, userOf(username));
}

/** `/auth/callback/*` belongs to the door: forward it as it came. */
function forward(req, res) {
  const out = httpRequest(
    {
      host: door.hostname,
      port: door.port,
      method: req.method,
      path: req.url,
      headers: { ...req.headers, host: door.host, 'x-forwarded-proto': 'https' },
    },
    (up) => {
      res.writeHead(up.statusCode ?? 502, up.headers);
      up.pipe(res);
    },
  );
  out.on('error', (e) => send(res, 502, `<p>The door did not answer: ${escape(e.message)}</p>`));
  req.pipe(out);
}

const provider = createHttps({ cert: fixture('localhost.pem'), key: fixture('localhost-key.pem') }, (req, res) => {
  const url = new URL(req.url ?? '/', `https://localhost:${providerPort}`);
  const route = `${req.method} ${url.pathname}`;
  if (url.pathname.startsWith('/auth/callback/')) return forward(req, res);
  switch (route) {
    case 'GET /oauth2/authorize':
      return send(res, 200, authorizePage(url.searchParams));
    case 'GET /oauth2/decide':
      return decide(url.searchParams, res);
    case 'POST /api/oauth2/token':
      return void token(req, res).catch((e) => send(res, 500, { error: String(e) }));
    case 'GET /api/users/@me':
      return me(req, res);
    case 'GET /health':
      return send(res, 200, { ok: true, exchanges });
    default:
      return send(res, 404, { error: 'not_found', route });
  }
});

// ── The mail catcher ──────────────────────────────────────────────────────

/** Every message received: who it was for, and its text as sent. */
const messages = [];

const smtp = createTcp((sock) => {
  sock.setEncoding('utf8');
  let buf = '';
  let inData = false;
  let mail = { from: '', to: [], data: '' };
  const say = (line) => sock.write(`${line}\r\n`);
  say('220 stand-in mail catcher ESMTP');
  sock.on('data', (chunk) => {
    buf += chunk;
    for (;;) {
      if (inData) {
        const end = buf.indexOf('\r\n.\r\n');
        if (end < 0) return;
        mail.data = buf.slice(0, end).replace(/^\.\./gm, '.');
        buf = buf.slice(end + 5);
        inData = false;
        messages.push({ ...mail, at: Date.now() });
        log(`mail for ${mail.to.join(', ')}`);
        mail = { from: '', to: [], data: '' };
        say('250 2.0.0 kept');
        continue;
      }
      const nl = buf.indexOf('\r\n');
      if (nl < 0) return;
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 2);
      const verb = line.slice(0, 4).toUpperCase();
      if (verb === 'EHLO') {
        sock.write('250-stand-in\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n');
      } else if (verb === 'HELO') say('250 stand-in');
      else if (verb === 'MAIL') {
        mail.from = (/<([^>]*)>/.exec(line) ?? [])[1] ?? '';
        say('250 2.1.0 ok');
      } else if (verb === 'RCPT') {
        mail.to.push(((/<([^>]*)>/.exec(line) ?? [])[1] ?? '').toLowerCase());
        say('250 2.1.5 ok');
      } else if (verb === 'DATA') {
        inData = true;
        say('354 end with <CRLF>.<CRLF>');
        // The body starts on the next line; prefix CRLF so a body that is
        // only the end marker still matches it.
        buf = `\r\n${buf}`;
      } else if (verb === 'RSET') {
        mail = { from: '', to: [], data: '' };
        say('250 ok');
      } else if (verb === 'NOOP') say('250 ok');
      else if (verb === 'QUIT') {
        say('221 bye');
        sock.end();
        return;
      } else say('502 5.5.2 not here');
    }
  });
  sock.on('error', () => {});
});

const mailHttp = createHttp((req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost');
  if (req.method === 'GET' && url.pathname === '/messages') {
    const to = url.searchParams.get('to')?.toLowerCase();
    return send(res, 200, to ? messages.filter((m) => m.to.includes(to)) : messages);
  }
  if (req.method === 'GET' && url.pathname === '/health') return send(res, 200, { ok: true });
  send(res, 404, { error: 'not_found' });
});

provider.listen(providerPort, () => log(`provider and callback on https://localhost:${providerPort}`));
smtp.listen(smtpPort, () => log(`smtp on localhost:${smtpPort}`));
mailHttp.listen(mailHttpPort, () => log(`mail list on http://localhost:${mailHttpPort}/messages`));

for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => {
    provider.close();
    smtp.close();
    mailHttp.close();
    process.exit(0);
  });
}
