//! The identity service's server: boot, first start, the router's frame
//! (CORS, strict headers, cookies), serving, and the minute tick.
//!
//! What each call does is in [`crate::flows`]; the HTTP shape of each route
//! is in [`crate::routes`].
//!
//! Browser calls carry the session cookie and are allowed from the
//! service's own origin and `ORDO_IDENTITY_CLIENT_ORIGINS` only, with CORS
//! for those origins and credentials ([`cors`]). A CLI sends its session
//! secret in the body and no `Origin`. The console and machines call the
//! admin routes with a deployment token as the bearer.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_MAX_AGE, CACHE_CONTROL, CONTENT_SECURITY_POLICY,
    COOKIE, ORIGIN, REFERRER_POLICY, VARY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use ordo_identity::clock::Clock;
use ordo_identity::origin::Origin;
use ordo_identity::passkey::PasskeyProvider;
use ordo_identity::provider::PasswordHasher;
use ordo_identity::sealing::SealingKey;
use ordo_identity::Argon2idHasher;

use crate::audit;
use crate::config::{IdentityConfig, KEY_FILE_ENV, LISTEN_ENV};
use crate::error::IdentityError;
use crate::import::ImportStep;
use crate::invites::{self, Minted, FIRST_BOOT};
use crate::keys::KeyRing;
use crate::machine::{self, MachineSignIn, PROVISIONER};
use crate::people::Actor;
use crate::sessions::Lifetimes;
use crate::store::{dep, Store};

/// The JWKS's cache life: what a verifier may hold it for (4.2).
pub const JWKS_MAX_AGE_SECS: u64 = 600;
/// How often the service checks for a due rotation, retirement or
/// checkpoint.
pub const TICK_SECS: u64 = 60;
/// The file the first invite is written to on the service's volume (5.2).
pub const FIRST_INVITE_FILE: &str = "first-invite";

/// The API answers JSON and is never framed or scripted. The page sets
/// its own, [`crate::page::PAGE_CSP`], on its own routes.
const CSP: &str = "default-src 'none'; frame-ancestors 'none'; base-uri 'none'";

/// What a test may change about boot.
#[derive(Clone)]
pub struct BootOptions {
    pub clock: Clock,
    /// How recovery codes are hashed: argon2id.
    pub hasher: Arc<dyn PasswordHasher>,
}

impl Default for BootOptions {
    fn default() -> Self {
        BootOptions {
            clock: Clock::system(),
            hasher: Arc::new(Argon2idHasher::new()),
        }
    }
}

/// The running service.
#[derive(Clone)]
pub struct Service(pub(crate) Arc<Inner>);

pub(crate) struct Inner {
    pub(crate) cfg: IdentityConfig,
    pub(crate) store: Store,
    pub(crate) keys: KeyRing,
    pub(crate) clock: Clock,
    pub(crate) allowed: Vec<Origin>,
    /// Every ceremony but the touch: the service's own origin only.
    pub(crate) provider: Arc<PasskeyProvider>,
    /// The touch: the service's origin and each client origin under the
    /// RP ID ([`crate::passkeys::ceremony_origins`]).
    pub(crate) touch_provider: Arc<PasskeyProvider>,
    pub(crate) hasher: Arc<dyn PasswordHasher>,
    pub(crate) machines: MachineSignIn,
}

/// What the first start did (5.1, 5.2).
#[derive(Debug, Default)]
pub struct FirstStart {
    /// The provisioner's machine key id, when one was made.
    pub provisioner: Option<String>,
    /// The first invite, when one was minted.
    pub first_invite: Option<Minted>,
    /// What the console import did at this start (6.1, 6.2).
    pub import: ImportStep,
}

impl Service {
    /// Open the key file and the database, migrate, and make or load the
    /// signing keys. Nothing here acts on people: a host command boots the
    /// same way and runs alone. [`Service::first_start`] runs before
    /// serving.
    pub async fn boot(cfg: IdentityConfig, opts: BootOptions) -> Result<Service, IdentityError> {
        let loaded = SealingKey::load_or_create(&cfg.key_file, KEY_FILE_ENV, "identity service")
            .map_err(|e| match e.cause {
                Some(c) => IdentityError::setting_caused(e.setting, e.problem, e.fix, c),
                None => IdentityError::setting(e.setting, e.problem, e.fix),
            })?;
        if loaded.made {
            eprintln!(
                "ordo-identity-service: made a new sealing key at {}. Back it up with the \
                 identity database: the signing keys do not open without it.",
                cfg.key_file.display()
            );
        }
        let now = opts.clock.now();
        let store = Store::open(&cfg).await?;
        store.migrate(now).await?;
        let keys = KeyRing::boot(
            store.pool(),
            &cfg.deployment,
            loaded.key,
            cfg.key_rotate_days,
            cfg.token_secs,
            now,
        )
        .await?;
        // Sign-in, join, passkey registration and recovery run on the
        // service's own page only. The touch alone may run in a client's
        // page (5.4 step 6), so it has its own relying party with the
        // widened list. Two providers, two challenge stores: a challenge
        // begun for one ceremony cannot be finished as the other.
        let provider = crate::passkeys::provider(
            store.pool(),
            &cfg.deployment,
            &cfg.rp_id,
            std::slice::from_ref(&cfg.origin),
        )?;
        let touch_origins =
            crate::passkeys::ceremony_origins(&cfg.rp_id, &cfg.origin, &cfg.client_origins);
        let touch_provider =
            crate::passkeys::provider(store.pool(), &cfg.deployment, &cfg.rp_id, &touch_origins)?;
        let mut allowed = cfg.client_origins.clone();
        if let Some(own) = Origin::parse(&cfg.origin) {
            allowed.push(own);
        }
        Ok(Service(Arc::new(Inner {
            cfg,
            store,
            keys,
            clock: opts.clock,
            allowed,
            provider: Arc::new(provider),
            touch_provider: Arc::new(touch_provider),
            hasher: opts.hasher,
            machines: MachineSignIn::default(),
        })))
    }

    /// Before serving: first the console import (6.1), then, on a
    /// deployment with no live admin, make the provisioning service's
    /// machine key from `ORDO_IDENTITY_PROVISIONER_KEY` (5.1), or, with no
    /// such key, mint the first invite, print it once and write it to
    /// `<data dir>/first-invite` (5.2). Each of the last two happens once in
    /// the deployment's life. An import that brought a live admin means
    /// neither happens.
    pub async fn first_start(&self) -> Result<FirstStart, IdentityError> {
        let now = self.now();
        let handle = self.handle();
        let pool = self.store().pool();
        let mut out = FirstStart {
            import: crate::import::run(
                pool,
                handle,
                &self.0.keys,
                self.0.cfg.import_console.as_ref(),
                &self.0.cfg.rp_id,
                now,
            )
            .await?,
            ..FirstStart::default()
        };
        let tx = pool.begin().await?;
        tx.rows(&format!(
            "SELECT pg_advisory_xact_lock(hashtext({}))",
            ordo_exec::sql_lit(&format!("ordo-identity-first-start:{handle}"))
        ))
        .await?;
        let live_admin = !tx
            .rows(&format!(
                "SELECT 1 FROM people WHERE {} AND ended_at IS NULL LIMIT 1",
                dep(handle)
            ))
            .await?
            .is_empty();
        if live_admin {
            tx.commit().await?;
            return Ok(out);
        }
        let boot = Actor {
            id: "boot",
            client: "",
        };
        if let Some(key) = self.0.cfg.provisioner_key {
            if !machine::ever_named(&tx, handle, PROVISIONER).await? {
                let made =
                    machine::add(&tx, handle, &self.0.keys, PROVISIONER, Some(key), boot, now)
                        .await?;
                out.provisioner = Some(made.id);
            }
        } else {
            let minted_before = !tx
                .rows(&format!(
                    "SELECT 1 FROM invites WHERE {} AND issued_by = {} LIMIT 1",
                    dep(handle),
                    ordo_exec::sql_lit(FIRST_BOOT)
                ))
                .await?
                .is_empty();
            if !minted_before {
                let minted = invites::mint(
                    &tx,
                    handle,
                    &self.0.keys,
                    &self.0.cfg.origin,
                    "first admin",
                    boot,
                    self.0.cfg.invite_days,
                    now,
                )
                .await?;
                out.first_invite = Some(minted);
            }
        }
        tx.commit().await?;
        if let Some(m) = &out.first_invite {
            let path = self.first_invite_path();
            if let Err(e) = write_private(&path, &format!("{}\n", m.link)) {
                eprintln!(
                    "ordo-identity-service: could not write the first invite to {}: {e}",
                    path.display()
                );
            }
            eprintln!(
                "ordo-identity-service: no deployment people yet. The first admin signs up at \
                 this link, once, within {} days:\n\n  {}\n\nIt is also in {}. Another link \
                 later: `ordo-identity-service invite --label \"…\"` on the host.",
                self.0.cfg.invite_days,
                m.link,
                path.display()
            );
        }
        Ok(out)
    }

    pub fn first_invite_path(&self) -> PathBuf {
        self.0.cfg.data_dir.join(FIRST_INVITE_FILE)
    }

    pub fn config(&self) -> &IdentityConfig {
        &self.0.cfg
    }

    pub fn store(&self) -> &Store {
        &self.0.store
    }

    pub fn keys(&self) -> &KeyRing {
        &self.0.keys
    }

    pub fn now(&self) -> u64 {
        self.0.clock.now()
    }

    pub fn lifetimes(&self) -> Lifetimes {
        Lifetimes {
            idle_hours: self.0.cfg.session_idle_hours,
            max_hours: self.0.cfg.session_max_hours,
        }
    }

    /// The deployment handle this service serves.
    pub fn handle(&self) -> &str {
        &self.0.cfg.deployment
    }

    /// What is due each minute: a rotation, a retirement, a checkpoint.
    pub async fn tick(&self) -> Result<(), IdentityError> {
        let now = self.now();
        self.0.keys.tick(self.0.store.pool(), now).await?;
        audit::checkpoint_if_due(self.0.store.pool(), self.handle(), &self.0.keys, now).await?;
        Ok(())
    }

    /// The routes, inside CORS and strict headers: at the root, and under
    /// the base path too when one is set (`ORDO_IDENTITY_BASE_PATH`), so
    /// the console can pass browsers through on its own origin while the
    /// door and the console keep calling the root on loopback.
    pub fn router(&self) -> Router {
        let app = crate::routes::routes().merge(crate::page::routes());
        let app = match self.0.cfg.base_path.as_str() {
            "" => app,
            // `{base}/` as well as `{base}`: both open the account page.
            base => app
                .clone()
                .nest(base, app)
                .route(&format!("{base}/"), axum::routing::get(crate::page::home)),
        };
        app.layer(axum::middleware::from_fn_with_state(self.clone(), cors))
            .layer(axum::middleware::map_response(strict_headers))
            .with_state(self.clone())
    }

    /// Run the first start, then listen and serve until interrupted,
    /// ticking every minute.
    pub async fn serve(self) -> Result<(), IdentityError> {
        self.first_start().await?;
        let addr = self.0.cfg.listen;
        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
            IdentityError::setting_caused(
                LISTEN_ENV,
                format!("is {addr}, and the identity service could not listen there"),
                format!("Set {LISTEN_ENV} to a free address and port"),
                e,
            )
        })?;
        eprintln!(
            "ordo-identity-service: listening on {addr} for deployment `{}`, issuer {}",
            self.handle(),
            self.0.cfg.origin
        );
        let ticker = self.clone();
        let tick = tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(TICK_SECS));
            loop {
                every.tick().await;
                if let Err(e) = ticker.tick().await {
                    eprintln!("ordo-identity-service: the minute tick failed: {e}");
                }
            }
        });
        let served = axum::serve(
            listener,
            self.router()
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(stop_signal())
        .await
        .map_err(|e| IdentityError::Internal(format!("the server stopped: {e}")));
        tick.abort();
        served
    }

    /// `__Host-` over https at the root. Under a base path the cookie is
    /// scoped to that path, which `__Host-` forbids, so it is `__Secure-`.
    fn cookie_name(&self) -> &'static str {
        match (self.0.cfg.secure_origin(), self.0.cfg.base_path.is_empty()) {
            (true, true) => "__Host-ordo-identity",
            (true, false) => "__Secure-ordo-identity",
            (false, _) => "ordo-identity",
        }
    }

    /// The cookie's path: the base path, so a browser sends it to the
    /// service and never to the console beside it.
    fn cookie_path(&self) -> &str {
        match self.0.cfg.base_path.as_str() {
            "" => "/",
            base => base,
        }
    }

    /// The `Set-Cookie` for a browser session: `HttpOnly`, `SameSite=Lax`
    /// so the redirect back from sign-in carries it (5.3), `Secure` and
    /// `__Host-` over https.
    pub fn session_cookie(&self, secret: &str) -> String {
        format!(
            "{}={secret}; Path={}; HttpOnly; SameSite=Lax; Max-Age={}{}",
            self.cookie_name(),
            self.cookie_path(),
            self.0.cfg.session_max_secs(),
            self.secure_attr()
        )
    }

    pub(crate) fn clear_cookie(&self) -> String {
        format!(
            "{}=; Path={}; HttpOnly; SameSite=Lax; Max-Age=0{}",
            self.cookie_name(),
            self.cookie_path(),
            self.secure_attr()
        )
    }

    fn secure_attr(&self) -> &'static str {
        if self.0.cfg.secure_origin() {
            "; Secure"
        } else {
            ""
        }
    }

    fn cookie(&self, headers: &HeaderMap) -> Option<String> {
        let name = self.cookie_name();
        headers
            .get_all(COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|kv| kv.trim().split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
            .filter(|v| !v.is_empty())
    }

    /// The `Origin` a browser sent, when it is one this service answers.
    fn allowed_origin(&self, headers: &HeaderMap) -> Option<String> {
        let sent = headers.get(ORIGIN)?.to_str().ok()?;
        let origin = Origin::parse(sent)?;
        self.0.allowed.contains(&origin).then(|| sent.to_string())
    }

    /// A call that may come from a browser: an `Origin`, when sent, must be
    /// one this service answers.
    pub(crate) fn origin_ok(&self, headers: &HeaderMap) -> Result<(), IdentityError> {
        if headers.contains_key(ORIGIN) && self.allowed_origin(headers).is_none() {
            return Err(IdentityError::OriginRefused);
        }
        Ok(())
    }

    /// The session secret a call carries: in the body (a CLI), or in the
    /// cookie (a browser). A cookie is honoured only with an allowed
    /// `Origin`, so another site's page cannot spend it.
    pub(crate) fn secret(
        &self,
        headers: &HeaderMap,
        body: &serde_json::Value,
    ) -> Result<String, IdentityError> {
        self.origin_ok(headers)?;
        if let Some(s) = body.get("session") {
            return s
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| IdentityError::BadRequest("`session` is not a string".into()));
        }
        match self.cookie(headers) {
            Some(c) if self.allowed_origin(headers).is_some() => Ok(c),
            Some(_) => Err(IdentityError::OriginRefused),
            None => Err(IdentityError::NotSignedIn),
        }
    }
}

/// Write a file only its owner reads, replacing any older one.
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

/// CORS for the service's own origin and `ORDO_IDENTITY_CLIENT_ORIGINS`,
/// with credentials. A preflight from any other origin is refused; a
/// request from one gets no CORS headers, so its page cannot read the
/// answer.
async fn cors(State(s): State<Service>, req: Request, next: Next) -> Response {
    let allowed = s.allowed_origin(req.headers());
    if req.method() == Method::OPTIONS {
        let Some(origin) = allowed else {
            return IdentityError::OriginRefused.into_response();
        };
        let mut res = StatusCode::NO_CONTENT.into_response();
        let h = res.headers_mut();
        add_cors(h, &origin);
        h.insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST"),
        );
        h.insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type, authorization"),
        );
        h.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        return res;
    }
    let mut res = next.run(req).await;
    if let Some(origin) = allowed {
        add_cors(res.headers_mut(), &origin);
    }
    res
}

fn add_cors(h: &mut HeaderMap, origin: &str) {
    if let Ok(v) = HeaderValue::from_str(origin) {
        h.insert(ACCESS_CONTROL_ALLOW_ORIGIN, v);
        h.insert(
            ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }
    h.append(VARY, HeaderValue::from_static("origin"));
}

async fn strict_headers(mut res: Response) -> Response {
    let h = res.headers_mut();
    if !h.contains_key(CONTENT_SECURITY_POLICY) {
        h.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    }
    h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    if !h.contains_key(CACHE_CONTROL) {
        h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
