//! Ops from functions (`plan-backend-functions.md` increment 4):
//! `ctx.op(name, args)` calls an op in `uses` through the seam, the host
//! refuses a name not in `uses` before the seam is asked, a refusal keeps
//! its class only when the host issued it, and a sealed field reaches the
//! function as a handle that `ctx.sealed` puts in a header.

use std::sync::{Arc, Mutex};

use ordo_fn::{
    FetchError, FnCall, FnError, FnFetch, FnFetchRequest, FnFetchResponse, FnHost, FnOpAnswer,
    FnOps, HeaderValue, HostConfig, Limits, NoScrub, OpError,
};
use serde_json::{json, Map, Value};

/// Answers `recentTurns` with two rows and `myAccount` with a sealed
/// token; refuses `adminOnly` as the person's access would; keeps every
/// call it was asked for.
struct Ops {
    asked: Mutex<Vec<(String, Map<String, Value>)>>,
}

#[async_trait::async_trait]
impl FnOps for Ops {
    async fn call(&self, op: &str, args: Map<String, Value>) -> Result<FnOpAnswer, OpError> {
        self.asked.lock().unwrap().push((op.to_string(), args));
        match op {
            "recentTurns" => Ok(FnOpAnswer {
                rows: vec![json!({ "text": "hi" }), json!({ "text": "hello" })],
                sealed: Vec::new(),
            }),
            "addTurns" => Ok(FnOpAnswer {
                rows: vec![json!({ "id": "turn_1" })],
                sealed: Vec::new(),
            }),
            "myAccount" => Ok(FnOpAnswer {
                rows: vec![json!({ "email": "ana@example.com", "token": "ya29.person-token" })],
                sealed: vec!["token".into()],
            }),
            "adminOnly" => Err(OpError::Denied { op: op.into() }),
            other => panic!("the double has no op {other}"),
        }
    }
}

/// The far end, keeping what it was sent.
struct Far {
    seen: Mutex<Vec<FnFetchRequest>>,
}

#[async_trait::async_trait]
impl FnFetch for Far {
    async fn fetch(
        &self,
        _space: &str,
        req: FnFetchRequest,
    ) -> Result<FnFetchResponse, FetchError> {
        self.seen.lock().unwrap().push(req);
        Ok(FnFetchResponse {
            status: 200,
            headers: Vec::new(),
            body: b"{\"items\":[]}".to_vec(),
        })
    }

    async fn mac(
        &self,
        _space: &str,
        _req: ordo_fn::FnMacRequest,
    ) -> Result<ordo_fn::FnMacAnswer, FetchError> {
        panic!("this double holds no secret")
    }
}

struct Doubles {
    ops: Arc<Ops>,
    far: Arc<Far>,
}

fn doubles() -> Doubles {
    Doubles {
        ops: Arc::new(Ops {
            asked: Mutex::new(Vec::new()),
        }),
        far: Arc::new(Far {
            seen: Mutex::new(Vec::new()),
        }),
    }
}

fn call(code: &str, uses: &[&str], d: &Doubles) -> FnCall {
    FnCall {
        space: "acme".into(),
        function: "faceChat".into(),
        version: "v1".into(),
        run_id: "run-1".into(),
        code: Arc::from(code.as_bytes()),
        input: json!({ "message": "how are you" }),
        egress: vec!["www.googleapis.com".into()],
        secrets: Vec::new(),
        limits: Limits::default(),
        fetch: d.far.clone(),
        scrub: Arc::new(NoScrub),
        blobs: Arc::new(ordo_fn::NoBlobs),
        blob_args: Vec::new(),
        uses: uses.iter().map(|s| s.to_string()).collect(),
        ops: d.ops.clone(),
    }
}

async fn host() -> FnHost {
    FnHost::new(HostConfig::default()).await.expect("engine")
}

/// The shape of denizens' `face-chat` (plan §11.4), without the model.
const FACE_CHAT: &str = r#"
export default {
  async call(input, ctx) {
    const turns = await ctx.op("recentTurns", { denizen: "dnz_1" });
    const reply = "you said " + input.message + " after " + turns.length + " turns";
    const written = await ctx.op("addTurns", { said: input.message, reply });
    return { reply, id: written[0].id };
  },
};
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_function_calls_the_ops_it_uses_and_gets_their_rows() {
    let h = host().await;
    let d = doubles();
    let run = h
        .call(call(FACE_CHAT, &["recentTurns", "addTurns"], &d))
        .await;
    assert_eq!(
        run.outcome.unwrap(),
        json!({ "reply": "you said how are you after 2 turns", "id": "turn_1" })
    );
    assert_eq!(run.report.op_calls, 2);
    let asked = d.ops.asked.lock().unwrap();
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[0].0, "recentTurns");
    assert_eq!(asked[0].1["denizen"], "dnz_1");
    assert_eq!(asked[1].0, "addTurns");
    assert_eq!(asked[1].1["said"], "how are you");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_op_not_in_uses_is_refused_before_the_seam_is_asked() {
    let h = host().await;
    let d = doubles();
    // `addTurns` is not listed: the read lands, the write is refused here.
    let run = h.call(call(FACE_CHAT, &["recentTurns"], &d)).await;
    match run.outcome {
        Err(FnError::Op(OpError::NotUsed { op })) => assert_eq!(op, "addTurns"),
        other => panic!("{other:?}"),
    }
    assert_eq!(run.report.op_calls, 2);
    assert_eq!(
        d.ops.asked.lock().unwrap().len(),
        1,
        "the seam never saw addTurns"
    );

    // The function may catch it and read the reason word.
    let caught = r#"
export default {
  async call(input, ctx) {
    try {
      await ctx.op("addTurns", {});
      return { landed: true };
    } catch (e) {
      return { code: e.code, name: e.name, says: e.message };
    }
  },
};
"#;
    let run = h.call(call(caught, &[], &d)).await;
    let out = run.outcome.unwrap();
    assert_eq!(out["code"], "op_not_in_uses");
    assert_eq!(out["name"], "OpRefused");
    assert!(
        out["says"].as_str().unwrap().contains("`uses` line"),
        "the refusal says what to change: {out}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_seams_refusal_ends_the_call_with_its_class_and_a_thrown_code_does_not() {
    let h = host().await;
    let d = doubles();
    let denied = r#"
export default {
  async call(input, ctx) {
    return await ctx.op("adminOnly", {});
  },
};
"#;
    let run = h.call(call(denied, &["adminOnly"], &d)).await;
    let e = run.outcome.unwrap_err();
    assert_eq!(e.code(), "op_access_denied");
    assert!(matches!(e, FnError::Op(OpError::Denied { .. })), "{e:?}");

    // A function cannot claim a host refusal by throwing its word.
    let claims = r#"
export default {
  async call(input, ctx) {
    const e = new Error("pretend");
    e.code = "op_access_denied";
    throw e;
  },
};
"#;
    let run = h.call(call(claims, &[], &d)).await;
    assert_eq!(run.outcome.unwrap_err().code(), "function_error");
}

#[tokio::test(flavor = "multi_thread")]
async fn op_calls_per_call_are_capped() {
    let h = host().await;
    let d = doubles();
    let many = r#"
export default {
  async call(input, ctx) {
    for (let i = 0; i < 5; i++) await ctx.op("recentTurns", {});
    return { done: true };
  },
};
"#;
    let mut c = call(many, &["recentTurns"], &d);
    c.limits.max_ops = 3;
    let run = h.call(c).await;
    assert_eq!(run.outcome.unwrap_err().code(), "op_limit");
    assert_eq!(d.ops.asked.lock().unwrap().len(), 3);
}

/// Ellanor's calendar proxy (plan §11.3): the person's token is a sealed
/// field, read through an op as them. The function holds a handle and
/// never the value: it cannot print it, return it, or read it, and
/// `ctx.sealed` puts it in a header the host fills in.
#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_field_reaches_the_function_as_a_handle_for_a_header() {
    let h = host().await;
    let d = doubles();
    let calendar = r#"
export default {
  async call(input, ctx) {
    const [account] = await ctx.op("myAccount", {});
    console.log("token is", account.token, String(account.token));
    const r = await fetch("https://www.googleapis.com/calendar/v3/users/me/calendarList", {
      headers: { authorization: ctx.sealed(account.token).bearer() },
    });
    let refused = null;
    try {
      ctx.sealed("ya29.guessed");
    } catch (e) {
      refused = e.name;
    }
    return {
      email: account.email,
      token: account.token,
      seen: JSON.stringify(account),
      status: r.status,
      refused,
    };
  },
};
"#;
    let run = h.call(call(calendar, &["myAccount"], &d)).await;
    let logs: Vec<String> = run.report.logs.iter().map(|l| l.text.clone()).collect();
    let out = run.outcome.unwrap();
    assert_eq!(out["email"], "ana@example.com");
    assert_eq!(out["token"], "[sealed]");
    assert!(!out.to_string().contains("ya29"), "{out}");
    assert_eq!(out["status"], 200);
    assert_eq!(out["refused"], "TypeError", "only an op's sealed field");
    assert!(logs.iter().all(|l| !l.contains("ya29")), "{logs:?}");
    let seen = d.far.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].headers.iter().any(|(k, v)| k == "authorization"
        && matches!(v, HeaderValue::Held { template, .. } if template == "Bearer {}")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_function_without_uses_reaches_no_op() {
    let h = host().await;
    let d = doubles();
    let mut c = call(FACE_CHAT, &[], &d);
    c.ops = Arc::new(ordo_fn::NoOps);
    let run = h.call(c).await;
    assert_eq!(run.outcome.unwrap_err().code(), "op_not_in_uses");
    assert!(d.ops.asked.lock().unwrap().is_empty());
}
