//! The mail channel's API kinds, Resend and SendGrid (`plan-auth-federated.md`
//! step 4, §7.6): one JSON `POST` with a bearer key through the egress pins,
//! to a real TLS listener standing in for the provider.

mod mail_support;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use mail_support::{deny, roots, FakeApi, FROM, TO};
use ordo_egress::HttpsUrl;
use ordo_host::mail::{ApiKey, ApiMailer, MailAddress, MailApi, MailError, Mailer, Message};

const KEY: &str = "re_test_9d1f-never-printed";

fn mailer(api: MailApi, fake: &FakeApi) -> Arc<dyn Mailer> {
    Arc::new(ApiMailer::at(
        api,
        HttpsUrl::parse(&fake.url("/emails")).unwrap(),
        roots(),
        ApiKey::new(KEY).unwrap(),
        MailAddress::parse(FROM).unwrap(),
        deny(),
    ))
}

fn message() -> Message {
    Message::new(
        MailAddress::parse(TO).unwrap(),
        "Your sign-in code",
        "Your code is 12345678.",
    )
    .unwrap()
}

async fn submit(m: Arc<dyn Mailer>) -> Result<(), MailError> {
    tokio::task::spawn_blocking(move || m.submit(&message()))
        .await
        .expect("send thread")
}

#[tokio::test(flavor = "multi_thread")]
async fn one_message_is_posted_with_a_bearer_key_and_an_accepted_status_is_the_only_success() {
    // Resend: 200 is accepted.
    let resend = FakeApi::start(200).await;
    submit(mailer(MailApi::Resend, &resend))
        .await
        .expect("accepted");
    let seen = resend.seen();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].head.starts_with("POST /emails HTTP/1.1"),
        "{}",
        seen[0].head
    );
    assert_eq!(
        seen[0].header("authorization").as_deref(),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(
        seen[0].header("content-type").as_deref(),
        Some("application/json")
    );
    let body: serde_json::Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(body["from"], FROM);
    assert_eq!(body["to"][0], TO);
    assert_eq!(body["subject"], "Your sign-in code");
    assert_eq!(body["text"], "Your code is 12345678.");

    // Another 2xx is not counted as sent, and is not retried either: it may
    // have been sent, and a retry would send it twice.
    resend.status.store(201, Ordering::SeqCst);
    let err = submit(mailer(MailApi::Resend, &resend))
        .await
        .expect_err("201 is not Resend's accepted");
    assert!(err.is_permanently_refused(), "{err}");
    assert_eq!(err.relay_code(), Some(201));
    assert!(!err.retryable());

    // SendGrid: 202 is accepted, and 200 is not.
    let sendgrid = FakeApi::start(202).await;
    submit(mailer(MailApi::SendGrid, &sendgrid))
        .await
        .expect("accepted");
    let body: serde_json::Value = serde_json::from_slice(&sendgrid.seen()[0].body).unwrap();
    assert_eq!(body["personalizations"][0]["to"][0]["email"], TO);
    assert_eq!(body["from"]["email"], FROM);
    assert_eq!(body["content"][0]["value"], "Your code is 12345678.");
    sendgrid.status.store(200, Ordering::SeqCst);
    assert!(submit(mailer(MailApi::SendGrid, &sendgrid)).await.is_err());

    // A 5xx is retried; a 4xx and a redirect are final, and a redirect is
    // never followed.
    for (status, retryable) in [(503, true), (401, false), (302, false)] {
        let api = FakeApi::start(status).await;
        let err = submit(mailer(MailApi::Resend, &api))
            .await
            .expect_err("not accepted");
        assert_eq!(err.retryable(), retryable, "{status}: {err}");
        assert_eq!(api.seen().len(), 1, "{status}: one request, no second hop");
        assert!(!err.to_string().contains(KEY), "{err}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rate_limit_is_waited_out_never_counted_as_a_refusal() {
    // Resend and SendGrid answer 429 when the account's rate is reached.
    // The message stays queued and is tried again, after the wait the
    // provider names.
    let api = FakeApi::start(429).await;
    *api.retry_after.lock().unwrap() = Some("120".into());
    let err = submit(mailer(MailApi::Resend, &api))
        .await
        .expect_err("rate limited");
    assert!(err.retryable(), "{err}");
    assert!(err.is_deferred(), "{err}");
    assert_eq!(err.relay_code(), Some(429));
    assert!(std::error::Error::source(&err).is_some());
    assert_eq!(err.retry_after_secs(), Some(120));

    // Without a Retry-After, still retried, on the outbox's own backoff.
    for status in [429, 408, 500, 503] {
        let api = FakeApi::start(status).await;
        let err = submit(mailer(MailApi::SendGrid, &api))
            .await
            .expect_err("not accepted");
        assert!(err.retryable(), "{status}: {err}");
        assert_eq!(err.retry_after_secs(), None, "{status}");
    }

    // A real refusal is still final.
    for status in [400, 422] {
        let api = FakeApi::start(status).await;
        let err = submit(mailer(MailApi::Resend, &api))
            .await
            .expect_err("refused");
        assert!(err.is_permanently_refused(), "{status}: {err}");
        assert_eq!(err.relay_code(), Some(status));
    }
    // A key the provider does not accept is the deployment's setting.
    for status in [401, 403] {
        let api = FakeApi::start(status).await;
        let err = submit(mailer(MailApi::Resend, &api))
            .await
            .expect_err("not authorised");
        assert!(
            err.is_misconfigured() && !err.retryable(),
            "{status}: {err}"
        );
        assert_eq!(err.relay_code(), Some(status));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_key_appears_in_no_label_and_no_error() {
    let api = FakeApi::start(500).await;
    let m = mailer(MailApi::SendGrid, &api);
    assert!(!m.label().contains(KEY));
    let err = submit(m).await.expect_err("500");
    assert!(!format!("{err:?}").contains(KEY));
    assert!(ApiKey::new("has a space").is_err());
    assert!(ApiKey::new("line\r\nbreak").is_err());
}
