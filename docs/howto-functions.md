# Backend functions

A backend function is your own code that the deployment runs on request.
It runs inside the Ordo binary, in a fresh WebAssembly instance per call.
It can call outside APIs through `fetch`, use a secret by name, and log.
It holds no database URL, no service key and no secret value.

Increment 1 builds one kind: a function the page calls, JSON in and JSON
out. Schedules, change triggers, webhooks and calling ops from a function
come later (`plan-backend-functions.md` §12).

The worked example is `suggestTitle` in `examples/notes-v2`.

## Declaring a function

A `function` block sits in the `.ordo` program beside `entity` and `op`:

```ordo
function suggestTitle in notes {
  call($text: text(20000)) -> json by member
  code "fn/suggest-title.js"
  egress api.anthropic.com
  secrets llmKey
  limit wall 30s, memory 64 mb
}
```

| Clause | Means |
|---|---|
| `call($p: type, ...) -> json` | Called from the page. The parameters are typed and checked at the door like an op's. It answers JSON. |
| `by member` | Who may call it: a signed-in member of the tenant. The default. |
| `code "fn/x.js"` | The bundled ES module, relative to the program's directory. `.js` or `.mjs`, at most 20 MB. |
| `egress host, ...` | The hosts it may reach. Any other host is refused before DNS. A name with a hyphen goes in quotes: `"my-model.example.com"`. |
| `secrets name, ...` | The secrets it may use, by name. Each must be a declared `secret`. A secret bound to hosts must be bound to at least one of its `egress` hosts; `ordo check` refuses it otherwise, naming both lists. A signing key (`secret hookKey`, no host) needs no `egress`. |
| `limit wall 30s, cpu 2s, memory 64 mb` | Optional. Unset limits take the defaults below. Write a size with a space: `64 mb`. |
| `nopause` | Optional. It keeps answering while the tenant is paused. |

`ordo check` compiles the block and lists it under `FUNCTIONS`. A function
may not share a name with an op or a routine.

### The code

The module's default export has a `call(input, ctx)` method:

```js
export default {
  async call(input, ctx) {
    const r = await fetch("https://api.anthropic.com/v1/messages", {
      method: "POST",
      headers: {
        "x-api-key": ctx.secret("llmKey"),
        "content-type": "application/json",
      },
      body: JSON.stringify({ /* ... */ }),
    });
    if (!r.ok) throw new Error("the model answered " + r.status);
    return { title: (await r.json()).content[0].text };
  },
};
```

- `input` holds the declared parameters.
- `fetch`, `Response` and `Headers` follow the Web standard. A body is a
  string; use `JSON.stringify`. Only `https://` URLs to declared hosts work.
- `ctx.secret(name)` returns a handle, never the value. Put it in a header
  as it is, or as `ctx.secret(name).bearer()` for `Bearer <value>`, or
  `ctx.secret(name).template("Token {}")`. The host fills the value in on
  the way out, for the secret's bound hosts only.
- `console.log`, `console.warn` and `console.error` go to the run's log.
- `ctx.tenant`, `ctx.function`, `ctx.version` and `ctx.runId` describe the
  call. `ctx.runId` is the page's `Idempotency-Key` when it sent one.
- Throwing ends the call with `function_error` and your message.

Bundle anything you import into the one file, with esbuild or similar.
The engine is Boa, a JavaScript engine written in Rust and compiled to
WebAssembly. It runs ES2023 with promises and `async`. It has no Node APIs
and no `setTimeout`.

### What the engine offers

Beside `fetch`, `Response`, `Headers` and `console`:

| API | What works |
|---|---|
| `TextEncoder` | `encode` and `encodeInto`, UTF-8. A lone surrogate becomes U+FFFD. |
| `TextDecoder` | UTF-8 only, with `fatal` and `ignoreBOM`. `stream: true` throws; decode the whole input at once. |
| `crypto.getRandomValues` | Integer typed arrays, up to 65536 bytes per call. |
| `crypto.randomUUID` | A version 4 UUID. |
| `crypto.subtle.digest` | SHA-1, SHA-256, SHA-384, SHA-512. |
| `crypto.subtle.importKey` | `raw` HMAC keys, and `"secret"` keys the host holds (below). |
| `crypto.subtle.generateKey` | HMAC keys. |
| `crypto.subtle.exportKey` | `raw`, for a key made extractable. A `"secret"` key refuses. |
| `crypto.subtle.sign`, `verify` | HMAC with any of the hashes above. `verify` compares in constant time. |

Random bytes come from the server's operating system, fresh for every call.
Two calls never share a sequence. The hashing runs in Rust inside the
engine. RSA, ECDSA, AES and other `crypto.subtle` algorithms reject with
`NotSupportedError`.

A webhook check, for a sender that signs the body with HMAC-SHA256 in hex:

```js
const unhex = (s) => new Uint8Array(s.match(/../g).map((h) => parseInt(h, 16)));

export default {
  async call(input) {
    const enc = new TextEncoder();
    const key = await crypto.subtle.importKey(
      "raw", enc.encode(input.signingKey), { name: "HMAC", hash: "SHA-256" },
      false, ["verify"]);
    const ok = await crypto.subtle.verify(
      "HMAC", key, unhex(input.signature), enc.encode(input.body));
    return { ok };
  },
};
```

That example takes the key as input. A key kept as a secret is used by
its name instead: see [Checking a webhook's signature](#checking-a-webhooks-signature).

### Limits

| Limit | Default | Ceiling | Over it |
|---|---|---|---|
| Memory | 128 MB | 512 MB | `function_oom` |
| CPU | 10 s | 60 s | `function_timeout` |
| Wall time | 60 s | 15 min | `function_timeout` |
| Calls at once, per tenant | 10 | | `function_busy` (429) |
| Calls at once, per server | 100 | | `function_busy` (429) |
| Outbound calls per call | 50 | | the `fetch` rejects |
| Log per call | 64 KB | | the rest is dropped and marked |
| Answer | 1 MB | | `function_error` |
| Input | 1 MB | | refused at the door |

CPU is counted as WebAssembly fuel. Waiting on the network counts toward
wall time and not toward CPU.

### Start time

The first call after a server starts needs the engine compiled, which
takes about a second. The server keeps the compiled engine on disk, in
`ORDO_FN_CACHE_DIR` (`$XDG_CACHE_HOME/ordo/fn`, else
`$HOME/.cache/ordo/fn`). The one image sets it to `/var/cache/ordo-fn`,
the `fn-cache` volume. A restart loads it in about 30 ms. A new Ordo
release compiles once and replaces the file. `off` turns the cache off.
See `docs/settings.md`.

## Secrets

An app secret is a key or token a function sends to an outside API, such as
an OpenAI key. The function never holds the value. It holds a handle, and
the egress kernel puts the value into the request as it leaves.

### Declare it in the program

```ordo
secret openaiKey for api.openai.com
secret mistralKey for api.mistral.ai, eu.mistral.ai
```

- The name is how a function asks for it.
- The hosts are the only places the value is ever sent.
- A host is an exact name. A URL, a port or a wildcard is refused
  (`invalid_secret_host`).
- A host whose first label starts with a digit is written in quotes:
  `secret k for "1password.example"`.
- One line per name. A second line for the same name is `duplicate_secret`.
- A binding is part of the program, so changing a host is a new version.

### Set the value

The deployment owner sets it through the door. The value is read from
standard input, never from the command line:

```sh
ordo secret set openaiKey --host <h> --space <s> < openai-key.txt
```

- The value is sealed under the deployment's keys in `$secrets`, in the
  control database.
- Each set is a new version.
- Setting needs a passkey touch by default, like every platform change.
- The deployment's security record gets one `secret` entry per set: the
  name, the version and who set it. Never the value.
- A value may be set before the program declares the secret. It is sent
  nowhere until a declaration binds it.

### A signing key

A secret declared with no host is a signing key:

```ordo
secret githubHookKey
```

- It is sent to no host. A `fetch` that puts it in a header is refused
  with `secret_host_mismatch`.
- A function that lists it signs or verifies an HMAC with it. The host
  does the HMAC.
- A secret bound to hosts can be used for an HMAC too.

### Checking a webhook's signature

GitHub signs each delivery with HMAC-SHA256 over the body. It sends
`X-Hub-Signature-256: sha256=<hex>`. The function checks it with a key
the host holds:

```ordo
secret githubHookKey

function checkGithubHook in repos {
  call($body: text(20000), $signature: text(200)) -> json
  code "fn/github-hook.js"
  secrets githubHookKey
}
```

```js
const unhex = (s) => new Uint8Array((s.match(/../g) || []).map((h) => parseInt(h, 16)));

export default {
  async call(input) {
    const key = await crypto.subtle.importKey(
      "secret", "githubHookKey", { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
    const [scheme, hex] = input.signature.split("=");
    if (scheme !== "sha256" || !hex) return { valid: false };
    const valid = await crypto.subtle.verify(
      "HMAC", key, unhex(hex), new TextEncoder().encode(input.body));
    return { valid };
  },
};
```

- `importKey("secret", name, ...)` takes the secret's name, or
  `ctx.secret(name)`. The hash is SHA-1, SHA-256, SHA-384 or SHA-512.
- The key holds only the name. `sign` and `verify` go to the host, which
  opens the value, computes, and answers the MAC or `true` or `false`.
- `verify` compares in constant time.
- `extractable` must be `false`. `exportKey` on the key throws
  `InvalidAccessError`.
- The checks are those of `fetch`. A secret the function does not list is
  `secret_not_listed`. One the program does not declare is
  `secret_unknown`. One with no value is `secret_not_set`, and the message
  names `ordo secret set <name>`.
- A refusal rejects with a `SecretRefused` error whose `code` is that word.
  Uncaught, it ends the call as `egress_refused`, and the run's outcome is
  the word.

Until webhooks arrive (`plan-backend-functions.md` §12), the page or a
relay passes the body and the header in as parameters.

### List and remove

```sh
ordo secret ls --host <h> --space <s>
ordo secret rm openaiKey --host <h> --space <s>
```

- `ls` shows each name, its bound hosts, its version, and who set it when.
- No command and no op reads a value back.
- `rm` removes the value. A function that uses it is then refused with
  `secret_not_set`.

The ops behind these are `std.deployment.setSecret`,
`std.deployment.secrets` and `std.deployment.removeSecret`. They answer only
the deployment owner, only in the deployment's own space.

### Per environment

Each deployment is one environment, with its own control database and its
own keys. Set each secret in each deployment. Staging's value never reaches
production.

### What a function's fetch is refused for

Every refusal comes back with one reason word:

| Word | Meaning | What to do |
|---|---|---|
| `host_not_allowed` | the host is not on the function's `egress` list | add it to the function |
| `host_denied` | the host resolves inside the deployment | use a public host |
| `secret_not_listed` | the function does not list the secret | add it to the function's `secrets` |
| `secret_unknown` | the program declares no such secret | add a `secret` line |
| `secret_not_set` | no value is set in this deployment | `ordo secret set <name>` |
| `secret_host_mismatch` | the secret is not bound to this host | bind the host, or call the bound one |
| `body_too_large` | a request or response body is over its limit | send or ask for less |
| `timed_out` | the request took longer than its time | try again |

`secret_host_mismatch` is checked before any DNS lookup. So code steered to
another allowed host still cannot carry the key there.

### Limits

| Limit | Default |
|---|---|
| Request body | 10 MB |
| Response body | 25 MB |
| One request's time | 60 s |
| A secret's value | 64 KiB |

A redirect is returned to the function as an answer. It is never followed.
If an answer repeats a secret's value, the value is replaced with `[secret]`
before the function sees it. A value in the function's own answer, log
line or error message is replaced the same way.

## Deploying a function

`ordo apply` deploys the code with the program:

```
ordo apply examples/notes-v2 --db <database>
```

1. It reads each `code` file and refuses a missing one or one over 20 MB.
2. It puts each file's SHA-256 in the program's manifest, so a change to
   the code alone is a new version.
3. It stores the code in `$fncode` by that hash, then stores and activates
   the version.

Each server loads the active version's functions at boot and on every
deploy. New calls run the new code; calls already running finish on the
old one. A rollback brings its own code back, because the old version's
code is still stored by hash.

An apply over `--host` does the same through the door:

- The CLI reads and hashes each code file and sends it with the program.
- The door hashes each file again. A file that is missing or hashes to
  something else is refused as `function_code_refused`, naming the file.
  Apply again.
- The door stores each file by hash before the version that names it.
- Over `--host` all code files together may be at most 1,048,576
  characters, because the door takes a body of at most 2 MB. A larger
  bundle goes through `ordo apply --db`.

A secret with no value does not block an apply, on either path. The
function answers `secret_not_set` until `ordo secret set` runs.

### A new version of the program

What an apply of a new version does to the functions:

- A changed code file is a new hash. Every tenant runs the new code once
  the version is served.
- A secret's value is the deployment's, not the version's. It stays set
  and keeps its version number. It is never read back.
- `$fnruns` keeps every run from earlier versions. Each row names the
  version it ran under.
- A function the new version no longer declares answers `access_denied`
  (403), the same answer as a name that never existed. The apply does not
  refuse the removal. Pages that still call it get that refusal.
- Renaming a function is a removal and a new function. The old name
  answers `access_denied`. The new name answers once the version is
  served. Its runs start under the new name; the old runs keep the old one.
- A rollback brings the old names and the old code back, because each
  version's code stays stored by hash.

Upgrading Ordo itself changes none of this. The new binary serves the
functions, secrets and run records the old one left.
`crates/ordo-host/tests/upgrade_continuity.rs` checks both kinds of
upgrade.

## Calling a function

The page calls `POST /fn/<name>` with the parameters as JSON, the same
way it calls a write op:

```
POST /fn/suggestTitle
Authorization: Bearer <token>
x-ordo-space: <tenant>
Idempotency-Key: <optional>
Content-Type: application/json

{ "text": "milk, eggs, bread, coffee" }
```

The answer is the function's JSON under `result`:

```json
{ "result": { "title": "Groceries for the week" } }
```

From the SDK, the client makes this call and throws the usual `OrdoRefusal`
on a refusal:

```ts
const { result } = await client.callFunction<{ title: string }>('suggestTitle', {
  text: note.body,
});
```

In React, `useFunction('suggestTitle')` returns `call`, `busy` and `error`.
`call` answers the result, or `null` after a refusal. `error.message` is the
door's words. Notes v2 uses it for its "Suggest title" button and shows the
message under the title field. With `llmKey` unset the message names
`ordo secret set llmKey`.

The SDK sends no `Idempotency-Key` unless you pass `idempotencyKey`. It
checks the name and that the parameters are JSON before the wire. The
interface descriptor does not list functions yet, so the generated code has
no typed call per function. You give the name and the result type.

The door runs the op door's steps first: sign-in, the access check, quotas,
the tenant binding and parameter types. `by member` lets every role in the
tenant call it: viewer, editor, manager and owner. A guest, who holds no
role, may not. `ORDO_ACCESS_TEMPLATE` may name a function as it names an
op. An unknown function name gets the
same `access_denied` as a function the caller may not call. A paused tenant
answers `space_paused` with its pause answer, unless the function says
`nopause`.

The call runs as the signed-in person. In this increment a function
reaches no ops, so that identity shows in the run record:
`function:suggestTitle@<version> on behalf of <person>`.

With an `Idempotency-Key`, a repeated call with the same body gets the
first answer back and the function does not run again. The same key with a
different body is refused `idempotency_mismatch`. A failed call frees the
key, so a retry runs again.

### Refusals

| Class | Status | Means |
|---|---|---|
| `function_error` | 500 | The code threw, or answered something that is not JSON or is too large. |
| `function_timeout` | 504 | It ran past its wall time or its CPU limit. |
| `function_oom` | 500 | It needed more memory than its limit. |
| `function_busy` | 429 | Too many calls are running. Retry after the `Retry-After` seconds. |
| `egress_refused` | 400 | An outbound call was refused, and the code let it end the call. The message names the reason. |

### What is recorded

Each call writes one row to `$fnruns` in the deployment's database: the
function, the version, the tenant, who called, the wall time, CPU, peak
memory, the number of outbound calls and their hosts, bytes sent and
received, the outcome class and the log. The input, the answer and request
bodies are never recorded. The log is scrubbed of live secret values.

## Runs and logs

Each call is one run. The deployment owner lists them and reads each one's
log through the door:

```sh
ordo fn runs --host <h> --space <s>
ordo fn runs suggestTitle --limit 20 --host <h> --space <s>
ordo fn log 412 --host <h> --space <s>
```

- `runs` lists the newest runs first: the run number, when it started, the
  function, its version, the tenant, the outcome class, the wall time in
  milliseconds, the number of log lines, and who it ran as.
- A name shows only that function's runs. `--limit` is 50 by default and
  500 at most.
- `log <run>` shows one run: when, who, the outcome and its message, wall
  time, CPU, peak memory, outbound calls and their hosts, then each log
  line with its level.

### What a log holds

- The lines the function wrote with `console.log` (`info`),
  `console.warn` (`warn`) and `console.error` (`error`).
- Every live secret value is replaced with `[secret]`.
- At most 64 KB per run. The rest is dropped, and the last line says the
  log was truncated.
- The input and the answer are in the log only when the function logs them
  itself. Ordo never writes them there.

### Who may read them

Only the deployment owner, in the deployment's own space. Both are reads,
so they need no passkey touch. The ops are
`std.deployment.functionRuns($function?, $tenant?, $limit?)` and
`std.deployment.functionRun($seq)`. A tenant cannot read its own runs yet.

### How long they are kept

14 days. The `functions.runs.sweep` worker deletes older runs, with their
logs, once an hour. `ORDO_FN_RUN_RETENTION_DAYS` changes it, from 1 to 3650
days. Any other value refuses the boot and names the setting.
