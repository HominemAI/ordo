/**
 * `@hominemai/ordo-react` — the headless runtime over an Ordo interface descriptor.
 *
 * Zero DOM opinions. Every export here is state, a verb, or a type, with one
 * exception: `TenantGate` renders the app developer's own content while a
 * person's tenant is set up, and a plain unstyled default for a slot left
 * unset (owner, 2026-10-21). `ui-gen.md` §2: *the hook returns state and
 * verbs; what a table looks like is not this layer's business.*
 */

export * from './descriptor.js';
export { isJsonValue, type JsonValue } from './json.js';
export {
  parseInterface,
  parseTypedId,
  OrdoInterface,
  SchemaVersionError,
  MalformedDescriptorError,
  type TypedId,
  type IdTypeEntryWithOp,
} from './load.js';
export {
  EDGE_CODES,
  REFUSAL_CLASSES,
  isDoorCode,
  isEdgeCode,
  isRefusalClass,
  dispositionOf,
  decodeRefusal,
  isErrorEnvelope,
  OrdoRefusal,
  OrdoLocalRefusal,
  type DoorCode,
  type EdgeCode,
  type RefusalClass,
  type Refusal,
  type RefusalOf,
  type Disposition,
  type ErrorEnvelope,
} from './refusal.js';
export { OrdoNetworkError, type NetworkFailure } from '@hominemai/ordo-sdk/core/network.js';
export type { BusyRetryOptions, ConflictRetryOptions, OnRetry, RetryReason } from './busy.js';
export {
  DescriptorClient,
  DOWNLOAD_NAME,
  assertTerminal,
  downloadName,
  paramOf,
  type OwnDownload,
  type DescriptorClientOptions,
  type DescriptorCallOptions,
  type OpResponse,
  type Params,
  type ParamValue,
  type Row,
  type TokenSource,
} from './client.js';
export {
  REFINE_OPS,
  SEALED_OPS,
  checkRefine,
  encodeRefine,
  fieldsProxy,
  opRef,
  refinementSurface,
  resolveRefine,
  type AnyFields,
  type Conditions,
  type DescriptorRefineOp,
  type OpenField,
  type OpRef,
  type Picked,
  type Refine,
  type RefineCondition,
  type RefineInput,
  type RefineMode,
  type RefineOp,
  type RefinementSurface,
  type RefineTarget,
  type SealedField,
} from './refine.js';
export {
  deriveForm,
  validateForm,
  toParams,
  candidateSource,
  enumOptionsFor,
  isRevParam,
  type FormModel,
  type FormField,
  type FormIssue,
  type FormValues,
  type FieldKind,
  type CandidateSource,
} from './form.js';
export {
  discover,
  actionMenu,
  type RowKnowledge,
  type DiscoveredOp,
  type ActionMenu,
} from './actions.js';
export {
  commitOnce,
  retryAfterConflict,
  commitAddress,
  type CommitInput,
  type CommitOutcome,
  type CasConflict,
  type ConflictLookup,
  type CommitAddress,
} from './commit.js';
export {
  advance,
  collectAll,
  DEFAULT_MAX_PAGES,
  DEFAULT_PUBLISH_EVERY,
  type CollectOptions,
  emptyPageState,
  sameRows,
  nextPage,
  type PageState,
  type PageStep,
} from './pages.js';

export {
  OrdoProvider,
  useOrdo,
  useOrdoRuntime,
  useInterface,
  type OrdoProviderProps,
  type OrdoRuntime,
  type LiveConfig,
} from './hooks/context.js';
export {
  LiveHub,
  createPacer,
  effectivePaceMs,
  DEFAULT_LIVE_PACE_MS,
  DEFAULT_LIVE_CONNECT_MS,
  type LiveStatus,
  type LiveHubOptions,
  type Pacer,
  type PacerTimers,
} from './live.js';
export {
  deviceCopy,
  asDeviceCopy,
  DEFAULT_SYNC_IDLE_MS,
  planDevice,
  isSyncable,
  isRevoking,
  isAccessMap,
  toSyncSpec,
  type DeviceCopy,
  type DeviceCopyOptions,
  type DeviceSource,
  type DeviceObservation,
  type DeviceSnapshot,
  type ObservedLike,
  DEFAULT_DEVICE_REFINE_CHUNK,
  type DeviceFallback,
  type DevicePlan,
  type DeviceOpState,
  type DeviceQuery,
  type ManagerLike,
  type ReplicaLike,
  type SyncSpec,
} from './device.js';
export {
  DeviceRegistry,
  DeviceRegistryProvider,
  useSpaceDevice,
  type DeviceKey,
  type DeviceLease,
  type DeviceRegistryOptions,
  type DeviceRegistryProviderProps,
  type SpaceDevice,
} from './registry.js';
export {
  PLATFORM_OPS,
  isPlatformOp,
  withPlatformOps,
  type MemberChange,
  type PlatformOpName,
  type PlatformRows,
} from './platform.js';
export { useOpForm, type UseOpForm } from './hooks/useOpForm.js';
export { useOpQuery, type UseOpQuery, type UseOpQueryOptions } from './hooks/useOpQuery.js';
export { useOpPages, type ExhaustOptions, type UseOpPages, type UseOpPagesOptions } from './hooks/useOpPages.js';
export { useRow, type UseRow } from './hooks/useRow.js';
export { useOwnDownload, type UseOwnDownload } from './hooks/useOwnDownload.js';
export {
  useCommit,
  type UseCommit,
  type UseCommitOptions,
  type CommitState,
  type ConflictCurrent,
} from './hooks/useCommit.js';
export { guestClient, type GuestFeed, type GuestClientOptions } from './guest.js';
export {
  readTenantStatus,
  retryTenantSetup,
  tenantPause,
  TenantStatusError,
  TENANT_PAUSES_MS,
  DEFAULT_STUCK_PAUSE_MS,
  type TenantEndpoint,
  type TenantProblem,
  type TenantState,
  type TenantStatus,
} from './tenant.js';
export {
  useTenantStatus,
  type TenantPhase,
  type TenantStatusResult,
  type UseTenantStatusOptions,
} from './hooks/useTenantStatus.js';
export { TenantGate, SETTING_UP_TEXT, STUCK_TEXT, type TenantGateProps, type TenantSlot } from './gate.js';
export {
  AccountEmailError,
  accountEmail,
  beginAccountEmail,
  confirmAccountEmail,
  isEmailRequired,
  needsAccountEmail,
  readAccountEmail,
  removeAccountEmail,
  type AccountEmail,
  type AccountEmailEndpoint,
  type AccountEmailProblem,
  type ConfirmedEmail,
  type EmailCodeSent,
} from './email.js';
export {
  useAccountEmail,
  type AccountEmailResult,
  type UseAccountEmailOptions,
} from './hooks/useAccountEmail.js';
export { useResumedSession, type ResumePhase, type ResumedSession } from './hooks/useResumedSession.js';
export { useOrdoAuth, type OrdoAuthState } from './hooks/useOrdoAuth.js';
// The sign-in front door, so a React page imports everything from here.
export {
  createOrdoAuth,
  refreshSession,
  signOut,
  DEFAULT_RENEW_BEFORE_S,
  type OrdoAuth,
  type OrdoAuthOptions,
  type SessionListener,
} from '@hominemai/ordo-sdk/auth/session.js';
export { PasskeyError, type PasskeyEndpoint, type PasskeySession } from '@hominemai/ordo-sdk/auth/passkey.js';
export { TenantError } from '@hominemai/ordo-sdk/auth/provisioning.js';
export type { SignInResult } from '@hominemai/ordo-sdk/auth/second-step.js';
export type { EmailComplete, EmailSignInStart } from '@hominemai/ordo-sdk/auth/email.js';
export type { ListedSpace } from '@hominemai/ordo-sdk/auth/share.js';
export {
  MAX_FILE_BYTES,
  downloadFile,
  fileInfo,
  fileUrl,
  readFile,
  saveBlob,
  uploadFile,
  type DownloadOptions,
  type FileCallOptions,
  type FileClient,
  type FileDoor,
  type FileInfo,
  type FileProgress,
  type FileTarget,
  type OnFileProgress,
  type ShownFile,
  type UploadOptions,
  type XhrLike,
} from './files.js';
export {
  useFile,
  useFileDownload,
  useFileInfo,
  useFileUpload,
  type TransferPhase,
  type UseFile,
  type UseFileDownload,
  type UseFileInfo,
  type UseFileUpload,
} from './hooks/useFiles.js';
