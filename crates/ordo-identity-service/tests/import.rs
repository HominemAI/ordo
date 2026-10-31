//! The console import (`plan-identity-service.md` 6.1, 6.2, 8 "Import" and
//! "Upgrade"): a console database made by the console itself, through its
//! own migrations and sign-up, moved into the identity service; the same
//! passkey then signs in at the service, and a printed recovery code works.

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_console::config::{
    DATABASE_URL_ENV as CONSOLE_DB, DATA_DIR_ENV as CONSOLE_DATA, MODE_ENV,
    ORIGIN_ENV as CONSOLE_ORIGIN_ENV, PAGE_DIR_ENV,
};
use ordo_console::{BootOptions as ConsoleBoot, Console, ConsoleConfig};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::Argon2idHasher;
use ordo_identity_service::audit;
use ordo_identity_service::config::{IMPORT_CONSOLE_URL_ENV, ORIGIN_ENV, RP_ID_ENV};
use serde_json::{json, Value};
use support::*;
use tower::ServiceExt;

const CONSOLE_ORIGIN: &str = "http://localhost:8081";

/// What a console holds once its owners signed up.
struct Made {
    url: String,
    /// The first owner's two passkeys, as the console's page used them.
    adam: Key,
    adam_second: Key,
    /// The removed second owner's.
    bea: Key,
    /// Adam's recovery codes, as printed.
    codes: Vec<String>,
}

/// One passkey: its seed and credential id, used at an origin.
#[derive(Clone, Copy)]
struct Key(u8, &'static [u8]);

impl Key {
    fn at(self, origin: &str) -> TestAuthenticator {
        TestAuthenticator {
            origin: origin.to_string(),
            ..authenticator(self.0, self.1)
        }
    }
}

async fn console_post(
    router: &axum::Router,
    path: &str,
    body: Value,
    cookie: Option<&str>,
) -> (StatusCode, Value, Option<String>) {
    let mut req = Request::post(format!("/d/main/api/{path}"))
        .header("origin", CONSOLE_ORIGIN)
        .header("content-type", "application/json");
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    let res = router
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let cookie = res
        .headers()
        .get("set-cookie")
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cookie,
    )
}

/// A console, booted and signed up through its own routes: Adam with two
/// passkeys, Bea invited, enrolled and removed, and one invite left
/// unspent.
async fn console(name: &str) -> Made {
    let db = format!("ordo_identity_t_console_{}", name.replace('-', "_"));
    let c = cluster();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let url = url_for(&db);
    let dir = temp(&format!("console-{name}"));
    std::fs::create_dir_all(dir.join("page")).unwrap();
    std::fs::write(dir.join("page/index.html"), "<!doctype html>").unwrap();
    let vars = HashMap::from([
        (CONSOLE_DB.to_string(), url.clone()),
        (CONSOLE_ORIGIN_ENV.to_string(), CONSOLE_ORIGIN.to_string()),
        (CONSOLE_DATA.to_string(), dir.display().to_string()),
        (
            PAGE_DIR_ENV.to_string(),
            dir.join("page").display().to_string(),
        ),
        (MODE_ENV.to_string(), "door-only".to_string()),
    ]);
    let cfg = ConsoleConfig::from_vars(&|k| vars.get(k).cloned()).expect("console config");
    let console = Console::boot(
        cfg,
        ConsoleBoot {
            clock: ordo_identity::clock::Clock::manual(START - 3_600),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        },
    )
    .await
    .expect("console boot");
    let router = console.router();
    let setup = std::fs::read_to_string(dir.join("setup-code"))
        .unwrap()
        .trim()
        .to_string();

    let adam = Key(21, b"adam-phone");
    let (s, opts, _) = console_post(&router, "setup/begin", json!({ "code": setup }), None).await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    let mut body = registration(&adam.at(CONSOLE_ORIGIN), &challenge(&opts));
    body["code"] = json!(setup);
    body["label"] = json!("Adam");
    let (s, done, cookie) = console_post(&router, "setup/finish", body, None).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    let cookie = cookie.expect("a console session");
    let codes: Vec<String> = done["recoveryCodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();

    let adam_second = Key(22, b"adam-laptop");
    let (s, opts, _) = console_post(&router, "passkeys/begin", json!({}), Some(&cookie)).await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    let (s, v, _) = console_post(
        &router,
        "passkeys/finish",
        registration(&adam_second.at(CONSOLE_ORIGIN), &challenge(&opts)),
        Some(&cookie),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");

    let (s, inv, _) = console_post(&router, "owners/invite", json!({}), Some(&cookie)).await;
    assert_eq!(s, StatusCode::OK, "{inv}");
    let code = inv["code"].as_str().unwrap().to_string();
    let bea = Key(23, b"bea-phone");
    let (_, opts, _) = console_post(&router, "setup/begin", json!({ "code": code }), None).await;
    let mut body = registration(&bea.at(CONSOLE_ORIGIN), &challenge(&opts));
    body["code"] = json!(code);
    body["label"] = json!("Bea");
    let (s, done, _) = console_post(&router, "setup/finish", body, None).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    let bea_id = done["owner"].as_str().unwrap().to_string();
    let (s, v, _) = console_post(
        &router,
        "owners/remove",
        json!({ "owner": bea_id }),
        Some(&cookie),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v, _) = console_post(&router, "owners/invite", json!({}), Some(&cookie)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    console.shutdown().await;
    Made {
        url,
        adam,
        adam_second,
        bea,
        codes,
    }
}

/// A fingerprint of the console's owner tables, to prove the import wrote
/// nothing there.
async fn fingerprint(url: &str) -> Vec<String> {
    let cfg = ordo_exec::dburl::parse_database_url(url, 1).unwrap();
    let pool = ordo_exec::Pool::connect_own(cfg, ordo_exec::PoolLimits::default())
        .await
        .unwrap();
    let mut out = Vec::new();
    for t in [
        "owner",
        "passkeys",
        "recovery_codes",
        "setup_codes",
        "sessions",
    ] {
        let rows = pool
            .rows(&format!(
                "SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), '')) FROM {t} t"
            ))
            .await
            .unwrap();
        out.push(rows[0][0].clone().unwrap_or_default());
    }
    out
}

async fn console_count(url: &str, sql: &str) -> u64 {
    let cfg = ordo_exec::dburl::parse_database_url(url, 1).unwrap();
    let pool = ordo_exec::Pool::connect_own(cfg, ordo_exec::PoolLimits::default())
        .await
        .unwrap();
    pool.rows(sql).await.unwrap()[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap()
}

async fn sign_in(h: &H, auth: &TestAuthenticator) -> (StatusCode, Value) {
    let (_, opts, _) = h.post("/sign-in/begin", None, json!({})).await;
    let (s, v, _) = h
        .post("/sign-in/finish", None, assertion(auth, &challenge(&opts)))
        .await;
    (s, v)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_moves_the_owners_in_and_their_passkeys_and_codes_still_work() {
    let made = console("upgrade").await;
    // One of Adam's codes was used at the console; it must stay behind.
    let used_lookup: String = made.codes[9].replace('-', "")[..8].to_string();
    {
        let cfg = ordo_exec::dburl::parse_database_url(&made.url, 1).unwrap();
        let pool = ordo_exec::Pool::connect_own(cfg, ordo_exec::PoolLimits::default())
            .await
            .unwrap();
        pool.rows(&format!(
            "UPDATE recovery_codes SET used_at = 1 WHERE lookup = '{used_lookup}'"
        ))
        .await
        .unwrap();
    }
    let console_passkeys = console_count(&made.url, "SELECT count(*) FROM passkeys").await;
    let console_live_codes = console_count(
        &made.url,
        "SELECT count(*) FROM recovery_codes WHERE used_at IS NULL AND voided_at IS NULL",
    )
    .await;
    assert_eq!(console_live_codes, 9);
    let before = fingerprint(&made.url).await;

    let h = boot_with("import", &[(IMPORT_CONSOLE_URL_ENV, &made.url)]).await;
    let first = h.svc.first_start().await.unwrap();
    let imported = first.import.imported.clone().expect("an import");
    assert_eq!(imported.people, 2);
    assert_eq!(imported.live_admins, 1);
    assert_eq!(imported.passkeys, console_passkeys);
    assert_eq!(imported.recovery_codes, console_live_codes);
    assert_eq!(imported.setup_codes_voided, 1);
    assert!(
        first.first_invite.is_none(),
        "a live admin came in: no first invite"
    );
    assert!(first.provisioner.is_none());
    assert_eq!(
        fingerprint(&made.url).await,
        before,
        "the console's tables are untouched"
    );

    // A second start imports nothing. A start that lost its `imports` row
    // (as a crash before it would) redoes the copy and duplicates nothing.
    assert!(h.svc.first_start().await.unwrap().import.imported.is_none());
    h.svc
        .store()
        .pool()
        .rows("DELETE FROM imports")
        .await
        .unwrap();
    let redo = h.svc.first_start().await.unwrap().import.imported.unwrap();
    assert_eq!((redo.people, redo.passkeys, redo.recovery_codes), (0, 0, 0));
    let people = ordo_identity_service::people::list(h.svc.store().pool(), "main")
        .await
        .unwrap();
    assert_eq!(people.len(), 2);
    let adam_p = people.iter().find(|p| p.display_name == "Adam").unwrap();
    let bea_p = people.iter().find(|p| p.display_name == "Bea").unwrap();
    assert!(adam_p.ended_at.is_none());
    assert!(bea_p.ended_at.is_some(), "a removed owner arrives ended");

    // The upgrade's proof: the same passkeys, at the service's origin.
    for key in [made.adam, made.adam_second] {
        let (s, v) = sign_in(&h, &key.at(ORIGIN)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["person"], adam_p.id.as_str());
    }
    let (s, _) = sign_in(&h, &made.bea.at(ORIGIN)).await;
    assert_eq!(
        s,
        StatusCode::UNAUTHORIZED,
        "an ended person does not sign in"
    );

    // A printed code works once; the code used at the console does not.
    let (s, v, _) = h
        .post("/recover", None, json!({ "code": made.codes[0] }))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["person"], adam_p.id.as_str());
    for c in [&made.codes[0], &made.codes[9]] {
        let (s, _, _) = h.post("/recover", None, json!({ "code": c })).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    let rows = audit::rows(h.svc.store().pool(), "main").await.unwrap();
    let count = |a: &str| rows.iter().filter(|r| r.action == a).count();
    assert_eq!(count("person.imported"), 2);
    assert_eq!(count("import.console"), 2, "the first run and the redo");
    assert!(count("setup-code.voided") >= 1);
    let checked = audit::verify(
        h.svc.store().pool(),
        "main",
        &audit::verifiers(h.svc.store().pool(), "main")
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert!(checked.rows > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_start_under_another_rp_id_warns_and_the_old_passkeys_do_not_sign_in() {
    let made = console("rp-change").await;
    let h = boot_with("rp-change", &[(IMPORT_CONSOLE_URL_ENV, &made.url)]).await;
    h.svc.first_start().await.unwrap();

    let mut vars = h.vars.clone();
    vars.insert(
        ORIGIN_ENV.to_string(),
        "http://id.localhost:8082".to_string(),
    );
    vars.insert(RP_ID_ENV.to_string(), "id.localhost".to_string());
    let moved = reboot(&vars, &h.clock)
        .await
        .expect("boot under another RP ID");
    let step = moved.first_start().await.unwrap().import;
    assert_eq!(
        step.rp_id_changed,
        Some(("localhost".to_string(), "id.localhost".to_string()))
    );
    let elsewhere = H {
        svc: moved,
        clock: h.clock.clone(),
        vars,
    };
    let auth = made.adam.at("http://id.localhost:8082");
    let (_, opts, _) = elsewhere
        .call(
            "POST",
            "/sign-in/begin",
            &[("origin", "http://id.localhost:8082")],
            json!({}),
        )
        .await;
    let (s, _, _) = elsewhere
        .call(
            "POST",
            "/sign-in/finish",
            &[("origin", "http://id.localhost:8082")],
            assertion(&auth, &challenge(&opts)),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_console_database_without_owner_tables_imports_nothing_and_the_first_invite_follows() {
    let db = "ordo_identity_t_console_empty";
    let c = cluster();
    ordo_exec::drop_database(&c, db).await.unwrap();
    ordo_exec::create_database(&c, db).await.unwrap();
    let h = boot_with("import-empty", &[(IMPORT_CONSOLE_URL_ENV, &url_for(db))]).await;
    let first = h.svc.first_start().await.unwrap();
    assert_eq!(first.import.imported, Some(Default::default()));
    assert!(first.first_invite.is_some(), "no live admin came in");

    // An import URL that cannot be read refuses the start, naming it.
    let bad = boot_with(
        "import-bad",
        &[(
            IMPORT_CONSOLE_URL_ENV,
            &url_for("ordo_identity_t_no_such_db"),
        )],
    )
    .await;
    let e = bad.svc.first_start().await.unwrap_err();
    assert_eq!(e.setting_name(), Some(IMPORT_CONSOLE_URL_ENV), "{e}");
}

/// The opt-in (owner, 2026-10-30): with no database named and
/// `ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true`, the service makes
/// `ordo_identity` on the console's server with the console's role at its
/// first start, and moves the owners in; a second start finds it made.
/// Unset, the default, it refuses instead, naming both settings: it never
/// makes a database by itself.
#[tokio::test(flavor = "multi_thread")]
async fn with_no_database_named_the_service_makes_its_own_beside_the_consoles() {
    use ordo_identity_service::config::{
        BESIDE_CONSOLE_DB, DATABASE_BESIDE_CONSOLE_ENV, DATABASE_URL_ENV,
    };
    let made = console("beside").await;
    ordo_exec::drop_database(&cluster(), BESIDE_CONSOLE_DB)
        .await
        .unwrap();
    let mut h = boot_with("beside", &[(IMPORT_CONSOLE_URL_ENV, &made.url)]).await;
    h.vars.remove(DATABASE_URL_ENV);
    // The default: refused, naming both settings, and no database made.
    let e = reboot(&h.vars, &h.clock)
        .await
        .err()
        .expect("refused by default");
    assert_eq!(e.setting_name(), Some(DATABASE_URL_ENV), "{e}");
    assert!(e.to_string().contains(DATABASE_BESIDE_CONSOLE_ENV), "{e}");
    assert!(
        !ordo_exec::drop_database(&cluster(), BESIDE_CONSOLE_DB)
            .await
            .unwrap(),
        "no database was made without the opt-in"
    );
    h.vars
        .insert(DATABASE_BESIDE_CONSOLE_ENV.to_string(), "true".to_string());
    let svc = reboot(&h.vars, &h.clock)
        .await
        .expect("boots with no database named");
    assert_eq!(svc.config().database.db, BESIDE_CONSOLE_DB);
    let first = svc.first_start().await.unwrap();
    assert_eq!(first.import.imported.expect("an import").live_admins, 1);
    drop(svc);
    let again = reboot(&h.vars, &h.clock).await.expect("a second start");
    assert!(again.first_start().await.unwrap().import.imported.is_none());
    drop(again);

    h.vars
        .insert(DATABASE_BESIDE_CONSOLE_ENV.to_string(), "false".to_string());
    let e = reboot(&h.vars, &h.clock).await.err().expect("refused");
    assert_eq!(e.setting_name(), Some(DATABASE_URL_ENV), "{e}");
    assert!(e.to_string().contains(DATABASE_BESIDE_CONSOLE_ENV), "{e}");
    ordo_exec::drop_database(&cluster(), BESIDE_CONSOLE_DB)
        .await
        .unwrap();
}

/// The upgrade as the one image runs it (3.1, 6.1, 6.2): the deployment
/// owner's compose file names only the console's settings and the
/// service's own database. The supervisor fills the service's origin (the
/// console's own, with the base path `/identity` the console passes
/// through, step 7), its RP ID and the import from the console's, and
/// hands it its secrets as files. The console's owners move in, no first
/// invite is made, and their passkeys sign in on the console's origin,
/// under the base path.
#[tokio::test(flavor = "multi_thread")]
async fn the_supervisors_defaults_alone_move_the_owners_in_and_their_passkeys_sign_in() {
    let made = console("supervised").await;
    let own = fresh_db("supervised").await;
    let dir = temp("supervised");
    let url_file = dir.join("console-database-url");
    std::fs::write(&url_file, format!("{}\n", made.url)).unwrap();
    // The container's variables, as compose.one.yml sets them.
    let container: Vec<(String, String)> = [
        (CONSOLE_ORIGIN_ENV, CONSOLE_ORIGIN.to_string()),
        (
            "ORDO_CONSOLE_DATABASE_URL_FILE",
            url_file.display().to_string(),
        ),
        ("ORDO_IDENTITY_DATABASE_URL", own),
        (
            "ORDO_IDENTITY_DATA_DIR",
            dir.join("data").display().to_string(),
        ),
        ("ORDO_PG_DB", "ordo".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let split = ordo_supervisor::children::split(
        &container,
        &|p: &std::path::Path| std::fs::read_to_string(p),
        START,
    );
    assert!(split.notes.is_empty(), "{:?}", split.notes);
    let vars: HashMap<String, String> =
        ordo_supervisor::children::write_env(&split.identity, &dir.join("run"), None)
            .unwrap()
            .into_iter()
            .collect();
    assert_eq!(
        vars[ORIGIN_ENV], CONSOLE_ORIGIN,
        "the console's own origin: no new port"
    );
    assert_eq!(vars["ORDO_IDENTITY_BASE_PATH"], "/identity");
    assert_eq!(vars[RP_ID_ENV], "localhost");
    assert!(!vars.contains_key(IMPORT_CONSOLE_URL_ENV), "never a value");
    assert!(vars.contains_key(&format!("{IMPORT_CONSOLE_URL_ENV}_FILE")));

    let clock = ordo_identity::clock::Clock::manual(START);
    let svc = reboot(&vars, &clock).await.expect("boot");
    let first = svc.first_start().await.unwrap();
    let imported = first.import.imported.clone().expect("an import");
    assert_eq!((imported.people, imported.live_admins), (2, 1));
    assert!(first.first_invite.is_none(), "the owners came in");
    let h = H { svc, clock, vars };
    // As the browser does through the console: the console's origin, the
    // service's routes under the base path.
    let page = [("origin", CONSOLE_ORIGIN)];
    for key in [made.adam, made.adam_second] {
        let (_, opts, _) = h
            .call("POST", "/identity/sign-in/begin", &page, json!({}))
            .await;
        let auth = key.at(CONSOLE_ORIGIN);
        let (s, v, headers) = h
            .call(
                "POST",
                "/identity/sign-in/finish",
                &page,
                assertion(&auth, &challenge(&opts)),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let set = headers["set-cookie"].to_str().unwrap();
        assert!(set.contains("Path=/identity;"), "{set}");
    }
}
