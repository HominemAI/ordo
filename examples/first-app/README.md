# The first Ordo app

One table, one field, displayed in a browser. The smallest thing that is
still an app, built to be walked end to end and to find
out what breaks when someone does.

Everything below was run against `target/debug/ordo` on 2026-09-26 and the
output is pasted as it came back. One thing in here was **not** run, and it
is named as such in §5.

## 1 · The whole program

[`readings.ordo`](readings.ordo), in full:

```
domain readings @ 0.1.0

entity readings {
  label: text
}

op addReading($label: text) =
  set readings { label: $label }

op listReadings =
  readings[id, label]
  sort -id
```

One shape, one write, one read. `readings` carries no `acl`, so it has no
row-level guard, which is the right shape for a first app and the wrong
shape for a second one: the moment two principals share a space, the guard
is what decides who sees which row.

`ordo check examples/first-app/readings.ordo` derives this, and the two
entities nobody declared are platform-provided on every space:

```
ENTITIES
  members  (id key: mem_)         [platform-provided]
  groupMembers  (id key: gm_)     [platform-provided]
  readings  (id key: read_)
    label: text
    guards: NONE — unguarded

OPS
  addReading     write   mode creates     reaches readings writes {label}
  listReadings   class sql   rows unbounded (pageable)   [id: id, label: text]
  std.readings.count / create / delete / get / list / update
```

**Six `std.*` ops came for free.** The moment the shape exists the platform
derives get-by-id, a paged list, create, a full-row CAS update, a delete and
a count. The app uses none of them and they cost nothing to leave alone,
which is worth seeing on a program this small: two op declarations produced
eight callable ops.

## 2 · Running it

```
ordo dev examples/first-app
```

That brings up its own disposable Postgres cluster, creates the schema and
serves on `127.0.0.1:4000`. Real output, trimmed to what matters here:

```
saved  …/examples/first-app       ✓ 18ms
       read 4 · compile 4 · delta 2 (3 changes) · reset 7 (no seed.sql, empty)
schema created — the database held none of this program's tables, so the loop created and seeded it:
A1 — new entity `members`
A1 — new entity `groupMembers`
A1 — new entity `readings`
  boot        gate + swap in 68ms
ordo-host 0.1.0  addr=127.0.0.1:4000  space=dev
  program     dev-boot  ops=8  gate: 4 of 8 registered ops shape-verified against an empty shadow
  auth        none (ORDO_AUTH_METHODS is not set)
  token       Bearer …   local only — signed by a key that dies with this process
```

Then, in a second terminal, the page. Paste the token `ordo dev` printed:

```
ORDO_TOKEN='<the token>' ORDO_SPACE=dev python3 examples/first-app/web/serve.py
```

and open <http://127.0.0.1:4100>.

## 3 · The two calls, verified

```
$ curl -s -X POST http://127.0.0.1:4000/op/addReading \
    -H 'x-ordo-space: dev' -H "authorization: Bearer $T" \
    -H 'content-type: application/json' -d '{"label":"first reading"}'

{"mode":"creates","op":"addReading","rows":[{"created":"read_w4f73ab3cca30ab8c_1","entity":"readings"}],"version":"5ef12ac378659085"}

$ curl -s http://127.0.0.1:4000/op/listReadings \
    -H 'x-ordo-space: dev' -H "authorization: Bearer $T"

{"op":"listReadings","rows":[{"id":"read_w4f73ab3cca30ab8c_4","label":"second reading"},
                            {"id":"read_w4f73ab3cca30ab8c_1","label":"first reading"}],
 "version":"3b74d38a144d1048"}
```

No route was written for either. A read is `GET /op/{name}`, a write is
`POST /op/{name}`, the space is a header and the caller is a bearer token.
That is the whole HTTP surface, for every op.

## 4 · Why there is a `serve.py`, and what it found

[`web/serve.py`](web/serve.py) is 117 lines of stdlib Python that serves
[`web/index.html`](web/index.html) and forwards `/op/*` and `/interface` to
the door. It exists for two reasons, and neither is convenience.

### Without `ORDO_CORS_ORIGINS`, a browser cannot call the door cross-origin

The door has a CORS allowlist (`ORDO_CORS_ORIGINS`, `crates/ordo-api/src/cors.rs`),
and it is off until the deployment owner sets it. This app does not set it.
Verified against the running door with the setting unset:

```
$ curl -s -D - -o /dev/null -H 'origin: http://localhost:5173' \
    -H 'x-ordo-space: dev' -H "authorization: Bearer $T" \
    http://127.0.0.1:4000/op/listReadings | grep -iE '^HTTP|access-control'
HTTP/1.1 200 OK

$ curl -o /dev/null -w '%{http_code}\n' -X OPTIONS \
    -H 'origin: http://localhost:5173' \
    -H 'access-control-request-method: GET' \
    http://127.0.0.1:4000/op/listReadings
405
```

With the setting unset, the answer carries no `Access-Control-Allow-Origin`, so a
browser discards a body it did in fact receive. The preflight is worse: **405**.
Every Ordo call carries `Authorization` and `X-Ordo-Space`, which makes every
browser call a preflighted one, so the 405 is not an edge case. With
`ORDO_CORS_ORIGINS=http://localhost:5173` (exact origins, no `*`) the door
answers the preflight and a page on that origin can call it directly. This app
leaves it unset and forwards through `web/serve.py`, so the page and the door
share one origin. `@hominemai/ordo-react` is a browser runtime, so a deployed
page sets the list.

Serving the page and the ops from one origin removes the question rather
than answering it, which is what a first app should do with a finding it
did not come for.

### The token never enters the browser

The proxy reads `ORDO_TOKEN` from its own environment and adds the header on
the way through, so the page holds no credential, keeps none in
`localStorage`, and leaks none into a screenshot. The page knows no host
either: it fetches its own origin. Everything it can reach is what that one
file forwards, which is `/op/*` and `/interface` and nothing else:

```
GET /                    -> 200 text/html
GET /op/listReadings     -> the rows, page holds no token
POST /op/addReading      -> {"mode":"creates", …}
GET /op/nope             -> {"error":{"code":"access_denied", …}}  [403]
GET /health/deep         -> 404      (the proxy does not forward it)
```

The refusal line is there on purpose. An unknown op and an unauthorised one
answer identically, so a guess reveals nothing, and the page shows the
envelope and does not flatten every failure into its own. A display that
cannot tell "no rows" from "refused" is a display that lies.

## 5 · What is not verified

**The rendering.** The page's HTML, its table and its refusal banner have
not been opened in a browser. Every path it depends on was exercised with
`curl` through the same proxy it uses, and the JavaScript is 63 lines with
no dependencies, but nobody has looked at it. That is one glance from being
settled and the glance has not happened.

## 6 · The next rung, deliberately not built

This page uses `fetch` and knows the two op names by hand. The runtime that
removes that is [`@hominemai/ordo-react`](../../clients/ordo-react), which reads
`interface.json` at run time and derives the form, the paging, the row
actions and the commit from the descriptor, with no op-name heuristic
anywhere in its source. The same table through `useOpQuery('listReadings')`
is the honest second version of this app, and it needs a build step (React,
a bundler) that this version deliberately does not have.

Order matters here. A hand-written `fetch` page proves the door, the token,
the two routes and the refusal envelope with nothing in between to blame. A
descriptor-driven page proves the descriptor. Doing the second one first
would have hidden which half was broken, and it would have hidden the CORS
finding behind a bundler's dev proxy, which papers over exactly that gap by
default.
