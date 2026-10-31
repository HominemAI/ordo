//! The mail channel's SMTP relay (`plan-auth-federated.md` step 4, §7.6):
//! one message over implicit TLS or required STARTTLS to a real listener, through the egress
//! pins, and the refusals that keep a message, a credential or an address
//! from going anywhere it should not.

mod mail_support;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use mail_support::{
    deny, roots, FakeRelay, PlainRelay, StartTlsMode, StartTlsRelay, FROM, PASSWORD, PLAIN, TO,
    USER,
};
use ordo_egress::smtp::{self, Mail, SmtpLogin, SmtpTls};
use ordo_host::mail::{
    MailAddress, MailChannel, MailError, Mailer, Message, SmtpMailer, CREDENTIALS_FILE_ENV,
    FROM_ENV, HOST_ENV, KIND_ENV, PORT_ENV, TLS_ENV,
};

fn mailer(port: u16) -> Arc<dyn Mailer> {
    mailer_over(port, SmtpTls::Implicit)
}

fn mailer_over(port: u16, tls: SmtpTls) -> Arc<dyn Mailer> {
    Arc::new(
        SmtpMailer::with_roots(
            "localhost",
            port,
            MailAddress::parse(FROM).unwrap(),
            Some(SmtpLogin::new(USER, PASSWORD).unwrap()),
            roots(),
            deny(),
        )
        .with_tls(tls),
    )
}

/// Nothing that matters was written in plaintext.
fn no_secret_in(text: &str) {
    for never in ["AUTH", "MAIL FROM", "RCPT TO", PLAIN, PASSWORD, TO] {
        assert!(
            !text.contains(never),
            "`{never}` was sent in plaintext: {text}"
        );
    }
}

fn code_message() -> Message {
    Message::new(
        MailAddress::parse(TO).unwrap(),
        "Your sign-in code",
        "Your code is 12345678.\n.\nIf you did not ask for it, ignore this message.\n",
    )
    .unwrap()
}

/// `Mailer::submit` blocks on the runtime, as it does on the worker's
/// blocking thread.
async fn submit(m: Arc<dyn Mailer>, msg: Message) -> Result<(), MailError> {
    tokio::task::spawn_blocking(move || m.submit(&msg))
        .await
        .expect("send thread")
}

#[tokio::test(flavor = "multi_thread")]
async fn one_message_is_submitted_over_tls_to_a_real_listener() {
    let relay = FakeRelay::start(None).await;
    submit(mailer(relay.port), code_message())
        .await
        .expect("accepted");

    let sessions = relay.sessions_after(1).await;
    assert_eq!(sessions.len(), 1, "one connection, one message");
    let s = &sessions[0];
    assert_eq!(
        s.commands,
        vec![
            "EHLO example.com".to_string(),
            format!("AUTH PLAIN {PLAIN}"),
            format!("MAIL FROM:<{FROM}>"),
            format!("RCPT TO:<{TO}>"),
            "DATA".to_string(),
            "QUIT".to_string(),
        ]
    );
    let data = s.data.as_deref().expect("a message");
    assert!(data.contains(&format!("\r\nTo: {TO}\r\n")), "{data}");
    assert!(
        data.contains("\r\nSubject: Your sign-in code\r\n"),
        "{data}"
    );
    assert!(data.contains("Your code is 12345678."), "{data}");
    // A line that is a lone dot is doubled, so it cannot end the message.
    assert!(data.contains("\r\n..\r\n"), "{data}");
    assert_eq!(data.matches(TO).count(), 1, "the address is in one header");
}

#[tokio::test(flavor = "multi_thread")]
async fn any_port_boots_and_an_unknown_tls_mode_names_the_choices() {
    let files = |_: &str| Ok::<_, String>(format!("{USER}\n{PASSWORD}\n"));
    for (port, tls) in [
        ("25", "starttls"),
        ("587", "starttls"),
        ("2525", "implicit"),
        ("1025", "none"),
    ] {
        let vars = |k: &str| match k {
            KIND_ENV => Some("smtp".to_string()),
            FROM_ENV => Some(FROM.to_string()),
            HOST_ENV => Some("smtp.example.com".to_string()),
            PORT_ENV => Some(port.to_string()),
            TLS_ENV => Some(tls.to_string()),
            CREDENTIALS_FILE_ENV => Some("/run/secrets/mail".to_string()),
            _ => None,
        };
        let ch = MailChannel::from_vars(&vars, &files)
            .expect("any port boots")
            .expect("on");
        assert!(ch.label.contains(&format!(":{port} ")), "{}", ch.label);
        assert!(!ch.label.contains(PASSWORD), "{}", ch.label);
    }
    let vars = |k: &str| match k {
        KIND_ENV => Some("smtp".to_string()),
        FROM_ENV => Some(FROM.to_string()),
        HOST_ENV => Some("smtp.example.com".to_string()),
        TLS_ENV => Some("plain".to_string()),
        CREDENTIALS_FILE_ENV => Some("/run/secrets/mail".to_string()),
        _ => None,
    };
    let (var, detail) = MailChannel::from_vars(&vars, &files)
        .err()
        .expect("not a mode");
    assert_eq!(var, TLS_ENV);
    for word in ["ORDO_MAIL_TLS", "implicit", "starttls", "none"] {
        assert!(detail.contains(word), "{detail}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn starttls_upgrades_before_the_credential_and_asks_again_inside() {
    let relay = StartTlsRelay::start(StartTlsMode::Honest).await;
    submit(mailer_over(relay.port, SmtpTls::StartTls), code_message())
        .await
        .expect("accepted");
    let sessions = relay.sessions_after(1).await;
    assert_eq!(sessions.len(), 1, "one connection, one message");
    assert_eq!(
        sessions[0].commands,
        vec![
            "EHLO example.com".to_string(),
            "STARTTLS".to_string(),
            "EHLO example.com".to_string(),
            format!("AUTH PLAIN {PLAIN}"),
            format!("MAIL FROM:<{FROM}>"),
            format!("RCPT TO:<{TO}>"),
            "DATA".to_string(),
            "QUIT".to_string(),
        ],
        "EHLO and STARTTLS alone before TLS, and EHLO again inside it"
    );
    assert_eq!(
        relay.plaintext(),
        "EHLO example.com\r\nSTARTTLS\r\n",
        "nothing else crossed in plaintext"
    );
    assert!(sessions[0].data.as_deref().unwrap().contains("12345678"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_that_does_not_offer_starttls_is_sent_no_credential() {
    let relay = StartTlsRelay::start(StartTlsMode::NotOffered).await;
    let err = submit(mailer_over(relay.port, SmtpTls::StartTls), code_message())
        .await
        .expect_err("no STARTTLS, no message");
    assert!(err.to_string().contains("tls_failed"), "{err}");
    assert!(err.to_string().contains("STARTTLS"), "{err}");
    // The log line names the settings that would fix it.
    for fix in ["ORDO_MAIL_TLS=none", "port 465 with ORDO_MAIL_TLS=implicit"] {
        assert!(err.to_string().contains(fix), "{err}");
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let seen = relay.plaintext();
    assert_eq!(seen, "EHLO example.com\r\n");
    no_secret_in(&seen);
    assert_eq!(relay.handshakes.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn bytes_injected_after_the_starttls_answer_are_refused() {
    let relay = StartTlsRelay::start(StartTlsMode::Injects).await;
    let err = submit(mailer_over(relay.port, SmtpTls::StartTls), code_message())
        .await
        .expect_err("injected bytes, no message");
    assert!(err.to_string().contains("tls_failed"), "{err}");
    assert!(err.to_string().contains("ahead of its turn"), "{err}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    no_secret_in(&relay.plaintext());
    assert_eq!(relay.handshakes.load(Ordering::SeqCst), 0, "no handshake");
    assert!(relay.sessions.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plaintext_relay_on_starttls_is_sent_no_credential() {
    // One that answers 220 to STARTTLS and stays in plaintext: the
    // handshake fails and nothing follows it.
    let relay = StartTlsRelay::start(StartTlsMode::StaysPlain).await;
    let err = submit(mailer_over(relay.port, SmtpTls::StartTls), code_message())
        .await
        .expect_err("no TLS, no message");
    assert!(err.retryable(), "{err}");
    assert!(err.to_string().contains("tls_failed"), "{err}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    no_secret_in(&relay.plaintext());

    // And one that answers every line with `250 ok`: no STARTTLS is offered.
    let plain = PlainRelay::start().await;
    let err = submit(mailer_over(plain.port, SmtpTls::StartTls), code_message())
        .await
        .expect_err("no TLS, no message");
    assert!(err.to_string().contains("tls_failed"), "{err}");
    let seen = plain.received.lock().unwrap().clone();
    no_secret_in(&String::from_utf8_lossy(&seen));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_that_answers_in_plaintext_is_sent_no_credential() {
    let relay = PlainRelay::start().await;
    let err = submit(mailer(relay.port), code_message())
        .await
        .expect_err("no TLS, no message");
    assert!(err.retryable(), "a failed handshake may pass: {err}");
    assert!(err.to_string().contains("tls_failed"), "{err}");
    for fix in ["ORDO_MAIL_TLS=starttls", "ORDO_MAIL_TLS=none"] {
        assert!(err.to_string().contains(fix), "{err}");
    }
    let seen = relay.received.lock().unwrap().clone();
    let text = String::from_utf8_lossy(&seen);
    for never in ["AUTH", "MAIL FROM", PLAIN, PASSWORD, TO] {
        assert!(!text.contains(never), "`{never}` reached a plaintext relay");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_with_a_line_break_is_refused_before_a_socket_opens() {
    // At the host's door: the address has no type to travel in.
    for bad in [
        "kim@example.org\r\nBcc: everyone@example.org",
        "kim@example.org\n",
        "Kim <kim@example.org>",
    ] {
        assert!(MailAddress::parse(bad).is_err(), "{bad:?}");
    }
    // And at the egress door, for a caller that skips the host's type: the
    // listener is never dialled.
    let relay = FakeRelay::start(None).await;
    let addr = tokio::net::lookup_host(("localhost", relay.port))
        .await
        .unwrap()
        .next()
        .unwrap();
    let dest = ordo_egress::dest::pin_for_test("localhost", addr, "");
    let login = SmtpLogin::new(USER, PASSWORD).unwrap();
    let err = smtp::submit(
        dest,
        &roots(),
        Some(&login),
        Mail {
            from: FROM,
            to: "kim@example.org\r\nRCPT TO:<everyone@example.org>",
            subject: "Your sign-in code",
            text: "12345678",
            date: 0,
            message_id: None,
        },
        Duration::from_secs(5),
    )
    .await
    .expect_err("refused");
    assert_eq!(err.kind(), "not_representable");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(relay.accepted.load(Ordering::SeqCst), 0, "no socket opened");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_recipient_is_final_and_a_busy_relay_is_retried() {
    let refused = FakeRelay::start(Some(("RCPT TO", 550))).await;
    let err = submit(mailer(refused.port), code_message())
        .await
        .expect_err("550");
    assert!(!err.retryable(), "{err}");
    assert!(err.to_string().contains("550"), "{err}");
    assert!(
        !err.to_string().contains(TO),
        "the relay's words are not kept"
    );
    assert!(
        refused.sessions_after(1).await[0].data.is_none(),
        "no DATA after a 550"
    );

    let busy = FakeRelay::start(Some(("MAIL FROM", 451))).await;
    let err = submit(mailer(busy.port), code_message())
        .await
        .expect_err("451");
    assert!(err.retryable(), "a 4xx asks for a later attempt: {err}");

    let auth = FakeRelay::start(Some(("AUTH", 535))).await;
    let err = submit(mailer(auth.port), code_message())
        .await
        .expect_err("535");
    assert!(!err.retryable());
    assert!(!err.to_string().contains(PASSWORD), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_inside_the_perimeter_is_refused_by_the_deny_list() {
    // The same listener, with the deployment's real deny list: loopback is
    // refused before a connection is made.
    let relay = FakeRelay::start(None).await;
    let m: Arc<dyn Mailer> = Arc::new(SmtpMailer::with_roots(
        "localhost",
        relay.port,
        MailAddress::parse(FROM).unwrap(),
        Some(SmtpLogin::new(USER, PASSWORD).unwrap()),
        roots(),
        ordo_egress::DenyList::categories_only(),
    ));
    let err = submit(m, code_message()).await.expect_err("denied");
    assert!(err.to_string().contains("denied_address"), "{err}");
    assert!(
        err.to_string()
            .contains("ORDO_EGRESS_PRIVATE_HOSTS=localhost"),
        "{err}"
    );
    assert_eq!(relay.accepted.load(Ordering::SeqCst), 0);
}

fn mailer_with_deny(port: u16, tls: SmtpTls, deny: ordo_egress::DenyList) -> Arc<dyn Mailer> {
    Arc::new(
        SmtpMailer::with_roots(
            "localhost",
            port,
            MailAddress::parse(FROM).unwrap(),
            Some(SmtpLogin::new(USER, PASSWORD).unwrap()),
            roots(),
            deny,
        )
        .with_tls(tls),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_private_host_is_reached_only_when_it_is_named() {
    let relay = FakeRelay::start(None).await;
    // Named: the real deny list, no test permission, and the name on it.
    let named = ordo_egress::DenyList::categories_only().with_private_hosts(
        &ordo_egress::DenyList::parse_private_hosts("mailpit.internal, LocalHost").unwrap(),
    );
    submit(
        mailer_with_deny(relay.port, SmtpTls::Implicit, named),
        code_message(),
    )
    .await
    .expect("a named private host is reached");
    assert_eq!(relay.sessions_after(1).await.len(), 1);

    // Another name on the list does not stand for this one.
    let other = ordo_egress::DenyList::categories_only().with_private_hosts(
        &ordo_egress::DenyList::parse_private_hosts("mailpit.internal").unwrap(),
    );
    let err = submit(
        mailer_with_deny(relay.port, SmtpTls::Implicit, other),
        code_message(),
    )
    .await
    .expect_err("not on the list");
    assert!(err.to_string().contains("denied_address"), "{err}");
    assert_eq!(relay.accepted.load(Ordering::SeqCst), 1, "no second dial");
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_none_sends_to_a_plaintext_relay_when_named() {
    let relay = FakeRelay::start_plaintext().await;
    submit(mailer_over(relay.port, SmtpTls::None), code_message())
        .await
        .expect("accepted in plaintext");
    let sessions = relay.sessions_after(1).await;
    assert_eq!(
        sessions[0].commands,
        vec![
            "EHLO example.com".to_string(),
            format!("AUTH PLAIN {PLAIN}"),
            format!("MAIL FROM:<{FROM}>"),
            format!("RCPT TO:<{TO}>"),
            "DATA".to_string(),
            "QUIT".to_string(),
        ]
    );
    // The boot line says plainly what `none` costs.
    let files = |_: &str| Ok::<_, String>(format!("{USER}\n{PASSWORD}\n"));
    let vars = |k: &str| match k {
        KIND_ENV => Some("smtp".to_string()),
        FROM_ENV => Some(FROM.to_string()),
        HOST_ENV => Some("localhost".to_string()),
        PORT_ENV => Some("1025".to_string()),
        TLS_ENV => Some("none".to_string()),
        CREDENTIALS_FILE_ENV => Some("/run/secrets/mail".to_string()),
        "ORDO_EGRESS_PRIVATE_HOSTS" => Some("localhost".to_string()),
        _ => None,
    };
    let ch = MailChannel::from_vars(&vars, &files).unwrap().unwrap();
    assert!(ch.label.contains("NO TLS"), "{}", ch.label);
    assert!(ch.label.contains("readable"), "{}", ch.label);
    assert!(
        ch.label.contains("ORDO_EGRESS_PRIVATE_HOSTS"),
        "{}",
        ch.label
    );
    assert!(!ch.label.contains(PASSWORD), "{}", ch.label);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_that_wants_tls_on_none_names_the_fix() {
    // A plaintext relay that answers AUTH with 530: it wants TLS first.
    let relay = FakeRelay::start_plaintext_failing(Some(("AUTH", 530))).await;
    let err = submit(mailer_over(relay.port, SmtpTls::None), code_message())
        .await
        .expect_err("530");
    assert!(err.to_string().contains("530"), "{err}");
    assert!(err.to_string().contains("ORDO_MAIL_TLS=starttls"), "{err}");
    assert!(!err.to_string().contains(PASSWORD), "{err}");
}

fn mailer_no_login(port: u16) -> Arc<dyn Mailer> {
    Arc::new(
        SmtpMailer::with_roots(
            "localhost",
            port,
            MailAddress::parse(FROM).unwrap(),
            None,
            roots(),
            deny(),
        )
        .with_tls(SmtpTls::None),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn without_credentials_no_auth_is_sent() {
    let relay = FakeRelay::start_plaintext().await;
    submit(mailer_no_login(relay.port), code_message())
        .await
        .expect("accepted with no login");
    let sessions = relay.sessions_after(1).await;
    assert_eq!(
        sessions[0].commands,
        vec![
            "EHLO example.com".to_string(),
            format!("MAIL FROM:<{FROM}>"),
            format!("RCPT TO:<{TO}>"),
            "DATA".to_string(),
            "QUIT".to_string(),
        ]
    );
    // The boot line says so, and the channel boots with no credentials file.
    let files = |_: &str| Err::<String, _>("no file is read".to_string());
    let vars = |k: &str| match k {
        KIND_ENV => Some("smtp".to_string()),
        FROM_ENV => Some(FROM.to_string()),
        HOST_ENV => Some("localhost".to_string()),
        PORT_ENV => Some("1025".to_string()),
        TLS_ENV => Some("none".to_string()),
        "ORDO_EGRESS_PRIVATE_HOSTS" => Some("localhost".to_string()),
        _ => None,
    };
    let ch = MailChannel::from_vars(&vars, &files).unwrap().unwrap();
    assert!(ch.label.contains("no login"), "{}", ch.label);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_that_demands_auth_names_the_credentials_file() {
    let relay = FakeRelay::start_plaintext_failing(Some(("MAIL FROM", 530))).await;
    let err = submit(mailer_no_login(relay.port), code_message())
        .await
        .expect_err("530");
    assert!(err.to_string().contains("530"), "{err}");
    assert!(err.to_string().contains(CREDENTIALS_FILE_ENV), "{err}");
}
