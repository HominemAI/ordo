/**
 * Refusals: the closed inventories, typed, and the one error a door answer
 * becomes.
 *
 * `api-engine-design.md` §4 states the op door's inventory and it is
 * *closed*: the door and the executor between them cannot construct a class
 * outside {@link REFUSAL_CLASSES}. The other surfaces (sign-in, accounts,
 * delegation, links, shares) answer with the words in {@link EDGE_CODES}.
 * Both lists are the server's `ErrorCode` and `EdgeCode`
 * (`crates/ordo-api/src/error.rs`), and `crates/ordo-api/tests/client_classes.rs`
 * holds them equal.
 *
 * Every refusal the door sends reaches a caller as an {@link OrdoRefusal}:
 * the wire `code`, the HTTP `status`, the door's `Retry-After` and what to do
 * about it (`disposition`, `retryable`). A module adds a typed subclass only
 * where its callers switch on something of their own (`PasskeyError.failure`).
 * `@hominemai/ordo-react` re-exports this class rather than carrying its own
 * (audit-service-interfaces.md, fix order step 7). A refusal is never a
 * string in this package.
 */

export const REFUSAL_CLASSES = [
  'unknown_op',
  'unknown_param',
  'missing_param',
  'invalid_param',
  'bound_violation',
  'invalid_refinement',
  'invalid_cursor',
  'not_live',
  'method_mismatch',
  'missing_space',
  'null_key_address',
  'access_denied',
  'access_guard',
  'delegation_required',
  'not_found',
  'idempotency_in_flight',
  'idempotency_applied',
  'version_skew',
  'expected_absent',
  'conflicting_identity',
  'duplicate',
  'cas_miss',
  'idempotency_mismatch',
  'refinement_unbounded',
  'serialization_conflict',
  'caller_supplied',
  'lease_fenced',
  'has_referrers',
  'tree_cycle',
  'tree_has_children',
  'tree_depth_exceeded',
  'tree_bulk_reparent',
  'reserved_derived_name',
  'ambiguous_identity',
  'unbound_required_param',
  'projection_unknown_field',
  'invalid_binding_expr',
  'param_type_mismatch',
  'arithmetic_error',
  'check_violation',
  'unknown_source',
  'sealed_open_failed',
  'sealed_unreadable',
  'log_append_failed',
  'sealed_unsupported',
  'not_confirmed',
  'space_pinned',
  'schema_drift',
  'server_busy',
  'apply_in_progress',
  'apply_refused',
  'bucket_refused',
  'egress_refused',
  'import_refused',
  'not_activated',
  'proof_unavailable',
  'quota_exceeded',
  'human_check_failed',
  'space_paused',
  'function_error',
  'function_timeout',
  'function_oom',
  'function_busy',
  'space_unavailable',
  'statement_timeout',
  'tenant_refused',
  'tenant_not_ready',
  'email_required',
  'presence_required',
  'internal',
] as const;

export type RefusalClass = (typeof REFUSAL_CLASSES)[number];

const SET: ReadonlySet<string> = new Set<string>(REFUSAL_CLASSES);

export function isRefusalClass(code: string): code is RefusalClass {
  return SET.has(code);
}

/**
 * The words of the door's other surfaces: sign-in, accounts, delegation,
 * links and shares. The server's `EdgeCode`, in its wire order.
 */
export const EDGE_CODES = [
  'invalid_request',
  'tenant_providers_off',
  'presence_not_available',
  'authentication_failed',
  'remember_off',
  'link_password_required',
  'link_password_wrong',
  'password_change_required',
  'reauthentication_required',
  'presence_required',
  'delegation_refused',
  'delegation_declined',
  'link_refused',
  'linking_off',
  'email_unavailable',
  'guests_off',
  'codes_disabled',
  'share_refused',
  'invitation_refused',
  'recovery_contestable',
  'identifier_taken',
  'last_authenticator',
  'originating_admin',
  'memberships_remain',
  'delegation_expired',
  'delegation_redeemed',
  'delegation_limit',
  'link_locked',
  'too_many_attempts',
  'auth_unconfigured',
  'credential_store_unavailable',
  'session_store_unavailable',
  'delegation_store_unavailable',
  'link_unavailable',
  'tickets_unavailable',
  'share_unavailable',
  'identity_provider_unavailable',
] as const;

export type EdgeCode = (typeof EDGE_CODES)[number];

const EDGE: ReadonlySet<string> = new Set<string>(EDGE_CODES);

export function isEdgeCode(code: string): code is EdgeCode {
  return EDGE.has(code);
}

/** Any code the door's error envelope can carry. */
export type DoorCode = RefusalClass | EdgeCode;

export function isDoorCode(code: string): code is DoorCode {
  return SET.has(code) || EDGE.has(code);
}

/**
 * The refusal classes that mean "nothing ran; ask again shortly". The one
 * list: the request helper (`core/door.ts`) sends a call again on these, the
 * sync manager and ordo-react's live hub reconnect on them, and
 * {@link OrdoRefusal.retryable} is true for them.
 */
export const RETRY_CLASSES: ReadonlySet<string> = new Set<string>([
  'server_busy',
  'tenant_not_ready',
  'serialization_conflict',
] satisfies RefusalClass[]);

/**
 * What the caller is supposed to *do*:
 *
 * - `conflict`: stored data refused it. The identical request lands once
 *   that data changes; show the conflict rather than retry (`cas_miss`).
 * - `restart`: the request's cursor is stale, not its data. Drop the cursor
 *   and page again from the top.
 * - `retry`: the platform asked to be asked again. Safe under the same
 *   idempotency key.
 * - `terminal`: asking again will never change the answer.
 * - `request`: fix the request and ask again.
 * - `denied`: the caller may not; sign in, or ask for access.
 * - `fault`: the platform failed.
 */
export type Disposition = 'conflict' | 'restart' | 'retry' | 'terminal' | 'request' | 'denied' | 'fault';

const DISPOSITIONS: Partial<Record<DoorCode, Disposition>> = {
  cas_miss: 'conflict',
  duplicate: 'conflict',
  conflicting_identity: 'conflict',
  expected_absent: 'conflict',
  has_referrers: 'conflict',
  tree_cycle: 'conflict',
  tree_has_children: 'conflict',
  tree_depth_exceeded: 'conflict',
  check_violation: 'conflict',
  space_pinned: 'conflict',

  // Stale cursor, stale pin: start over from page one.
  version_skew: 'restart',
  invalid_cursor: 'restart',

  idempotency_in_flight: 'retry',
  serialization_conflict: 'retry',
  lease_fenced: 'retry',
  // Nothing ran; the client already waited out its retries.
  server_busy: 'retry',
  tenant_not_ready: 'retry',

  idempotency_applied: 'terminal',
  idempotency_mismatch: 'terminal',

  access_denied: 'denied',
  access_guard: 'denied',
  delegation_required: 'denied',
  caller_supplied: 'denied',
  // The app developer requires a verified email address and the person has
  // none: the page asks for one (`GET /auth/email`), then calls again.
  email_required: 'denied',
  not_found: 'denied',

  internal: 'fault',
  unknown_source: 'fault',
  sealed_open_failed: 'fault',
  log_append_failed: 'fault',
  schema_drift: 'fault',

  // The edge words, by what their status says.
  authentication_failed: 'denied',
  remember_off: 'denied',
  link_password_required: 'denied',
  link_password_wrong: 'denied',
  password_change_required: 'denied',
  reauthentication_required: 'denied',
  presence_required: 'denied',
  delegation_refused: 'denied',
  delegation_declined: 'denied',
  link_refused: 'denied',
  linking_off: 'denied',
  email_unavailable: 'denied',
  guests_off: 'denied',
  codes_disabled: 'denied',
  share_refused: 'denied',
  invitation_refused: 'denied',
  recovery_contestable: 'conflict',
  identifier_taken: 'conflict',
  last_authenticator: 'conflict',
  originating_admin: 'conflict',
  memberships_remain: 'conflict',
  delegation_expired: 'terminal',
  delegation_redeemed: 'terminal',
  delegation_limit: 'retry',
  link_locked: 'retry',
  too_many_attempts: 'retry',
  auth_unconfigured: 'fault',
  credential_store_unavailable: 'fault',
  session_store_unavailable: 'fault',
  delegation_store_unavailable: 'fault',
  link_unavailable: 'fault',
  tickets_unavailable: 'fault',
  share_unavailable: 'fault',
  identity_provider_unavailable: 'fault',
};

/**
 * What to do about a code. Everything unlisted is a malformed request: fix
 * the request, ask again. `sealed_unsupported` is one of these (422, ruled
 * 2026-10-05): the op's shape is what the app developer fixes. A code no
 * list names is a `fault`: the server is newer than this client.
 */
export function dispositionOf(code: string): Disposition {
  if (!isDoorCode(code)) return 'fault';
  return DISPOSITIONS[code] ?? 'request';
}

export interface RefusalDetail<C extends string = RefusalClass> {
  /** The class: a code this client knows, or `internal` for one it does not. */
  readonly class: C;
  /**
   * The code exactly as the door sent it. It differs from `class` only when
   * the door is newer than this client and sent a code it does not know:
   * then `class` is `internal` and this keeps the evidence. For a refusal
   * raised before the wire, the class.
   */
  readonly code: string;
  readonly message: string;
  readonly op: string | null;
  readonly param: string | null;
  /** The HTTP status; 0 for a refusal raised before the wire. */
  readonly status: number;
  /**
   * True iff the op's own generated union named this class.
   *
   * A `false` is not a bug in the caller: the door has refusals that precede
   * any op (`missing_space`, `caller_supplied`, `delegation_required`), so
   * they are legal answers that the op's declared set does not carry.
   */
  readonly declared: boolean;
}

/** What {@link OrdoRefusal} is built from: a detail whose `code` defaults to its class. */
export type RefusalInput<C extends string = RefusalClass> = Omit<RefusalDetail<C>, 'code'> & {
  readonly code?: string;
};

/** How a subclass words and chains its refusal. */
export interface RefusalOptions {
  /** The error's `message`. Default `"<class>: <detail.message>"`. */
  readonly message?: string;
  /** What caused it, kept as the error's `cause`. */
  readonly cause?: unknown;
}

/**
 * A refusal the door returned, or one raised before the wire because the
 * door could only have refused it. The one error shape of ordo-sdk and
 * ordo-react.
 */
export class OrdoRefusal<C extends string = RefusalClass> extends Error {
  readonly detail: RefusalDetail<C>;
  /**
   * The door's `Retry-After`, in milliseconds, when it sent one (a
   * `server_busy` or `tenant_not_ready`); otherwise `null`.
   */
  readonly retryAfterMs: number | null;

  constructor(detail: RefusalInput<C>, retryAfterMs: number | null = null, options: RefusalOptions = {}) {
    super(
      options.message ?? `${detail.class}: ${detail.message}`,
      options.cause === undefined ? undefined : { cause: options.cause },
    );
    this.name = 'OrdoRefusal';
    this.detail = { ...detail, code: detail.code ?? detail.class };
    this.retryAfterMs = retryAfterMs;
  }

  get class(): C {
    return this.detail.class;
  }

  /** The wire code, verbatim. See {@link RefusalDetail.code}. */
  get code(): string {
    return this.detail.code;
  }

  /** The HTTP status; 0 for a refusal raised before the wire. */
  get status(): number {
    return this.detail.status;
  }

  /** The detail under the name `@hominemai/ordo-react` 0.3 gave it. */
  get refusal(): RefusalDetail<C> {
    return this.detail;
  }

  /** What the caller is supposed to do about it. */
  get disposition(): Disposition {
    return dispositionOf(this.detail.code);
  }

  /** Nothing ran, and the same call can be sent again: a class in {@link RETRY_CLASSES}. */
  get retryable(): boolean {
    return RETRY_CLASSES.has(this.detail.class);
  }
}

/**
 * A refusal raised **before** the wire.
 *
 * It wears the door's own class rather than an invented one, because the
 * request it describes could only ever have been refused there: a
 * `method_mismatch` raised locally and one raised remotely are the same
 * mistake, and a caller with two names for it writes two handlers.
 * `local: true` tells the two apart when it matters (it rarely does).
 */
export class OrdoLocalRefusal<C extends string = RefusalClass> extends OrdoRefusal<C> {
  readonly local = true as const;

  constructor(detail: Omit<RefusalInput<C>, 'status' | 'declared'> & Partial<Pick<RefusalDetail<C>, 'status' | 'declared'>>) {
    super({ status: 0, declared: false, ...detail } as RefusalInput<C>);
    this.name = 'OrdoLocalRefusal';
  }
}

/** What the door answered, for a module's own refusal: its status and code, when it answered. */
export interface WireAnswer {
  /** The HTTP status. Absent or 0 when the refusal was raised before the wire. */
  status?: number | undefined;
  /** The envelope's `code`, when it sent one. */
  code?: string | null | undefined;
}

/**
 * The detail a module's own refusal (`PasskeyError`, `TenantError`, ...)
 * carries. Its class is the door's code when the door sent one it knows,
 * else the module's own word for what happened (`failure`); `code` is the
 * door's code verbatim, else that word.
 */
export function failureDetail(failure: string, message: string, wire: WireAnswer = {}): RefusalInput<string> {
  const code = wire.code ?? null;
  return {
    class: code !== null && isDoorCode(code) ? code : failure,
    code: code ?? failure,
    message,
    op: null,
    param: null,
    status: wire.status ?? 0,
    declared: false,
  };
}

/** The wire's error envelope: `{ "error": { code, message, op?, param? } }`. */
export interface ErrorEnvelope {
  readonly error: {
    readonly code: string;
    readonly message: string;
    readonly op?: string;
    readonly param?: string;
  };
}

export function isErrorEnvelope(body: unknown): body is ErrorEnvelope {
  if (typeof body !== 'object' || body === null) return false;
  const err = (body as { error?: unknown }).error;
  return (
    typeof err === 'object' &&
    err !== null &&
    typeof (err as { code?: unknown }).code === 'string' &&
    typeof (err as { message?: unknown }).message === 'string'
  );
}

/**
 * Decode the door's error envelope (§4) into a refusal's detail.
 *
 * One shape, whether the door wrapped the refusal in `error` or sent it
 * flat. The field is `code`: what the door writes (`ApiError::envelope`,
 * `EdgeError::envelope`) and what `api-engine-design.md` §4 documents.
 *
 * A code this client does not know is `internal`, with the code kept
 * verbatim in `code`: the inventories are closed by contract, so an unknown
 * code means the deployment is newer than this client, and guessing which
 * class it resembles would be worse than saying so. A non-2xx without an
 * envelope is `internal` too, and says so.
 */
export function decodeRefusal(
  payload: unknown,
  status: number,
  op: string,
  declared: readonly string[],
): RefusalDetail<RefusalClass> {
  interface Envelope {
    code?: unknown;
    message?: unknown;
    param?: unknown;
    op?: unknown;
  }
  const body = (payload !== null && typeof payload === 'object' ? payload : {}) as Envelope & { error?: Envelope };
  const envelope: Envelope = body.error !== null && typeof body.error === 'object' ? body.error : body;
  const raw = typeof envelope.code === 'string' ? envelope.code : '';
  // An edge word is typed `RefusalClass` here so an op's generated union
  // stays as narrow as it is; `isEdgeCode` and `code` say which it is.
  const cls = (isDoorCode(raw) ? raw : 'internal') as RefusalClass;
  const message =
    typeof envelope.message === 'string'
      ? envelope.message
      : raw
        ? `${raw} (a class this client does not know; regenerate with \`ordo gen ts\`)`
        : `the server answered ${status} without an Ordo error envelope; ` +
          `body: ${truncate(JSON.stringify(payload) ?? String(payload), 200)}`;
  return {
    class: cls,
    code: raw || cls,
    message,
    op: typeof envelope.op === 'string' ? envelope.op : op,
    param: typeof envelope.param === 'string' ? envelope.param : null,
    status,
    declared: declared.includes(raw),
  };
}

function truncate(s: string, n: number): string {
  return s.length <= n ? s : `${s.slice(0, n)}…`;
}
