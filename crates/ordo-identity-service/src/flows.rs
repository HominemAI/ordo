//! What each call does (`plan-identity-service.md` 5): sign-up by invite,
//! sign-in, recovery, touch, passkeys, CLI grants, machine sign-in, the
//! admin acts, and break-glass. The HTTP shape is in [`crate::routes`];
//! these are methods on [`Service`] so a test or a host command calls them
//! directly.
//!
//! Who may do what:
//!
//! - A **browser session** (the cookie) signs a person in at this service:
//!   touch, enrol a passkey, renew recovery codes, approve a CLI.
//! - A **bearer token** this service issued ([`Caller`]) does the admin
//!   acts: invites, people, machine keys. A write needs a fresh touch from
//!   a person (300 seconds, the door's step-up window); a machine key and
//!   break-glass satisfy it (Q2). The token's session must still be live,
//!   so ending access is at once here even though the door waits for
//!   expiry (Q3).

use ordo_identity::deployment_token::{self, Expect, Kind, DEFAULT_LEEWAY_SECS};
use ordo_identity::jose::Jwks;
use ordo_identity::passkey::{creation_json, request_json, StoredPasskey};
use ordo_identity::{AuthProvider, ChallengeId, CompleteRequest, PrincipalId, Timestamp};
use serde_json::Value;

use crate::audit::{self, Entry};
use crate::cli::{self, Redeemed};
use crate::error::IdentityError;
use crate::invites::{self, Minted, FIRST_BOOT};
use crate::machine;
use crate::passkeys::{self, RP_NAME};
use crate::people::{self, Actor};
use crate::recovery::{self, blocking};
use crate::secrets::mint_id;
use crate::server::Service;
use crate::sessions::{self, Live, NewSession, Opened, SessionKind};
use crate::throttle::{Source, Try};
use crate::token::{self, Issued, Issuer, Subject};

/// A write needs a passkey touch this recent: the door's step-up window.
pub const TOUCH_WINDOW_SECS: u64 = ordo_identity::DEFAULT_STEP_UP_MAX_AGE_SECS;
/// A break-glass token lives at most an hour.
pub const MAX_BREAK_GLASS_MINUTES: u64 = 60;

/// A passkey registration as the browser sends it back.
#[derive(Debug, Clone)]
pub struct Registration {
    pub challenge: String,
    pub client_data_json: Vec<u8>,
    pub attestation_object: Vec<u8>,
}

/// Who called an admin route, from the bearer token.
#[derive(Debug, Clone)]
pub struct Caller {
    pub sub: String,
    pub kind: Kind,
    pub client: String,
    pub sid: String,
    pub touched_at: Option<u64>,
}

impl Caller {
    /// The audit row's actor and client.
    pub fn actor(&self) -> Actor<'_> {
        Actor {
            id: &self.sub,
            client: &self.client,
        }
    }

    /// A write: a person needs a fresh touch; a machine key and
    /// break-glass do not.
    pub fn may_write(&self, now: u64) -> Result<(), IdentityError> {
        match self.kind {
            Kind::Machine | Kind::Breakglass => Ok(()),
            Kind::Person => match self.touched_at {
                Some(t) if now.saturating_sub(t) <= TOUCH_WINDOW_SECS => Ok(()),
                _ => Err(IdentityError::TouchNeeded {
                    window_secs: TOUCH_WINDOW_SECS,
                }),
            },
        }
    }
}

/// What the account page shows ([`Service::account`]).
#[derive(Debug, Clone)]
pub struct Account {
    pub person: people::Person,
    pub passkeys: Vec<passkeys::Listed>,
    pub recovery_codes_left: u64,
    /// A recovery code opened this session: enrol a passkey first.
    pub must_enrol: bool,
    /// The last touch is within [`TOUCH_WINDOW_SECS`], so a change needs
    /// no new one.
    pub touch_fresh: bool,
    pub session_ends_at: u64,
}

/// What sign-up returns: shown once.
#[derive(Debug, Clone)]
pub struct Joined {
    pub person: String,
    pub recovery_codes: Vec<String>,
    pub session: Opened,
}

impl Service {
    pub(crate) fn issuer(&self) -> (String, String, u64) {
        (
            self.0.cfg.origin.clone(),
            self.0.cfg.audience(),
            self.0.cfg.token_secs,
        )
    }

    fn sign(&self, who: Subject<'_>, life_secs: u64) -> Result<Issued, IdentityError> {
        let (origin, audience, token_secs) = self.issuer();
        token::issue(
            Issuer {
                origin: &origin,
                audience: &audience,
                token_secs,
            },
            &self.0.keys,
            who,
            self.now(),
            life_secs,
        )
    }

    fn session_token(&self, live: &Live) -> Result<Issued, IdentityError> {
        let (origin, audience, token_secs) = self.issuer();
        token::for_session(
            Issuer {
                origin: &origin,
                audience: &audience,
                token_secs,
            },
            &self.0.keys,
            live,
            self.now(),
        )
    }

    /// Count one try of a guessable secret, shared by every instance.
    async fn guess(&self, kind: Try, from: &Source, now: u64) -> Result<(), IdentityError> {
        let limits = &self.0.cfg.limits;
        crate::throttle::attempt(self.0.store.pool(), self.handle(), kind, from, limits, now).await
    }

    fn check_audience(&self, aud: &str) -> Result<(), IdentityError> {
        let serves = self.0.cfg.audience();
        if aud != serves {
            return Err(IdentityError::WrongAudience {
                asked: aud.to_string(),
                serves,
            });
        }
        Ok(())
    }

    /// The live session `secret` opens, marked used.
    pub async fn browser(&self, secret: &str) -> Result<Live, IdentityError> {
        sessions::use_session(
            self.0.store.pool(),
            self.handle(),
            &self.0.keys,
            self.lifetimes(),
            secret,
            self.now(),
        )
        .await
    }

    /// A token for the session `secret` opens, for audience `aud`. A
    /// session a recovery code opened gets none until it enrols a passkey.
    pub async fn token(&self, secret: &str, aud: &str) -> Result<Issued, IdentityError> {
        self.check_audience(aud)?;
        let live = self.browser(secret).await?;
        if live.must_enrol {
            return Err(IdentityError::MustEnrolPasskey);
        }
        self.session_token(&live)
    }

    /// Sign out the session `secret` opens.
    pub async fn sign_out(&self, secret: &str) -> Result<(), IdentityError> {
        sessions::sign_out(
            self.0.store.pool(),
            self.handle(),
            &self.0.keys,
            secret,
            self.now(),
        )
        .await
    }

    /// The caller a bearer token names: one this service issued for this
    /// deployment, unexpired, from a session still live.
    pub async fn caller(&self, bearer: &str) -> Result<Caller, IdentityError> {
        let jwks = Jwks::parse(self.0.keys.jwks().to_string().as_bytes())
            .map_err(|e| IdentityError::Internal(format!("the service's own JWKS: {}", e.0)))?;
        let audience = self.0.cfg.audience();
        let claims = deployment_token::verify(
            bearer,
            &jwks,
            &Expect {
                issuer: &self.0.cfg.origin,
                audience: &audience,
                now: self.now(),
                leeway_secs: DEFAULT_LEEWAY_SECS,
            },
        )
        .map_err(IdentityError::TokenRefused)?;
        if claims.kind != Kind::Breakglass
            && !sessions::is_live(self.0.store.pool(), self.handle(), &claims.sid).await?
        {
            return Err(IdentityError::SessionEnded(crate::SessionEnd::Ended));
        }
        Ok(Caller {
            sub: claims.sub,
            kind: claims.kind,
            client: claims.client,
            sid: claims.sid,
            touched_at: claims.touched_at,
        })
    }

    // ───────────────────────────── sign-up ─────────────────────────────

    /// Begin sign-up with an invite secret: the passkey creation options.
    pub async fn join_begin(&self, typed: &str, from: &Source) -> Result<Value, IdentityError> {
        let now = self.now();
        self.guess(Try::Invite, from, now).await?;
        let live =
            invites::check(self.0.store.pool(), self.handle(), &self.0.keys, typed, now).await?;
        let provider = self.0.provider.clone();
        let binding = format!("invite:{}", live.hash);
        let opts = blocking(move || {
            provider.begin_registration_bound(Timestamp(now), &[], Some(&binding))
        })
        .await??;
        Ok(creation_json(&opts, RP_NAME, "Deployment person"))
    }

    /// Finish sign-up: the person, their passkey, ten recovery codes, the
    /// invite spent, and a browser session. One transaction.
    pub async fn join_finish(
        &self,
        typed: &str,
        display_name: &str,
        reg: Registration,
        from: &Source,
    ) -> Result<Joined, IdentityError> {
        let now = self.now();
        self.guess(Try::Invite, from, now).await?;
        let pool = self.0.store.pool();
        let live = invites::check(pool, self.handle(), &self.0.keys, typed, now).await?;
        let provider = self.0.provider.clone();
        let pending = blocking(move || {
            provider.verify_registration(
                &ChallengeId(reg.challenge),
                &reg.client_data_json,
                &reg.attestation_object,
                Timestamp(now),
            )
        })
        .await??;
        if pending.binding() != Some(format!("invite:{}", live.hash).as_str()) {
            return Err(IdentityError::CodeRefused);
        }
        let (codes, rows) = recovery::mint(&self.0.hasher).await?;
        let tx = pool.begin().await?;
        let by = Actor {
            id: &live.id,
            client: "",
        };
        let person = people::add(&tx, self.handle(), &self.0.keys, display_name, by, now).await?;
        if !invites::spend(&tx, self.handle(), &live.id, &person.id, now).await? {
            return Err(IdentityError::CodeRefused);
        }
        passkeys::insert(&tx, self.handle(), &person.id, pending.stored(), now).await?;
        recovery::replace(&tx, self.handle(), &person.id, &rows, now).await?;
        audit::append(
            &tx,
            self.handle(),
            &self.0.keys,
            &Entry {
                actor: &person.id,
                action: "invite.used",
                subject: &live.id,
                outcome: "ok",
                client: "",
                detail: &live.label,
            },
            now,
        )
        .await?;
        let session = sessions::open(
            &tx,
            self.handle(),
            &self.0.keys,
            self.lifetimes(),
            NewSession {
                subject: &person.id,
                kind: SessionKind::Browser,
                client: "console",
                touched_at: Some(now),
                must_enrol: false,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        if live.issued_by == FIRST_BOOT {
            let _ = std::fs::remove_file(self.first_invite_path());
        }
        Ok(Joined {
            person: person.id,
            recovery_codes: codes,
            session,
        })
    }

    // ───────────────────────────── sign-in ─────────────────────────────

    /// Begin a passkey sign-in: any enrolled credential.
    pub async fn sign_in_begin(&self) -> Result<Value, IdentityError> {
        let provider = self.0.provider.clone();
        let now = self.now();
        let opts = blocking(move || provider.begin_assertion(Timestamp(now), &[])).await??;
        Ok(request_json(&opts))
    }

    /// Finish a passkey sign-in: a browser session, touched now.
    pub async fn sign_in_finish(
        &self,
        assertion: CompleteRequest,
        client: &str,
    ) -> Result<(String, Opened), IdentityError> {
        let provider = self.0.provider.clone();
        let now = self.now();
        let subject = blocking(move || provider.complete(&assertion, Timestamp(now))).await??;
        let pool = self.0.store.pool();
        let person = passkeys::person_of(pool, self.handle(), &subject.authenticator.0)
            .await?
            .ok_or(IdentityError::Passkey(
                ordo_identity::AuthError::BadCredentials,
            ))?;
        let tx = pool.begin().await?;
        let session = sessions::open(
            &tx,
            self.handle(),
            &self.0.keys,
            self.lifetimes(),
            NewSession {
                subject: &person,
                kind: SessionKind::Browser,
                client,
                touched_at: Some(now),
                must_enrol: false,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok((person, session))
    }

    /// Sign a browser in with a recovery code.
    pub async fn recover(
        &self,
        typed: &str,
        from: &Source,
    ) -> Result<(String, Opened), IdentityError> {
        let now = self.now();
        self.guess(Try::RecoveryCode, from, now).await?;
        recovery::redeem(
            self.0.store.pool(),
            self.handle(),
            &self.0.keys,
            &self.0.hasher,
            self.lifetimes(),
            typed,
            now,
        )
        .await
    }

    // ─────────────────────────────── touch ──────────────────────────────

    /// Begin a touch inside a session: the person's own credentials.
    pub async fn touch_begin(&self, live: &Live) -> Result<Value, IdentityError> {
        if live.must_enrol {
            return Err(IdentityError::MustEnrolPasskey);
        }
        let allow =
            passkeys::credentials_of(self.0.store.pool(), self.handle(), &live.subject).await?;
        let provider = self.0.touch_provider.clone();
        let now = self.now();
        let principal = PrincipalId(live.subject.clone());
        let opts =
            blocking(move || provider.begin_step_up(Timestamp(now), &principal, &allow)).await??;
        Ok(request_json(&opts))
    }

    /// Finish a touch: the session's `touched_at` is now, and the answer is
    /// a token that carries it.
    pub async fn touch_finish(
        &self,
        live: &Live,
        assertion: CompleteRequest,
    ) -> Result<Issued, IdentityError> {
        let CompleteRequest::PasskeyAssertion {
            challenge,
            credential_id,
            client_data_json,
            authenticator_data,
            signature,
        } = assertion
        else {
            return Err(IdentityError::BadRequest("not an assertion".into()));
        };
        let provider = self.0.touch_provider.clone();
        let now = self.now();
        let (subject, principal) = blocking(move || {
            provider.complete_step_up(
                &challenge,
                &credential_id,
                &client_data_json,
                &authenticator_data,
                &signature,
                Timestamp(now),
            )
        })
        .await??;
        let pool = self.0.store.pool();
        let holder = passkeys::person_of(pool, self.handle(), &subject.authenticator.0).await?;
        if principal.0 != live.subject || holder.as_deref() != Some(live.subject.as_str()) {
            return Err(IdentityError::Passkey(
                ordo_identity::AuthError::BadCredentials,
            ));
        }
        let tx = pool.begin().await?;
        sessions::touched(&tx, self.handle(), &live.id, now).await?;
        audit::append(
            &tx,
            self.handle(),
            &self.0.keys,
            &Entry {
                actor: &live.subject,
                action: "touch",
                subject: &live.id,
                outcome: "ok",
                client: &live.client,
                detail: "",
            },
            now,
        )
        .await?;
        tx.commit().await?;
        let fresh = Live {
            touched_at: Some(now),
            ..live.clone()
        };
        self.session_token(&fresh)
    }

    // ───────────────────────────── passkeys ─────────────────────────────

    fn may_change_credentials(&self, live: &Live) -> Result<(), IdentityError> {
        if live.must_enrol || live.fresh_touch(self.now(), TOUCH_WINDOW_SECS) {
            Ok(())
        } else {
            Err(IdentityError::TouchNeeded {
                window_secs: TOUCH_WINDOW_SECS,
            })
        }
    }

    /// Begin enrolling another passkey: after a recovery code, or with a
    /// fresh touch.
    pub async fn passkey_begin(&self, live: &Live) -> Result<Value, IdentityError> {
        self.may_change_credentials(live)?;
        let exclude =
            passkeys::credentials_of(self.0.store.pool(), self.handle(), &live.subject).await?;
        let provider = self.0.provider.clone();
        let now = self.now();
        let binding = format!("session:{}", live.id);
        let opts = blocking(move || {
            provider.begin_registration_bound(Timestamp(now), &exclude, Some(&binding))
        })
        .await??;
        Ok(creation_json(&opts, RP_NAME, "Deployment person"))
    }

    /// Finish enrolling. After a recovery code, the person's other passkeys
    /// go, a new recovery set is issued (returned, shown once), and the
    /// session is a whole one again.
    pub async fn passkey_finish(
        &self,
        live: &Live,
        reg: Registration,
    ) -> Result<Option<Vec<String>>, IdentityError> {
        self.may_change_credentials(live)?;
        let provider = self.0.provider.clone();
        let now = self.now();
        let pending = blocking(move || {
            provider.verify_registration(
                &ChallengeId(reg.challenge),
                &reg.client_data_json,
                &reg.attestation_object,
                Timestamp(now),
            )
        })
        .await??;
        if pending.binding() != Some(format!("session:{}", live.id).as_str()) {
            return Err(IdentityError::BadRequest(
                "that passkey ceremony was begun by another session".into(),
            ));
        }
        let stored: StoredPasskey = pending.stored().clone();
        let codes = if live.must_enrol {
            Some(recovery::mint(&self.0.hasher).await?)
        } else {
            None
        };
        let pool = self.0.store.pool();
        let tx = pool.begin().await?;
        passkeys::insert(&tx, self.handle(), &live.subject, &stored, now).await?;
        let mut detail = String::new();
        if let Some((_, rows)) = &codes {
            let retired =
                passkeys::retire_others(&tx, self.handle(), &live.subject, &stored).await?;
            recovery::replace(&tx, self.handle(), &live.subject, rows, now).await?;
            sessions::enrolled(&tx, self.handle(), &live.id, now).await?;
            detail = format!("after a recovery code; retired={retired}; new recovery set");
        }
        audit::append(
            &tx,
            self.handle(),
            &self.0.keys,
            &Entry {
                actor: &live.subject,
                action: "passkey.added",
                subject: &live.subject,
                outcome: "ok",
                client: &live.client,
                detail: &detail,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(codes.map(|(shown, _)| shown))
    }

    /// A new set of ten recovery codes, the old ones voided. Needs a fresh
    /// touch.
    pub async fn renew_recovery(&self, live: &Live) -> Result<Vec<String>, IdentityError> {
        if live.must_enrol {
            return Err(IdentityError::MustEnrolPasskey);
        }
        self.may_change_credentials(live)?;
        let (codes, rows) = recovery::mint(&self.0.hasher).await?;
        let now = self.now();
        let tx = self.0.store.pool().begin().await?;
        recovery::replace(&tx, self.handle(), &live.subject, &rows, now).await?;
        audit::append(
            &tx,
            self.handle(),
            &self.0.keys,
            &Entry {
                actor: &live.subject,
                action: "recovery-codes.renewed",
                subject: &live.subject,
                outcome: "ok",
                client: &live.client,
                detail: "",
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(codes)
    }

    /// What the account page shows the person a browser session belongs
    /// to: who they are, their passkeys, how many recovery codes are left,
    /// and whether their last touch is fresh enough for a change.
    pub async fn account(&self, live: &Live) -> Result<Account, IdentityError> {
        let pool = self.0.store.pool();
        let person = people::get(pool, self.handle(), &live.subject)
            .await?
            .ok_or(IdentityError::SessionEnded(crate::SessionEnd::Ended))?;
        let now = self.now();
        Ok(Account {
            person,
            passkeys: passkeys::list(pool, self.handle(), &live.subject).await?,
            recovery_codes_left: recovery::left(pool, self.handle(), &live.subject).await?,
            must_enrol: live.must_enrol,
            touch_fresh: !live.must_enrol && live.fresh_touch(now, TOUCH_WINDOW_SECS),
            session_ends_at: live.ends_at,
        })
    }

    /// Remove one of the signed-in person's passkeys, never their last.
    /// Needs a fresh touch. Recorded.
    pub async fn remove_passkey(&self, live: &Live, credential: &str) -> Result<(), IdentityError> {
        if live.must_enrol {
            return Err(IdentityError::MustEnrolPasskey);
        }
        self.may_change_credentials(live)?;
        let now = self.now();
        let tx = self.0.store.pool().begin().await?;
        passkeys::remove_one(&tx, self.handle(), &live.subject, credential).await?;
        audit::append(
            &tx,
            self.handle(),
            &self.0.keys,
            &Entry {
                actor: &live.subject,
                action: "passkey.removed",
                subject: &live.subject,
                outcome: "ok",
                client: &live.client,
                detail: credential,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    // ─────────────────────────────── CLI ───────────────────────────────

    /// Approve a CLI from a browser session with a fresh touch: a 60 second
    /// code for the loopback redirect.
    pub async fn cli_approve(&self, live: &Live, challenge: &str) -> Result<String, IdentityError> {
        if live.must_enrol {
            return Err(IdentityError::MustEnrolPasskey);
        }
        let now = self.now();
        let touched_at = live
            .touched_at
            .filter(|_| live.fresh_touch(now, TOUCH_WINDOW_SECS))
            .ok_or(IdentityError::TouchNeeded {
                window_secs: TOUCH_WINDOW_SECS,
            })?;
        cli::approve(
            self.0.store.pool(),
            self.handle(),
            &self.0.keys,
            &live.subject,
            challenge,
            touched_at,
            now,
        )
        .await
    }

    /// Redeem a CLI code with its PKCE verifier.
    pub async fn cli_redeem(
        &self,
        code: &str,
        verifier: &str,
        existing: Option<&str>,
        from: &Source,
    ) -> Result<Redeemed, IdentityError> {
        let now = self.now();
        self.guess(Try::CliCode, from, now).await?;
        cli::redeem(
            self.0.store.pool(),
            self.handle(),
            &self.0.keys,
            self.lifetimes(),
            code,
            verifier,
            existing,
            now,
        )
        .await
    }

    // ───────────────────────────── machines ─────────────────────────────

    /// A sign-in challenge for a machine key.
    pub fn machine_begin(&self, id: &str) -> String {
        self.0.machines.begin(id, self.now())
    }

    /// A machine key's signed challenge for a token, `kind: machine`.
    pub async fn machine_complete(
        &self,
        id: &str,
        challenge: &str,
        signature: &str,
        from: &Source,
    ) -> Result<Issued, IdentityError> {
        let now = self.now();
        self.guess(Try::MachineSignIn, from, now).await?;
        let signed = self
            .0
            .machines
            .complete(
                self.0.store.pool(),
                self.handle(),
                &self.0.keys,
                self.lifetimes(),
                id,
                challenge,
                signature,
                now,
            )
            .await?;
        self.sign(
            Subject {
                sub: &signed.id,
                kind: Kind::Machine,
                client: &signed.name,
                sid: &signed.session.id,
                touched_at: None,
            },
            self.0.cfg.token_secs,
        )
    }

    // ─────────────────────────── admin acts ────────────────────────────

    /// Mint an invite link.
    pub async fn invite(&self, by: Actor<'_>, label: &str) -> Result<Minted, IdentityError> {
        let tx = self.0.store.pool().begin().await?;
        let minted = invites::mint(
            &tx,
            self.handle(),
            &self.0.keys,
            // The link opens the page: under the base path when there is one.
            &self.0.cfg.public_url(),
            label,
            by,
            self.0.cfg.invite_days,
            self.now(),
        )
        .await?;
        tx.commit().await?;
        Ok(minted)
    }

    /// Make a machine key; with `public`, register one the machine made.
    pub async fn add_machine_key(
        &self,
        by: Actor<'_>,
        name: &str,
        public: Option<[u8; 32]>,
    ) -> Result<machine::Made, IdentityError> {
        let tx = self.0.store.pool().begin().await?;
        let made = machine::add(
            &tx,
            self.handle(),
            &self.0.keys,
            name,
            public,
            by,
            self.now(),
        )
        .await?;
        tx.commit().await?;
        Ok(made)
    }

    /// A break-glass token for a unix user on the host (5.8): admin, a
    /// fresh touch, `minutes` long (1 to 60). Recorded.
    pub async fn break_glass(
        &self,
        unix_user: &str,
        minutes: u64,
    ) -> Result<Issued, IdentityError> {
        if !(1..=MAX_BREAK_GLASS_MINUTES).contains(&minutes) {
            return Err(IdentityError::BadRequest(format!(
                "--minutes is 1 to {MAX_BREAK_GLASS_MINUTES}"
            )));
        }
        let now = self.now();
        let sub = format!("breakglass:{unix_user}");
        let sid = mint_id("bg_");
        let issued = self.sign(
            Subject {
                sub: &sub,
                kind: Kind::Breakglass,
                client: "host",
                sid: &sid,
                touched_at: Some(now),
            },
            minutes * 60,
        )?;
        audit::record(
            self.0.store.pool(),
            self.handle(),
            &self.0.keys,
            &Entry {
                actor: &format!("unix:{unix_user}"),
                action: "break-glass",
                subject: &sid,
                outcome: "ok",
                client: "host",
                detail: &format!("exp={}", issued.claims.exp),
            },
            now,
        )
        .await?;
        Ok(issued)
    }

    /// End a person's access.
    pub async fn end_person(
        &self,
        by: Actor<'_>,
        id: &str,
    ) -> Result<people::Ended, IdentityError> {
        people::end(
            self.0.store.pool(),
            self.handle(),
            &self.0.keys,
            id,
            by,
            self.now(),
        )
        .await
    }

    /// End a machine key.
    pub async fn end_machine_key(&self, by: Actor<'_>, id: &str) -> Result<u64, IdentityError> {
        machine::end(
            self.0.store.pool(),
            self.handle(),
            &self.0.keys,
            id,
            by,
            self.now(),
        )
        .await
    }

    /// Revoke an unspent invite.
    pub async fn revoke_invite(&self, by: Actor<'_>, id: &str) -> Result<(), IdentityError> {
        invites::revoke(
            self.0.store.pool(),
            self.handle(),
            &self.0.keys,
            id,
            by,
            self.now(),
        )
        .await
    }
}
