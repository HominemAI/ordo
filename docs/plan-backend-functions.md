# Backend functions: custom code the deployment runs

Status: **plan, 2026-10-08.** Nothing built. Board `backend-functions` (18aa),
step 4 of the onboarding order ruled 2026-10-08. The owner's decisions are
the numbered questions at the end.

The owner's words: *"running actual functions and custom code in the backend,
deployed through code and infrastructure as code, to fill the last gaps of a
backend as a service."*

---

## 0 · The plan on one screen

**What a function is.** A piece of the app developer's code, shipped inside
the program, that the deployment runs on request, on a schedule, on a change
or on an inbound webhook.

**The v1 recommendation.**

- **Run it as WebAssembly, inside the deployment.** Wasmtime, with a built-in
  JavaScript engine, so a function is a plain bundled JS file.
- **It can do four things and no more:** call out through the egress kernel,
  use a secret by name, call the program's own ops, and log.
- **It ships with the program.** `ordo apply` stores the code by hash beside
  the compiled ops. One version of the program is one version of its code.
- **Secrets are sealed by the deployment's custody** and set per environment.
  The function never sees the value. The host puts it into the outbound
  request, and only to the hosts the secret is bound to.
- **It runs as whoever woke it.** The calling person by delegation, a
  `function:` principal on a schedule or change, a `source:` principal on a
  webhook.

**The first increment** lets ellanor move one real function: a page-called
function with JSON in and out, one secret and egress. `classify-notes` fits.

**What the apps actually need** (survey of 2026-10-08):

- Ellanor has about 40 real endpoints. Nearly all hold a server key, call one
  outside API and return JSON. The client writes the result back.
- None streams to the browser except an SSE passthrough to Ellapi.
- There is no inbound webhook and no scheduled function. Ellanor's sweeps live
  in its separate Ellapi server.
- Denizens has 6 edge functions, not 5. Same shape, plus Supabase admin calls.

---

## 1 · What the apps need

### 1.1 Ellanor

About 85 route files under `ellanor/functions`. About 40 are endpoints; the
rest re-export a shared proxy.

| Group | Count | Shape | Secrets | Result |
|---|---|---|---|---|
| LLM calls (`classify-notes`, `compare-quotes`, `project-digest`, `translate` and others) | 13 | JSON in, JSON out, slow, no stream | OpenAI, Anthropic, local model URL and key | returned; the client writes it |
| OCR and extraction (`extract-document`, `extract-with-schema`, `extract-quote`) | 3 | multipart file up to 20 MB in, JSON out | Mistral, LLM keys | returned |
| Vision and segmentation (`fortress-trace`, `fortress-sam`) | 2 | multipart file in; SAM polls Replicate or a local server | Replicate, local SAM token | returned |
| Web search | 1 | JSON in, JSON out | a SearXNG URL from config | returned |
| Google Calendar, Contacts, Mail proxies | 3 | the person's refresh token in the body, an action | Google client id and secret | returned |
| GitHub OAuth (start, callback, refresh, config) | 4 | redirect flow, HMAC-signed state | GitHub App id, secret, state secret | tokens handed to the client |
| Google OAuth start and callback | 2 | redirect flow | Google client id and secret | refresh token to the client |
| `notify`: Signal and email | 1 | JSON in, side effect | signal-cli URL and token, Resend key | sent |
| Storage (R2 upload and serve) | 2 | multipart up to 40 MB | R2 binding | key returned |
| Ellapi proxies | about 15 | forward with a server bearer; one SSE passthrough | Ellapi URL and token | returned |

What this tells the design:

- **Return-only covers most of it.** A function that returns JSON is enough
  to move the LLM, OCR and Google groups.
- **Files are the second need.** OCR and vision take uploads up to 20 MB.
- **Private hosts are real.** A local model and a local SAM server sit on the
  owner's network, likely on plain `http`.
- **Per-person OAuth tokens are held by the client today.** The function adds
  only the app's client secret.
- **Auth is mostly missing in the functions.** Cloudflare Zero Trust guards
  them. Ordo's door would guard them by default.
- **Streaming is a small need.** Only the agent events passthrough streams.
  Ellanor's chat does not stream through a function today.
- **The Ellapi proxies are not functions to port.** They exist because Ellapi
  is a second backend. They go away as Ellapi's data moves into Ordo.

### 1.2 Denizens

Sources are in `omni-agent-platform/supabase/functions/`. Six are called.

| Function | Shape | Writes | Auth |
|---|---|---|---|
| `username-auth` | sign-up or sign-in with a username | Supabase Auth | none |
| `face-chat` | one-sentence OpenAI reply with memory | both turns to `face_chat_turns` | JWT |
| `face-tts` | OpenAI speech, returns an MP3 | none | JWT |
| `admin-invite` | invite users, assign roles | roles and subscriptions | JWT plus admin |
| `generate-api-key` | mint a key, store its hash | `denizens.api_key_hash` | JWT plus admin |
| `fetch-models` | list models from 3 providers | none; the client inserts | JWT plus admin |

Three of these are not functions in Ordo. `username-auth` is sign-in
(onboarding step 5). `admin-invite` is the membership ops. `generate-api-key`
is the API key work in `design-public-path.md` §7.

---

## 2 · What Ordo has to build on

| Piece | State | What it gives functions |
|---|---|---|
| Routines (`howto-routines.md`) | built 2026-10-08 | schedule and change triggers, per-tenant runs, retries, dead letters, pause skips |
| Delegation (`howto-workers.md`) | built | a worker acting for a person, `agent:x on behalf of user:y` in the chain |
| `ordo-egress` | built for attestation | 6 pins: https, resolve-then-pin, no redirects, signing, holds-nothing, allowlist |
| Custody and sealed values (`howto-custody.md`) | built | envelope sealing under the deployment's key provider |
| Usage ledger and quotas | built | per-tenant counters at the door, `ordo quota set` |
| Worker slots (`ORDO_WORKERS`) | built | run one role, and only it, in its own pod |
| `ordo-wasm` | built | **the compiler for the browser.** It is not a WASM runtime and gives functions nothing |
| `policy llm-egress { redact [...] }` | spec only, unparsed | a later home for redaction rules on model calls |

Two facts shape the plan.

**The settled spec says no foreign code in v1.** `definition-language.md` §8,
Tier 2: *"The platform hosts no foreign code in v1 (in-process WASM hosting
deferred until third-party in-process logic is actually wanted)."* The
owner's words on `backend-functions` ask for exactly that. This plan treats
his words as the newer ruling, and question 1 asks him to confirm.

**The egress kernel returns nothing.** `deliver` hands back a status code and
drops the body, by design (SSRF exfiltration). A function calling a model
needs the body. Functions need a second entry point (§6).

---

## 3 · What a function is: the options

| Option | Isolation | Tenant safety | Cost to build | Cost to run | IaC fit | Ellanor port |
|---|---|---|---|---|---|---|
| **A. WASM in the deployment** (wasmtime, JS engine as a component) | strong: no I/O except what the host gives | fresh instance per call, no shared memory | L | low: about 1 ms start from a snapshot | code ships in the program | small: Web `fetch`, `Request`, `Response` |
| B. JS isolates (V8 through `deno_core`) | good, but V8 bugs want a process sandbox too | same | L, plus a 40 MB dependency | low | same | smallest: nearly Workers |
| C. Containers (a pod per function) | strongest | strong | XL: images, a scheduler, cold starts | high | the deployment owner runs more infrastructure | any language |
| D. Out-call to code hosted elsewhere | none needed in Ordo | the app's problem | S | the app pays elsewhere | split: code and keys live outside Ordo | none: it stays on Cloudflare |
| E. In-process Rust plugins | none | none | M | lowest | needs a rebuild of the binary | rewrite |

**Recommendation: A for v1, with D kept as the existing escape hatch.**

- **A is the one that keeps the promise.** The code, its secrets' names, its
  hosts and its limits all live in the program. One `ordo apply` deploys all
  of it.
- **The language stays pure.** `.ordo` gains no I/O. A routine still cannot
  misbehave. The code that can call out is a separate, named, bounded thing.
- **WASM gives the limits for free.** Memory caps, CPU fuel, epoch timeouts,
  and no socket, file or clock the host does not hand over.
- **It is language-neutral.** JS first, because both apps are JS. Any WASI
  component (Rust, Go) later, with no new runtime.
- **D already exists.** A worker holding a delegation calls ops today. An app
  that wants to keep code elsewhere keeps doing that. Nothing new to build.
- **B was close.** It ports faster. It also ties Ordo to one language and
  pulls V8 into the binary. A's JS engine (SpiderMonkey compiled to WASM,
  from the Bytecode Alliance) runs the same Web APIs more slowly, and these
  functions spend their time waiting on the network.
- **C belongs to the deployment owner, not to Ordo.** A function that truly
  needs a container is option D with the container as the host.

**The trust model.** The app developer writes the code. It has the trust of
the program, not of a tenant. Isolation protects against three things:

- a bug in one call reaching another tenant: a fresh instance per call;
- data steering the code to leak a key: secrets are injected by the host and
  bound to hosts (§5);
- one function starving the deployment: limits (§9).

**Mechanism, not policy.** The deployment owner decides whether functions run
at all, where (`ORDO_WORKERS=functions` in its own pod, or in every process),
which hosts may be reached, and the ceilings. The app developer decides
everything inside those.

---

## 4 · Declaring, versioning and deploying

### 4.1 The declaration

A `function` block, beside `op` and `routine`. Its exact grammar is question
3; this is the recommended shape.

```ordo
secret openaiKey   for api.openai.com
secret mistralKey  for api.mistral.ai

function classifyNotes in notes {
  call($notes: json) -> json by member     # from the page; who may call
  code "fn/classify-notes.js"
  egress api.openai.com
  secrets openaiKey
  limit wall 60s, memory 128mb
}

function nightlyDigest in notes {
  trigger every day 07:00Z                 # the routine trigger grammar
  code "fn/digest.js"
  egress api.anthropic.com
  secrets anthropicKey
  uses openProjects, saveDigest            # ops it may call
}

function githubPush in repos {
  webhook github verify hmac-sha256 "X-Hub-Signature-256" with githubHookSecret
  code "fn/github-push.js"
  uses recordPush
}
```

- **One wake-up per function:** `call`, `trigger` or `webhook`.
- **`code`** names a bundled ES module, or a `.wasm` component.
- **`egress`** lists the hosts. A host not listed is refused before any DNS.
- **`secrets`** lists the secrets it may use, by name.
- **`uses`** lists the ops it may call. Nothing else is reachable.
- **`limit`** sets limits within the deployment's ceilings.
- `nopause` works as it does on ops and routines.
- The CDK gets `fn(...)` with the same fields, synthesising the same text.

### 4.2 The code

```js
// fn/classify-notes.js
export default {
  async call(input, ctx) {
    const r = await fetch("https://api.openai.com/v1/chat/completions", {
      method: "POST",
      headers: { authorization: ctx.secret("openaiKey").bearer(),
                 "content-type": "application/json" },
      body: JSON.stringify({ model: "gpt-4.1-mini", messages: build(input.notes) }),
    });
    return parse(await r.json());
  },
};
```

- `fetch`, `Request`, `Response`, `ReadableStream`, `TextEncoder` and
  `crypto.subtle` are the Web standard ones.
- `ctx.secret(name)` returns a handle, never a string (§5).
- `ctx.op(name, args)` calls an op from `uses` (§7).
- `ctx.log`, `ctx.usage(quantity, n)`, `ctx.tenant`, `ctx.trigger`.
- A small adapter accepts Cloudflare's `onRequest(context)` shape, so a
  Pages Function moves with few edits (§11).

### 4.3 Versioning and deploy

| Step | What happens |
|---|---|
| `ordo check` | compiles the block, checks `uses` names real ops, checks secret names are declared, lists FUNCTIONS |
| `ordo apply` | reads each `code` file, checks size (default 20 MB), hashes it, stores it in the program registry by hash, beside the compiled ops |
| The manifest | per function: code hash, `egress`, `secrets` names, `uses`, wake-up, limits. A change to any of these is a new manifest hash |
| A hot deploy | new calls run the new code. Calls in flight finish on the old one |
| Rollback | the old version's code is still stored by hash, so it comes back with it |
| Environments | dev, staging and production each run their own version's code. Branch serving works the same way |
| Missing secret | `ordo apply` names each declared secret with no value in the target environment. The function answers `secret_missing` until it is set |

The code is in the artifact. It is reviewed, diffed and committed like the
`.ordo` text, which is what "deployed through code" means here.

---

## 5 · Secrets

**Recommendation: sealed under the deployment's custody, stored outside any
chain, set per environment by the deployment owner.**

| Question | Answer |
|---|---|
| Where the value lives | a control-database table `$secrets`, each value sealed with the deployment's key provider, like a sealed field |
| Who sets it | the deployment owner, with `ordo secret set <name> --env prod` reading the value from stdin or a file, or `std.deployment.setSecret` (presence required by default) |
| Can it be read back | no. `ordo secret ls` shows name, version, set-at and a short keyed fingerprint |
| What the chain holds | that secret `openaiKey` moved to version 3, by whom. Never the value |
| Other sources | a file or a KMS reference instead of a stored value, per the D-H ruling (*"KMS first with optional environment or file"*). A setting chooses |
| Per environment | yes. Staging's value is never production's. `plan-environments.md` §3 already says environments differ only in secrets |

**The function never holds the value.**

- `ctx.secret("openaiKey")` returns an opaque handle.
- A handle may be placed in a request header or a body template.
- The host replaces it with the value as the bytes leave, inside the egress
  kernel.
- It does so **only if the destination is a host the secret is bound to**
  (`secret openaiKey for api.openai.com`). Any other host is refused with
  `secret_host_mismatch`.
- So a prompt injection that steers the code to another allowed host still
  cannot carry the key there.
- The guest cannot print it, return it or write it, because it never has it.
- As a second guard, every log line is scrubbed of each live secret value.

**Per-person secrets are data, not settings.** Ellanor's Google refresh
tokens and a person's own OpenAI key belong to the person. They are sealed
fields in the app's own entities. The function reads them through an op, as
the person, and passes them as a handle too (`ctx.sealed(value)`), so they
get the same host binding. The app's Google client secret is a deployment
secret.

---

## 6 · Outbound calls

A second entry point in `ordo-egress`, `fetch`, beside `deliver`.

| Pin | `deliver` (sinks) | `fetch` (functions) |
|---|---|---|
| 1 · https, TLS 1.2 floor | yes | yes |
| 2 · resolve, then connect to the pinned address | yes | yes |
| 3 · no redirects | yes | yes. A 3xx is returned to the function, which may fetch again, checked again |
| 4 · signed | yes | replaced by secret injection |
| 5 · holds nothing | yes | yes: the crate still cannot name the key provider or the pool. The host unseals and passes the bytes |
| 6 · allowlist, before DNS | per-space consented | the function's `egress` list, intersected with the deployment's allowlist |
| The response | status only | the body, bounded, readable as a stream |

**Why the body may come back here.** The sink rule stops a tenant naming a URL
and reading what answers. A function's hosts are named in the program and
approved by the deployment. Data can choose a path, never a host.

**Limits, all settings with defaults:**

| Limit | Default | Layer |
|---|---|---|
| Requests per call | 50 | deployment ceiling, function may lower |
| Request body | 32 MiB (`ORDO_FN_FETCH_BODY_BYTES`), so a full 20 MiB file fits as base64 | same |
| Response body | 25 MB | same |
| One request's time | 60 s | same |
| Concurrent requests per call | 6 | same |

**Private hosts.** The deny list refuses loopback, RFC1918 and the pod range,
and no setting turns a category off. Ellanor's local model and SAM server are
on a private network, likely on `http`. Two mechanisms, both deployment-owner
choices, both question 6:

- **`ORDO_EGRESS_PRIVATE_HOSTS`**: exact host names the deployment owner lets
  through the deny list. It is the `ORDO_EGRESS_OPERATOR_HOSTS` that
  `plan-auth-federated.md` already plans for a private mail relay. One list,
  not two.
- **Plain `http` only for those named private hosts.** Never for a public one. Still planned.

**Built 2026-10-08** (commit `3af386be`): the list is in `ordo-egress` (`DenyList::check_host`). It takes exact names. It opens loopback, RFC1918, unique-local, CGNAT and the deployment's own ranges, never link-local, multicast or reserved. The mail channel reads it today. Functions do not yet.

**Refusals** come back to the function as `egress_refused` with one reason
word. Same rule as the kernel: no reason that turns the deny list into an
oracle.

---

## 7 · Triggers, identity and results

| Woken by | Runs as | Can call | Chain attribution | Result goes |
|---|---|---|---|---|
| **The page**, `POST /v1/fn/<name>` | the caller. A delegation from the caller to `function:<name>@<version>`, for this call only | ops in `uses`, intersected with what the caller may call | `function:<name>@<v> on behalf of user:<id>` | returned to the caller, and any writes it makes through ops |
| **A schedule** (`trigger every ...`) | `function:<name>@<version>`, with routine rules: guards pass | ops in `uses` | `function:<name>@<v>` | writes through ops; the return value is logged as a status |
| **A change** (`trigger when ...`) | same, bounded to the changed rows and one ref away, unless `unguarded` | ops in `uses`, narrowed | same | same |
| **An inbound webhook** | `source:<name>`: write-only, as the spec's source principals | write ops in `uses` only | `source:<name>` | writes; the HTTP answer the function returns, for senders that need one |

**From the page.** A function in the interface descriptor is called like an
op, through the same door:

- sign-in, the access check (`by member`, a role list, or a guard on an
  argument), quotas, `Idempotency-Key`, the pause check;
- arguments are typed like op parameters;
- JSON in, up to 1 MB by default; JSON or a stream out (§8).

The per-call delegation needs no consent ceremony. The person pressing the
button is the consent, it lasts one call, and it cannot reach further than
the person can. Question 7.

**From a routine.** The trigger engine built in 12a enqueues one run per
tenant. A function run takes the place of a write body. Retries (1, 2, 4, 8,
16 s), dead letters and pause skips are the same code. Runs are
at-least-once, so the function must be safe to run twice. `ctx.runId` is
stable across retries, for use as an idempotency key outside.

**From a webhook.** `POST /hooks/<tenant>/<name>`, or `/hooks/<name>` for the
deployment's own space.

- The host verifies the signature before the guest starts. Built-in forms:
  `hmac-sha256` over the body with a named header (GitHub, most senders), and
  `bearer` (a shared token). A sender with its own scheme verifies in code.
- A failed check answers 401. No code runs.
- The tenant id in the path is not a secret. The signature is the
  credential, and its secret can be per tenant.
- Answer within 10 s by default; longer work hands off by writing a row a
  change trigger picks up.

**Results.** A function's return value goes to whoever called it. Ordo never
writes a return value on its own. Durable results are written by the
function through ops, so they carry an attribution and land in the chain like
any other write. Ellanor can keep writing results from the client at first,
then move the write into the function.

---

## 8 · Streaming

- A page-called function may return a `ReadableStream`, or write to
  `ctx.stream`.
- The door relays it as `text/event-stream` (SSE) or as chunked bytes,
  chosen by the function's declared return type (`-> stream sse`).
- `fetch` reads a response as a stream, so a model's SSE can be passed
  through with no buffering.
- A stream has its own caps: time (default 5 min) and bytes (default 10 MB).
- The client SDK gets `callStream`, and `@hominemai/ordo-react` a hook.
- Writes made during a stream land as they are made. A final write at the end
  (the full message) is the usual shape.

Neither app streams through a function today. Streaming is increment 3, not
the first.

---

## 9 · Limits, metering and pause

All are settings with defaults. The deployment owner sets ceilings; a
function's `limit` sets values within them.

| Limit | Default | How it is enforced |
|---|---|---|
| Memory per call | 128 MB (ceiling 512 MB) | wasmtime memory limit; over it is `function_oom` |
| CPU per call | 10 s of CPU | wasmtime fuel, checked by epoch; waiting on the network does not count |
| Wall time per call | 60 s page, 15 min schedule or change, 10 s webhook | epoch interruption; over it is `function_timeout` |
| Concurrent calls per tenant | 10 | a semaphore; a page call over it answers 429 `function_busy` |
| Concurrent calls per deployment | 100 per process | same |
| Code size | 20 MB | `ordo apply` refuses |
| Log per call | 64 KB | truncated, marked |

**Metering.** New ledger quantities, counted where they happen, so a quota on
them is exact:

| Quantity | Counted |
|---|---|
| `function_calls` | at the door or at enqueue |
| `function_ms` | wall time, per call |
| `function_cpu_ms` | fuel used, converted |
| `egress_bytes` | sent plus received |
| custom, by `ctx.usage("llm_tokens", n)` | the app developer's own quantities, declared in the program, so a typo refuses the apply |

Custom quantities let ellanor meter model tokens per tenant through the same
ledger and quotas. Question 9.

**A paused tenant.**

| Woken by | While paused |
|---|---|
| The page | refused `space_paused`, with the tenant's pause answer, unless the function says `nopause` |
| A schedule or change | skipped, finished, recorded in `$routineskips`, unless `nopause` |
| A webhook | answered with the pause answer (default 423); no code runs, so the sender retries later |

---

## 10 · Observability

- **One row per call** in the control database, `$fnruns`: function,
  version, tenant, wake-up, principal, start, wall, CPU, peak memory, egress
  count and hosts, bytes, outcome class.
- **Not in the tenant's chain.** The chain holds the writes the function made,
  as itself or for the person. The run record is operational, like
  `$routineskips`.
- **Logs** from `ctx.log` and `console.*`, scrubbed of every live secret
  value, capped per call, kept 14 days by default.
- **Never logged by default:** request bodies, response bodies, function
  arguments and return values. A setting turns on argument logging per
  function, for development.
- **Outcome classes:** `ok`, `function_error` (a thrown error; message kept,
  stack kept), `function_timeout`, `function_oom`, `egress_refused`,
  `secret_missing`, `secret_host_mismatch`, `function_busy`.
- **Who reads them.** The deployment owner: `ordo fn logs <name>`, `ordo fn
  runs`, and `/metrics`. A tenant's own runs: a platform op, closed by default,
  opened by the app developer.
- **Metrics:** calls, errors by class, duration histogram, egress refusals,
  per function, with the tenant label only when `ORDO_METRICS_PER_SPACE` is on.

---

## 11 · Moving the apps

### 11.1 Ellanor: `api/classify-notes` (LLM, return only)

| Today | In Ordo |
|---|---|
| Pages Function, POST JSON, reads `OPENAI_API_KEY` from `env`, returns JSON; the client writes the classes into Jazz | `function classifyNotes { call($notes: json) -> json by member; egress api.openai.com; secrets openaiKey }` |
| No auth in the function; Zero Trust in front | the door: signed in, a member of the tenant |
| Provider switch in `_lib/llm` | bundled with the function by esbuild; `env.OPENAI_API_KEY` becomes `ctx.secret("openaiKey")` through the adapter |

Moves with increment 1 alone.

### 11.2 Ellanor: `api/extract-document` (OCR, a file in)

| Today | In Ordo |
|---|---|
| multipart upload up to 20 MB, Mistral OCR with `MISTRAL_API_KEY`, JSON back | the page uploads to the blob door first, then calls `extractDocument($file: blob)` |
| | `ctx.blob(file)` reads the bytes as the caller; `egress api.mistral.ai`; `secrets mistralKey` |
| | returns the extracted JSON; later, writes it with `uses saveExtraction` |

Moves with increment 2 (blob input).

### 11.3 Ellanor: `api/google-calendar/proxy` (a person's OAuth token)

| Today | In Ordo |
|---|---|
| the client sends its Google refresh token in the body; the function adds the client id and secret, refreshes, calls Calendar, returns JSON | the refresh token is a sealed field on the person's own `googleAccounts` row, written once at OAuth callback |
| the token lives in the browser | `uses myGoogleAccount`; `ctx.op` reads it as the person; `ctx.sealed(token)` keeps it a handle |
| | `secret googleClientSecret for oauth2.googleapis.com`; `egress oauth2.googleapis.com, www.googleapis.com` |

Moves with increment 4 (ops from functions). The OAuth callback itself is a
page-reachable function returning a redirect, or the platform's own OAuth
sign-in (onboarding step 5) if that covers connected accounts. Question 10.

### 11.4 Denizens: `face-chat` (LLM, writes back)

| Today | In Ordo |
|---|---|
| Supabase JWT, reads `platform_config` and the last turns with the service role key, calls OpenAI, writes both turns to `face_chat_turns`, returns `{reply, model}` | `function faceChat { call($message: text, $denizen: id) -> json by member; uses recentTurns, addTurns, faceConfig; egress api.openai.com; secrets openaiKey }` |
| the service role key bypasses row security | no bypass: it reads and writes as the caller, through ops the caller may call |
| | the turns land in the chain as `function:faceChat@v on behalf of user:<id>` |

Moves with increment 4. The other denizens functions map elsewhere (§1.2).

### 11.5 What stays out

- **Ellapi proxies.** They go when Ellapi's data moves to Ordo.
- **Storage.** The blob door replaces it.
- **`notify`.** Email is the platform's mail (`plan-auth-federated.md`).
  Signal is a function with a private host (question 6) until channels exist.
- **Ellapi's sweeps.** Approval expiry and retention become routines, with no
  code.

---

## 12 · Increments

Each ships alone and is useful alone.

| # | Increment | Done when | Tier | Size |
|---|---|---|---|---|
| 1 | **A page-called function.** `secret` and `function` blocks (`call` only); `$secrets` sealed store and `ordo secret set/ls/rm`; `ordo-egress` `fetch` with host-bound secret injection and body caps; wasmtime with the JS engine component; code stored by hash at apply; `POST /v1/fn/<name>` through the door with auth, access, quotas, idempotency and pause; JSON in and out; limits; `$fnruns` and scrubbed logs | ellanor's `classify-notes` runs in Ordo and its page calls it | Opus | L |
| 2 | **Files in.** `$x: blob` arguments read through the blob door as the caller; request body caps | `extract-document` moves | Opus | S |
| 3 | **Streaming.** `-> stream sse`, `fetch` streaming reads, SDK `callStream`, React hook, stream caps | a model's SSE reaches the page unbuffered | Opus | M |
| 4 | **Ops from functions.** `uses`, `ctx.op`, the per-call delegation, attribution, `ctx.sealed` | `face-chat` writes its turns as the person | Opus | M |
| 5 | **Schedule and change.** `trigger` on a function through the 12a engine; `function:` principal; retries, dead letters, pause skips | a nightly digest runs per tenant | Opus | M |
| 6 | **Inbound webhooks.** `webhook` wake-up, `/hooks/...`, `hmac-sha256` and `bearer` verification, `source:` principal | a GitHub push lands as a row | Opus | M |
| 7 | **Metering.** `function_calls`, `function_ms`, `function_cpu_ms`, `egress_bytes`, declared custom quantities, quotas on them, concurrency caps as settings | a quota on `llm_tokens` refuses at the door | Sonnet | M |
| 8 | **Private hosts.** `ORDO_EGRESS_PRIVATE_HOSTS`, `http` for those only | ellanor's local model answers | Opus | S |
| 9 | **Developer loop.** CDK `fn(...)`; `ordo dev` runs functions locally with a dev secret file; the Cloudflare `onRequest` adapter; `ordo fn logs`, `ordo fn runs`; howto | a Pages Function moves with only the declaration and the `env` lines changed | Sonnet | M |
| 10 | **Any WASI component.** `code "x.wasm"` for Rust or Go, same host interface | a Rust function runs | Sonnet | S |

**The smallest first increment that moves a real ellanor function is 1.** It
is large because a function with no secret or no egress is useless to these
apps. It can be built in three commits inside the increment: the secrets
store, then `fetch`, then the runtime and the door.

Placement: the board files `backend-functions` as future work, and the
onboarding order asks for the plan now. Which increments come before the
first deployment is question 2.

---

## 13 · Questions for the owner

1. **Does Ordo now host the app developer's code?** The spec says no foreign
   code in v1. *Recommendation: yes; your words on this task supersede that
   line, and the spec is amended to say so.*
2. **Which increments come before the first deployment?** *Recommendation:
   1, 2 and 4 now, which move ellanor's model, OCR and Google calls; the rest
   after.*
3. **Is a function a `function` block in `.ordo`, beside `op` and
   `routine`?** *Recommendation: yes, with one wake-up (`call`, `trigger` or
   `webhook`) and the code in a separate file the block names.*
4. **Does v1 run functions as WebAssembly with a built-in JS engine?**
   *Recommendation: yes; it is language-neutral, bounded by construction, and
   needs no infrastructure beyond the Ordo binary.*
5. **Are app secrets sealed under the deployment's custody and set per
   environment by the deployment owner, with file or KMS as options?**
   *Recommendation: yes, and the function only ever holds a handle the host
   fills in for the secret's bound hosts.*
6. **May the deployment owner name private hosts that functions can reach,
   on `http` for those hosts only?** *Recommendation: yes, one exact-name list
   shared with the planned mail-relay exception, so ellanor's local model and
   SAM server work.* The list is built and read by mail; plain `http` for named hosts is still planned.
7. **Does a page call run as the person, by a delegation that lasts one call
   with no consent screen?** *Recommendation: yes; pressing the button is the
   consent, and it can never reach past what the person can do.*
8. **Is a function's return value only returned, never written by Ordo on its
   own?** *Recommendation: yes; durable results are written by the function
   through ops so they are attributed in the chain.*
9. **May the app developer declare custom usage quantities, such as model
   tokens, that functions report?** *Recommendation: yes; they go in the same
   ledger and can carry quotas, which is how a tenant's model spend is capped.*
10. **Are a person's connected accounts (Google, GitHub) part of the OAuth
    sign-in work, or built by the app with functions?** *Recommendation: the
    app builds them with functions and sealed fields; sign-in stays about
    signing in.*
11. **Does a paused tenant's webhook answer with the pause answer and run no
    code?** *Recommendation: yes; the sender retries, and nothing new happens
    inside a paused tenant.*
12. **May the deployment owner run functions in their own pod?**
    *Recommendation: yes, as a worker slot (`ORDO_WORKERS=functions`); the
    default runs them in every process.*
