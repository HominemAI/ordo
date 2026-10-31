//! The natives behind `TextEncoder`, `TextDecoder` and `crypto`.
//!
//! The prelude (`prelude.js`) holds the Web-shaped classes; the byte work
//! is here, in Rust. A byte range crosses as three arguments: an
//! `ArrayBuffer`, an offset and a length, which the prelude takes from the
//! caller's view. Randomness comes from getrandom, which this crate routes to
//! the host's `random` import, so it is fresh per call and never seeded.

use boa_engine::object::builtins::{AlignedVec, JsArrayBuffer};
use boa_engine::object::ObjectInitializer;
use boa_engine::{
    js_string, Context, JsArgs, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};

/// Adds the natives to `native`, the object the prelude reads and removes.
pub fn install(native: &mut ObjectInitializer<'_>) {
    native
        .function(
            NativeFunction::from_fn_ptr(utf8_encode),
            js_string!("utf8Encode"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(utf8_encode_into),
            js_string!("utf8EncodeInto"),
            4,
        )
        .function(
            NativeFunction::from_fn_ptr(utf8_decode),
            js_string!("utf8Decode"),
            5,
        )
        .function(NativeFunction::from_fn_ptr(digest), js_string!("digest"), 4)
        .function(
            NativeFunction::from_fn_ptr(hmac_sign),
            js_string!("hmacSign"),
            7,
        )
        .function(
            NativeFunction::from_fn_ptr(hmac_verify),
            js_string!("hmacVerify"),
            10,
        )
        .function(
            NativeFunction::from_fn_ptr(fill_random),
            js_string!("fillRandom"),
            3,
        );
}

fn type_error(msg: &str) -> boa_engine::JsError {
    JsNativeError::typ().with_message(msg.to_owned()).into()
}

/// The buffer, offset and length at `args[i..i + 3]`.
fn range(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<(JsArrayBuffer, usize, usize)> {
    let obj = args
        .get_or_undefined(i)
        .as_object()
        .ok_or_else(|| type_error("expected an ArrayBuffer"))?
        .clone();
    let buf = JsArrayBuffer::from_object(obj)?;
    let off = args.get_or_undefined(i + 1).to_index(ctx)? as usize;
    let len = args.get_or_undefined(i + 2).to_index(ctx)? as usize;
    let have = buf.data().map(|d| d.len()).unwrap_or(0);
    if off.checked_add(len).is_none_or(|end| end > have) {
        return Err(type_error(
            "the view is outside its buffer, or the buffer is detached",
        ));
    }
    Ok((buf, off, len))
}

/// A copy of the bytes at `args[i..i + 3]`.
pub(crate) fn bytes(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<Vec<u8>> {
    let (buf, off, len) = range(args, i, ctx)?;
    let data = buf
        .data()
        .ok_or_else(|| type_error("the buffer is detached"))?;
    Ok(data[off..off + len].to_vec())
}

pub(crate) fn array_buffer(v: Vec<u8>, ctx: &mut Context) -> JsResult<JsValue> {
    let block = AlignedVec::from_iter(0, v);
    Ok(JsArrayBuffer::from_byte_block(block, ctx)?.into())
}

/// `utf8Encode(text) -> ArrayBuffer`. A lone surrogate becomes U+FFFD, as
/// the Encoding standard says.
fn utf8_encode(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let s = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_lossy();
    array_buffer(s.into_bytes(), ctx)
}

/// `utf8EncodeInto(text, buffer, offset, length) -> { read, written }`.
/// Writes whole characters only; `read` counts UTF-16 code units.
fn utf8_encode_into(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let units = args.get_or_undefined(0).to_string(ctx)?.to_vec();
    let (buf, off, len) = range(args, 1, ctx)?;
    let (mut read, mut written) = (0usize, 0usize);
    {
        let mut data = buf
            .data_mut()
            .ok_or_else(|| type_error("the buffer is detached"))?;
        let out = &mut data[off..off + len];
        for c in char::decode_utf16(units.iter().copied()) {
            let (c, n_units) = match c {
                Ok(c) => (c, c.len_utf16()),
                Err(_) => (char::REPLACEMENT_CHARACTER, 1),
            };
            let n = c.len_utf8();
            if written + n > out.len() {
                break;
            }
            c.encode_utf8(&mut out[written..written + n]);
            written += n;
            read += n_units;
        }
    }
    let o = ObjectInitializer::new(ctx).build();
    o.set(js_string!("read"), read as u32, false, ctx)?;
    o.set(js_string!("written"), written as u32, false, ctx)?;
    Ok(o.into())
}

/// `utf8Decode(buffer, offset, length, fatal, ignoreBOM) -> string`.
fn utf8_decode(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let v = bytes(args, 0, ctx)?;
    let fatal = args.get_or_undefined(3).to_boolean();
    let ignore_bom = args.get_or_undefined(4).to_boolean();
    let v = if !ignore_bom && v.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &v[3..]
    } else {
        &v[..]
    };
    let s = if fatal {
        std::str::from_utf8(v)
            .map_err(|_| type_error("The encoded data was not valid utf-8"))?
            .to_owned()
    } else {
        String::from_utf8_lossy(v).into_owned()
    };
    Ok(JsValue::from(JsString::from(s.as_str())))
}

/// The hash a name stands for, already normalized by the prelude.
#[derive(Clone, Copy)]
enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

fn hash_named(v: &JsValue, ctx: &mut Context) -> JsResult<Hash> {
    match v.to_string(ctx)?.to_std_string_escaped().as_str() {
        "SHA-1" => Ok(Hash::Sha1),
        "SHA-256" => Ok(Hash::Sha256),
        "SHA-384" => Ok(Hash::Sha384),
        "SHA-512" => Ok(Hash::Sha512),
        other => Err(type_error(&format!("unsupported hash: {other}"))),
    }
}

/// `digest(hash, buffer, offset, length) -> ArrayBuffer`.
fn digest(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let h = hash_named(args.get_or_undefined(0), ctx)?;
    let v = bytes(args, 1, ctx)?;
    let out = match h {
        Hash::Sha1 => Sha1::digest(&v).to_vec(),
        Hash::Sha256 => Sha256::digest(&v).to_vec(),
        Hash::Sha384 => Sha384::digest(&v).to_vec(),
        Hash::Sha512 => Sha512::digest(&v).to_vec(),
    };
    array_buffer(out, ctx)
}

fn mac(h: Hash, key: &[u8], data: &[u8]) -> Vec<u8> {
    macro_rules! run {
        ($t:ty) => {{
            // HMAC takes a key of any length, so this cannot fail.
            let mut m = <Hmac<$t>>::new_from_slice(key).expect("HMAC takes any key length");
            m.update(data);
            m.finalize().into_bytes().to_vec()
        }};
    }
    match h {
        Hash::Sha1 => run!(Sha1),
        Hash::Sha256 => run!(Sha256),
        Hash::Sha384 => run!(Sha384),
        Hash::Sha512 => run!(Sha512),
    }
}

/// `hmacSign(hash, key..., data...) -> ArrayBuffer`.
fn hmac_sign(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let h = hash_named(args.get_or_undefined(0), ctx)?;
    let key = bytes(args, 1, ctx)?;
    let data = bytes(args, 4, ctx)?;
    array_buffer(mac(h, &key, &data), ctx)
}

/// `hmacVerify(hash, key..., signature..., data...) -> boolean`. The
/// comparison takes the same time wherever the bytes differ.
fn hmac_verify(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let h = hash_named(args.get_or_undefined(0), ctx)?;
    let key = bytes(args, 1, ctx)?;
    let sig = bytes(args, 4, ctx)?;
    let data = bytes(args, 7, ctx)?;
    let want = mac(h, &key, &data);
    let same = want.len() == sig.len()
        && want
            .iter()
            .zip(sig.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    Ok(JsValue::from(same))
}

/// `fillRandom(buffer, offset, length)`: fills the range from the host.
fn fill_random(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let (buf, off, len) = range(args, 0, ctx)?;
    let mut data = buf
        .data_mut()
        .ok_or_else(|| type_error("the buffer is detached"))?;
    getrandom::fill(&mut data[off..off + len])
        .map_err(|_| JsNativeError::error().with_message("no randomness from the host"))?;
    Ok(JsValue::undefined())
}
