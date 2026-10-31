//! `S3Store` — the supported binding. SigV4 over [`crate::http`].
//!
//! One binding for every S3-compatible target, per the owner's ruling:
//! hosted deployments point it at S3 or R2, self-hosters point it at a
//! local gateway pod (MinIO, SeaweedFS). A second storage engine was ruled
//! out; a filesystem binding exists and is a **test double**
//! ([`crate::fs`]).
//!
//! # SigV4, hand-rolled
//!
//! `ordo-identity`'s token MAC and `ordo-commandlog`'s chain hashing are
//! the precedents: a canonical byte construction over `sha2`, written out,
//! because the bytes are the specification. AWS Signature Version 4 is
//! exactly that shape — a canonical request, a string to sign, a four-step
//! key derivation — and it is about eighty lines when nothing else is
//! wanted from it.
//!
//! ```text
//! canonical = METHOD \n URI \n QUERY \n CANONICAL_HEADERS \n SIGNED_HEADERS \n PAYLOAD_HASH
//! sts       = "AWS4-HMAC-SHA256" \n TIMESTAMP \n SCOPE \n sha256(canonical)
//! kDate     = HMAC("AWS4" + secret, yyyymmdd)
//! kRegion   = HMAC(kDate,   region)
//! kService  = HMAC(kRegion, "s3")
//! kSigning  = HMAC(kService,"aws4_request")
//! signature = hex(HMAC(kSigning, sts))
//! ```
//!
//! # `UNSIGNED-PAYLOAD`, and the honest reason
//!
//! A signed payload hash means hashing the object before sending it, which
//! means buffering it — the one thing a streaming upload exists not to do
//! — or implementing AWS's streaming-chunked signature, a second framing
//! protocol on top of the first. `UNSIGNED-PAYLOAD` is what the S3 API
//! offers instead and it is what this binding uses.
//!
//! What that gives up, stated rather than glossed: an on-path attacker who
//! can modify the body cannot be detected *by the signature*. What it does
//! not give up, and why this is acceptable here rather than in general:
//!
//! - The headers are signed, including the key, so the request cannot be
//!   redirected to another object.
//! - The bytes are **already sealed** when they reach this binding —
//!   XChaCha20-Poly1305 with an AEAD tag over the whole object, under a
//!   key the bucket has never seen. A modified body does not decrypt. The
//!   platform's integrity claim rests on the tag, not on the transport,
//!   and it is stronger than the transport's would be.
//! - TLS is required for any non-local endpoint (see [`S3Config::new`]).
//!
//! # No listing, no multipart, no presigning — yet
//!
//! [`crate::BlobStore`] has no `list`, deliberately (see its doc).
//! Multipart is deferred with a stated trigger — when a real upload
//! exceeds the request timeout — and is an addition inside this file, not
//! a change to the trait. Presigned URLs are `blob private`'s
//! (`f7-vault-plan.md` Phase 6): a presigned PUT would let a client put
//! *unsealed* bytes into a space-sealed field's key, which is the one
//! thing the proxy-through-the-API rule exists to prevent.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ordo_crypto::custody::{BlobHandle, SpaceId};
use sha2::{Digest, Sha256};

use crate::http::{self, Endpoint, Request};
use crate::{object_key, BlobError, BlobStore, ByteRange, ByteStream, ObjectHead};

const SERVICE: &str = "s3";
const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const UNSIGNED: &str = "UNSIGNED-PAYLOAD";

#[derive(Clone)]
pub struct S3Config {
    pub endpoint: Endpoint,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    /// `https://host/bucket/key` rather than `https://bucket.host/key`.
    ///
    /// Required by every local gateway and by IP-address endpoints;
    /// optional on S3 proper. Defaulted **on** for a non-TLS or
    /// IP-literal endpoint, because virtual-host style against either is
    /// a DNS lookup that will not resolve and an error that names the
    /// wrong thing.
    pub path_style: bool,
    /// One segment placed **ahead of the space segment** in every key this
    /// binding addresses, so a bucket that holds other things can be told
    /// where this platform's objects are allowed to live.
    ///
    /// `None` is today's layout, `<space>/<hex handle>`, and it is what
    /// [`S3Config::new`] and [`S3Config::from_env`] both produce: the
    /// deployment's own bucket is the deployment's, and a prefix there
    /// would be a rename of every object already in it. The field exists
    /// for an **attachment** (`plan-tenant-storage.md` §2 B-C2), where the
    /// bucket belongs to the tenant and the platform is the guest.
    ///
    /// It is deliberately one segment and not a path. The value reaches
    /// here having been refused at the door on [`crate::space_prefix`]'s
    /// charset, which has no `/`, no `.` and no `..`, for that function's
    /// own stated reason: a prefix that can be sanitised is a prefix under
    /// which two different inputs can collapse onto one location, and this
    /// prefix is exactly what separates an attachment's objects from
    /// whatever else is in the tenant's bucket. Nothing re-checks it here,
    /// so a caller constructing this field directly owes the same check;
    /// `ordo_exec::bucket::AttachedStores` is the one that does.
    pub key_prefix: Option<String>,
    /// How a transient failure of a read, a HEAD or a delete is retried.
    pub retry: S3Retry,
}

/// Retries of a transient failure ([`BlobError::is_transient`]): a read, a
/// HEAD or a delete is sent again up to `retries` times, waiting `base`,
/// then doubling, at most `max`. An upload is not retried here: its body is
/// a stream that has been spent, so its transient failure goes back to the
/// caller as `server_busy`, to send again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct S3Retry {
    /// `ORDO_BLOB_S3_RETRIES`, default 2, from 0 to 10. 0 turns it off.
    pub retries: u32,
    /// `ORDO_BLOB_S3_RETRY_BASE_MS`, default 200, from 1 to 10000.
    pub base: std::time::Duration,
    /// The longest wait: 2 seconds, or the base when that is longer.
    pub max: std::time::Duration,
}

impl Default for S3Retry {
    fn default() -> S3Retry {
        S3Retry {
            retries: 2,
            base: std::time::Duration::from_millis(200),
            max: std::time::Duration::from_secs(2),
        }
    }
}

impl S3Retry {
    /// The wait before retry `n` (from 0).
    pub fn wait(&self, n: u32) -> std::time::Duration {
        self.base
            .saturating_mul(1u32 << n.min(16))
            .min(self.max.max(self.base))
    }

    /// Read `ORDO_BLOB_S3_RETRIES` and `ORDO_BLOB_S3_RETRY_BASE_MS` through
    /// `var`. Unset is the default; a value out of range is refused and the
    /// refusal names the setting.
    pub fn from_vars(var: &dyn Fn(&str) -> Option<String>) -> Result<S3Retry, BlobError> {
        let mut r = S3Retry::default();
        let num = |k: &str, lo: u64, hi: u64| -> Result<Option<u64>, BlobError> {
            match var(k) {
                None => Ok(None),
                Some(v) => v
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|n| (lo..=hi).contains(n))
                    .map(Some)
                    .ok_or_else(|| {
                        BlobError::Config(format!("{k}={v} is not a number from {lo} to {hi}"))
                    }),
            }
        };
        if let Some(n) = num("ORDO_BLOB_S3_RETRIES", 0, 10)? {
            r.retries = n as u32;
        }
        if let Some(ms) = num("ORDO_BLOB_S3_RETRY_BASE_MS", 1, 10_000)? {
            r.base = std::time::Duration::from_millis(ms);
        }
        Ok(r)
    }
}

impl S3Config {
    /// Build a configuration, refusing the two combinations that are
    /// mistakes rather than choices.
    ///
    /// **Plaintext HTTP to a non-local endpoint is refused.** Not because
    /// the bytes need it — they are sealed — but because the *credential*
    /// rides the wire in every request's `Authorization` header, and a
    /// SigV4 signature replayed within its five-minute window is a valid
    /// request. A local gateway on loopback is the exception the
    /// self-host story needs, and it is the only one.
    pub fn new(
        endpoint: &str,
        region: impl Into<String>,
        bucket: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Result<S3Config, BlobError> {
        let endpoint = Endpoint::parse(endpoint)?;
        let local = is_local(&endpoint.host);
        if !endpoint.tls && !local {
            return Err(BlobError::Config(format!(
                "endpoint {} is plaintext http and is not loopback — the object bytes are \
                 sealed, but the SigV4 credential is not, and a captured signature is a \
                 valid request for five minutes",
                endpoint.host
            )));
        }
        let bucket = bucket.into();
        if bucket.is_empty()
            || !bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        {
            return Err(BlobError::Config(format!(
                "bucket {bucket:?} is not an S3 bucket name"
            )));
        }
        let secret_access_key = secret_access_key.into();
        let access_key_id = access_key_id.into();
        if access_key_id.is_empty() || secret_access_key.is_empty() {
            return Err(BlobError::Config(
                "an S3 binding with no credential cannot sign; anonymous access to a \
                 bucket holding customer content is not a configuration this refuses to \
                 guess at"
                    .into(),
            ));
        }
        let path_style =
            !endpoint.tls || local || endpoint.host.parse::<std::net::IpAddr>().is_ok();
        Ok(S3Config {
            endpoint,
            region: region.into(),
            bucket,
            access_key_id,
            secret_access_key,
            path_style,
            key_prefix: None,
            retry: S3Retry::default(),
        })
    }

    /// Read the binding from the environment, in the shape
    /// `LocalKeyProvider::from_env` uses: every variable required, no
    /// silent defaults, and the refusal names the variable.
    pub fn from_env() -> Result<S3Config, BlobError> {
        let var = |k: &str| {
            std::env::var(k)
                .map_err(|_| BlobError::Config(format!("{k} is unset; the S3 binding needs it")))
        };
        let mut cfg = S3Config::new(
            &var("ORDO_BLOB_S3_ENDPOINT")?,
            var("ORDO_BLOB_S3_REGION")?,
            var("ORDO_BLOB_S3_BUCKET")?,
            var("ORDO_BLOB_S3_ACCESS_KEY_ID")?,
            // Or `ORDO_BLOB_S3_SECRET_ACCESS_KEY_FILE`, a file that holds it.
            ordo_env::secret("ORDO_BLOB_S3_SECRET_ACCESS_KEY")
                .map_err(|e| BlobError::Config(e.to_string()))?
                .ok_or_else(|| {
                    BlobError::Config(
                        "ORDO_BLOB_S3_SECRET_ACCESS_KEY is unset (nor its _FILE form); the S3 \
                         binding needs it"
                            .into(),
                    )
                })?,
        )?;
        cfg.retry = S3Retry::from_vars(&|k| std::env::var(k).ok())?;
        Ok(cfg)
    }
}

/// `Debug` by hand, because the derive printed the secret.
///
/// `S3Config` holds an access key and its secret, and a derived `Debug`
/// writes both wherever a config is formatted: a panic message, a
/// `dbg!`, a tracing field, an error chain that carries its context. The
/// credential then lives in whatever collected that line. The endpoint,
/// the region and the bucket are not secrets and stay legible, because a
/// redacted line nobody can read is its own kind of problem.
impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("path_style", &self.path_style)
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

fn is_local(host: &str) -> bool {
    host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host.ends_with(".localhost")
        || host == "minio"
}

pub struct S3Store {
    cfg: S3Config,
}

impl S3Store {
    pub fn new(cfg: S3Config) -> Self {
        S3Store { cfg }
    }

    pub fn shared(cfg: S3Config) -> Arc<dyn BlobStore> {
        Arc::new(S3Store::new(cfg))
    }

    /// The request path for one object.
    ///
    /// `key_prefix` goes ahead of the space segment rather than anywhere
    /// else, so the tenant-visible layout is `<prefix>/<space>/<hex>` and
    /// the space segment keeps the property the crate doc rests on: an
    /// operator holding the bucket and an incident can still answer
    /// "which objects are this tenant's" from a listing. Prepending here,
    /// in the one function every verb's URI comes from, rather than in
    /// [`object_key`], because `object_key` is what `FsStore` and every
    /// caller outside this module mean by "the key", and a prefix that
    /// only one binding honours does not belong in a shared function.
    /// The request target this store would use for an object, without
    /// opening a socket.
    ///
    /// **Public because it is otherwise observable only by connecting**,
    /// and connecting is what makes an assertion about it depend on the
    /// machine it runs on. `path_style: Some(false)` against a loopback
    /// gateway addresses `<bucket>.<host>`; macOS resolves any
    /// `*.localhost` to 127.0.0.1 and glibc does not, so a test that
    /// proved the override by dialling passed on a developer machine and
    /// failed on CI with `Name or service not known`.
    ///
    /// The URI **is** the contract between an attachment and a gateway:
    /// `path_style` decides whether the bucket is a host label or a path
    /// segment, and `key_prefix` decides what precedes the space. Making
    /// that readable is what lets those be asserted as the pure function
    /// they are, with the socket left to prove the separate claim that a
    /// request goes out at all.
    pub fn request_target(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
    ) -> Result<String, BlobError> {
        self.uri(space, handle)
    }

    fn uri(&self, space: &SpaceId, handle: &BlobHandle) -> Result<String, BlobError> {
        let key = match &self.cfg.key_prefix {
            Some(prefix) => format!("{prefix}/{}", object_key(space, handle)?),
            None => object_key(space, handle)?,
        };
        Ok(if self.cfg.path_style {
            format!(
                "/{}/{}",
                uri_encode(&self.cfg.bucket, false),
                encode_key(&key)
            )
        } else {
            format!("/{}", encode_key(&key))
        })
    }

    /// Run `attempt` again while it fails transiently, up to the binding's
    /// [`S3Retry`]. Only for a request with no body: it is sent whole each
    /// time.
    async fn retrying<T, F, Fut>(&self, attempt: F) -> Result<T, BlobError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, BlobError>>,
    {
        let mut n = 0;
        loop {
            match attempt().await {
                Err(e) if e.is_transient() && n < self.cfg.retry.retries => {
                    // The store's own `Retry-After` stretches the wait, up
                    // to the longest wait the binding allows.
                    let wait = self.cfg.retry.wait(n);
                    let wait = match e.retry_after() {
                        Some(ra) => ra
                            .max(wait)
                            .min(self.cfg.retry.max.max(self.cfg.retry.base)),
                        None => wait,
                    };
                    tokio::time::sleep(wait).await;
                    n += 1;
                }
                other => return other,
            }
        }
    }

    fn host_header(&self) -> String {
        if self.cfg.path_style {
            self.cfg.endpoint.authority()
        } else {
            format!("{}.{}", self.cfg.bucket, self.cfg.endpoint.authority())
        }
    }

    async fn call(
        &self,
        method: &'static str,
        uri: &str,
        extra: Vec<(String, String)>,
        body: Option<(u64, ByteStream)>,
    ) -> Result<http::Response, BlobError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| BlobError::Transport("the clock is before the epoch".into()))?
            .as_secs();
        let (date, timestamp) = (yyyymmdd(now), iso8601(now));

        let mut headers: Vec<(String, String)> = vec![
            ("host".into(), self.host_header()),
            ("x-amz-content-sha256".into(), UNSIGNED.into()),
            ("x-amz-date".into(), timestamp.clone()),
        ];
        headers.extend(extra);
        headers.sort_by(|a, b| a.0.cmp(&b.0));

        let signed_headers = headers
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical_headers: String = headers
            .iter()
            .map(|(k, v)| format!("{k}:{}\n", v.trim()))
            .collect();

        let canonical =
            format!("{method}\n{uri}\n\n{canonical_headers}\n{signed_headers}\n{UNSIGNED}");
        let scope = format!("{date}/{}/{SERVICE}/aws4_request", self.cfg.region);
        let sts = format!(
            "{ALGORITHM}\n{timestamp}\n{scope}\n{}",
            hex(&Sha256::digest(canonical.as_bytes()))
        );
        let signature = hex(&ordo_crypto::hmac_sha256(
            &signing_key(&self.cfg.secret_access_key, &date, &self.cfg.region),
            sts.as_bytes(),
        ));
        let authorization = format!(
            "{ALGORITHM} Credential={}/{scope}, SignedHeaders={signed_headers}, \
             Signature={signature}",
            self.cfg.access_key_id
        );

        // `host` is emitted by the transport from the endpoint, and it
        // must be the same string that was signed — so it is dropped here
        // rather than sent twice.
        let mut wire: Vec<(String, String)> =
            headers.into_iter().filter(|(k, _)| k != "host").collect();
        wire.push(("authorization".into(), authorization));

        http::send(
            &Endpoint {
                host: if self.cfg.path_style {
                    self.cfg.endpoint.host.clone()
                } else {
                    format!("{}.{}", self.cfg.bucket, self.cfg.endpoint.host)
                },
                ..self.cfg.endpoint.clone()
            },
            Request {
                method,
                path: uri.to_string(),
                headers: wire,
                body,
            },
        )
        .await
    }
}

/// The four-step derivation. Written out because it is the specification.
fn signing_key(secret: &str, date: &str, region: &str) -> [u8; 32] {
    let k_date = ordo_crypto::hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = ordo_crypto::hmac_sha256(&k_date, region.as_bytes());
    let k_service = ordo_crypto::hmac_sha256(&k_region, SERVICE.as_bytes());
    ordo_crypto::hmac_sha256(&k_service, b"aws4_request")
}

fn hex(b: &[u8]) -> String {
    ordo_crypto::hex(b)
}

/// RFC 3986 unreserved-set encoding, AWS's variant: `/` is encoded unless
/// the caller says it is a path separator.
fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved || (keep_slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// An object key is `<space>/<hex>`, and both halves are already drawn
/// from `[A-Za-z0-9_-]` — `space_prefix` refuses anything else and the
/// handle is hex — so this encoding is the identity today. It is applied
/// anyway, because the day someone widens the space charset is the day a
/// key stops being URL-safe, and an unencoded key is a signature that does
/// not match rather than a bug anyone can see.
fn encode_key(key: &str) -> String {
    uri_encode(key, true)
}

fn yyyymmdd(epoch_secs: u64) -> String {
    let (y, m, d) = civil_from_days((epoch_secs / 86_400) as i64);
    format!("{y:04}{m:02}{d:02}")
}

fn iso8601(epoch_secs: u64) -> String {
    let (y, m, d) = civil_from_days((epoch_secs / 86_400) as i64);
    let s = epoch_secs % 86_400;
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        s / 3600,
        (s % 3600) / 60,
        s % 60
    )
}

/// Howard Hinnant's `civil_from_days`. The workspace has no date library
/// and this is the only place a wall-clock date is needed; four lines of
/// arithmetic beats a dependency, and the algorithm is a published one
/// rather than an invented one.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Drain and discard a response body. Every non-streaming call must do
/// this before returning, or the connection is dropped mid-response and
/// the peer logs a client abort for a request that succeeded.
async fn drain(r: http::Response) -> u16 {
    let status = r.status;
    let _ = crate::collect(r.body).await;
    status
}

async fn body_text(r: http::Response, limit: usize) -> String {
    match crate::collect(r.body).await {
        Ok(b) => String::from_utf8_lossy(&b[..b.len().min(limit)]).into_owned(),
        Err(_) => String::new(),
    }
}

#[async_trait::async_trait]
impl BlobStore for S3Store {
    fn binding(&self) -> &'static str {
        "s3"
    }

    fn production_ready(&self) -> bool {
        true
    }

    async fn put(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
        len: u64,
        body: ByteStream,
    ) -> Result<ObjectHead, BlobError> {
        let uri = self.uri(space, handle)?;
        // `If-None-Match: *` is the conditional-write S3 added for exactly
        // this: refuse if the key exists, atomically, without a
        // HEAD-then-PUT window. A gateway that does not implement it
        // answers 200 and the write goes through — which is why the
        // contract suite asserts the refusal against every binding rather
        // than trusting the header, and why a gateway that fails that
        // assertion is a gateway this platform does not support.
        let r = self
            .call(
                "PUT",
                &uri,
                vec![("if-none-match".into(), "*".into())],
                Some((len, body)),
            )
            .await?;
        match r.status {
            200 | 201 => {
                drain(r).await;
                Ok(ObjectHead { bytes: len })
            }
            409 | 412 => {
                drain(r).await;
                Err(BlobError::AlreadyExists)
            }
            _ => Err(status_failure(r).await),
        }
    }

    /// A read, sent again on a transient failure ([`S3Retry`]).
    async fn get(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
        range: Option<ByteRange>,
    ) -> Result<ByteStream, BlobError> {
        self.retrying(|| self.get_once(space, handle, range)).await
    }

    async fn head(&self, space: &SpaceId, handle: &BlobHandle) -> Result<ObjectHead, BlobError> {
        self.retrying(|| self.head_once(space, handle)).await
    }

    async fn delete(&self, space: &SpaceId, handle: &BlobHandle) -> Result<(), BlobError> {
        self.retrying(|| self.delete_once(space, handle)).await
    }
}

/// One attempt of each bodiless verb; the trait methods retry them.
impl S3Store {
    async fn get_once(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
        range: Option<ByteRange>,
    ) -> Result<ByteStream, BlobError> {
        let uri = self.uri(space, handle)?;
        let extra = match range {
            None => vec![],
            Some(r) => vec![(
                "range".into(),
                match r.last() {
                    Some(last) => format!("bytes={}-{last}", r.offset),
                    None => format!("bytes={}-", r.offset),
                },
            )],
        };
        let r = self.call("GET", &uri, extra, None).await?;
        match r.status {
            200 | 206 => Ok(r.body),
            404 => {
                drain(r).await;
                Err(BlobError::NotFound)
            }
            416 => {
                drain(r).await;
                Err(BlobError::Range)
            }
            _ => Err(status_failure(r).await),
        }
    }

    async fn head_once(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
    ) -> Result<ObjectHead, BlobError> {
        let uri = self.uri(space, handle)?;
        let r = self.call("HEAD", &uri, vec![], None).await?;
        match r.status {
            200 => {
                let bytes = r
                    .header("content-length")
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| {
                        BlobError::Transport("a HEAD answered without a content-length".into())
                    })?;
                drain(r).await;
                Ok(ObjectHead { bytes })
            }
            404 => {
                drain(r).await;
                Err(BlobError::NotFound)
            }
            _ => Err(status_failure(r).await),
        }
    }

    async fn delete_once(&self, space: &SpaceId, handle: &BlobHandle) -> Result<(), BlobError> {
        let uri = self.uri(space, handle)?;
        let r = self.call("DELETE", &uri, vec![], None).await?;
        match r.status {
            // S3 answers 204 whether or not the key was there, which is
            // the idempotence phase 2 of the sweep depends on. 404 is
            // folded in for gateways that answer it anyway.
            200 | 204 | 404 => {
                drain(r).await;
                Ok(())
            }
            _ => Err(status_failure(r).await),
        }
    }
}

/// The error for a response that is not a success: its status, its
/// `Retry-After` and the start of its body, classified once on
/// [`BlobError`] ([`BlobError::class`]).
async fn status_failure(r: http::Response) -> BlobError {
    let status = r.status;
    let retry_after = r.retry_after();
    let body = body_text(r, 512).await;
    BlobError::status(status, retry_after, body.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> S3Config {
        S3Config::new(
            "https://s3.eu-west-1.amazonaws.com",
            "eu-west-1",
            "ordo-blobs",
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        )
        .unwrap()
    }

    /// AWS's own published test vector for the SigV4 key derivation
    /// (`AWS4-HMAC-SHA256` developer guide, `us-east-1`/`iam`), adapted to
    /// this crate's `s3` service constant by re-deriving the last two
    /// steps. Pinning the derivation against a published vector is what
    /// makes "hand-rolled" a claim rather than a hope — the same standard
    /// `hmac_matches_rfc4231_case_2` holds `hmac_sha256` to.
    #[test]
    fn the_signing_key_derivation_is_the_published_one() {
        let secret = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
        let k_date = ordo_crypto::hmac_sha256(format!("AWS4{secret}").as_bytes(), b"20120215");
        assert_eq!(
            hex(&k_date),
            "969fbb94feb542b71ede6f87fe4d5fa29c789342b0f407474670f0c2489e0a0d",
            "kDate is the published vector"
        );
        let k_region = ordo_crypto::hmac_sha256(&k_date, b"us-east-1");
        assert_eq!(
            hex(&k_region),
            "69daa0209cd9c5ff5c8ced464a696fd4252e981430b10e3d3fd8e2f197d7a70c"
        );
        // And the whole four-step chain agrees with itself.
        assert_eq!(signing_key(secret, "20120215", "us-east-1"), {
            let s = ordo_crypto::hmac_sha256(&k_region, b"s3");
            ordo_crypto::hmac_sha256(&s, b"aws4_request")
        });
    }

    #[test]
    fn plaintext_http_is_refused_off_loopback_and_allowed_on_it() {
        assert!(S3Config::new("http://s3.example.com", "r", "b", "k", "s").is_err());
        let local = S3Config::new("http://127.0.0.1:9000", "r", "b", "k", "s").unwrap();
        assert!(local.path_style, "a gateway needs path style");
    }

    #[test]
    fn an_anonymous_binding_is_refused() {
        assert!(S3Config::new("https://h", "r", "b", "", "s").is_err());
        assert!(S3Config::new("https://h", "r", "b", "k", "").is_err());
    }

    #[test]
    fn the_uri_carries_the_space_and_the_handle_and_nothing_else() {
        let s = S3Store::new(cfg());
        let uri = s.uri(&SpaceId("tenant-a".into()), &[0x0f; 16]).unwrap();
        assert_eq!(uri, "/tenant-a/0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f");
        let mut path = S3Store::new(cfg());
        path.cfg.path_style = true;
        assert_eq!(
            path.uri(&SpaceId("tenant-a".into()), &[0x0f; 16]).unwrap(),
            "/ordo-blobs/tenant-a/0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f"
        );
    }

    #[test]
    fn a_space_id_that_could_escape_the_bucket_never_reaches_a_uri() {
        let s = S3Store::new(cfg());
        assert!(s.uri(&SpaceId("../other".into()), &[0; 16]).is_err());
    }

    #[test]
    fn uri_encoding_is_awss_variant() {
        assert_eq!(uri_encode("a/b", true), "a/b");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
        assert_eq!(uri_encode("a b+c~d", false), "a%20b%2Bc~d");
    }

    #[test]
    fn the_date_arithmetic_agrees_with_known_instants() {
        assert_eq!(yyyymmdd(0), "19700101");
        assert_eq!(iso8601(0), "19700101T000000Z");
        // 2026-09-06T12:34:56Z
        assert_eq!(iso8601(1_788_698_096), "20260906T123456Z");
        // A leap day, which is where hand-rolled calendars go wrong.
        assert_eq!(yyyymmdd(1_709_164_800), "20240229");
    }
}
