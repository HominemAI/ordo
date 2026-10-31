# Public data: what a visitor who is not signed in can see

Status: **ruled** (`public-data` asks 1 to 23, 2026-10-03). Nothing here is
built, and there is no go to build it yet (checklist 27ad).

---

## 1 · Today

- **A visitor with no token can read nothing.** The API refuses every
  request without a token before any op runs (`crates/ordo-api/src/router.rs:805`).
- **Share links are the only way in without signing in.** They are secret
  capabilities for chosen rows in one tenant, not public access.
- **The spec says otherwise.** DL G15 says unguarded ops answer anonymous
  callers. That is reachable only below the API.
- **The system space's public read window** was promised by a ruling
  (2026-09-12) and never built.

## 2 · Words

**The deployment's space** (the glossary's *system space*) is where the
deployment itself lives. The owner: *"It belongs to the App, i.e. the
deployment."* Login and user information already live there. It is the app's
own home, separate from every tenant.

**A tenant** is private by default. The owner (ask 3): *"under what
circumstances would a tenant have public data that's not within its boundary?
Aside from sharing things by choice."* Usually none.

## 3 · The model the answers point to

**Public data lives in the deployment's space.** A tenant does not publish
from inside itself. It contributes to the app's public face by an explicit act.
What a tenant writes stays in its own space (ask 22, keeping the 2026-09-12
authorship test); submissions are for content the app itself owns:

| Use case | How it works |
|---|---|
| Landing page, pricing, catalogue | The app's own rows, in the deployment's space, marked public. |
| A forum | A post stays in its **author's own space** and is shown through a feed and the derived public index (ask 22). Erasure and consent follow the author. In a single-tenant deployment everyone is in that one space, so the post is simply there. |
| A public feed across tenants | The same: posts stay with their authors, pushed out through a feed (asks 8, 22). |
| A marketplace | A tenant **grants** the deployment's space its listings, and the deployment exposes them through a public op (ask 7). Data is shared, the privacy wall stays. |
| A single-owner app | One tenant deployment: the deployment owns everything and the public signs into it (ask 7). |
| A contact form or waitlist | A public op that writes into a private entity. Write-only: nobody reads it back publicly (asks 6, 17). The write can also come from a grant the deployment holds, such as a form or an upload (ask 17). |

**Public is a qualifier, declared** (ruled, ask 16): a `public` attribute on
an entity or a field, with a cascading form (`public*`) for children. Rules:

- The compiler refuses `public` on a `sealed` field.
- A public op may read only public fields. A row can be half public: its
  public fields are served, the rest never leave.
- Files follow their row: a public row's files are public.

## 4 · How others do it

| System | Public data | What goes wrong |
|---|---|---|
| **Supabase** | An `anon` role, and RLS policies per table. The anon key is meant to be in the browser. | A table with RLS off is open to anyone with the anon key. In May 2025 a scan found 170 AI-built apps exposing data this way. Public is the default unless a policy says otherwise. |
| **Hasura** | An anonymous role, plus a response cache keyed by query, variables and role. | Cache invalidation is time-based; the cache does not know when the data changed. |
| **Large sites** (forums, shops) | Pages cached at a CDN with `s-maxage` and `stale-while-revalidate`, or rebuilt in the background (ISR). The database sees a small fraction of public traffic. | Freshness is traded for load, by a timer. |

**What Ordo can do better:**

- **Nothing is public unless declared.** The Supabase failure (a forgotten
  policy) cannot happen, because public is opt-in per entity, field and op,
  and checked by the compiler.
- **Exact cache invalidation.** Ordo already knows every entity an op reads
  (its scope manifest; live updates use it, DL G16). A cached public result can
  be dropped the moment one of those entities changes, instead of on a timer.

## 5 · Load (asks 9, 11)

The owner: *"Public load would be the biggest load"*, and it should be moved
off the database onto *"something dedicated and horizontally scalable"*.
Proposed, in layers:

1. **Public op results are cached**, keyed by op, arguments and version,
   dropped exactly when an entity they read changes (§4).
2. **A public read layer** serves those cached results. It scales out on its
   own and never holds a database connection per visitor.
3. **CDN headers** on public responses (`s-maxage`, `stale-while-revalidate`).
4. **Live public pages** share one stream from the read layer, not one per
   visitor.
5. **Build-time accounts** (asks 9, 20): a credential that runs public ops
   while a static site builds, so the data is baked into the pages. Ruled a
   backup option for the future roadmap.

Rate limits stay as the last line, applied at the read layer, so the
database is protected before limits ever bite visitors.

## 5a · The public path (owner, 2026-10-03, in conversation)

**A site key is an optional label, not security.** It ships in the page, so
anyone can read it, and everything it unlocks is public anyway. Anonymous
access is built as the reserved anonymous principal
(`access-model-explained.html`), which keeps the door's one rule that every
call carries a credential. A key per site only lets one deployment tell its
sites apart and switch one off.

**The public path is light.** The owner: *"cache layer and no standard Ordo
audit and permission overhead."* Whether something is public is settled at
compile time: an op is declared public and reads only public fields. So a
public read needs none of the per-call machinery an ordinary read pays:

- no per-call audit row (ask 10);
- no guard walk and no membership lookup;
- served from the cache layer (asks 18, 19), reaching the database only on a
  miss.

The compiler's check is what makes skipping the runtime checks safe. Public
**writes** (the contact form) are not on this path: they are ordinary writes,
checked and logged.

## 6 · Audit (ask 10)

**Ruled 2026-10-03:** *"Public pulls should not be logged or audited."*
Counts for performance belong to the web layer. Public **writes** (the
contact form) are ordinary writes and are logged.

## 7 · Credentials for an API product (asks 13, 14)

**Ruled:** an API key is a credential, not anonymous access: a capability
token with an allowlist of ops and its own quota. The owner asks how this
works when the app's product *is* an API. Proposed: the app's customer is a
principal (or a tenant); the key is issued to them, names the ops it may
call, and is revocable. CORS origins become a per-tenant setting, since an
embedded page belongs to a tenant.

## 7a · How a tenant reads the deployment's space (owner's question, 2026-10-03)

*"How do tenants access the system space for data exposed to them? How is
that decided?"*

**Today:** the 2026-09-12 ruling gives the system space a **universal read
window**. Everybody signed in reads all of it. Nothing narrower exists, and
nothing of it is built for anonymous callers.

**Ruled** (ask 23, *"Agreed"*): three levels, every one decided by the app
developer in the program:

| Level | Who reads | How it is declared |
|---|---|---|
| Public | Anyone, signed in or not | `public` on the entity or field (ask 16) |
| Signed in | Any person signed in to the deployment | The default for the rest of the deployment's space: the universal window |
| Narrowed | Only callers a guard admits | A guard on the entity or op, for example the caller's role, or something about the caller's tenant such as its plan |

- **The deployment decides what a tenant sees of its space,** not the
  tenant. The developer can still hand a choice to tenants as a setting, for
  example opting in to a feed.
- **To check before building:** a guard that reads something about the
  caller's tenant needs the caller's tenant in the guard's context. Whether
  the guard context carries it today is not yet confirmed.

## 8 · Ruled so far

- Erased or unpublished content: Ordo stops serving it at once and tells
  caches to drop it; copies already taken are out of reach (ask 12).
- API keys are credentials with allowlists and quotas (ask 13).
- Public reads are not logged (ask 10).
- Allowed origins are configurable per tenant (ask 14).
- Public data lives in the deployment's space; tenants contribute by grant,
  and what a tenant writes stays in its own space (asks 15, 22).
- Tenants read the deployment's space at three levels: public, signed in by
  default, or narrowed by the developer's guard (ask 23).
- `public` and `public*` are declared qualifiers, refused on sealed fields
  (ask 16).
- A public op may write into a private entity, write-only; the deployment
  may also hold a grant for it, such as a form or an upload (ask 17).
- Public results are cached and dropped exactly when their data changes,
  served from a read layer of their own (asks 18, 19).
- A build-time credential is a future backup option (ask 20).
- API keys for an app's customers name their ops; default quotas are
  unlimited (ask 21).

## 9 · Open

Nothing on the board. Before building:

- Confirm a guard can read the caller's tenant (§7a).
- The derived public index for search and feeds (asks 7, 8, 22) is its own
  build: rebuilt from what tenants consent to expose, pruned on erasure.
