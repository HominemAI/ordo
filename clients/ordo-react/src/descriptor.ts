/**
 * TypeScript types for the Interface Descriptor — `interface.json`,
 * `schema_version: "ordo.interface/2"`.
 *
 * These mirror `crates/ordo-plan/src/interface.rs` field for field. They are
 * a *transcription*, not an invention: where the emitter writes `null`, the
 * type says `| null`; where the emitter's comment says a value is verbatim
 * from its producer, the type does not narrow it beyond what the producer
 * guarantees. The golden `crates/ordo-cli/tests/golden/market.interface.json`
 * is the conformance case, and `test/descriptor.test.ts` asserts that every
 * value set below covers it.
 */

/**
 * The document's own contract version — major is the compatibility gate.
 *
 * `/2` closed the seven gaps this package reported against `/1`. Four were
 * additions and would not have moved the major on their own; three were
 * *changed meanings*, and those are why nothing here reads `/1` any more:
 * `call.cas` meant "is a mutation" and now means "asserts a revision",
 * `call.refinable` was `true` against a `none` tier and now agrees with it,
 * and single-row ops no longer advertise a second page. A reader that kept
 * `/1`'s meanings would get wrong answers rather than missing ones, which is
 * exactly the line `interface.rs` draws for when the number moves.
 */
export const SUPPORTED_SCHEMA_MAJOR = 2;
export const SCHEMA_PREFIX = 'ordo.interface/';

/**
 * The closed scalar inventory (`api-engine-design.md` §5, and the format-hint
 * vocabulary of `ui-gen.md` §1 — deliberately the same list, because §1 pins
 * the hint vocabulary to the type inventory rather than opening a second
 * metadata channel).
 *
 * `null` is legal on an output: a *platform* op's result columns carry no
 * declared type, and the emitter refuses to call one `text` on its own
 * authority. A renderer must handle the null — see `formatHint`.
 */
export type OrdoType =
  | 'int'
  | 'money'
  | 'text'
  | 'date'
  | 'datetime'
  | 'bool'
  | 'id'
  | 'duration'
  | 'decimal'
  // Content types (DL §2). `blob` is what makes a **file input** derivable:
  // §1 pins the format-hint vocabulary to the type inventory rather than
  // opening a second metadata channel, so until the inventory had a member
  // meaning "file", no renderer could offer one without inventing a channel.
  //
  // A `blob` VALUE on the wire is the opaque hex handle and nothing else —
  // the bytes never ride a JSON envelope in either direction. A renderer
  // uploads through the blob door and downloads from `GET /blob/{ref}`; it
  // never puts a file in an op parameter, because `blob` is not a parameter
  // type (class `blob_parameter`).
  | 'blob'
  | 'uri'
  // A JSON document (28b): any JSON value, carried as JSON. No condition
  // takes one, so it is never refinable.
  | 'json'
  | 'enum';

/**
 * Derived from the type by the emitter, so eight skins do not derive it eight ways.
 *
 * `center` is the blob column: a cell holding a file renders as an
 * affordance — a thumbnail, a name, a download — not as a sentence, and it
 * must not be laid out beside prose as though it were some.
 */
export type Align = 'left' | 'right' | 'tabular' | 'center';

/** The verb a call-site builder must terminate in (§B3). */
export type TerminalVerb = 'fetch' | 'live' | 'pages' | 'commit';

/** The op's derived kind, mirrored onto the HTTP method. */
export type OpKind = 'read' | 'write' | 'delete';

export type HttpMethod = 'GET' | 'POST' | 'DELETE';

/** The compiler's static mode label; present on writes and deletes only. */
export type OpMode = string;

// ── entities ─────────────────────────────────────────────────────────────

export interface FieldDescriptor {
  readonly name: string;
  readonly type: OrdoType | null;
  /** Target entity name when the field is a ref, else `null`. A picker is derivable from this and nothing else. */
  readonly ref: string | null;
  readonly owned: boolean;
  readonly tree: boolean;
  readonly optional: boolean;
  readonly unique: boolean;
  readonly sealed: boolean;
  readonly indexed: boolean;
  readonly refinable: boolean;
  readonly default: unknown;
  readonly normalize: string | null;
  /** A dropdown is derivable from this and nothing else. */
  readonly enum_values: readonly string[] | null;
  readonly align: Align;
}

export interface ReferencedBy {
  readonly entity: string;
  readonly field: string;
  readonly owned: boolean;
}

export interface EntityDescriptor {
  readonly entity: string;
  /** The typed-id prefix. `prod` in `prod_01J8ZK…`. May contain `-`, never `_`. */
  readonly key: string;
  readonly fields: readonly FieldDescriptor[];
  readonly checks: readonly unknown[];
  readonly unique_keys: readonly (readonly string[])[];
  readonly referenced_by: readonly ReferencedBy[];
  readonly guarded: boolean;
  readonly tree: boolean;
  readonly acl_levels: readonly string[] | null;
}

// ── typed-id action discovery (`ui-gen.md` §2) ───────────────────────────

/** One op that accepts an id of some entity type, and the param it accepts it in. */
export interface IdTypeEntry {
  readonly op: string;
  readonly param: string;
  readonly kind: OpKind;
  readonly mode: OpMode | null;
  readonly required: boolean;
}

/**
 * One op that *returns* rows of some entity — the half `/1` did not index.
 *
 * `actions` and `detail` key on what an op **accepts**; these two key on what
 * it **returns** (`result.row_identity`, which is the anchor the binder
 * proved). That is what makes a picker readable rather than reconstructible.
 */
export interface ListingEntry {
  readonly op: string;
  /** The addressing param, on `get`. Always `null` on `list`. */
  readonly param: string | null;
  readonly single_row: boolean;
}

export interface IdTypeDescriptor {
  readonly entity: string;
  readonly key: string;
  /** Write/delete ops this row type can be handed to. */
  readonly actions: readonly IdTypeEntry[];
  /**
   * Read ops that take this row type as their address — whatever they
   * return. An op taking a category id and returning *products* is here;
   * that is why it is not the right thing to refetch a row through.
   */
  readonly detail: readonly IdTypeEntry[];
  /**
   * Read ops returning this entity's rows and **not** addressed by one of
   * its ids: the candidates for a picker. Empty is a real answer — for
   * `members` in the market fixture it is the true one.
   */
  readonly list: readonly ListingEntry[];
  /**
   * Read ops returning this entity's rows **and** addressed by a required id
   * of it: the ops that read one row back. What a picker's label resolver
   * and a `cas_miss` refetch want, and a strict subset of `detail`.
   */
  readonly get: readonly ListingEntry[];
}

// ── ops ──────────────────────────────────────────────────────────────────

/** The declared field a parameter's value fills. */
export interface FieldRef {
  readonly entity: string;
  readonly field: string;
}

export interface ParamDescriptor {
  readonly name: string;
  readonly type: OrdoType;
  readonly required: boolean;
  /**
   * Declared `<=` ceiling, or a platform op's `text(n)` length. `null` when
   * unbounded.
   *
   * **Always a string**, never a number: the emitter canonicalises every
   * bound to exact digits (`"100"`, `"12.50"`, `"2026-03-01"`) because a
   * `money` ceiling through a JSON float is a rounded ceiling. `/1` typed
   * this `number | string` and nothing in the golden was populated, so the
   * numeric branch of every consumer check was dead code that looked live.
   */
  readonly max: string | null;
  /**
   * The places of a `money(p)` parameter, or the scale of a `decimal(p)`;
   * `null` for other types. Optional so an older document still reads.
   */
  readonly places?: number | null;
  /**
   * Derived floor. `"0"` for a parameter used as a `take` count, else `null`.
   *
   * There is no `>=` in the language — `declparse::param_bound` refuses one
   * by name — so this is never a transcription of a declaration. It is
   * derived from the construct, and the door enforces it: `LIMIT -1` is an
   * error and `take[keys] -1` is a silently empty page.
   */
  readonly min: string | null;
  /** Which entity an `id` param addresses. The whole basis of action discovery. */
  readonly id_entity: string | null;
  /**
   * The declared field this parameter's value fills — the link `/1` did not
   * have, so a form had to join `statements[].entity` against a same-named
   * field and refuse on ambiguity. `null` where the op lands it in no
   * declared field or in two different ones.
   */
  readonly field_ref: FieldRef | null;
  /**
   * That field's `enum(…)` values, resolved by the emitter. `null` — never
   * `[]` — where there is no value set. A dropdown is derivable from this
   * and nothing else.
   */
  readonly enum_values: readonly string[] | null;
}

export interface OutputDescriptor {
  readonly name: string;
  readonly type: OrdoType | null;
  readonly sealed: boolean;
  readonly refinable: boolean;
  readonly id_entity: string | null;
  readonly align: Align;
}

export interface RowIdentity {
  readonly entity: string;
  readonly key: string;
}

/**
 * What the response's `rows` array holds.
 *
 * - `rows` — one object per result row, keyed by `outputs[].name`. Every
 *   read, and a *platform* write (which returns declared columns).
 * - `write_summary` — one object per statement: `{entity}` plus one of
 *   `created`/`converged` (a row id) or `updated`/`deleted` (a count), as
 *   `statements[].summary` states per statement. **The written row is not
 *   returned**, so a commit does not hand back the new `rev`.
 */
export type ResultShape = 'rows' | 'write_summary';

export interface ResultDescriptor {
  readonly outputs: readonly OutputDescriptor[];
  /** The anchor entity's typed id; `null` for writes and synthetic-row results. */
  readonly row_identity: RowIdentity | null;
  /** Statically one row (`ui-gen.md` §5) — a metric, not a table. */
  readonly single_row: boolean;
  /** Grouped/reduced rows: no identity, no detail link, no actions. */
  readonly synthetic_rows: boolean;
  /**
   * See {@link ResultShape}. `/1` left a write's `result` all-negative, so a
   * form could not know what came back; the honest answer turned out to be
   * "not the row", which `api-engine-design.md` §3 still says otherwise
   * about — the descriptor describes the executor.
   */
  readonly shape: ResultShape;
  /**
   * Whether a device can keep this op's result in sync
   * (`plan-replica-feed.md` §1). `id` is the output that carries each row's
   * id. `reason` names the rule an op fails: `not_a_read`, `grouped`,
   * `measure`, `no_anchor`. Optional so a document without the key reads as
   * "cannot be synced" rather than failing to parse.
   */
  readonly sync?: { readonly syncable: boolean; readonly id?: string; readonly reason?: string };
  /**
   * The order the rows arrive in (27o, ruled 2026-10-01): output names, the
   * id last, so the device copy returns what it holds in the server's order.
   * `stated: false` where the language cannot state it (a grouped result, a
   * sort key the device cannot evaluate); the keys are then the id alone.
   * `null` for a write. Optional so an older document still reads.
   */
  readonly order?: {
    readonly keys: readonly { readonly path: string; readonly desc: boolean }[];
    readonly stated: boolean;
  } | null;
}

export interface CallContract {
  readonly method: HttpMethod;
  /**
   * The verb a builder must end in. `["commit"]` for a mutation;
   * `["fetch","live","pages"]` for a read, minus `pages` when the result is
   * statically one row — such an op is answered whole, with no cursor.
   */
  readonly terminal: readonly TerminalVerb[];
  /**
   * **Asserts a revision** — true iff some statement declares a `cas_param`.
   * In `/1` this was `kind !== 'read'`, so a `std.E.delete($id)` claimed CAS
   * it could not perform and this package had to detect the real thing from
   * the parameter list.
   */
  readonly cas: boolean;
  /** `refine.tier !== 'none'`. The two can no longer disagree. */
  readonly refinable: boolean;
  readonly live: boolean;
  /** False for a write and for a single-row read. */
  readonly paged: boolean;
}

export interface RefineContract {
  /**
   * `basic` for every program read (ruled 2026-10-05); `none` for a write, a
   * platform op and a public read. See `refinementSurface` in `refine.ts`.
   */
  readonly tier: 'none' | 'basic';
  /** The outputs a condition may name. */
  readonly fields: readonly string[];
  /**
   * The size limits, when the descriptor states them: at most `conditions`
   * conditions and `in_values` values in one `in`. They are deployment
   * settings; absent, the door alone checks them.
   */
  readonly limits?: { readonly conditions: number; readonly in_values: number } | null;
}

export interface CostContract {
  readonly class: string | null;
  readonly rows: string | null;
}

export interface GuardDisclosure {
  readonly entity: string;
  readonly read: string | null;
  readonly write: string | null;
  readonly source: string;
}

export interface KeyRelease {
  readonly purpose: 'read' | 'write' | string;
  readonly keys: readonly {
    readonly entity: string;
    readonly field: string;
    readonly key: string;
    readonly output: string | null;
    readonly stage: string;
  }[];
}

export interface Disclosures {
  readonly guards: readonly GuardDisclosure[];
  readonly bypassed_guards: readonly GuardDisclosure[];
  readonly key_release: KeyRelease | null;
  readonly unbounded: boolean;
  readonly unguarded: boolean;
}

/** What one statement contributes to a `write_summary` response. */
export interface StatementSummary {
  /** The key(s) the summary object may carry beside `entity`. */
  readonly keys: readonly ('created' | 'converged' | 'updated' | 'deleted')[];
  /** `id` — a minted row id. `count` — how many rows the statement touched. */
  readonly value: 'id' | 'count';
}

export interface StatementDescriptor {
  readonly entity: string;
  readonly label: string;
  readonly verb: string;
  /** `null` only for `ambiguous-identity`, which never compiles. */
  readonly summary: StatementSummary | null;
  /**
   * The parameter this statement asserts as the row's revision — the CAS
   * input — or `null`. This is where `rev` is found: by declaration, not by
   * name.
   */
  readonly cas_param: string | null;
}

/**
 * A standing, call-independent refusal: this op cannot execute in this
 * deployment at all (an unbuilt venue, for instance). It is knowable before
 * the first call, and a runtime that ignores it makes the user click a
 * button that can only fail.
 */
export interface StandingError {
  readonly code: string;
  readonly message: string;
}

export interface OpDescriptor {
  readonly op: string;
  readonly version: string;
  readonly kind: OpKind;
  readonly mode: OpMode | null;
  readonly platform: boolean;
  /** Present and true for a public op: a read that needs no token and takes no `$refine`. */
  readonly public?: boolean;
  readonly params: readonly ParamDescriptor[];
  readonly result: ResultDescriptor;
  readonly call: CallContract;
  readonly refine: RefineContract;
  readonly cost: CostContract;
  readonly disclosures: Disclosures;
  readonly statements: readonly StatementDescriptor[];
  readonly refusals: readonly string[];
  readonly error: StandingError | null;
}

export interface InterfaceDocument {
  readonly schema_version: string;
  readonly digest: string;
  readonly domain: string;
  readonly entities: readonly EntityDescriptor[];
  readonly id_types: readonly IdTypeDescriptor[];
  readonly ops: readonly OpDescriptor[];
}
