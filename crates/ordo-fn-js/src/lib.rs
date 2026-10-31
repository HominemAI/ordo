//! The JavaScript engine a backend function runs in.
//!
//! This crate compiles to one WebAssembly module: the Boa engine, a small
//! Web-shaped prelude (`fetch`, `Response`, `Headers`, `console`,
//! `TextEncoder`, `TextDecoder`, `crypto`; byte work in `web.rs`) and four
//! exports the host drives. The host (`ordo-fn`) pre-initializes it once with
//! `ordo_init`, snapshots the result, and makes a fresh instance of that
//! snapshot for every call. Nothing in here outlives one call.
//!
//! The module imports exactly what the host hands it, from the `ordo` module:
//!
//! | Import | What it does |
//! |---|---|
//! | `fetch(ptr, len) -> i32` | one outbound request, as JSON; the reply length, negative for a refusal |
//! | `take_reply(ptr)` | copies the pending reply into guest memory |
//! | `log(level, ptr, len)` | one log line; the host scrubs and caps it |
//! | `result(ptr, len)` | the call's answer, as JSON |
//! | `now_ms() -> f64` | the wall clock, for `Date` |
//!
//! There is no socket, file, environment or secret value here. A secret is a
//! handle naming the secret; the host fills the value in on the way out.

mod web;

use std::cell::RefCell;
use std::rc::Rc;

use boa_engine::context::time::JsInstant;
use boa_engine::context::Clock;
use boa_engine::object::builtins::JsPromise;
use boa_engine::{
    js_string, Context, JsArgs, JsNativeError, JsResult, JsString, JsValue, Module,
    NativeFunction, Source,
};

#[link(wasm_import_module = "ordo")]
extern "C" {
    fn fetch(ptr: *const u8, len: usize) -> i32;
    fn take_reply(ptr: *mut u8);
    fn log(level: i32, ptr: *const u8, len: usize);
    fn result(ptr: *const u8, len: usize);
    fn now_ms() -> f64;
    fn random(ptr: *mut u8, len: usize);
}

/// `Math.random` draws from the host. The host fills the bytes per instance,
/// so two calls never share a sequence even though they share a snapshot.
///
/// # Safety
/// `dest` is valid for `len` bytes; getrandom guarantees it.
#[no_mangle]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    random(dest, len);
    Ok(())
}

const PRELUDE: &str = include_str!("prelude.js");

thread_local! {
    static ENGINE: RefCell<Option<Context>> = const { RefCell::new(None) };
}

/// `Date.now()` and friends read the host's clock.
struct HostClock;

impl Clock for HostClock {
    fn now(&self) -> JsInstant {
        // SAFETY: a plain import with no pointers.
        let ms = unsafe { now_ms() };
        let ms = if ms.is_finite() && ms > 0.0 {
            ms as u64
        } else {
            0
        };
        JsInstant::new(ms / 1000, ((ms % 1000) * 1_000_000) as u32)
    }
}

/// Memory for the host to write a call's input into.
#[no_mangle]
pub extern "C" fn ordo_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Boots the engine and runs the prelude. The host runs this once, before
/// the snapshot, so a call starts with the engine already built.
#[no_mangle]
pub extern "C" fn ordo_init() {
    match boot() {
        Ok(ctx) => ENGINE.with(|e| *e.borrow_mut() = Some(ctx)),
        Err(e) => {
            let msg = format!("engine boot failed: {e}");
            send_log(3, &msg);
            // A trap: the host refuses to snapshot an engine that did not boot.
            std::process::abort();
        }
    }
}

/// Runs one call: `code` is the function's ES module, `env` the JSON
/// envelope `{ input, ctx }`. The answer goes out through `result` as
/// `{ "ok": value }` or `{ "error": { message, stack, code } }`.
#[no_mangle]
pub extern "C" fn ordo_run(
    code_ptr: *const u8,
    code_len: usize,
    env_ptr: *const u8,
    env_len: usize,
) -> i32 {
    // SAFETY: the host wrote these ranges through `ordo_alloc` just before.
    let code = unsafe { std::slice::from_raw_parts(code_ptr, code_len) };
    let env = unsafe { std::slice::from_raw_parts(env_ptr, env_len) };
    let env = String::from_utf8_lossy(env).into_owned();
    let answer = ENGINE.with(|e| {
        let mut slot = e.borrow_mut();
        match slot.as_mut() {
            Some(ctx) => match run(ctx, code, &env) {
                Ok(s) => s,
                Err(e) => error_json(&e.to_string()),
            },
            None => error_json("the engine was not initialized"),
        }
    });
    // SAFETY: a pointer and length into a live string.
    unsafe { result(answer.as_ptr(), answer.len()) };
    0
}

fn error_json(message: &str) -> String {
    let m = JsValue::from(JsString::from(message));
    let mut ctx = Context::default();
    let quoted = m
        .to_json(&mut ctx)
        .ok()
        .flatten()
        .map(|v| v.to_string())
        .unwrap_or_else(|| "\"error\"".into());
    format!("{{\"error\":{{\"message\":{quoted}}}}}")
}

fn send_log(level: i32, msg: &str) {
    // SAFETY: a pointer and length into a live string.
    unsafe { log(level, msg.as_ptr(), msg.len()) };
}

fn boot() -> JsResult<Context> {
    let mut ctx = Context::builder()
        .clock(Rc::new(HostClock))
        .build()
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))?;
    let mut init = boa_engine::object::ObjectInitializer::new(&mut ctx);
    init.function(NativeFunction::from_fn_ptr(native_fetch), js_string!("fetch"), 1)
        .function(NativeFunction::from_fn_ptr(native_log), js_string!("log"), 2);
    web::install(&mut init);
    let native = init.build();
    ctx.global_object().set(
        js_string!("__ordo_native"),
        native,
        false,
        &mut ctx,
    )?;
    ctx.eval(Source::from_bytes(PRELUDE))?;
    ctx.run_jobs()?;
    Ok(ctx)
}

fn run(ctx: &mut Context, code: &[u8], env: &str) -> JsResult<String> {
    let module = Module::parse(Source::from_bytes(code), None, ctx)?;
    let loaded = module.load_link_evaluate(ctx);
    ctx.run_jobs()?;
    settle(&loaded)?;
    let default = module.namespace(ctx).get(js_string!("default"), ctx)?;
    let invoke = ctx
        .global_object()
        .get(js_string!("__ordo_invoke"), ctx)?;
    let invoke = invoke
        .as_callable()
        .ok_or_else(|| JsNativeError::typ().with_message("the prelude is missing"))?;
    let out = invoke.call(
        &JsValue::undefined(),
        &[default, JsValue::from(JsString::from(env))],
        ctx,
    )?;
    ctx.run_jobs()?;
    let promise = out
        .as_promise()
        .ok_or_else(|| JsNativeError::typ().with_message("invoke did not return a promise"))?;
    let v = settle(&promise)?;
    Ok(v.to_string(ctx)?.to_std_string_escaped())
}

fn settle(p: &JsPromise) -> JsResult<JsValue> {
    use boa_engine::builtins::promise::PromiseState;
    match p.state() {
        PromiseState::Fulfilled(v) => Ok(v),
        PromiseState::Rejected(e) => Err(boa_engine::JsError::from_opaque(e)),
        PromiseState::Pending => Err(JsNativeError::error()
            .with_message("the call never finished: it awaits something that cannot resolve")
            .into()),
    }
}

/// `__ordo_native.fetch(requestJson) -> { ok: responseJson } | { refused: errorJson }`
fn native_fetch(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let req = args.get_or_undefined(0).to_string(ctx)?.to_std_string_escaped();
    // SAFETY: a pointer and length into a live string.
    let n = unsafe { fetch(req.as_ptr(), req.len()) };
    let len = n.unsigned_abs() as usize;
    let mut buf = vec![0u8; len];
    if len > 0 {
        // SAFETY: the buffer holds exactly the length the host reported.
        unsafe { take_reply(buf.as_mut_ptr()) };
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let o = boa_engine::object::ObjectInitializer::new(ctx).build();
    let key = if n >= 0 {
        js_string!("ok")
    } else {
        js_string!("refused")
    };
    o.set(key, JsValue::from(JsString::from(text)), false, ctx)?;
    Ok(o.into())
}

/// `__ordo_native.log(level, text)`
fn native_log(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let level = args.get_or_undefined(0).to_i32(ctx)?;
    let text = args.get_or_undefined(1).to_string(ctx)?.to_std_string_escaped();
    send_log(level, &text);
    Ok(JsValue::undefined())
}
