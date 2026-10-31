# Files: upload, show and download

For app developers. A file is a row and its bytes. An op writes the row.
The blob door stores the bytes. The SDK and `@hominemai/ordo-react` have
helpers for the door, so a page makes no door call by hand.

## The schema

Declare a `blob` field, optional, on the entity that owns the file.

```
entity attachments {
  note -> notes owned
  name: text
  file: blob?
}
```

The field is empty until the bytes arrive. A blob is never an op
parameter. An op that answers the field answers its handle, or null.

## The door

Three routes. Each carries `X-Ordo-Space` and the bearer token.

| Route | Does |
|---|---|
| `PUT /blob/{entity}/{field}/{id}` | Stores the bytes and sets the row's field. Needs `Content-Length`. At most 100 MiB. |
| `GET /blob/{handle}` | Reads the bytes. Takes `Range`. |
| `GET /blob/{handle}/meta` | Answers `{ "mime": "image/png", "size": 1234 }`. No bytes. |

The row guard decides who reads, on all three. A person who may not see
the row gets `not_found` for its bytes and for its type and size.

An upload is a write of the row. It moves `rev`, it can be reverted, and it
fires the same events as any write.

When file storage is down or busy (S3 answers a 5xx, a 408, 425 or 429, or
a connection fails), the door answers `503 server_busy` with `Retry-After`,
not a 500. Nothing was changed; send the same request again.

- A read, a `meta` or a delete is retried by the server first:
  `ORDO_BLOB_S3_RETRIES` times (default 2), waiting
  `ORDO_BLOB_S3_RETRY_BASE_MS` (default 200), doubling, at most 2 s.
- An upload is not retried by the server, since its body is already spent.
- A refusal that will not pass (a 403, a bad key) is still a 500. Its words
  are in the server's log, never in the answer.

## How a file is served

A stored type is the uploader's claim. Shown as claimed, a file uploaded as
`text/html` or `image/svg+xml` would run its script on the door's origin.
That script could read a fresh token from `/auth/refresh`. So every route
that sends stored bytes (`GET /blob/{handle}`, public reads included, and
`GET /download`) sets four headers in one place
(`ordo_api::blob::stored_bytes`):

| Header | Value |
|---|---|
| `Content-Security-Policy` | `sandbox; default-src 'none'`: no script, no same origin, nothing loads |
| `X-Content-Type-Options` | `nosniff` |
| `Content-Disposition` | `inline` or `attachment` |
| `Content-Type` | the stored type when inline; `application/octet-stream` when saved |

Whether a file renders or is saved is the app developer's choice (owner,
2026-10-24). Until a field can say so (filed for v2), every file takes the
default, one table (`DEFAULT_SHOWN` in `crates/ordo-api/src/blob.rs`):

| Shown inline | Served as |
|---|---|
| `image/png`, `image/jpeg`, `image/gif`, `image/webp`, `image/avif` | itself |
| `video/*`, `audio/*` | itself; ranges work as before |
| `application/pdf` | itself |
| `text/plain` | `text/plain; charset=utf-8` |

Everything else is saved: `text/html`, `image/svg+xml`, XML, and any type
not on the table.

- The stored type is unchanged. `GET /blob/{handle}/meta` and `fileInfo`
  still answer `image/svg+xml`, so an app can show "photo.svg".
- `fileUrl` and `readFile` give a `Blob` of the served type. An SVG comes
  back `application/octet-stream`, so `<img>` will not draw it. Keep an
  app's own SVGs out of uploads, or wait for the per-field choice.
- A PDF shown inline is the one exception to `sandbox`. Chrome's PDF viewer
  shows a blank page under it. It gets `default-src 'none'; object-src
  'self'`: nothing loads but the viewer. A PDF's own script runs inside the
  viewer, with no page, fetch or cookie on the door's origin.
- An HTML file an app later chooses to render still renders under
  `sandbox`, without script.

## Media type and size

The door records both at upload, from `Content-Type` and the byte count.
Read them with `GET /blob/{handle}/meta`, or `fileInfo` in the SDK.

An op cannot read them yet. `photo.mime` and `photo.size` answer `not_a_ref`
(`definition-language.md` §2). So an app that lists many files and must show
each one's type without a door call per file keeps a `kind` and `size` field
on the row. An app that shows one file at a time does not need them.

## In the SDK

```ts
import { uploadFile, fileInfo, fileUrl, downloadFile } from '@hominemai/ordo-sdk';

// The row first, then the bytes.
const id = createdId(await createAttachment(...));
await uploadFile(client, { entity: 'attachments', field: 'file', id }, file, {
  onProgress: (p) => bar.value = p.fraction ?? 0,
  signal,
});

const { type, size } = await fileInfo(client, handle);

const shown = await fileUrl(client, handle);   // shown.url for <img src>
shown.release();                               // when the image goes

await downloadFile(client, handle, 'report.pdf');
```

A page cannot put a token in `<img src>`, so `fileUrl` reads the bytes with
the token and gives an object URL. Call `release()` when done.

Progress uses `XMLHttpRequest`, the only way a browser reports upload
progress. Without it (Node), `onProgress` is called with 0 and then 100%.

There is no deadline on a file call, because a large upload on a slow line
needs longer than any fixed one. Pass a `signal` to stop it.

## In React

```tsx
const up = useFileUpload();
await up.upload({ entity: 'attachments', field: 'file', id }, file);
// up.phase, up.progress.fraction, up.error, up.cancel()

const img = useFile(row.file);       // img.url, img.type, img.size, img.phase
const info = useFileInfo(row.file);  // info.info: { type, size }, no bytes read
const dl = useFileDownload();
await dl.download(row.file, row.name);
```

A `null` handle leaves `useFile` idle. `useFile` releases its URL when the
handle changes or the component goes.

## Errors

Nothing new to catch.

| Error | Means |
|---|---|
| `OrdoRefusal` | The door refused: `not_found`, `access_denied`, `access_guard`, `bound_violation`, or `server_busy` when file storage did not answer (send it again). |
| `OrdoLocalRefusal` (`bound_violation`) | The file is over the cap. Nothing was sent. |
| `OrdoNetworkError` | No answer. In the React package, use the SDK's `namedFetch` to get this. |
| `AbortError` | The caller's own abort. |

## What is not here

- Reading a type or size inside an op. It waits on the compiler (`not_a_ref`).
- Resumable uploads. A failed upload is sent again from the start.
- Opening `blob private` and `blob sealed` fields. The door refuses them by name.
