# SDK signatures

Copied from `clients/ordo-react/src` and `clients/ordo-sdk/src`. Every
export an app touches, with its import path. `readonly` is left off the
fields here; every field is read-only. `?` marks an optional field.

Import paths:

- `@hominemai/ordo-react`: everything in this package, from the root only.
- `@hominemai/ordo-sdk/<folder>/<file>.js`: one file of the SDK. Prefer
  these subpaths. The root `@hominemai/ordo-sdk` also exports all of it
  (never a generated surface: that is your app's own).
- `./generated/ops.js`, `./generated/ids.js`: your app's, from
  `ordo gen ts <dir> --app <src>`.

## Shared types

```ts skip
// @hominemai/ordo-sdk/core/client.js (Row, Params, OpResponse, TokenSource also from ordo-react)
type Row = Record<string, unknown>;
type JsonValue = string | number | boolean | null | JsonValue[] | { [k: string]: JsonValue };
type ParamValue = JsonValue;
type Params = Readonly<Record<string, ParamValue>>;
type TokenSource = string | (() => string | Promise<string>);
interface OpResponse<R = Row> {
  op: string; version: string; mode?: string;
  rows: readonly R[];
  next?: string;              // present iff another page exists; absence is the only end
  servedVersion: string | null; stale: boolean; replayed: boolean; pin: Pin | null;
}
```

## @hominemai/ordo-react

```ts skip
parseInterface(raw: unknown): OrdoInterface        // throws SchemaVersionError | MalformedDescriptorError
withPlatformOps(iface: OrdoInterface, ...names: PlatformOpName[]): OrdoInterface

new DescriptorClient(options: DescriptorClientOptions)
interface DescriptorClientOptions {
  baseUrl: string; space: string; iface: OrdoInterface;
  token?: TokenSource;
  renewToken?: () => Promise<boolean>;   // used by live streams only
  fetch?: typeof fetch; callTimeoutMs?: number;
  busyRetry?: BusyRetryOptions; busyRetries?: number;
  conflictRetry?: ConflictRetryOptions; conflictRetries?: number;
}
client.withSpace(space: string): DescriptorClient
client.fetchOp<R = Row>(name: string, o?: DescriptorCallOptions): Promise<OpResponse<R>>
client.fetchPage<R = Row>(name: string, o?: DescriptorCallOptions): Promise<OpResponse<R>>
client.pageAll<R = Row>(name: string, o?: DescriptorCallOptions): AsyncGenerator<OpResponse<R>>
client.commit<R = Row>(name: string, o?: DescriptorCallOptions): Promise<OpResponse<R>>
interface DescriptorCallOptions { params?: Params; page?: string; refine?: Refine; idempotencyKey?: string; signal?: AbortSignal; onRetry?: OnRetry }
// ordo-react has no `OrdoClient`: that name is only the SDK's generated-op client.

<OrdoProvider client={DescriptorClient} live?={boolean | { paceMs?: number }} device?={DeviceSource}
              stepUp?={() => Promise<void>}>{children}</OrdoProvider>
useOrdo(): DescriptorClient
useInterface(): OrdoInterface

useOpQuery<R = Row>(op: string | OpRef<R, F>, o?: UseOpQueryOptions): UseOpQuery<R>
useOpQuery<N extends PlatformOpName>(op: N, o?): UseOpQuery<PlatformRows[N]>
interface UseOpQueryOptions {
  params?: Params; enabled?: boolean;         // enabled default true
  refine?: Refine | ((fields: F) => Conditions);
  pick?: readonly (keyof R)[];
  live?: boolean; livePaceMs?: number; device?: boolean;
}
interface UseOpQuery<R> {
  rows: readonly R[]; row: R | undefined; singleRow: boolean;
  loading: boolean; refusal: Refusal | null; revoked: boolean;
  stale: boolean; retrying: boolean; refetch: () => void;
  refinement: RefinementSurface; source: 'network' | 'device';
  complete: boolean; live: LiveStatus | null;
  deviceFallback: DeviceFallback | null; deviceFallbackReason: string | null;
}

useOpPages<R = Row>(op: string | OpRef<R, F>, o?: UseOpQueryOptions & { autoFirstPage?: boolean }): UseOpPages<R>
interface UseOpPages<R> {
  rows: readonly R[]; hasMore: boolean; empty: boolean; loading: boolean;
  refusal: Refusal | null; revoked: boolean; stale: boolean;
  more: () => void; restart: () => void; exhaust: (maxPages?: number) => void;
}

useCommit<R = Row>(op: string, o?: { autoResolve?: boolean }): UseCommit<R>
useCommit<N extends PlatformOpName>(op: N, o?): UseCommit<PlatformRows[N]>
interface UseCommit<R> {
  commit: (values: Params, rev?: number) => Promise<CommitState<R>>;
  resolve: () => Promise<Row | null>;            // after a conflict: the row now, or null
  retry: (values: Params, rev: number) => Promise<CommitState<R>>;
  reset: () => void;
  state: CommitState<R>; committing: boolean;
  refusal: Refusal | null; conflict: CasConflict | null;
  cas: boolean;                                   // the op declares `rev`
  model: FormModel; refusals: readonly string[];
}
type CommitState<R> =
  | { status: 'idle' } | { status: 'committing' }
  | { status: 'committed'; response: OpResponse<R> }
  | { status: 'conflict'; conflict: CasConflict; current: Row | null;
      currentIs: 'not_fetched' | 'fetching' | 'found' | 'deleted' | 'unavailable' }
  | { status: 'refused'; refusal: Refusal };
interface CasConflict { kind: 'cas_miss'; refusal: Refusal; attempted: Params; sentRev: number | null }

useOpForm(op: string, initial?: FormValues): UseOpForm
type FormValues = Readonly<Record<string, ParamValue | undefined>>;
interface UseOpForm {
  model: FormModel; fields: readonly FormField[]; visible: readonly FormField[];
  values: FormValues;
  setValue: (name: string, value: ParamValue | undefined) => void;
  setValues: (next: FormValues) => void; reset: () => void;
  issues: readonly FormIssue[]; issueFor: (name: string) => FormIssue | undefined;
  valid: boolean;
  params: Record<string, ParamValue>;            // ready for commit(); drops undefined and null
  searchCandidates: (field: string, limit?: number) => Promise<readonly Row[]>;
}
interface FormField {
  name: string; type: OrdoType; required: boolean;
  max: string | null; min: string | null;        // digit strings
  kind: 'scalar' | 'reference' | 'enum' | 'rev' | 'json';
  options: readonly string[] | null;             // enum
  candidates: CandidateSource | null;            // reference
  hidden: boolean;                               // true for rev
}
interface FormIssue { class: 'missing_param' | 'invalid_param' | 'bound_violation'; message: string; param: string | null }

opRef<R, F = AnyFields>(name: string): OpRef<R, F> // an op name carrying its row type

useOrdoAuth(auth: OrdoAuth): { phase: ResumePhase; session; error; retry(): void; auth }
// also exports the SDK's createOrdoAuth and its types

useResumedSession<S>(resume: () => Promise<S | null>): ResumedSession<S>
interface ResumedSession<S> {
  phase: 'checking' | 'signed-in' | 'signed-out' | 'failed';
  session: S | null; error: unknown;
  retry: () => void; set: (session: S | null) => void;
}

useTenantStatus(o: { baseUrl: string; token: TokenSource; enabled?: boolean; fetch?: typeof fetch }): TenantStatusResult
interface TenantStatusResult {
  phase: 'checking' | 'setting-up' | 'stuck' | 'ready' | 'failed';
  status: TenantStatus | null; space: string | null;
  problem: TenantStatusError | null;   // .problem: 'refused' | 'limit_reached' | 'malformed' | 'unavailable'
  retry(): Promise<void>;
}
<TenantGate baseUrl token settingUp?={slot} stuck?={slot} failed?={slot}>
  {(space: string) => ReactNode}
</TenantGate>                          // slot: ReactNode | ((t: TenantStatusResult) => ReactNode)

useRow(id: string | null | undefined): { row: RowKnowledge; menu: ActionMenu; bind: (op: string) => Record<string, string> | null }
useFileUpload(): { phase; progress; error; upload(target: { entity; field; id }, file: Blob): Promise<void>; cancel(); reset() }
useFileDownload(): { phase; progress; error; download(handle: string, name: string): Promise<void> }
useFile(handle: string | null | undefined): { phase; url: string | null; type: string | null; size: number | null; progress; error }
useOwnDownload(): { download: () => Promise<{ blob: Blob; name: string } | null>; busy: boolean; error: Error | null }
useAccountEmail(o: { baseUrl; token: TokenSource; enabled? }): { email; pending; busy; problem; needed; begin(address); confirm(code); cancel(); remove(); refresh() }

// Refusals, re-exported from the SDK
class OrdoRefusal extends Error   // see below
type Refusal = RefusalDetail      // { class, code, message, op, param, status, declared }
```

## @hominemai/ordo-sdk: ops

```ts skip
// @hominemai/ordo-sdk/core/client.js
new OrdoClient(options: OrdoClientOptions)
interface OrdoClientOptions {
  baseUrl: string; space: string; token?: TokenSource;
  renewToken?: () => Promise<boolean>; fetch?: typeof fetch; callTimeoutMs?: number;
  busyRetry?: BusyRetryOptions; busyRetries?: number;          // default 2 retries
  conflictRetry?: ConflictRetryOptions; conflictRetries?: number; // default 3 retries
}
client.withSpace(space: string): OrdoClient
client.space: string

// ./generated/ops.js: one function per op
opName(client: OrdoClient, params: OpNameParams): ReadCall<OpNameRow, OpNameRefusal, OpNameFields>
//   ReadCall:       .where(f => cond | cond[]) .pick('a', 'b')
//                   .fetch(o?): Promise<Page<R>>  .pages(o?): AsyncGenerator<Page<R>>  .rows(o?): Promise<R[]>
//   SingleReadCall: .where() .pick() .fetch(o?): Promise<Page<R>>  .row(o?): Promise<R | null>
//   WriteCall:      .commit(o?): Promise<OpResponse<Row>>
// interface Page<R> extends OpResponse<R> { hasMore: boolean }
// o: { page?: string; idempotencyKey?: string; signal?: AbortSignal; onRetry?: OnRetry }
// Field conditions: f.x.eq(v) .ne(v) .lt(v) .le(v) .gt(v) .ge(v) .in([v]) .isNull()
//   (a sealed field: .eq and .in only). Several conditions are AND.

// @hominemai/ordo-sdk/core/brand.js
asId<B extends string>(value: string): Branded<string, B>

// @hominemai/ordo-sdk/core/refusal.js
class OrdoRefusal<C = RefusalClass> extends Error {
  class: C; code: string; status: number;
  refusal: RefusalDetail<C>;          // same object as .detail
  retryable: boolean; retryAfterMs: number | null;
  disposition: 'conflict' | 'restart' | 'retry' | 'terminal' | 'request' | 'denied' | 'fault';
}
interface RefusalDetail<C> { class: C; code: string; message: string; op: string | null; param: string | null; status: number; declared: boolean }
class OrdoLocalRefusal extends OrdoRefusal { local: true }   // refused before the wire

// @hominemai/ordo-sdk/core/door.js
refusalOf(response: Response, op: string, declared?: readonly string[]): Promise<OrdoRefusal>

// @hominemai/ordo-sdk/core/network.js
class OrdoNetworkError extends Error { kind: 'timeout' | 'unreachable' | 'blocked' }
```

## @hominemai/ordo-sdk: sign-in

`door` is `PasskeyEndpoint`. Sign-in calls throw `PasskeyError`;
tenant calls throw `TenantError`. Both extend `OrdoRefusal` and add
`.failure`.

```ts skip
// @hominemai/ordo-sdk/auth/session.js (also from @hominemai/ordo-react)
createOrdoAuth(o: PasskeyEndpoint & { renewBeforeS?: number; autoRenew?: boolean }): OrdoAuth
interface OrdoAuth {
  session(): PasskeySession | null;
  token(): Promise<string>;          // renews near expiry; signed out: PasskeyError 'refused'
  renew(): Promise<boolean>;         // false: the session ended
  resume(o?: ResumeOptions): Promise<PasskeySession | null>;
  beginEmail(start: EmailSignInStart): Promise<EmailCodeSent>;
  completeEmail(sent: EmailCodeSent, code: string, o?: EmailComplete): Promise<SignInResult>;
  signInWithPasskey(o?: SignInOptions): Promise<PasskeySession>;
  registerPasskey(o?: RegisterOptions): Promise<PasskeySession>;
  stepUp(): Promise<PasskeySession>;
  use(session: PasskeySession | null): void;   // a session from another ceremony
  signOut(): Promise<void>;          // not recorded: PasskeyError 'unavailable'
  spaces(): Promise<readonly ListedSpace[]>;
  waitForTenant(o?: WaitForTenantOptions): Promise<string>;
  clientOptions(): { baseUrl; token; renewToken };   // spread into a client
  subscribe(l: (s: PasskeySession | null) => void): () => void;
}
refreshSession(door, token: string): Promise<PasskeySession>   // POST /auth/refresh
signOut(door, token: string): Promise<void>                   // POST /auth/logout

// @hominemai/ordo-sdk/auth/passkey.js
interface PasskeyEndpoint { baseUrl: string; fetch?: typeof fetch; callTimeoutMs?: number; busyRetry?: BusyRetryOptions }
interface PasskeySession {
  token: string;
  expiresAt: number;            // seconds since the epoch
  sessionExpiresAt: number;     // when renewing stops working
  admission?: { space: string; outcome: string };   // 'admitted' | 'already-member' | 'refused'
  notices?: readonly string[]; device?: string;
  raw: Readonly<Record<string, unknown>>;
}
interface SignInOptions { space?: string; invitation?: string; human?: string }
interface RegisterOptions { display?: string; space?: string; invitation?: string; ticket?: string; human?: string }
registerPasskey(door, o?: RegisterOptions): Promise<PasskeySession>
signInWithPasskey(door, o?: SignInOptions): Promise<PasskeySession>
stepUp(door, token: string): Promise<PasskeySession>
class PasskeyError extends OrdoRefusal {
  failure: 'unsupported' | 'cancelled' | 'refused' | 'unavailable' | 'malformed'
         | 'reauthentication_required' | 'email_unavailable' | 'too_many_attempts' | ...;
  setting?: string; value?: string; retryAfter?: number;
}

// @hominemai/ordo-sdk/auth/email.js
interface EmailSignInStart { address: string; space?: string; invitation?: string; device?: string; human?: string }
interface EmailCodeSent { challenge: string; binder: string }
beginEmailSignIn(door, start: EmailSignInStart): Promise<EmailCodeSent>
interface EmailComplete extends SignInOptions { display?: string }   // display: a new person's name
completeEmailSignIn(door, sent: EmailCodeSent, code: string, o?: EmailComplete): Promise<SignInResult>

// @hominemai/ordo-sdk/auth/second-step.js
type SignInResult =
  | { kind: 'session'; session: PasskeySession }
  | { kind: 'second-step'; step: SecondStep };
signInWithPassword(door, c: { identifier: string; password: string }, o?: SignInOptions): Promise<SignInResult>
completeSecondStep(door, step: SecondStep, answer:
  { kind: 'totp'; code: string } | { kind: 'backup-code'; code: string } | { kind: 'security-key' }
): Promise<{ kind: 'session'; session: PasskeySession; via: string } | { kind: 'proof'; ... }>

// @hominemai/ordo-sdk/auth/methods.js
signInMethods(door, o?: { tenant?: string }): Promise<{
  passkey: boolean; password: boolean; email: boolean; guest: boolean;
  humanCheck: { kind: 'turnstile'; siteKey?: string } | null;
  connections: readonly { name: string; label: string; kind: 'oauth2' | 'oidc'; tenant: boolean }[];
}>

// @hominemai/ordo-sdk/auth/resume.js
resumeSession(door, o?: { onMiss?: (why: 'none' | 'remember_off', message?: string) => void }): Promise<PasskeySession | null>
//   null: nothing to resume. Throws when the door cannot be reached.

// @hominemai/ordo-sdk/auth/provisioning.js
tenantStatus(door, token: TokenSource): Promise<TenantStatus>
retryTenant(door, token: TokenSource): Promise<TenantStatus>
waitForTenant(door, token: TokenSource, o?: { onProgress?; signal?: AbortSignal; timeoutMs?: number }): Promise<string>  // the space id
interface TenantStatus { state: 'off' | 'none' | 'provisioning' | 'ready' | 'failed'; space?: string; step?: string; reason?: string; setting?: string; stuck?: boolean }
class TenantError extends OrdoRefusal { failure: 'failed' | 'off' | 'none' | 'timeout' | 'refused' | 'limit_reached' | 'aborted' }

// @hominemai/ordo-sdk/auth/share.js
listSpaces(door, token: string): Promise<readonly { space: string; name: string | null; role: string | null; via: 'member' | 'guest' }[]>
peekInvitation(door, ticket: string): Promise<{ space: string; role: string | null; ticketExpiresAt: number; named: boolean }>
declineInvitation(door, ticket: string, token?: string): Promise<void>

// @hominemai/ordo-sdk/auth/recovery.js
endEverySession(door, token: string): Promise<void>        // sign out everywhere; needs a fresh touch
```
