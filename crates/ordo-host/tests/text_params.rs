//! A text parameter reaches the row exactly as the caller sent it
//! (audit 2026-10-08).
//!
//! The executor carries a call to the write protocol as source text, so a
//! text value rides in a string literal. It used to strip every `"` from
//! the value and leave `\` unescaped, so `He said "hi"` was stored as
//! `He said hi` and a value holding a backslash failed to parse. The
//! literal is now escaped with the language's closed escape set, as the
//! import door already did.
//!
//! A regression looks like a quote, a backslash, a newline or a tab going
//! missing or changing between a write and the read that follows it, on an
//! open field or a sealed one.
//!
//! Needs the conformance cluster. Its own database, prefixed `ordo_text_`.

mod refine_support;

use axum::http::StatusCode;
use refine_support::{call, door};
use serde_json::json;

#[tokio::test]
async fn a_text_parameter_keeps_its_quotes_backslashes_newlines_and_tabs() {
    let (state, _) = door("ordo_text_params_exact", 50).await;
    let values = [
        "He said \"hi\"",
        "C:\\path\\n is two characters, not a newline",
        "line one\nline two\tand a tab",
        "ends in a backslash \\",
        "a \"quoted\" \\\"mix\\\" of both",
    ];
    for (i, v) in values.iter().enumerate() {
        let label = format!("{i} {v}");
        let (status, body) = call(
            &state,
            "POST",
            "/op/addJotting",
            Some(json!({"label": label, "body": v})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{v:?}: {body}");
    }
    let (status, body) = call(&state, "GET", "/op/jotList", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), values.len(), "{body}");
    for (i, v) in values.iter().enumerate() {
        let row = rows
            .iter()
            .find(|r| {
                r["label"]
                    .as_str()
                    .is_some_and(|l| l.starts_with(&format!("{i} ")))
            })
            .unwrap_or_else(|| panic!("row {i} is missing: {body}"));
        assert_eq!(row["label"], format!("{i} {v}"), "the open field, {v:?}");
        assert_eq!(row["body"], *v, "the sealed field, {v:?}");
    }
}
