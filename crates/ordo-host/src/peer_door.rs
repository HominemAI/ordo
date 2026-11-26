//! The holder's door for other Ordo deployments
//! (`plan-federation-grants.md` 2, 3, 5 and 7, step G3).
//!
//! A **consumer** deployment calls this one, the **holder**, with a token
//! the identity service signed (`ordo_identity::peer_token`, step G2):
//!
//! - `typ` `ordo-peer+jwt`: on behalf of a person, `sub` the person,
//!   `act.sub` the consumer;
//! - `typ` `ordo-client+jwt`: the consumer as itself, `kind` `client`.
//!
//! Both carry `aud` `ordo-peer:<this handle>` and `chain`, the deployments
//! the call has passed through, the consumer last.
//!
//! # What the door checks
//!
//! In [`PeerDoor::admit`], once per token (the token gate, which may wait):
//!
//! 1. The signature with the identity service's keys
//!    ([`crate::identity_keys`]), the `typ`, the issuer of this
//!    deployment's sign-in connection to the service, the audience, the
//!    expiry. A signature that fails gets the door's uniform denial; every
//!    other refusal is named.
//! 2. The loop: a chain that already names this deployment is refused
//!    (rule 9).
//! 3. On behalf of a person, the person is resolved exactly as a direct
//!    sign-in through that connection resolves them: the authenticator
//!    `oidc:<connection>:<sub>`, the deployment's sign-up rules for a
//!    stranger (open sign-up makes a principal, invite-only refuses), its
//!    sign-up ceiling.
//!
//! In [`PeerDoor::decide`], on every op call (pure, in memory):
//!
//! 4. The consumer's grants in the version serving now: each `grant <name>`
//!    whose `ORDO_CONSUMER_<NAME>_CLIENT` is the consumer's id, and, on
//!    behalf of a person only, `grant provider <connection>`.
//! 5. The op must be in one of them. As itself it must be in a grant marked
//!    `as itself`, and reaches only the deployment's own space.
//!
//! Then the caller's own access rules decide: on behalf of a person, the
//! access engine for that person, as for a direct sign-in
//! (`IdentityAccess::check`).
//!
//! Every refusal after the signature names the consumer, the op and the
//! line or setting that would allow it, and reaches the caller through the
//! door's one error edge (`AccessChecker::peer_refused`).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use ordo_exec::Pool;
use ordo_identity::peer_token::{self, Expect, PeerClaims, PeerKind, PeerTokenError};
use ordo_identity::{AuthenticatorId, PrincipalId, Timestamp};
use ordo_secevent::{RecordedDecision, Subject, TokenFacts};
use sha2::{Digest, Sha256};

use crate::consumer_grants::{self, Expanded};
use crate::identity_keys::{IdentityKeys, DEFAULT_DEPLOYMENT, DEPLOYMENT_ENV, ISSUER_ENV};

/// What the security record names as the op for a refusal the token gate
/// decided, before any op is known.
pub const PEER_GATE_OP: &str = "peer-gate";
/// The most tokens the door remembers at once. A token lives
/// `ORDO_IDENTITY_PEER_TOKEN_SECS` (300 by default).
pub const MAX_REMEMBERED: usize = 4096;
/// How a setting is read: the process environment, or a test's map.
pub type Settings = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// How far a holder's clock may sit behind the service's, for `iat`.
pub const LEEWAY_SECS: u64 = 30;

/// Where this deployment stands with the identity service, for peers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerSettings {
    /// This deployment's handle at the service (`ORDO_IDENTITY_DEPLOYMENT`).
    pub handle: String,
    /// `ordo-peer:<handle>`.
    pub audience: String,
    /// The issuer of [`Self::connection`], without a trailing `/`.
    pub issuer: String,
    /// The sign-in connection to Ordo's identity service: people are
    /// `oidc:<connection>:<sub>`, as a direct sign-in makes them.
    pub connection: String,
}

impl PeerSettings {
    /// Read the settings. `Err` says why the door takes no calls from
    /// other deployments, naming the settings that fix it.
    pub fn from_vars(var: &dyn Fn(&str) -> Option<String>) -> Result<PeerSettings, String> {
        let methods = var("ORDO_AUTH_METHODS").unwrap_or_default();
        let found: Vec<String> = methods
            .split([',', ' '])
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .filter(|m| consumer_grants::provider_is_identity_service(m, var).is_ok())
            .map(str::to_string)
            .collect();
        let connection = match found.as_slice() {
            [one] => one.clone(),
            [] => {
                return Err(format!(
                    "this deployment takes no calls from other deployments: no sign-in \
                     connection in ORDO_AUTH_METHODS is Ordo's identity service. Fix: add an \
                     OIDC connection <name> with ORDO_AUTH_<NAME>_KIND=oidc and \
                     ORDO_AUTH_<NAME>_ISSUER set to {ISSUER_ENV}"
                ))
            }
            many => {
                return Err(format!(
                    "this deployment takes no calls from other deployments: {} sign-in \
                     connections are Ordo's identity service ({}), so a person's name here is \
                     not one. Fix: keep one",
                    many.len(),
                    many.join(", ")
                ))
            }
        };
        let issuer = var(&format!("{}ISSUER", crate::federation::prefix(&connection)))
            .unwrap_or_default()
            .trim()
            .trim_end_matches('/')
            .to_string();
        let handle = var(DEPLOYMENT_ENV)
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| DEFAULT_DEPLOYMENT.to_string());
        Ok(PeerSettings {
            audience: peer_token::audience(&handle),
            handle,
            issuer,
            connection,
        })
    }
}

/// What the token gate learned about one token.
#[derive(Debug, Clone)]
enum Admission {
    /// Verified, no loop, and (on behalf) the person resolved.
    Caller {
        claims: PeerClaims,
        /// The person's principal here, on behalf of a person.
        person: Option<PrincipalId>,
    },
    /// Refused before any op. `why` is `None` for a signature that failed:
    /// the uniform denial.
    Refused {
        decision: RecordedDecision,
        subject: Subject,
        why: Option<String>,
    },
}

/// The door's decision on one op call with a peer token.
#[derive(Debug, Clone)]
pub enum PeerDecision {
    /// The token gate never saw this token, or forgot it: the uniform
    /// denial.
    Unknown,
    /// Refused. `why` is `None` for the uniform denial.
    Refused {
        decision: RecordedDecision,
        subject: Subject,
        why: Option<String>,
    },
    /// On behalf of `person`; the access engine decides next.
    OnBehalf {
        person: PrincipalId,
        consumer: String,
        subject: Subject,
    },
    /// The consumer as itself, in the deployment's own space.
    AsItself { consumer: String, subject: Subject },
}

/// Why a person could not be resolved.
enum PersonRefusal {
    Named(String),
    /// The directory or the sign-up count could not be read: retry.
    Busy,
}

/// The holder's door for other deployments. One per deployment, shared by
/// every door the runtime installs, so a token the gate admitted is known
/// to the door that checks it.
pub struct PeerDoor {
    settings: Result<PeerSettings, String>,
    keys: Arc<IdentityKeys>,
    auth: Option<Arc<crate::auth::AuthState>>,
    pool: Option<Pool>,
    var: Settings,
    now: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// The version serving when last read, and its expanded grants.
    grants: RwLock<(Option<String>, Arc<Vec<Expanded>>)>,
    /// Admissions by the token's SHA-256, with the token's `exp`.
    admitted: Mutex<HashMap<[u8; 32], (u64, Admission)>>,
}

impl PeerDoor {
    /// A door over the identity service's `keys`, resolving people through
    /// `auth`'s directory and reading grants from `pool`.
    pub fn new(
        keys: Arc<IdentityKeys>,
        auth: Option<Arc<crate::auth::AuthState>>,
        pool: Option<Pool>,
        var: Settings,
        now: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> PeerDoor {
        PeerDoor {
            settings: PeerSettings::from_vars(&*var),
            keys,
            auth,
            pool,
            var,
            now,
            grants: RwLock::new((None, Arc::new(Vec::new()))),
            admitted: Mutex::new(HashMap::new()),
        }
    }

    /// The settings, or why the door takes no peer calls.
    pub fn settings(&self) -> &Result<PeerSettings, String> {
        &self.settings
    }

    /// Hold `grants` as the serving version's, for a door with no pool.
    pub fn hold_grants(&self, version: &str, grants: Vec<Expanded>) {
        *self.grants.write().expect("peer grants") = (Some(version.to_string()), Arc::new(grants));
    }

    /// Read the serving version and, when it changed, its grants. A failed
    /// read keeps what is held and says so.
    async fn refresh_grants(&self) -> Result<(), String> {
        let Some(pool) = &self.pool else {
            return Ok(());
        };
        let active = crate::versions::Versions::on(pool)
            .active_version()
            .await
            .map_err(|e| e.to_string())?;
        if self.grants.read().expect("peer grants").0 == active {
            return Ok(());
        }
        let grants = match &active {
            Some(v) => consumer_grants::stored(pool, v)
                .await
                .map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        *self.grants.write().expect("peer grants") = (active, Arc::new(grants));
        Ok(())
    }

    fn key(token: &str) -> [u8; 32] {
        Sha256::digest(token.as_bytes()).into()
    }

    fn remembered(&self, token: &str, now: u64) -> Option<Admission> {
        let held = self.admitted.lock().expect("peer admissions");
        held.get(&Self::key(token))
            .filter(|(exp, _)| now < *exp)
            .map(|(_, a)| a.clone())
    }

    fn remember(&self, token: &str, exp: u64, a: Admission) {
        let now = (self.now)();
        let mut held = self.admitted.lock().expect("peer admissions");
        if held.len() >= MAX_REMEMBERED {
            held.retain(|_, (e, _)| now < *e);
            if held.len() >= MAX_REMEMBERED {
                held.clear();
            }
        }
        held.insert(Self::key(token), (exp, a));
    }

    /// The token gate's half: verify, check the chain, resolve the person,
    /// and remember the answer for the token's life. `Busy` when the
    /// grants, the directory or the sign-up count could not be read.
    pub async fn admit(&self, token: &str) -> ordo_api::TokenGate {
        let now = (self.now)();
        if let Err(e) = self.refresh_grants().await {
            eprintln!("peer door: the serving grants could not be read: {e}");
            return ordo_api::TokenGate::Busy;
        }
        if self.remembered(token, now).is_some() {
            return ordo_api::TokenGate::Pass;
        }
        self.keys.prepare(token).await;
        let settings = match &self.settings {
            Ok(s) => s,
            Err(why) => {
                // Named only to a caller the service did sign for.
                let why = self.keys.signature_holds(token).then(|| why.clone());
                let a = Admission::Refused {
                    decision: RecordedDecision::PeerTokenRefused,
                    subject: Subject::Unverified,
                    why,
                };
                self.remember(token, now + 60, a);
                return ordo_api::TokenGate::Pass;
            }
        };
        let expect = Expect {
            issuer: &settings.issuer,
            audience: &settings.audience,
            now,
            leeway_secs: LEEWAY_SECS,
        };
        let claims = match self.keys.verify_peer(token, &expect) {
            Ok(c) => c,
            Err(e) => {
                let why = token_refusal(&e, settings);
                let a = Admission::Refused {
                    decision: RecordedDecision::PeerTokenRefused,
                    subject: Subject::Unverified,
                    why,
                };
                // Kept briefly: a token refused now is refused for good,
                // except one whose key the door had not fetched yet.
                self.remember(token, now + 60, a);
                return ordo_api::TokenGate::Pass;
            }
        };
        let subject = subject_of(&claims, None);
        if claims.chain.contains(&settings.handle) {
            let shown = claims.chain.join(" → ");
            let a = Admission::Refused {
                decision: RecordedDecision::PeerLoop,
                subject,
                why: Some(format!(
                    "consumer `{}`: this call already passed through `{}` (chain {shown} → \
                     {}), and a chain that comes back to a deployment is refused. Fix: break \
                     the cycle of grants, so no deployment calls back into one the call came \
                     through",
                    claims.consumer, settings.handle, settings.handle
                )),
            };
            self.remember(token, claims.exp, a);
            return ordo_api::TokenGate::Pass;
        }
        let person = match claims.kind {
            PeerKind::AsItself => None,
            // A consumer no grant covers is refused by `decide`, naming the
            // setting; nobody is signed up for it.
            PeerKind::OnBehalf if self.grants_of(&claims.consumer, true).is_empty() => None,
            PeerKind::OnBehalf => match self.person(settings, &claims.sub, now).await {
                Ok(p) => Some(p),
                Err(PersonRefusal::Busy) => return ordo_api::TokenGate::Busy,
                Err(PersonRefusal::Named(why)) => {
                    let a = Admission::Refused {
                        decision: RecordedDecision::ConsumerNotGranted,
                        subject,
                        why: Some(format!("consumer `{}`: {why}", claims.consumer)),
                    };
                    self.remember(token, claims.exp, a);
                    return ordo_api::TokenGate::Pass;
                }
            },
        };
        let exp = claims.exp;
        self.remember(token, exp, Admission::Caller { claims, person });
        ordo_api::TokenGate::Pass
    }

    /// The person `sub` names, resolved as a direct sign-in through the
    /// connection resolves them.
    async fn person(
        &self,
        settings: &PeerSettings,
        sub: &str,
        now: u64,
    ) -> Result<PrincipalId, PersonRefusal> {
        let Some(auth) = &self.auth else {
            return Err(PersonRefusal::Named(
                "this deployment has no sign-in (ORDO_AUTH_METHODS), so it cannot tell who \
                 the person is. Fix: sign people in here through the identity service"
                    .into(),
            ));
        };
        let authenticator = AuthenticatorId(format!("oidc:{}:{sub}", settings.connection));
        let known = auth.lock_directory().principal_for(&authenticator);
        match known {
            Ok(Some(_)) => {}
            Ok(None) => {
                // A first call on behalf of a person is their sign-up here,
                // counted as one (`plan-auth-federated.md` 2.6).
                if let Some(signup) = auth.signup() {
                    match signup
                        .admit(crate::self_serve::Counted::Registration, now)
                        .await
                    {
                        Ok(()) => {}
                        Err(crate::self_serve::Refusal::DeploymentFull) => {
                            return Err(PersonRefusal::Named(
                                "the person is new here, and this deployment's sign-ups for \
                                 the hour are used. Fix: try again later, or raise its \
                                 sign-up ceiling"
                                    .into(),
                            ))
                        }
                        Err(crate::self_serve::Refusal::Unavailable(_)) => {
                            return Err(PersonRefusal::Busy)
                        }
                    }
                }
            }
            Err(_) => return Err(PersonRefusal::Busy),
        }
        let subject = ordo_identity::AuthenticatedSubject {
            authenticator,
            method: ordo_identity::AuthMethodKind::Oidc,
            attributes: Default::default(),
            presence: None,
            upstream_presence: None,
        };
        let mut dir = auth.lock_directory();
        match ordo_identity::sign_in_as(
            &mut dir,
            &subject,
            sub,
            Timestamp(now),
            ordo_identity::Onboarding::ByPolicy,
        ) {
            Ok(s) => Ok(s.principal),
            Err(ordo_identity::DirectoryError::NotInvited(_)) => Err(PersonRefusal::Named(
                "the person is not a member of this deployment, and its sign-up is invite-only \
                 (ORDO_AUTH_PROVISIONING). Fix: invite them here, or open sign-up"
                    .into(),
            )),
            Err(ordo_identity::DirectoryError::Deleted(_)) => Err(PersonRefusal::Named(
                "the person's account in this deployment was deleted".into(),
            )),
            Err(ordo_identity::DirectoryError::AuthenticatorRetired { .. }) => {
                Err(PersonRefusal::Named(
                    "a recovery retired the person's sign-in through the identity service \
                     here. Fix: the person signs in here directly"
                        .into(),
                ))
            }
            Err(ordo_identity::DirectoryError::StoreUnavailable(_)) => Err(PersonRefusal::Busy),
            Err(_) => Err(PersonRefusal::Named(
                "the person could not be signed in here".into(),
            )),
        }
    }

    /// The grants that cover `consumer`: each `grant <name>` whose
    /// `ORDO_CONSUMER_<NAME>_CLIENT` is its id, and, on behalf of a person,
    /// `grant provider <connection>`.
    fn grants_of(&self, consumer: &str, on_behalf: bool) -> Vec<Expanded> {
        let held = self.grants.read().expect("peer grants").1.clone();
        let connection = self.settings.as_ref().ok().map(|s| s.connection.as_str());
        held.iter()
            .filter(|g| {
                if g.provider {
                    on_behalf && Some(g.name.as_str()) == connection
                } else {
                    (self.var)(&consumer_grants::client_env(&g.name))
                        .is_some_and(|id| id.trim() == consumer)
                }
            })
            .cloned()
            .collect()
    }

    /// The decision on `op` in `space` for a token the gate admitted.
    /// `home` is the deployment's own space, the only one a call as itself
    /// reaches.
    pub fn decide(&self, token: &str, space: &str, op: &str, home: Option<&str>) -> PeerDecision {
        let now = (self.now)();
        let (claims, person) = match self.remembered(token, now) {
            None => return PeerDecision::Unknown,
            Some(Admission::Refused {
                decision,
                subject,
                why,
            }) => {
                return PeerDecision::Refused {
                    decision,
                    subject,
                    why,
                }
            }
            Some(Admission::Caller { claims, person }) => (claims, person),
        };
        // A remembered token past its `exp` is not remembered: checked by
        // `remembered`, so expiry holds on every call, not only the first.
        let subject = subject_of(&claims, person.as_ref());
        let consumer = claims.consumer.clone();
        let refuse = |why: String| PeerDecision::Refused {
            decision: RecordedDecision::ConsumerNotGranted,
            subject: subject.clone(),
            why: Some(format!("consumer `{consumer}`, op `{op}`: {why}")),
        };
        let on_behalf = claims.kind == PeerKind::OnBehalf;
        let grants = self.grants_of(&consumer, on_behalf);
        if grants.is_empty() {
            let names = self.grant_names();
            return refuse(format!(
                "`{consumer}` is not a consumer of this deployment: no grant is tied to it{}. \
                 Fix: set ORDO_CONSUMER_<NAME>_CLIENT={consumer} for the grant it should use{}",
                if on_behalf {
                    ""
                } else {
                    ", and a `grant provider` covers calls on behalf of a person only"
                },
                if names.is_empty() {
                    String::from(", after adding a `grant <name> { … }` block")
                } else {
                    format!(" ({})", names.join(", "))
                }
            ));
        }
        match claims.kind {
            PeerKind::OnBehalf => {
                if !grants.iter().any(|g| g.ops.iter().any(|o| o == op)) {
                    let g = &grants[0];
                    return refuse(format!(
                        "`{}` does not grant it. Fix: add the line `{op}` to `{}` in this \
                         deployment's file, and apply",
                        g.header(),
                        g.header()
                    ));
                }
                let Some(person) = person else {
                    return PeerDecision::Unknown;
                };
                PeerDecision::OnBehalf {
                    person,
                    consumer,
                    subject,
                }
            }
            PeerKind::AsItself => {
                if !grants
                    .iter()
                    .any(|g| g.as_itself && g.ops.iter().any(|o| o == op))
                {
                    return refuse(unreachable_as_itself(&grants, op));
                }
                if home != Some(space) {
                    return refuse(format!(
                        "a call as itself reaches only this deployment's own space{}, and this \
                         one named `{space}`. Fix: call on behalf of a person for a tenant's \
                         data",
                        home.map(|h| format!(" `{h}`")).unwrap_or_default()
                    ));
                }
                PeerDecision::AsItself { consumer, subject }
            }
        }
    }

    /// The record's subject for a peer token, when the gate saw it.
    pub fn subject_of_token(&self, token: &str) -> Option<Subject> {
        match self.remembered(token, (self.now)())? {
            Admission::Caller { claims, person } => Some(subject_of(&claims, person.as_ref())),
            Admission::Refused { subject, .. } => Some(subject),
        }
    }

    fn grant_names(&self) -> Vec<String> {
        let held = self.grants.read().expect("peer grants").1.clone();
        let names: BTreeSet<String> = held
            .iter()
            .filter(|g| !g.provider)
            .map(|g| consumer_grants::client_env(&g.name))
            .collect();
        names.into_iter().collect()
    }
}

/// Why a consumer's call as itself is refused: no grant it holds is marked
/// `as itself` and opens `op`.
fn unreachable_as_itself(grants: &[Expanded], op: &str) -> String {
    match grants.iter().find(|g| g.ops.iter().any(|o| o == op)) {
        Some(g) => format!(
            "it called as itself, and `{}` allows calls on behalf of a person only. Fix: write \
             `grant {} as itself {{ … }}` in this deployment's file, and apply",
            g.header(),
            g.name
        ),
        None => format!(
            "no grant marked `as itself` opens it to the consumer. Fix: add the line `{op}` to \
             a grant marked `as itself` (`grant <name> as itself {{ … }}`), and apply"
        ),
    }
}

/// The named refusal for a token that failed after its signature held, or
/// `None` for the uniform denial.
fn token_refusal(e: &PeerTokenError, s: &PeerSettings) -> Option<String> {
    let p = crate::federation::prefix(&s.connection);
    match e {
        PeerTokenError::Unverifiable(_) => None,
        PeerTokenError::Malformed(why) => Some(format!(
            "the token's claims do not fit its type ({why}). Fix: ask the identity service \
             for a new one"
        )),
        PeerTokenError::WrongIssuer(iss) => Some(format!(
            "the token was issued by `{iss}`, and this deployment trusts `{}` ({p}ISSUER). \
             Fix: ask the identity service this deployment signs in through",
            s.issuer
        )),
        PeerTokenError::WrongAudience(aud) => Some(format!(
            "the token is for `{aud}`, and this deployment is `{}` ({DEPLOYMENT_ENV}). Fix: \
             ask the identity service for a token with audience `{}`",
            s.audience, s.handle
        )),
        PeerTokenError::Expired(exp) => Some(format!(
            "the token expired at {exp}. Fix: exchange a new one"
        )),
        PeerTokenError::NotYetValid => Some(
            "the token was issued in the future by this deployment's clock. Fix: check the \
             clocks of this deployment and the identity service"
                .into(),
        ),
    }
}

/// The record's subject: the person (their principal here when known) or
/// the consumer, with the consumer and the token's id beside it.
fn subject_of(claims: &PeerClaims, person: Option<&PrincipalId>) -> Subject {
    let (who, kind) = match claims.kind {
        PeerKind::OnBehalf => (
            person
                .map(|p| p.0.clone())
                .unwrap_or_else(|| claims.sub.clone()),
            "peer-person",
        ),
        PeerKind::AsItself => (format!("consumer:{}", claims.consumer), "peer-client"),
    };
    Subject::deployment(
        &who,
        TokenFacts {
            kind: kind.to_string(),
            client: claims.consumer.clone(),
            sid: claims.jti.clone(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn the_settings_find_the_one_connection_to_the_identity_service() {
        let s = PeerSettings::from_vars(&env(&[
            ("ORDO_AUTH_METHODS", "passkey, ids"),
            ("ORDO_AUTH_IDS_KIND", "oidc"),
            ("ORDO_AUTH_IDS_ISSUER", "https://ids.example/"),
            ("ORDO_IDENTITY_ORIGIN", "https://ids.example"),
            ("ORDO_IDENTITY_DEPLOYMENT", "notes"),
        ]))
        .unwrap();
        assert_eq!(s.connection, "ids");
        assert_eq!(s.issuer, "https://ids.example");
        assert_eq!(s.audience, "ordo-peer:notes");

        let none = PeerSettings::from_vars(&env(&[("ORDO_AUTH_METHODS", "passkey")]));
        assert!(none.unwrap_err().contains("ORDO_AUTH_<NAME>_KIND=oidc"));
    }
}
