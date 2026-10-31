//! The "Deployment people" screen's routes (`plan-identity-service.md`
//! 5.6, 5.7, 7, step 6): the people who manage the deployment, their
//! invites, its machine keys, and the identity service's audit log.
//!
//! | Route | Relays to the identity service |
//! |---|---|
//! | `GET admins` | `GET /people`, `GET /invites`, `GET /machine-keys` |
//! | `GET admins/audit?before=&limit=` | `GET /audit` |
//! | `POST admins/invite {label}` | `POST /invites {role: admin, label}` |
//! | `POST admins/invite/revoke {id}` | `POST /invites/{id}/revoke` |
//! | `POST admins/end {id}` | `POST /people/{id}/end` |
//! | `POST admins/machine-keys/add {name, publicKey?}` | `POST /machine-keys` |
//! | `POST admins/machine-keys/end {id}` | `POST /machine-keys/{id}/end` |
//!
//! The console holds none of this. Every call goes through the one client
//! (`crate::identity::IdentityClient::call`) with the person's own token,
//! as `ordo people` does from the CLI, so the service decides who may and
//! records it under the person. A write needs a fresh touch: the console
//! checks the token's `touched_at` first, and the service's own
//! `presence_required` comes back as the same `step_up_required`. Each
//! write is in the console's audit log too, with the person's `sub` (plan
//! 7, "invites it relayed"). An invite link and a machine key's private
//! half pass through to the page once and are never logged.
//!
//! Without `ORDO_IDENTITY_URL` these answer `no_identity_service`, which
//! names the setting.

use axum::extract::Query;

use super::*;
use crate::identity::IdentityClient;

read!(admins_route, admins_get);
action!(invite_route, "admins.invite", invite);
action!(revoke_route, "admins.invite.revoke", revoke);
action!(end_route, "admins.end", end);
action!(key_add_route, "admins.machine_key.add", key_add);
action!(key_end_route, "admins.machine_key.end", key_end);

/// The routes, under `/d/<handle>/api/admins`.
pub(super) fn routes() -> Router<Console> {
    Router::new()
        .route("/d/{handle}/api/admins", get(admins_route))
        .route("/d/{handle}/api/admins/audit", get(audit_route))
        .route("/d/{handle}/api/admins/invite", post(invite_route))
        .route("/d/{handle}/api/admins/invite/revoke", post(revoke_route))
        .route("/d/{handle}/api/admins/end", post(end_route))
        .route(
            "/d/{handle}/api/admins/machine-keys/add",
            post(key_add_route),
        )
        .route(
            "/d/{handle}/api/admins/machine-keys/end",
            post(key_end_route),
        )
}

/// The one client, or the refusal that names the setting.
fn client(c: &Console) -> Result<&IdentityClient, ConsoleError> {
    c.0.identity
        .as_deref()
        .ok_or(ConsoleError::NoIdentityService)
}

/// The person's token, checked: a person's, and touched within the window
/// when `need` asks for it.
async fn token(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    need: Need,
) -> Result<String, ConsoleError> {
    client(c)?;
    match c.who(call, headers, need).await? {
        Who::Person { token, .. } => Ok(token),
        Who::Owner(_) => Err(ConsoleError::NoIdentityService),
    }
}

/// One relayed call. The service's `presence_required` is the console's
/// `step_up_required`, so the page takes one touch for either.
async fn relay(
    c: &Console,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&Value>,
) -> Result<Value, ConsoleError> {
    client(c)?
        .call(method, path, token, body)
        .await
        .map_err(|e| match e.code() {
            Some("presence_required") => ConsoleError::StepUpNeeded {
                window_secs: STEP_UP_WINDOW_SECS,
            },
            _ => ConsoleError::Identity(e),
        })
}

#[derive(Deserialize)]
struct IdBody {
    id: String,
}

#[derive(Deserialize)]
struct InviteBody {
    #[serde(default)]
    label: String,
}

#[derive(Deserialize)]
struct KeyBody {
    name: String,
    #[serde(rename = "publicKey")]
    public_key: Option<String>,
}

/// An id that goes into a path at the service: the service's own shapes
/// (`p_…`, `m_…`, `i_…`), letters, digits, `_` and `-` only.
fn path_id(id: &str) -> Result<&str, ConsoleError> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(id)
    } else {
        Err(ConsoleError::BadRequest(
            "`id` is the service's id for it, such as p_…".into(),
        ))
    }
}

async fn admins_get(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let t = token(c, call, headers, Need::Signed).await?;
    let (people, invites, keys) = tokio::try_join!(
        relay(c, "GET", "/people", &t, None),
        relay(c, "GET", "/invites", &t, None),
        relay(c, "GET", "/machine-keys", &t, None),
    )?;
    Ok(Reply::json(json!({
        "people": people.get("people").cloned().unwrap_or(json!([])),
        "invites": invites.get("invites").cloned().unwrap_or(json!([])),
        "machineKeys": keys.get("machineKeys").cloned().unwrap_or(json!([])),
    })))
}

async fn audit_route(
    State(c): State<Console>,
    Path(handle): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let mut call = Call {
        deployment: handle,
        actor: "anonymous".into(),
        detail: String::new(),
    };
    let result = async {
        c.admit(&call, &headers, false)?;
        let t = token(&c, &mut call, &headers, Need::Signed).await?;
        let mut query = Vec::new();
        for k in ["before", "limit"] {
            if let Some(v) = q.get(k) {
                let n: u64 = v
                    .parse()
                    .map_err(|_| ConsoleError::BadRequest(format!("`{k}` is a whole number")))?;
                query.push(format!("{k}={n}"));
            }
        }
        let path = if query.is_empty() {
            "/audit".to_string()
        } else {
            format!("/audit?{}", query.join("&"))
        };
        Ok::<_, ConsoleError>(Reply::json(relay(&c, "GET", &path, &t, None).await?))
    }
    .await;
    match result {
        Ok(r) => r.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn invite(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let t = token(c, call, headers, Need::Touch).await?;
    let req: InviteBody = parse(body)?;
    let label = req.label.trim();
    let made = relay(
        c,
        "POST",
        "/invites",
        &t,
        Some(&json!({ "role": "admin", "label": label })),
    )
    .await?;
    // The link is a secret: the page shows it once; the log names the id.
    call.detail = format!(
        "invite={}",
        made.get("id").and_then(Value::as_str).unwrap_or("")
    );
    Ok(Reply::json(made))
}

async fn revoke(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let t = token(c, call, headers, Need::Touch).await?;
    let req: IdBody = parse(body)?;
    let id = path_id(&req.id)?;
    call.detail = format!("invite={id}");
    Ok(Reply::json(
        relay(c, "POST", &format!("/invites/{id}/revoke"), &t, None).await?,
    ))
}

async fn end(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let t = token(c, call, headers, Need::Touch).await?;
    let req: IdBody = parse(body)?;
    let id = path_id(&req.id)?;
    call.detail = format!("person={id}");
    Ok(Reply::json(
        relay(c, "POST", &format!("/people/{id}/end"), &t, None).await?,
    ))
}

async fn key_add(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let t = token(c, call, headers, Need::Touch).await?;
    let req: KeyBody = parse(body)?;
    let mut ask = json!({ "name": req.name.trim() });
    if let Some(p) = req.public_key.as_deref().filter(|p| !p.trim().is_empty()) {
        ask["publicKey"] = json!(p.trim());
    }
    let made = relay(c, "POST", "/machine-keys", &t, Some(&ask)).await?;
    // The private half, when the service made the pair, is shown once.
    call.detail = format!(
        "machine_key={} name={}",
        made.get("id").and_then(Value::as_str).unwrap_or(""),
        req.name.trim()
    );
    Ok(Reply::json(made))
}

async fn key_end(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let t = token(c, call, headers, Need::Touch).await?;
    let req: IdBody = parse(body)?;
    let id = path_id(&req.id)?;
    call.detail = format!("machine_key={id}");
    Ok(Reply::json(
        relay(c, "POST", &format!("/machine-keys/{id}/end"), &t, None).await?,
    ))
}
