/**
 * Files: upload, info, bytes, a URL to show, a download.
 *
 * The calls are `@hominemai/ordo-sdk`'s (`src/core/files.ts`); its header
 * describes them. This package re-exports them as they are: the SDK's
 * `OrdoRefusal` is this package's too (owner ruling, 2026-10-23;
 * audit-service-interfaces.md, fix order step 7), so nothing is translated.
 */
export {
  MAX_FILE_BYTES,
  downloadFile,
  fileInfo,
  fileUrl,
  readFile,
  saveBlob,
  uploadFile,
} from '@hominemai/ordo-sdk/core/files.js';
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
} from '@hominemai/ordo-sdk/core/files.js';
