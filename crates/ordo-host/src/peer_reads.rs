//! The consumer's peer reads through the holder's door
//! (`plan-federation-grants.md` §4, step G5): [`ordo_exec::peer_reads`]'s
//! [`PeerFetch`] over the one client, [`crate::peer_client`].
//!
//! Each fetch is one call to the holder's `std.<entity>.list` with a
//! `$refine`, made for the person this read is for, or as this deployment
//! itself when nobody is signed in:
//!
//! - **For a person**, the call carries a token the identity service
//!   exchanged for this deployment's own session statement about them
//!   ([`Acting::ForPerson`]): their `sub` at the service, from their
//!   sign-in link here (`oidc:<connection>:<sub>`, the connection whose
//!   issuer is the service). The holder applies its own rules for them.
//!   It holds as long as they are a member here, past their 10-minute ID
//!   token. A person signed in here some other way has no `sub` there, and
//!   the read is refused naming the connection.
//! - **As itself**, the holder's grant must be marked `as itself`.
//!
//! Failures keep their [`PeerClientError`] and answer by what a caller can
//! do: a holder that is down, slow (`ORDO_PEER_TIMEOUT_SECS`) or busy is
//! `peer_unavailable`; a refusal, or a setting to fix, is `peer_refused`,
//! with the holder's own words.
//!
//! Every call passes on the chain of the call being served (step G5b):
//! the service writes it into the token with this deployment last, and a
//! holder already on it refuses the call by name at once. Without it 2
//! deployments whose guards read each other would call each other until
//! `ORDO_PEER_TIMEOUT_SECS`.
//!
//! Tokens are held for no longer than one fetch: nothing here outlives the
//! call.

use std::sync::Arc;

use ordo_exec::peer_reads::{Fetch, PeerFetch, PeerReadFail};
use ordo_exec::Pool;
use serde_json::{json, Map, Value};

use crate::peer_client::{Acting, Action, Method, PeerClient, PeerClientError, Settings};

/// `ORDO_PEER_READ_MAX_ROWS`: the most rows one read takes from a holder
/// when nothing bounds it in one answer. Past it the read is refused,
/// naming the setting.
pub const MAX_ROWS_ENV: &str = "ORDO_PEER_READ_MAX_ROWS";
/// [`MAX_ROWS_ENV`]'s default.
pub const DEFAULT_MAX_ROWS: usize = 1000;
/// `ORDO_PEER_BATCH_KEYS`: the most refs one batched join call carries.
/// The holder's `ORDO_REFINE_MAX_IN_VALUES` and `ORDO_REFINE_MAX_LIMIT`
/// must be at least this.
pub const BATCH_KEYS_ENV: &str = "ORDO_PEER_BATCH_KEYS";
/// [`BATCH_KEYS_ENV`]'s default: the holder's defaults.
pub const DEFAULT_BATCH_KEYS: usize = 200;

/// [`PeerFetch`] over the one client.
pub struct DoorReads {
    client: Arc<PeerClient>,
    var: Settings,
    /// The deployment's own database, where the sign-in links
    /// (`$authenticator`) are. A tenant kept in a database of its own has
    /// its members there and its people's links here (step G7).
    directory: Option<Pool>,
}

impl DoorReads {
    pub fn new(client: Arc<PeerClient>, var: Settings) -> DoorReads {
        DoorReads {
            client,
            var,
            directory: None,
        }
    }

    /// Read sign-in links from `pool`, the deployment's own database,
    /// whatever database the read's tenant is in. Without it they are read
    /// from the tenant's database.
    pub fn with_directory(mut self, pool: Pool) -> DoorReads {
        self.directory = Some(pool);
        self
    }

    fn number(&self, var: &str, default: usize) -> usize {
        (self.var)(var)
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(default)
    }

    /// The person's id at the identity service, from their sign-in link
    /// here through the connection to the service.
    async fn subject(&self, pool: &Pool, member: &str) -> Result<String, PeerReadFail> {
        let settings = crate::peer_door::PeerSettings::from_vars(&*self.var).map_err(|why| {
            PeerReadFail::refused(format!(
                "a peer read for a person needs this deployment's sign-in connection to Ordo's \
                 identity service: {why}"
            ))
        })?;
        let prefix = format!("oidc:{}:", settings.connection);
        let busy = |e: ordo_exec::PoolError| PeerReadFail {
            code: if e.is_busy() {
                "server_busy"
            } else {
                PeerReadFail::UNAVAILABLE
            },
            message: format!("reading who the caller is at the identity service: {e}"),
        };
        // The member, in the tenant's database; their links, in the
        // deployment's own. 2 reads, since the 2 may be 2 databases.
        let principal = pool
            .rows(&format!(
                "SELECT \"principal\" FROM \"members\" WHERE \"id\" = {} \
                 AND \"kind\" = 'principal'",
                ordo_exec::sql_lit(member),
            ))
            .await
            .map_err(busy)?
            .into_iter()
            .find_map(|r| r.into_iter().next().flatten());
        let rows = match &principal {
            None => Vec::new(),
            Some(p) => self
                .directory
                .as_ref()
                .unwrap_or(pool)
                .rows(&format!(
                    "SELECT \"authenticator\" FROM \"$authenticator\" \
                     WHERE \"principal\" = {} AND \"retired_case\" = '' \
                     AND left(\"authenticator\", {}) = {} \
                     ORDER BY \"authenticator\" LIMIT 2",
                    ordo_exec::sql_lit(p),
                    prefix.chars().count(),
                    ordo_exec::sql_lit(&prefix),
                ))
                .await
                .map_err(busy)?,
        };
        let subs: Vec<String> = rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .filter_map(|a| a.strip_prefix(&prefix).map(str::to_string))
            .collect();
        match subs.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(PeerReadFail::refused(format!(
                "this person has no sign-in here through `{}`, Ordo's identity service, so \
                 another deployment cannot know them. Fix: sign in through `{}` once",
                settings.connection, settings.connection
            ))),
            _ => Err(PeerReadFail::refused(format!(
                "this person has 2 sign-ins here through `{}`, so they are 2 people at the \
                 identity service. Fix: retire one of them",
                settings.connection
            ))),
        }
    }
}

fn fail(e: PeerClientError) -> PeerReadFail {
    PeerReadFail {
        code: match e.action() {
            Action::Retry => PeerReadFail::UNAVAILABLE,
            Action::Refuse | Action::FixInput => PeerReadFail::REFUSED,
        },
        message: e.to_string(),
    }
}

#[async_trait::async_trait]
impl PeerFetch for DoorReads {
    async fn fetch(
        &self,
        pool: &Pool,
        caller: Option<&str>,
        chain: &[String],
        f: &Fetch,
    ) -> Result<Vec<Map<String, Value>>, PeerReadFail> {
        let subject = match caller {
            Some(member) => Some(self.subject(pool, member).await?),
            None => None,
        };
        let acting = match &subject {
            Some(s) => Acting::ForPerson { subject: s },
            None => Acting::AsItself,
        };
        let holder = self.client.holder(&f.peer).map_err(fail)?;
        let token = self
            .client
            .token(&holder.handle, acting, chain)
            .await
            .map_err(fail)?;
        let op = f.op.clone();
        let max = self.number(MAX_ROWS_ENV, DEFAULT_MAX_ROWS);
        let mut params = json!({ "$refine": f.refine().to_string() });
        let mut rows: Vec<Map<String, Value>> = Vec::new();
        loop {
            let answer = self
                .client
                .call_with(&holder, &token, &op, Method::Read, &params)
                .await
                .map_err(fail)?;
            rows.extend(
                answer["rows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r.as_object().cloned()),
            );
            let next = answer["next"].as_str().map(str::to_string);
            match next {
                Some(n) if f.whole => {
                    if rows.len() >= max {
                        return Err(PeerReadFail::refused(format!(
                            "this read takes more than {max} rows from `{op}` of peer `{}`, and \
                             nothing in it can be sent to the holder to narrow them. Fix: add \
                             a condition on the peer's own fields or a `take`, or raise \
                             {MAX_ROWS_ENV} (default {DEFAULT_MAX_ROWS})",
                            f.peer
                        )));
                    }
                    params["$page"] = Value::String(n);
                }
                _ => break,
            }
        }
        Ok(rows)
    }

    fn batch_keys(&self) -> usize {
        self.number(BATCH_KEYS_ENV, DEFAULT_BATCH_KEYS)
    }
}
