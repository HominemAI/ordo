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
| `secrets name, ...` | The secrets it may use, by name. Each must be a declared `secret`, bound to at least one of its `egress` hosts; `ordo check` refuses it otherwise, naming both lists. |
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
WebAssembly. It runs ES2023 with promises and `async`. It has no Node APIs,
no `setTimeout`, and no `TextEncoder` or `crypto.subtle` yet.

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
before the function sees it.

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

An apply over `--host` refuses a program with functions for now: the
door's apply does not carry code files yet. Use `--db`.

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

The door runs the op door's steps first: sign-in, the access check, quotas,
the tenant binding and parameter types. An unknown function name gets the
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
