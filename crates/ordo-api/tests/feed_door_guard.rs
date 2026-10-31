//! Every long-lived read asks the whole door through one function,
//! `router::Door` (AGENTS.md rule 9; security audit 2026-10-24, F-2).
//!
//! The sync once re-asked the access check alone and missed the token gate,
//! so a withdrawn delegation kept its feed. This guard fails when a feed path
//! calls either half of the door on its own, or stops asking the door.

const SYNC: &str = include_str!("../src/sync.rs");
const LIVE: &str = include_str!("../src/live.rs");
const ROUTER: &str = include_str!("../src/router.rs");

/// The source of the function that starts with `head`, up to the next item.
fn body<'a>(src: &'a str, head: &str) -> &'a str {
    let start = src.find(head).unwrap_or_else(|| panic!("`{head}` exists"));
    let rest = &src[start + head.len()..];
    let end = [
        "\nfn ",
        "\nasync fn ",
        "\npub(crate) fn ",
        "\npub(crate) async fn ",
        "\n///",
    ]
    .iter()
    .filter_map(|m| rest.find(m))
    .min()
    .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn feed_paths_never_call_half_the_door() {
    for (name, src) in [("sync.rs", SYNC), ("live.rs", LIVE)] {
        for half in ["check_access(", "token_refusal(", "token_gate("] {
            assert!(
                !src.contains(half),
                "{name} calls `{half}` directly; ask `router::Door` instead"
            );
        }
    }
    let live_inner = body(ROUTER, "async fn live_inner(");
    for half in ["check_access(", "token_refusal("] {
        assert!(
            !live_inner.contains(half),
            "live_inner calls `{half}` directly; ask `router::Door` instead"
        );
    }
}

#[test]
fn every_feed_kind_asks_the_door_on_each_pass_and_on_a_tick() {
    // Sync: before the first read, on every pass, and on the idle tick.
    let start = body(SYNC, "pub(crate) async fn start(");
    assert!(
        start.contains("door.admit("),
        "sync asks the door before its first read"
    );
    let run = body(SYNC, "async fn run(");
    assert!(
        run.matches("door.admit(").count() >= 2,
        "sync asks the door on every pass and on the tick"
    );
    assert!(run.contains("recheck_ticker("), "sync has an idle tick");

    // `?live=1`: before every invalidate and on the tick.
    let live_inner = body(ROUTER, "async fn live_inner(");
    assert!(live_inner.contains("Door::new("), "?live=1 holds a door");
    assert!(
        live_inner.matches("door.admit(").count() >= 2,
        "?live=1 asks the door per signal and on the tick"
    );

    // `/live`: per signal, and the tick's re-check.
    assert!(
        LIVE.contains("feed.door.admit("),
        "/live asks the door per signal"
    );
    assert!(LIVE.contains("feed.recheck()"), "/live re-checks on a tick");
    let open = body(ROUTER, "async fn live_open(");
    assert!(open.contains("Door::new("), "/live is opened with a door");

    // The door itself is both halves.
    let admit = body(ROUTER, "pub(crate) async fn admit(");
    assert!(admit.contains("self.token(") && admit.contains("self.access("));
}
