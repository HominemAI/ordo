# Plan: a `json` field type

Checklist 28b. Board `json-field`. First in the onboarding order
(`ellanor-readiness`, ruled 2026-10-08).

**Why.** ellanor keeps many fields as `*Json` blobs. denizens has Supabase
`jsonb` columns. Without a JSON type both would have to be `text`.

**Status.** Built, 2026-10-08, on this plan's recommended answers. The
questions at the end are open for the owner.

## The choices

One line each. The reason follows in a short bullet where it is not obvious.

| Question | Choice |
|---|---|
| Spelling | `json`, on fields and parameters. `json?` admits null. |
| Storage | Postgres `jsonb`. A sealed `json` is `bytea`, like any sealed field. |
| On the wire | Any JSON value, carried as JSON. Never a string holding JSON. |
| Size | `ORDO_JSON_MAX_BYTES`, default 262144 (256 KiB), of the compact form. |
| Depth | `ORDO_JSON_MAX_DEPTH`, default 64. `{}` is depth 1. |
| Over a limit | The door answers `bound_violation`, naming the setting. |
| A NUL in a string or key | `invalid_param`. Postgres `jsonb` cannot store it. |
| Writes and CAS | A write replaces the whole document and converges by `rev`. |
| Defaults | `= {}`, `= []`, or `= null` on a `json?` field. Nothing else. |
| Sealing | Stored as a sealed `text` is: the compact JSON text, encrypted. |
| Conditions on a whole document | Only `is null`. Anything else is `json_opaque`. |
| Conditions inside a document | Built 2026-10-08. A path, typed by the other side. See below. |
| Sort, group, aggregate, function | Refused on a document and on a path into one, `json_opaque`. |
| `unique`, `index`, `indexed`, `refinable`, `normalize` | Refused, `json_opaque`. |
| A `$refine` condition | `invalid_refinement`. The descriptor marks json fields not refinable. |
| Type mixing | A text never lands in a `json` field, a document never in a text: `param_type_mismatch`. |
| `text` to `json` | Applies in place when every stored value parses. Refused, naming the rows, when one does not. |
| `json` to `text` | Always applies. A document becomes its JSON text. |
| After either conversion | `rev` moves on every row, so a cached client refetches. |
| SDK type | `JsonValue`, exported by `@hominemai/ordo-sdk` and `@hominemai/ordo-react`. |
| React form | `useOpForm` gives a json parameter the kind `json`. |
| CDK | `t.json()`. |

- **No new wire class.** Size and depth reuse `bound_violation`, as `text(n)`
  does. Both client `REFUSAL_CLASSES` lists are unchanged.
- **Why no condition on a whole document.** Equality over whole documents
  would bring Postgres's `jsonb` rules in unannounced. A path into one is
  typed, so it brings in nothing the language does not already say.
- **Why the limit is a setting.** A deployment owner with larger documents
  raises it. The request body limit (2 MiB) stays the outer bound.
- **Exact means the same JSON value.** `jsonb` keeps numbers exactly but
  drops whitespace, orders keys its own way, and keeps the last of two
  equal keys. Strings are stored NFC, as every text is.
- **JSON `null` as a whole document is absence.** The door treats an
  explicit `null` parameter as absent, so a `json?` field stores SQL NULL.

## Filtering inside a document

The owner ruled "Add filtering now" on 2026-10-08. Filters only: conditions
in a pipeline, in `(...)` and after `|`.

| Question | Choice |
|---|---|
| Spelling | `prefs.theme`. Segments after a json field are steps inside it. |
| A key that is not a name | `prefs["my key"]`. `prefs["theme"]` is `prefs.theme`. |
| An array position | `tags[0]`. Digits only. |
| Through refs | `author.card.role`: hops first, then the document. |
| Typing | From the other side: text, `int` or `decimal` (exact `numeric`), bool. |
| Untyped | Path against path, against `null`, a whole document or another type, or alone: `json_path_type`. |
| Operators | `== != < <= > >=`, `in`, `starts`, `matches`, `contains`, `is null`. |
| Sealed json | `sealed_unsupported`. The document is ciphertext. |
| SQL | `doc #> ARRAY['k', '0']::text[]`, each key a quoted literal. |
| `$refine` | Still refused. The descriptor knows fields, not paths. |
| Index | None yet. A filter on a path reads every row the rest leaves. |

Three behaviours were ruled on 2026-10-08 (`json-field` asks 4 to 6). Each
lives in one function in `crates/ordo-plan/src/lib.rs`, with its own test in
`crates/ordo-conformance/tests/json_filter.rs`:

- **Ask 4, `doc_missing`.** `is null` is true for a missing key and for
  JSON `null`.
- **Ask 5, `doc_read`.** A value of the wrong JSON type reads as absent.
  It does not match, and nothing errors. `!=` is `not ==`, so an absent
  value is `!=` anything, as a null field is.
- **Ask 6, `doc_contains`.** An array contains an element (`@>`). A string
  contains a substring. The value's JSON type picks the reading, so they
  never meet. A number or a bool on the right asks the array question only.

## How it is built

- `crates/ordo-syntax`: the `json` type word, and the `{}` / `[]` defaults.
- `crates/ordo-schema`: `jsonb` DDL, `check_json_modifiers`, `json_opaque`
  on `unique`/`index` clauses and on a `check` that reads one.
- `crates/ordo-syntax`: bracketed segments in `path`.
- `crates/ordo-sema`: `refuse_json` at every condition, sort, group,
  aggregate, function and operator; `fit_json` for parameters.
  `Binder::operand` lets one side of a condition walk into a document;
  `doc_path`, `doc_side_ty`, `doc_list_ty` and `doc_text_lhs` type it.
  `BExpr::DocPath` carries it to the planner.
- `crates/ordo-plan`: `doc_at`, `doc_read`, `doc_missing`, `doc_contains`.
- `clients/ordo-cdk`: `printPath` brackets a key that is not a name.
- `crates/ordo-plan`: `check_json_slot` at registration; descriptor
  `refinable: false`.
- `crates/ordo-api/src/json.rs`: `JsonLimits` and the door's check.
- `crates/ordo-host`: the call carries a document as an escaped string
  literal (`json_source_literal`); the text and json conversion in
  `schema_apply.rs`; boot refuses a limit of 0 or a non-number.
- `crates/ordo-conformance/src/write.rs`: `fit_json` checks and casts to
  `jsonb` before any SQL runs.
- `crates/ordo-exec`: reads, export and import carry the document as JSON.
- `crates/ordo-gen`, `clients/`: `JsonValue`, `t.json()`, the form kind.

## Tests

- `crates/ordo-host/tests/json_field.rs`: round trip, default, limits,
  sealed, `rev` conflict, `$refine` refusal, all at the real door.
- `crates/ordo-host/tests/json_schema_change.rs`: both conversions.
- `crates/ordo-conformance/tests/json_field.rs`: every compile refusal.
- `crates/ordo-conformance/tests/json_filter.rs`: filtering against
  Postgres, and what stays refused.
- `crates/ordo-host/tests/json_field.rs`: a path filter with parameters
  bound at the door.
- `crates/ordo-cli/tests/gen_ts.rs`: the generated type is `JsonValue`.
- `crates/ordo-api/src/json.rs`, `crates/ordo-host/src/runtime.rs`: limits.
- `clients/ordo-sdk/test/generated.test.ts`, `clients/ordo-react/test/form.test.ts`.

## Questions for the owner

1. **Are 256 KiB and 64 levels the right defaults?**
   Recommend **yes**. Both are settings, and ellanor's largest blob is
   the test.
2. **Should `is null` be the one condition a json field allows?**
   Recommend **yes**. It asks whether a document is there, not what is in it.
3. **When should conditions inside a document come (`doc.status == "x"`)?**
   Ruled 2026-10-08: now. Built, filters only. A GIN or expression index
   plan is still open.
4. **A sealed field changing between `text` and `json` shows no column
   change. Is a later reseal step right for it?**
   Recommend **yes, with the reseal worker**. Until then a sealed text that
   is not JSON reads back as a JSON string, so nothing is lost.
5. **Should strings inside a document be stored NFC, as all text is?**
   Recommend **yes**. One stored form platform-wide; only unnormalized
   Unicode changes.
