//! Attestation layers 2 and 4, end to end, against a real database and a
//! real TLS listener.
//!
//! Every property here is about something *not* happening, which is why it
//! needs a test rather than a reading:
//!
//! - a delivery that lands does so **once**, and the receiver can
//!   authenticate it with the secret they were shown at grant;
//! - a destination the platform rewrote in the database is **refused**, on
//!   its first attempt, and the receiver never hears from it;
//! - a destination that resolves inside the perimeter is refused before a
//!   socket is opened, and does not retry;
//! - a chain proof exported here verifies under a binary that cannot reach
//!   this database — and stops verifying the moment a link is changed;
//! - the proof and the export bundle agree on the encoding of the three
//!   fields they share.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, Ed25519Signer, Image,
    Signer, SpaceId, Timestamp, Value,
};
use ordo_egress::{DenyList, TlsRoots};
use ordo_exec::cadence::{self, Attested, LogPolicy};
use ordo_exec::webhook::{self, AttestSink};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::attestdrain::{run_pass, DeliveryConfig, DeliveryHealth};
use ordo_host::metrics::Metrics;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

const CA: &[u8] = include_bytes!("fixture/ca.der");
/// A leaf for `localhost`, signed by the test CA above.
///
/// `localhost` rather than a made-up name because the destination has to
/// **resolve**: `resolve_and_pin` refuses an unresolvable host, correctly, and
/// RFC 2606 guarantees `.invalid` never resolves. So the suite points the sink
/// at `localhost`, permits loopback in the deny list (feature-gated, tests
/// only, loopback and never link-local), and leaves certificate validation
/// fully on with the CA as the anchor. Nothing about pin 1 is relaxed — the
/// handshake really validates the chain and really checks the name, which is
/// proved by `a_certificate_for_another_name_is_refused` below.
const LEAF: &[u8] = include_bytes!("fixture/localhost.der");
const LEAF_KEY: &[u8] = include_bytes!("fixture/localhost-key.der");

// ── database ──────────────────────────────────────────────────────────────

async fn fresh(db: &str) -> Pool {
    let _ = ordo_conformance::Pg::named(db);
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    cadence::ensure_tables(&pool).await.expect("$attestations");
    webhook::ensure_tables(&pool).await.expect("$attestsinks");
    for t in [
        "$commandlog",
        "$checkpoints",
        "$attestations",
        "$attestsinks",
        "$attestdeliveries",
        "$attestdeadletters",
        "$schedules",
    ] {
        pool.rows(&format!("DELETE FROM \"{t}\"")).await.unwrap();
    }
    pool
}

fn space() -> SpaceId {
    SpaceId("spc_att".into())
}

fn chain_signer() -> Ed25519Signer {
    Ed25519Signer::from_seed("chain-key-2026", &[9u8; 32])
}

fn owner() -> Ed25519Signer {
    Ed25519Signer::from_seed("owner-key-2026", &[23u8; 32])
}

fn group_at(n: usize, at: u64) -> CommandGroup {
    let mut after: Image = Image::new();
    after.insert("title".into(), Value::Text(format!("t{n}")));
    CommandGroup {
        correlation: CorrelationId(format!("cg_{n}")),
        actor: Actor::User(format!("usr_{n}")),
        commands: vec![Command {
            id: CommandId(format!("cmd_{n}")),
            space: space(),
            entity: "task".into(),
            row_id: format!("tsk_{n}"),
            kind: CommandKind::Create,
            before: None,
            after: Some(after),
            at: Timestamp(at),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

async fn append_n(pool: &Pool, n: usize, base: u64) {
    for i in 0..n {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&group_at(i, base + i as u64))
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
}

/// Append some history and sign one attestation over it.
async fn attest(pool: &Pool, now: u64) -> u64 {
    append_n(pool, 4, 1_757_000_000).await;
    match cadence::attest_due(
        pool,
        &space(),
        &chain_signer(),
        &LogPolicy::DEFAULT,
        Timestamp(now),
    )
    .await
    .expect("attest")
    {
        Attested::Signed(a) => a.upto,
        other => panic!("expected a signed attestation, got {other:?}"),
    }
}

// ── the listener ──────────────────────────────────────────────────────────

#[derive(Debug)]
struct Seen {
    head: String,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<String> {
        self.head
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}: ")))
            .map(str::to_string)
    }
}

async fn listener(response: &'static str) -> (SocketAddr, mpsc::Receiver<Seen>) {
    let cert = rustls::pki_types::CertificateDer::from(LEAF.to_vec());
    let key = rustls::pki_types::PrivateKeyDer::try_from(LEAF_KEY.to_vec()).expect("key");
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server config");
    let acceptor = TlsAcceptor::from(Arc::new(cfg));
    // Bound where the delivery will connect. The sink names `localhost`,
    // which answers both loopbacks in an order the resolver does not promise,
    // and the egress client pins the first address it is given; binding
    // `127.0.0.1` passed only while `127.0.0.1` came first. Asking the same
    // resolver the same question makes the receiver and the pin agree.
    let first = tokio::net::lookup_host("localhost:0")
        .await
        .expect("localhost resolves")
        .next()
        .expect("to at least one address");
    let tcp = TcpListener::bind((first.ip(), 0)).await.expect("bind");
    let addr = tcp.local_addr().unwrap();
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        while let Ok((sock, _)) = tcp.accept().await {
            let acceptor = acceptor.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(sock).await else {
                    return;
                };
                if let Ok(seen) = read_request(&mut tls).await {
                    let _ = tx.send(seen).await;
                }
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.flush().await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, rx)
}

async fn read_request<S>(s: &mut S) -> io::Result<Seen>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let end = loop {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::other("closed"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let want: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length: "))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[end + 4..].to_vec();
    while body.len() < want {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Ok(Seen { head, body })
}

// ── wiring ────────────────────────────────────────────────────────────────

/// The worker's configuration: a real trust anchor, a real timeout, and a
/// deny list that permits loopback and nothing else.
fn config(space: SpaceId) -> DeliveryConfig {
    DeliveryConfig {
        tick: Duration::from_millis(10),
        timeout: Duration::from_secs(5),
        space,
        policy: LogPolicy::DEFAULT,
        deny: DenyList::categories_only().permitting_loopback_for_tests(),
        roots: TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
    }
}

fn secret_hex() -> String {
    "00112233445566778899aabbccddeeff".to_string()
}

/// Register a sink the owner really signed, pointing at `url`.
async fn register(pool: &Pool, url: &str) -> AttestSink {
    let canonical = ordo_egress::HttpsUrl::parse(url)
        .expect("https")
        .canonical();
    let o = owner();
    let sig = o.sign(&webhook::consent_bytes(&space(), &canonical));
    webhook::register_sink(
        pool,
        &space(),
        &canonical,
        o.key_id(),
        &o.verifier().public_hex(),
        &sig,
        &secret_hex(),
        1_757_000_000,
    )
    .await
    .expect("register")
}

/// Register a sink and *then* accrue history, which is the order that
/// matters and the order a deployment does it in.
///
/// Doing it the other way round delivers nothing, on purpose: `register_sink`
/// starts the cursor at the current attestation, so a sink registered after
/// the fact does not replay the series into a receiver that was not watching.
/// Every delivery test here got zero units the first time it was written, for
/// exactly that reason -- which is the rule working.
async fn arm(pool: &Pool, url: &str) -> u64 {
    register(pool, url).await;
    attest(pool, 1_757_003_600).await
}

// ── layer 2 ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_round_trip_reaches_the_receiver_and_authenticates() {
    let pool = fresh("ordo_att_deliver").await;
    let (addr, mut rx) = listener("HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n").await;
    let upto = arm(&pool, &format!("https://localhost:{}/attest", addr.port())).await;
    assert_eq!(upto, 4);

    let health = DeliveryHealth::default();
    let metrics = Metrics::new();
    let cfg = config(space());
    let p = run_pass(&pool, &metrics, &cfg, &health, 1_757_003_601)
        .await
        .expect("pass");
    assert_eq!((p.sent, p.retrying, p.dead_lettered), (1, 0, 0));
    assert_eq!(webhook::outbox_depth(&pool).await.unwrap(), (0, 0));

    let seen = rx.recv().await.expect("the receiver got it");
    assert!(
        seen.head.starts_with("POST /attest HTTP/1.1"),
        "{}",
        seen.head
    );
    assert_eq!(seen.header("x-ordo-space").as_deref(), Some("spc_att"));
    assert_eq!(seen.header("x-ordo-position").as_deref(), Some("4"));
    assert_eq!(
        seen.header("x-ordo-schema").as_deref(),
        Some(webhook::PAYLOAD_SCHEMA)
    );

    // Pin 4 from the receiver's chair: the transport MAC verifies under the
    // secret they were shown at grant, over the timestamp, the canonical
    // destination and the body.
    let sig = seen.header("x-ordo-signature").expect("signature");
    let ts: u64 = seen
        .header("x-ordo-timestamp")
        .expect("timestamp")
        .parse()
        .unwrap();
    let canonical = format!("https://localhost:{}/attest", addr.port());
    let secret = ordo_egress::SinkSecret::from_hex(&secret_hex()).unwrap();
    assert!(
        ordo_egress::sign::verify(&secret, &sig, ts, &canonical, &seen.body),
        "the receiver can authenticate the platform"
    );
    assert!(
        !ordo_egress::sign::verify(&secret, &sig, ts, &canonical, b"{}"),
        "and cannot be made to accept a different body"
    );

    // And the payload itself verifies under the chain key, months later,
    // with no transport involved.
    let doc = String::from_utf8(seen.body.clone()).unwrap();
    let v = ordo_verify_proof::verify_attestation(&doc, Some(&chain_signer().verifier()))
        .expect("readable");
    assert!(v.ok(), "{:?}", v.findings);
    assert_eq!(v.upto, 4);

    // A second pass has nothing to do: the position landed and the outbox
    // row is gone, so at-least-once does not become at-least-twice on a
    // quiet tick.
    let p = run_pass(&pool, &metrics, &cfg, &health, 1_757_003_602)
        .await
        .expect("pass");
    assert_eq!(p.units(), 0);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(rx.try_recv().is_err(), "exactly one delivery");
    assert_eq!(health.sent(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_for_another_name_is_refused_even_though_the_address_is_pinned() {
    // The listener presents the `localhost` leaf; the sink says
    // `127.0.0.1`, which resolves to the same address. Pinning the address
    // must not weaken the name check, and this is the assertion that says so
    // at the worker level rather than only inside `ordo-egress`.
    let pool = fresh("ordo_att_namecheck").await;
    let (addr, mut rx) = listener("HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n").await;
    arm(&pool, &format!("https://127.0.0.1:{}/attest", addr.port())).await;

    let health = DeliveryHealth::default();
    let metrics = Metrics::new();
    let p = run_pass(&pool, &metrics, &config(space()), &health, 1_757_003_601)
        .await
        .expect("pass");
    assert_eq!(p.sent, 0);
    assert_eq!(p.retrying, 1, "a handshake failure is a transient");
    assert!(rx.try_recv().is_err(), "no payload byte was written");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_platform_cannot_redirect_a_delivery_by_editing_the_row() {
    // The hostile test. Everything is set up honestly, and then the
    // `destination` column is rewritten the way a compromised operator
    // account or a SQL injection would rewrite it.
    let pool = fresh("ordo_att_rewrite").await;
    let (addr, mut rx) = listener("HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n").await;
    arm(&pool, &format!("https://localhost:{}/attest", addr.port())).await;

    pool.rows("UPDATE \"$attestsinks\" SET \"destination\" = 'https://localhost:1/exfiltrate'")
        .await
        .expect("the platform rewrites the URL");

    // The stored signature no longer covers the stored destination.
    let sink = webhook::read_sink(&pool, &space())
        .await
        .unwrap()
        .expect("sink");
    assert_eq!(
        sink.verify_destination(),
        Err(webhook::SinkRefusal::NotSigned)
    );

    let health = DeliveryHealth::default();
    let metrics = Metrics::new();
    let p = run_pass(&pool, &metrics, &config(space()), &health, 1_757_003_601)
        .await
        .expect("pass");
    assert_eq!(p.sent, 0);
    assert_eq!(
        p.dead_lettered, 1,
        "an unsigned destination is not a transient; retrying it would give a redirect \
         attempt five more chances"
    );
    assert_eq!(p.retrying, 0);
    assert!(rx.try_recv().is_err(), "nothing was sent anywhere");

    let dead = webhook::dead_letters(&pool, &space()).await.unwrap();
    assert_eq!(dead.len(), 1);
    assert!(
        dead[0].reason.contains("destination refused"),
        "{}",
        dead[0].reason
    );
    // The dead letter names the destination that was attempted, so an
    // operator reading the ops surface sees where the platform was pointed.
    assert!(dead[0].destination.contains("exfiltrate"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_destination_inside_the_perimeter_is_refused_without_a_socket() {
    let pool = fresh("ordo_att_perimeter").await;
    // Registered honestly — the owner really signed it — and still refused,
    // because consent does not overrule the deny list. That ordering is the
    // whole of §7b: the lowest bar in the threat model is "anyone who pays",
    // and their consent is exactly what an SSRF needs.
    arm(&pool, "https://localhost:9/metadata").await;

    let health = DeliveryHealth::default();
    let metrics = Metrics::new();
    // The real deny list, with loopback denied as it is in every deployment.
    let cfg = DeliveryConfig {
        deny: DenyList::categories_only(),
        ..config(space())
    };
    let p = run_pass(&pool, &metrics, &cfg, &health, 1_757_003_601)
        .await
        .expect("pass");
    assert_eq!(p.sent, 0);
    assert_eq!(
        p.dead_lettered, 1,
        "a name that answers 127.0.0.1 will again"
    );

    let dead = webhook::dead_letters(&pool, &space()).await.unwrap();
    assert!(
        dead[0].reason.contains("denied_address") && dead[0].reason.contains("loopback"),
        "{}",
        dead[0].reason
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cursor_is_at_least_once_and_never_re_delivers_a_landed_position() {
    let pool = fresh("ordo_att_cursor").await;
    arm(&pool, "https://hooks.example.invalid/attest").await;

    let health = DeliveryHealth::default();
    let metrics = Metrics::new();
    let cfg = config(space());

    // First pass fans the attestation into the outbox and fails to deliver
    // it (the name does not resolve), so it stays.
    let p = run_pass(&pool, &metrics, &cfg, &health, 1_757_003_601)
        .await
        .expect("pass");
    assert_eq!(p.retrying, 1);
    let (pending, _) = webhook::outbox_depth(&pool).await.unwrap();
    assert_eq!(pending, 1);

    // A second fan-out must not duplicate it: the primary key absorbs the
    // replay, which is what makes a crash between the insert and the cursor
    // advance harmless.
    webhook::enqueue_due(&pool, &space(), 1_757_003_602)
        .await
        .unwrap();
    let (pending, _) = webhook::outbox_depth(&pool).await.unwrap();
    assert_eq!(pending, 1, "the outbox is keyed (space, upto)");

    // And a sink registered now starts at the current position rather than
    // replaying the series into a receiver that was not watching.
    let sink = register(&pool, "https://other.example.invalid/attest").await;
    assert_eq!(
        sink.cursor, 4,
        "a new sink starts at the head, not at genesis"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_backs_off_and_then_dead_letters() {
    let pool = fresh("ordo_att_backoff").await;
    arm(&pool, "https://hooks.example.invalid/attest").await;
    let health = DeliveryHealth::default();
    let metrics = Metrics::new();
    let cfg = config(space());

    // `triggers::backoff_secs` is 1 << attempts, so five attempts need a
    // clock that moves. The point of driving it by hand is that the backoff
    // is *observed* rather than slept through.
    let mut now = 1_757_003_601u64;
    let mut outcomes = Vec::new();
    for _ in 0..6 {
        let p = run_pass(&pool, &metrics, &cfg, &health, now)
            .await
            .expect("pass");
        outcomes.push((p.retrying, p.dead_lettered));
        now += 128;
    }
    assert_eq!(
        outcomes,
        vec![(1, 0), (1, 0), (1, 0), (1, 0), (0, 1), (0, 0)],
        "four retries then a dead letter, and then nothing left to try"
    );
    assert_eq!(
        webhook::dead_letters(&pool, &space()).await.unwrap().len(),
        1
    );
    let (pending, dead) = webhook::outbox_depth(&pool).await.unwrap();
    assert_eq!((pending, dead), (0, 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn deregistering_stops_delivery_and_clears_the_outbox() {
    let pool = fresh("ordo_att_dereg").await;
    arm(&pool, "https://hooks.example.invalid/attest").await;
    webhook::enqueue_due(&pool, &space(), 1_757_003_601)
        .await
        .unwrap();
    assert_eq!(webhook::outbox_depth(&pool).await.unwrap().0, 1);

    assert!(webhook::deregister_sink(&pool, &space()).await.unwrap());
    assert_eq!(
        webhook::outbox_depth(&pool).await.unwrap().0,
        0,
        "a delivery to a destination the owner withdrew is not one anybody wants completed"
    );
    assert!(!webhook::deregister_sink(&pool, &space()).await.unwrap());
}

// ── the payload, and the receiver's side of it ────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn the_payload_verifies_under_the_published_verifier() {
    let pool = fresh("ordo_att_payload").await;
    attest(&pool, 1_757_003_600).await;
    let sink = register(&pool, "https://hooks.example.invalid/attest").await;
    let record = cadence::latest_attestation(&pool, &space())
        .await
        .unwrap()
        .expect("an attestation");
    let floor = cadence::verifiable_from(&pool, &space()).await.unwrap();
    let body = webhook::payload(&record, &sink, &LogPolicy::DEFAULT, floor);
    let doc = String::from_utf8(body).unwrap();

    // The receiver's months-later check: Ed25519 under a key they discovered,
    // run by the published binary's library.
    let v = ordo_verify_proof::verify_attestation(&doc, Some(&chain_signer().verifier()))
        .expect("readable");
    assert!(v.ok(), "{:?}", v.findings);
    assert_eq!(v.upto, 4);
    assert_eq!(v.destination_key_id, owner().key_id());
    assert_eq!(v.cadence_or_secs, 3600);
    assert!(v.summary().starts_with("VERIFIED"));

    // Handed the wrong key, it refuses rather than shrugging.
    let wrong = Ed25519Signer::from_seed("chain-key-2026", &[11u8; 32]);
    let v = ordo_verify_proof::verify_attestation(&doc, Some(&wrong.verifier())).unwrap();
    assert!(!v.ok());

    // With no key at all it says the word.
    let v = ordo_verify_proof::verify_attestation(&doc, None).unwrap();
    assert!(v.ok());
    assert!(v.structure_only);
    assert!(v.summary().starts_with("UNVERIFIED"));
}

#[test]
fn the_two_consent_encodings_agree() {
    // `ordo_exec::webhook::consent_bytes` and
    // `ordo_verify_proof::proof::consent_bytes` are two implementations of
    // one signed encoding — one inside the platform, one in the binary an
    // owner runs to learn what to sign. Two implementations of a signed
    // encoding are only safe if something compares them.
    for (space, url) in [
        ("acme", "https://hooks.example.com/attest"),
        ("", "https://a/"),
        ("a space with spaces", "https://x.example.com:8443/p?q=1"),
    ] {
        assert_eq!(
            webhook::consent_bytes(&SpaceId(space.into()), url),
            ordo_verify_proof::proof::consent_bytes(space, url),
            "{space} / {url}"
        );
    }
}

// ── layer 4 ───────────────────────────────────────────────────────────────

async fn proof_of(pool: &Pool, from: u64, to: u64) -> String {
    let doc = ordo_host::attest_ops::build_proof(pool, &space(), from, to)
        .await
        .expect("proof");
    serde_json::to_string(&doc).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chain_proof_verifies_offline_and_stops_when_a_link_moves() {
    let pool = fresh("ordo_att_proof").await;
    attest(&pool, 1_757_003_600).await;
    let proof = proof_of(&pool, 0, 3).await;

    let v = ordo_verify_proof::verify_proof(&proof, Some(&chain_signer().verifier()), None)
        .expect("readable");
    assert!(v.ok(), "{:?}", v.findings);
    assert_eq!(v.links, 4);
    assert_eq!(
        v.markers, 1,
        "the attestation at position 4 is inside the range"
    );
    assert_eq!(v.verified_markers, 1);
    assert!(v.summary().starts_with("VERIFIED"));
    // The caveats print on success, and they name the two things this did
    // not establish.
    let caveats = v.caveats().join(" ");
    assert!(caveats.contains("digests, not command bodies"));
    assert!(caveats.contains("before the point you started watching"));

    // Pinning the head is what turns a consistency check into a rewrite
    // check.
    let ok =
        ordo_verify_proof::verify_proof(&proof, Some(&chain_signer().verifier()), Some(&v.head))
            .unwrap();
    assert!(ok.ok());
    let stale = ordo_verify_proof::verify_proof(
        &proof,
        Some(&chain_signer().verifier()),
        Some(&"aa".repeat(32)),
    )
    .unwrap();
    assert!(!stale.ok());
    assert!(stale.findings.iter().any(|f| f.step == "head"));

    // Move one link. The document still parses and still looks like a proof.
    let mut doc: serde_json::Value = serde_json::from_str(&proof).unwrap();
    doc["links"][2]["hash"] = serde_json::Value::String("bb".repeat(32));
    let tampered = serde_json::to_string(&doc).unwrap();
    let v =
        ordo_verify_proof::verify_proof(&tampered, Some(&chain_signer().verifier()), None).unwrap();
    assert!(!v.ok());
    assert!(v.findings.iter().any(|f| f.step == "link"));
    assert_eq!(v.code(), 3, "a failed verification is 3, never 1");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proof_without_a_key_says_unverified() {
    let pool = fresh("ordo_att_nokey").await;
    attest(&pool, 1_757_003_600).await;
    let proof = proof_of(&pool, 0, 3).await;
    let v = ordo_verify_proof::verify_proof(&proof, None, None).unwrap();
    // Structurally sound, and the report refuses to call that verified.
    assert!(v.ok());
    assert_eq!(v.code(), 0);
    assert!(v.structure_only);
    assert!(v.summary().contains("UNVERIFIED"));
    assert!(v.caveats()[0].starts_with("NO KEY WAS SUPPLIED"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_signature_from_a_key_you_do_not_trust_is_not_evidence() {
    let pool = fresh("ordo_att_wrongkey").await;
    attest(&pool, 1_757_003_600).await;
    let proof = proof_of(&pool, 0, 3).await;
    // A perfectly valid signature under a key the recipient never accepted.
    let other = Ed25519Signer::from_seed("attacker-2026", &[3u8; 32]);
    let v = ordo_verify_proof::verify_proof(&proof, Some(&other.verifier()), None).unwrap();
    assert!(!v.ok());
    assert!(v
        .findings
        .iter()
        .any(|f| f.step == "signature" && f.detail.contains("do not trust")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proof_and_the_export_bundle_agree_on_encoding() {
    // `plan-production-ops.md`'s R-track wants a chain proof and a bundle to
    // agree where they overlap. They overlap on exactly three fields, and
    // this is the assertion rather than the prose.
    let pool = fresh("ordo_att_encoding").await;
    attest(&pool, 1_757_003_600).await;
    let proof: serde_json::Value = serde_json::from_str(&proof_of(&pool, 0, 3).await).unwrap();

    let store =
        ordo_exec::commandlog::PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let entries = store.try_entries(&space(), 0, 4).await.unwrap();
    for (i, e) in entries.iter().enumerate() {
        let link = &proof["links"][i];
        assert_eq!(link["idx"], i as u64);
        // Lowercase hex, `ChainHash`'s `Display`, which is what
        // `export.rs::group_json` writes on every `commandlog.jsonl` line.
        assert_eq!(link["prev"], e.prev.to_string());
        assert_eq!(link["hash"], e.hash.to_string());
        assert_eq!(
            link["hash"].as_str().unwrap(),
            link["hash"].as_str().unwrap().to_ascii_lowercase(),
            "hex is lowercase in both artifacts"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_range_below_the_floor_is_proof_unavailable_and_not_not_found() {
    let pool = fresh("ordo_att_floor").await;
    attest(&pool, 1_757_003_600).await;
    // Past the head.
    let e = ordo_host::attest_ops::build_proof(&pool, &space(), 0, 99)
        .await
        .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::ProofUnavailable);
    assert!(e.message.contains("past the head"), "{}", e.message);
    // Below the retained floor — here, an index that was never written.
    let e = ordo_host::attest_ops::build_proof(&pool, &space(), 50, 60)
        .await
        .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::ProofUnavailable);
    assert!(
        e.message.contains("different answer from"),
        "the refusal distinguishes `gone` from `no such thing`: {}",
        e.message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_platform_refuses_to_publish_a_proof_over_its_own_broken_chain() {
    let pool = fresh("ordo_att_broken").await;
    attest(&pool, 1_757_003_600).await;
    // Somebody edits a stored hash. `verify_chain_from` catches it before a
    // proof leaves the building — which proves nothing to a third party, and
    // stops the deployment handing out a confident schema over a chain that
    // does not recompute.
    pool.rows(
        "UPDATE \"$commandlog\" SET \"hash\" = '\\xbb00000000000000000000000000000000000000000000000000000000000000'::bytea \
         WHERE \"idx\" = 2",
    )
    .await
    .unwrap();
    let e = ordo_host::attest_ops::build_proof(&pool, &space(), 0, 3)
        .await
        .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::ProofUnavailable);
    assert!(e.message.contains("does not recompute"), "{}", e.message);
}

// ── R2e: delivery after a restore ───────────────────────────────────────

/// Delete every chain entry at or above `from`: the rewind a restore leaves.
async fn rewind(pool: &Pool, from: usize) {
    pool.rows(&format!(
        "DELETE FROM \"$commandlog\" WHERE \"space\" = 'spc_att' AND \"idx\" >= {from}"
    ))
    .await
    .unwrap();
}

/// **A head at a position an orphaned attestation already holds is
/// delivered.**
///
/// The two mechanisms this closes were both position-keyed. `$attestations`
/// was `(space, upto)` with `ON CONFLICT DO NOTHING`, so the post-restore
/// head at a reused position was never recorded; and `enqueue_due` fanned
/// out only rows above the sink's cursor, so even a recorded one at a
/// position the cursor had passed was never sent. A receiver therefore never
/// heard the post-restore history at reused positions, which is exactly the
/// history R2d-iv asks them to compare against.
#[tokio::test(flavor = "multi_thread")]
async fn a_post_restore_head_at_a_reused_position_is_delivered() {
    let pool = fresh("ordo_att_reused").await;
    let (addr, mut rx) = listener("HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n").await;
    let upto = arm(&pool, &format!("https://localhost:{}/attest", addr.port())).await;
    assert_eq!(upto, 4);

    let health = DeliveryHealth::default();
    let metrics = Metrics::new();
    let cfg = config(space());
    let p = run_pass(&pool, &metrics, &cfg, &health, 1_757_003_601)
        .await
        .expect("the first delivery");
    assert_eq!(p.sent, 1);
    let first = rx.recv().await.expect("the receiver got the first head");
    assert_eq!(first.header("x-ordo-position").as_deref(), Some("4"));
    let first_head = head_of_payload(&first.body);

    // The restore: back to index 1, then grown again through the positions
    // the first attestation already holds. New correlations, because the
    // groups the restore discarded were different groups.
    rewind(&pool, 1).await;
    for i in 0..3 {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&group_at(10 + i, 1_757_100_000 + i as u64))
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    // And the cursor goes back to where history resumed, which is what
    // `undo::record_restore` does and what re-opens the window over the
    // positions the restore rewound through.
    assert!(webhook::reset_cursor(&pool, &space(), 1).await.unwrap());
    let head_now = head_hex(&pool, 3).await;
    assert_ne!(
        head_now, first_head,
        "the same position, a different history"
    );

    // Attesting again: the stored record at 4 is an orphan now, so it does
    // not cover this head and the cadence is not satisfied by it.
    match cadence::attest_due(
        &pool,
        &space(),
        &chain_signer(),
        &LogPolicy::DEFAULT,
        Timestamp(1_757_100_100),
    )
    .await
    .expect("attest")
    {
        Attested::Signed(a) => assert_eq!(a.upto, 4, "the same position, the new head"),
        other => panic!("expected a fresh signature, got {other:?}"),
    }
    assert_eq!(
        count_at(&pool, 4).await,
        2,
        "both heads are kept at position 4"
    );

    let p = run_pass(&pool, &metrics, &cfg, &health, 1_757_100_200)
        .await
        .expect("the second delivery");
    assert_eq!(
        p.sent, 1,
        "the outbox holds a second delivery for position 4"
    );
    let second = rx.recv().await.expect("the receiver got the second head");
    assert_eq!(second.header("x-ordo-position").as_deref(), Some("4"));
    assert_eq!(
        head_of_payload(&second.body),
        head_now,
        "the post-restore head"
    );

    // And the receiver can check it the way a receiver does.
    let verdict = ordo_verify_proof::verify_attestation(
        std::str::from_utf8(&second.body).unwrap(),
        Some(
            &ordo_commandlog::CheckpointVerifier::from_public_hex(
                "chain-key-2026",
                &chain_signer().verifier().public_hex(),
            )
            .unwrap(),
        ),
    )
    .expect("a payload");
    assert!(verdict.ok(), "{:?}", verdict.findings);
}

/// The orphan is **kept and not redelivered**. It was a true statement when
/// it was signed and it is not one now, so it stays in the table as evidence
/// and nothing sends it again.
#[tokio::test(flavor = "multi_thread")]
async fn an_orphaned_attestation_is_kept_and_not_redelivered() {
    let pool = fresh("ordo_att_orphan").await;
    let (addr, mut rx) = listener("HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n").await;
    arm(&pool, &format!("https://localhost:{}/attest", addr.port())).await;

    let health = DeliveryHealth::default();
    let metrics = Metrics::new();
    let cfg = config(space());
    run_pass(&pool, &metrics, &cfg, &health, 1_757_003_601)
        .await
        .expect("the first delivery");
    rx.recv().await.expect("the first head");

    // The restore, with no new history above the orphan's position: nothing
    // the chain agrees with is left at 4. The cursor goes back, so the
    // orphan is inside the window and is skipped on its merits rather than
    // by being out of range.
    rewind(&pool, 1).await;
    assert!(webhook::reset_cursor(&pool, &space(), 1).await.unwrap());
    let p = run_pass(&pool, &metrics, &cfg, &health, 1_757_100_200)
        .await
        .expect("a pass");
    assert_eq!(
        (p.sent, p.retrying, p.dead_lettered),
        (0, 0, 0),
        "an orphaned attestation is not sent again"
    );
    assert_eq!(
        count_at(&pool, 4).await,
        1,
        "and the row is still there: it is evidence of what was signed"
    );
}

/// `$attestations` rows at one position.
async fn count_at(pool: &Pool, upto: u64) -> i64 {
    pool.rows(&format!(
        "SELECT count(*) FROM \"$attestations\" WHERE \"space\" = 'spc_att' AND \"upto\" = {upto}"
    ))
    .await
    .unwrap()[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap()
}

/// The stored hash of the entry at `idx`, as lowercase hex.
async fn head_hex(pool: &Pool, idx: usize) -> String {
    pool.rows(&format!(
        "SELECT encode(\"hash\", 'hex') FROM \"$commandlog\" \
         WHERE \"space\" = 'spc_att' AND \"idx\" = {idx}"
    ))
    .await
    .unwrap()[0][0]
        .clone()
        .expect("an entry at this index")
}

/// The `head` an `ordo.attestation/1` body carries.
fn head_of_payload(body: &[u8]) -> String {
    let v: serde_json::Value = serde_json::from_slice(body).expect("json");
    v["head"].as_str().expect("head").to_string()
}

// ── the off switch and the read-backs (owner ruling 2026-10-05) ──────────

fn attest_ops(
    pool: &Pool,
    stream: Arc<ordo_secevent::Recorder>,
) -> ordo_host::attest_ops::AttestOps {
    ordo_host::attest_ops::AttestOps {
        pool: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        policy: LogPolicy::DEFAULT,
        stream: Some(stream),
        mint_secret: Arc::new(|| vec![7u8; 32]),
        now: Arc::new(|| 1_757_000_000),
    }
}

fn bound(pairs: &[(&str, &str)]) -> ordo_api::BoundParams {
    ordo_api::BoundParams(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect(),
    )
}

fn custody_actions(stream: &ordo_secevent::Recorder) -> Vec<String> {
    stream
        .drain_all(Timestamp(1_757_100_000))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::CustodyAction { action } => Some(action),
            _ => None,
        })
        .collect()
}

/// **The owner can switch the destination off and read it back.** `attestTo`
/// with no destination removes the sink and records it; the read op shows
/// the destination while it is set and nulls once it is gone, and never the
/// secret; a destination sent without the owner's signature is refused; and
/// removing nothing is `unchanged` and records nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_switches_the_destination_off_and_reads_it_back() {
    use ordo_host::attest_ops::{ATTESTATION, ATTESTATION_DEAD_LETTERS, ATTEST_TO};
    let pool = fresh("ordo_att_off").await;
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = attest_ops(&pool, stream.clone());
    let sp = space().0;

    // Nothing set: the read is a row of nulls.
    let read = ops.attestation(&sp).await.expect("read");
    assert_eq!(read.rows[0]["destination"], serde_json::Value::Null);
    assert_eq!(read.rows[0]["deadLetters"], serde_json::json!(0));

    // A destination without the owner's signature is refused.
    let refused = ops
        .attest_to(
            &sp,
            &bound(&[("destination", "https://hooks.example.invalid/attest")]),
            "usr_owner",
        )
        .await
        .expect_err("unsigned");
    assert_eq!(refused.code, ordo_api::ErrorCode::NotConfirmed);
    assert!(webhook::read_sink(&pool, &space()).await.unwrap().is_none());

    // Set, signed.
    let url = "https://hooks.example.invalid/attest";
    let canonical = ordo_egress::HttpsUrl::parse(url).unwrap().canonical();
    let o = owner();
    let sig = o.sign(&webhook::consent_bytes(&space(), &canonical));
    let sig_hex: String = sig.iter().map(|b| format!("{b:02x}")).collect();
    let set = ops
        .attest_to(
            &sp,
            &bound(&[
                ("destination", url),
                ("ownerKeyId", o.key_id()),
                ("ownerKey", &o.verifier().public_hex()),
                ("signature", &sig_hex),
            ]),
            "usr_owner",
        )
        .await
        .expect("set");
    assert_eq!(set.rows[0]["outcome"], "set");
    let secret = set.rows[0]["secret"].as_str().unwrap().to_string();

    let read = ops.attestation(&sp).await.expect("read");
    let row = &read.rows[0];
    assert_eq!(row["destination"], serde_json::json!(canonical));
    assert_eq!(row["destinationKeyId"], serde_json::json!(o.key_id()));
    assert_eq!(row["setAt"], serde_json::json!(1_757_000_000u64));
    assert!(
        !read.rows[0].to_string().contains(&secret),
        "the secret is shown once, at grant, and never read back"
    );

    // A signature with no destination is a set that lost its destination,
    // never a removal: refused, and the sink stands.
    let _ = custody_actions(&stream);
    let stray = ops
        .attest_to(
            &sp,
            &bound(&[("ownerKeyId", o.key_id()), ("signature", "00")]),
            "usr_owner",
        )
        .await
        .expect_err("a signature with no destination confirms nothing");
    assert_eq!(stray.code, ordo_api::ErrorCode::NotConfirmed);
    assert!(webhook::read_sink(&pool, &space()).await.unwrap().is_some());
    assert!(custody_actions(&stream).is_empty());

    // Off: no destination.
    let off = ops
        .attest_to(&sp, &bound(&[]), "usr_owner")
        .await
        .expect("removed");
    assert_eq!(off.rows[0]["outcome"], "removed");
    assert_eq!(off.rows[0]["destination"], serde_json::Value::Null);
    assert!(webhook::read_sink(&pool, &space()).await.unwrap().is_none());
    assert_eq!(
        custody_actions(&stream),
        vec!["attestation_destination_removed".to_string()]
    );
    let read = ops.attestation(&sp).await.expect("read");
    assert_eq!(read.rows[0]["destination"], serde_json::Value::Null);

    // Again: unchanged, and nothing recorded. An empty destination is the
    // same as none.
    let again = ops
        .attest_to(&sp, &bound(&[("destination", "")]), "usr_owner")
        .await
        .expect("unchanged");
    assert_eq!(again.rows[0]["outcome"], "unchanged");
    assert!(custody_actions(&stream).is_empty());

    // The policy: all three are the owner's, at the door.
    for op in [ATTEST_TO, ATTESTATION, ATTESTATION_DEAD_LETTERS] {
        assert!(ordo_host::platform_ops::owner_only(op), "{op}");
    }
}

/// **Dead letters are readable by the tenant owner and the deployment
/// owner, and nothing retries them.** The tenant's read shows the position,
/// the destination, the attempts and the kind of failure, never the stored
/// reason, which describes the deployment's network. The deployment-wide
/// listing carries every tenant's, newest first, with the reason. Removing
/// the destination keeps them: they are the record of what failed.
#[tokio::test(flavor = "multi_thread")]
async fn dead_letters_are_read_by_the_tenant_owner_and_the_deployment_owner() {
    let pool = fresh("ordo_att_deadread").await;
    arm(&pool, "https://hooks.example.invalid/attest").await;
    let health = DeliveryHealth::default();
    let metrics = Metrics::new();
    let cfg = config(space());
    let mut now = 1_757_003_601u64;
    for _ in 0..6 {
        run_pass(&pool, &metrics, &cfg, &health, now)
            .await
            .expect("pass");
        now += 128;
    }
    let stored = webhook::dead_letters(&pool, &space()).await.unwrap();
    assert_eq!(stored.len(), 1);

    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = attest_ops(&pool, stream);
    let listed = ops.dead_letters(&space().0).await.expect("listed");
    assert_eq!(listed.rows.len(), 1);
    let row = &listed.rows[0];
    assert_eq!(row["upto"], serde_json::json!(stored[0].upto));
    assert_eq!(row["attempts"], serde_json::json!(stored[0].attempts));
    assert_eq!(
        row["kind"],
        serde_json::json!(ordo_host::attest_ops::failure_kind(&stored[0].reason))
    );
    assert!(row.get("reason").is_none(), "{row}");
    let read = ops.attestation(&space().0).await.expect("read");
    assert_eq!(read.rows[0]["deadLetters"], serde_json::json!(1));

    let (total, recent) = webhook::recent_dead_letters(&pool, 10).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(recent[0].space, space());
    assert_eq!(recent[0].reason, stored[0].reason);

    // Off, and the dead letters stay.
    ops.attest_to(&space().0, &bound(&[]), "usr_owner")
        .await
        .expect("removed");
    assert_eq!(ops.dead_letters(&space().0).await.unwrap().rows.len(), 1);
}

#[test]
fn a_failure_kind_is_a_fixed_word_and_never_the_detail() {
    use ordo_host::attest_ops::failure_kind;
    assert_eq!(
        failure_kind("denied_address: `h` -> 10.0.0.4 (private)"),
        "denied_address"
    );
    assert_eq!(
        failure_kind("connect_failed: dial 10.0.0.4"),
        "connect_failed"
    );
    assert_eq!(
        failure_kind("the sink secret is not hex"),
        "delivery_failed"
    );
    assert_eq!(failure_kind(""), "delivery_failed");
}
