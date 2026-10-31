/**
 * `@hominemai/ordo-sdk` — the generated TypeScript client.
 *
 * Two halves, and the README says which is which:
 *
 * - `src/core/` — hand-written, hand-versioned, the same bytes for every
 *   deployment: the door client, the terminal-verb call types, the refusal
 *   inventory, the id brand.
 * - `src/generated/` — emitted by `ordo gen ts` from one deployment's
 *   `interface.json`: one typed function per op, its parameters, its row
 *   shape, its refusal union, and the op table the core reads.
 *
 * Regenerate with `ordo gen ts <dir>`; never edit `src/generated`. It is a
 * sample (the conformance fixture's ops) and the root does not re-export it.
 */

export { OrdoClient, DOWNLOAD_NAME, downloadName } from './core/client.js';
export type {
  OwnDownload,
  OpResponse,
  OrdoClientOptions,
  ParamValue,
  Params,
  Pin,
  RequestOptions,
  Row,
  TokenSource,
} from './core/client.js';
export { compareDecimal } from './core/client.js';
export type { FunctionCallOptions, FunctionResponse } from './core/function.js';
export type { BusyRetryOptions, ConflictRetryOptions, OnRetry, RetryReason } from './core/busy.js';
export { ReadCall, SingleReadCall, WriteCall } from './core/call.js';
export type { CallOptions, Page } from './core/call.js';
export {
  OrdoLocalRefusal,
  OrdoRefusal,
  REFUSAL_CLASSES,
  decodeRefusal,
  isRefusalClass,
} from './core/refusal.js';
export type { RefusalClass, RefusalDetail } from './core/refusal.js';
export {
  MAX_FILE_BYTES,
  downloadFile,
  fileInfo,
  fileUrl,
  readFile,
  saveBlob,
  uploadFile,
} from './core/files.js';
export type {
  DownloadOptions,
  FileCallOptions,
  FileClient,
  FileDoor,
  FileInfo,
  FileProgress,
  FileTarget,
  OnFileProgress,
  ShownFile,
  UploadOptions,
  XhrLike,
} from './core/files.js';
export { asId } from './core/brand.js';
export { isJsonValue } from './core/json.js';
export type { JsonValue } from './core/json.js';
export type { Branded } from './core/brand.js';
export type { OpSpec, OrderKey, ParamSpec, ParamType, ReadOrder } from './core/spec.js';
export { checkRefine, encodeRefine, fieldsProxy, SEALED_OPS } from './core/refine.js';
export type {
  AnyFields,
  Conditions,
  OpenField,
  Refine,
  RefineLimits,
  RefineMode,
  SealedField,
} from './core/refine.js';

export {
  LocalReplica,
  IndexedDbBackend,
  MemoryBackend,
  createDeviceKeys,
} from './replica/index.js';
export type { Backend, DeviceKeys, Frame, FrameDelta, HeldRow, ReplicaRecord } from './replica/index.js';
export { syncOp } from './replica/sync.js';
export type { SyncDelta, SyncOptions, SyncState } from './replica/sync.js';
export { refineOp, refineRows, refineRowsAsync, refineMerge, checkQuery, REFINE_OPS, DEFAULT_REFINE_CHUNK } from './replica/refine.js';
export type { LocalQuery, RefineAsyncOptions, RefineCondition, RefineOp, SortKey } from './replica/refine.js';
export { SyncManager, DEFAULT_TOUCH_EVERY_MS } from './replica/manager.js';
export { openDevice, DEFAULT_DEVICE_STORE, DEFAULT_SYNC_IDLE_MS, DEFAULT_ERASE_AFTER_MS } from './replica/device.js';
export type { TabEnvironment, TabLocks, TabChannel, TabVisibility } from './replica/tabs.js';
export type { OpenDeviceOptions, OrdoDevice } from './replica/device.js';
export type {
  ManagedOp,
  Observation,
  ObservedRows,
  SyncDeclaration,
  SyncManagerOptions,
  SyncView,
} from './replica/manager.js';

export { DEFAULT_CALL_TIMEOUT_MS } from './auth/deadline.js';
export {
  OrdoNetworkError,
  namedFetch,
  DEFAULT_CLIENT_CALL_TIMEOUT_MS,
  DEFAULT_PROBE_CACHE_MS,
  DEFAULT_PROBE_PATH,
  DEFAULT_PROBE_TIMEOUT_MS,
} from './core/network.js';
export type { NamedFetchOptions, NetworkFailure, NetworkOptions } from './core/network.js';

export {
  DEFAULT_STUCK_PAUSE_MS,
  DEFAULT_TENANT_WAIT_MS,
  TenantError,
  retryTenant,
  tenantStatus,
  waitForTenant,
  watchTenant,
} from './auth/provisioning.js';
export type {
  TenantFailure,
  TenantState,
  TenantStatus,
  TenantWatch,
  WaitForTenantOptions,
  WatchTenantOptions,
} from './auth/provisioning.js';

export {
  PasskeyError,
  fromBase64Url,
  registerPasskey,
  signInWithPasskey,
  stepUp,
  toBase64Url,
} from './auth/passkey.js';
export type {
  PasskeyEndpoint,
  PasskeyFailure,
  PasskeySession,
  RecoveryNotice,
  RegisterOptions,
  SignInOptions,
} from './auth/passkey.js';

export { ServerBusyError } from './auth/request.js';
export type { AuthEndpoint } from './auth/request.js';

export { RESUME_LOCK, resumeSession } from './auth/resume.js';
export type { ResumeMiss, ResumeOptions } from './auth/resume.js';

export {
  acknowledgeRecovery,
  contestRecovery,
  endEverySession,
  issueRecoveryCodes,
  linkPasskey,
  listAuthenticators,
  recoverWithTicket,
  redeemRecoveryCode,
  removePasskey,
} from './auth/recovery.js';
export type {
  AuthenticatorList,
  AuthenticatorRow,
  Recovered,
  RecoveryStart,
} from './auth/recovery.js';

export {
  DelegationError,
  approveDelegation,
  declineDelegation,
  myDelegations,
  redeemDelegation,
  requestDelegation,
  reviewDelegation,
  revokeDelegation,
  waitForDelegation,
} from './auth/delegation.js';
export type {
  ApprovedDelegation,
  DeclinedDelegation,
  DelegationClaim,
  DelegationDisclosure,
  DelegationEndpoint,
  DelegationFailure,
  DelegationRedemption,
  DelegationRequest,
  DelegationState,
  DisclosedAccess,
  DisclosedOp,
  GrantedDelegation,
  MyDelegation,
  RequestedDelegation,
  RevokedDelegation,
  WaitOptions,
} from './auth/delegation.js';

export {
  acceptShare,
  declineInvitation,
  declineShare,
  leaveShare,
  linkFeed,
  listSpaces,
  openLink,
  openWindow,
  peekInvitation,
  peekShare,
  windowFeed,
} from './auth/share.js';
export type {
  AcceptedShare,
  GuestFeed,
  LinkOptions,
  LinkState,
  ListedLink,
  ListedSpace,
  MaxOpensParam,
  PeekedInvitation,
  PeekedShare,
  SharedRow,
  WindowOptions,
} from './auth/share.js';
export {
  beginTotpEnrolment,
  completeSecondStep,
  confirmTotpEnrolment,
  enrolSecurityKey,
  listFactors,
  reauthenticateWithPasskey,
  reauthenticateWithPassword,
  removeFactor,
  setSecondStepLevel,
  signInWithPasskeyStep,
  signInWithPassword,
} from './auth/second-step.js';
export type {
  ChangeProof,
  Enroller,
  FactorList,
  FactorRow,
  PasswordCredentials,
  ReauthResult,
  SecondStep,
  SecondStepAnswer,
  SecondStepDone,
  SecondStepMethod,
  SignInResult,
  TotpEnrolment,
} from './auth/second-step.js';

export { beginEmailSignIn, completeEmailSignIn } from './auth/email.js';
export {
  accountEmail,
  beginAccountEmail,
  confirmAccountEmail,
  isEmailRequired,
  removeAccountEmail,
} from './auth/account-email.js';
export type { AccountEmail, ConfirmedEmail } from './auth/account-email.js';
export type { EmailCodeSent, EmailComplete, EmailSignInStart } from './auth/email.js';
export { DEFAULT_RENEW_BEFORE_S, createOrdoAuth, refreshSession, signOut } from './auth/session.js';
export type { OrdoAuth, OrdoAuthOptions, SessionListener } from './auth/session.js';
export {
  beginClaim,
  claimPasskey,
  completeClaim,
  emailCodeCeremony,
  handoffCeremony,
  passwordCeremony,
} from './auth/claim.js';
export type { ClaimCeremony, ClaimLinked, ClaimOpened } from './auth/claim.js';
export { guestCeremony, resumeGuest, startGuest } from './auth/guest.js';
export type { GuestStart, GuestStarted } from './auth/guest.js';
export { signInMethods } from './auth/methods.js';
export type {
  HumanCheckMethod,
  MethodConnection,
  MethodConnectionKind,
  SignInMethods,
  SignInMethodsOptions,
} from './auth/methods.js';

export {
  ConnectionError,
  addConnection,
  beginTenantSignIn,
  completeTenantSignIn,
  listConnections,
  retireConnection,
  updateConnection,
} from './auth/connections.js';
export type {
  Connection,
  ConnectionChanges,
  ConnectionEndpoint,
  ConnectionFailure,
  ConnectionKind,
  ConnectionRole,
  ConnectionSettings,
  NewConnection,
  TenantSignInBegun,
  TenantSignInFinish,
  TenantSignInStart,
} from './auth/connections.js';

export {
  AccessError,
  addToGroup,
  createGroup,
  deleteGroup,
  groups,
  profile,
  removeFromGroup,
  resetOpAccess,
  setMemberAccess,
  setOpAccess,
} from './auth/access.js';
export type {
  AccessEndpoint,
  AccessRole,
  Group,
  GroupCreated,
  GroupDeleted,
  GroupMembership,
  MemberAccessChange,
  MemberAccessSet,
  MemberEffect,
  OpAccess,
  OpAccessChange,
  OpAccessReset,
  OpAccessSet,
} from './auth/access.js';

export {
  HANDOFF_BINDER_KEY,
  binderHash,
  finishSubdomainSignIn,
  issueHandoff,
  makeBinder,
  readHandoffRequest,
  startSubdomainSignIn,
} from './auth/handoff.js';
export type {
  HandoffHistory,
  HandoffIssue,
  HandoffIssued,
  HandoffRequest,
  HandoffStorage,
  SubdomainSignInFinish,
  SubdomainSignInStart,
} from './auth/handoff.js';

// The generated surface is not re-exported here. `src/generated/` holds the
// conformance fixture's ops, a sample: an app imports its own, written by
// `ordo gen ts <dir> --app <src>` to `<src>/generated/`.
