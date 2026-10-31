// The Web-shaped surface a function sees. Run once at boot, before the
// snapshot. `__ordo_native` is removed from the global object here, so the
// function's own code reaches the host only through what this file defines.
(function () {
  "use strict";
  const native = globalThis.__ordo_native;
  delete globalThis.__ordo_native;

  const SECRET = Symbol("ordo.secret");

  // A secret is a handle that names it. The value never enters the guest:
  // the host fills it in on the way out, for the secret's bound hosts only.
  class SecretHandle {
    constructor(name, template) {
      this[SECRET] = { name: String(name), template: String(template) };
      Object.freeze(this);
    }
    bearer() {
      return new SecretHandle(this[SECRET].name, "Bearer {}");
    }
    template(t) {
      t = String(t);
      if (t.split("{}").length !== 2) {
        throw new TypeError("a secret template holds {} exactly once, where the value goes");
      }
      return new SecretHandle(this[SECRET].name, t);
    }
    toString() {
      return "[secret " + this[SECRET].name + "]";
    }
    toJSON() {
      return this.toString();
    }
  }

  class Headers {
    constructor(init) {
      this._h = [];
      if (init instanceof Headers) {
        this._h = init._h.slice();
      } else if (Array.isArray(init)) {
        for (const [k, v] of init) this.append(k, v);
      } else if (init && typeof init === "object") {
        for (const k of Object.keys(init)) this.append(k, init[k]);
      }
    }
    append(k, v) {
      this._h.push([String(k).toLowerCase(), v instanceof SecretHandle ? v : String(v)]);
    }
    set(k, v) {
      this.delete(k);
      this.append(k, v);
    }
    delete(k) {
      k = String(k).toLowerCase();
      this._h = this._h.filter((p) => p[0] !== k);
    }
    get(k) {
      k = String(k).toLowerCase();
      const all = this._h.filter((p) => p[0] === k).map((p) => String(p[1]));
      return all.length ? all.join(", ") : null;
    }
    has(k) {
      return this.get(k) !== null;
    }
    entries() {
      return this._h.map((p) => [p[0], String(p[1])])[Symbol.iterator]();
    }
    [Symbol.iterator]() {
      return this.entries();
    }
    forEach(f) {
      for (const [k, v] of this.entries()) f(v, k, this);
    }
  }

  class Response {
    constructor(body, init) {
      init = init || {};
      this._body = body == null ? "" : String(body);
      this.status = init.status === undefined ? 200 : init.status | 0;
      this.statusText = init.statusText || "";
      this.headers = new Headers(init.headers);
      this.ok = this.status >= 200 && this.status < 300;
      this.url = init.url || "";
      this.bodyUsed = false;
    }
    async text() {
      this.bodyUsed = true;
      return this._body;
    }
    async json() {
      this.bodyUsed = true;
      return JSON.parse(this._body);
    }
  }

  class FetchRefused extends Error {
    constructor(code, message) {
      super(message);
      this.name = "FetchRefused";
      this.code = code;
    }
  }

  // The host refused to use a secret as an HMAC key: not listed, not
  // declared, or not set. `code` is the same reason word fetch gives.
  class SecretRefused extends Error {
    constructor(code, message) {
      super(message);
      this.name = "SecretRefused";
      this.code = code;
    }
  }

  function fetch(resource, init) {
    try {
      init = init || {};
      const url = typeof resource === "string" ? resource : String(resource && resource.url);
      const method = String(init.method || "GET").toUpperCase();
      const headers = [];
      for (const [k, v] of new Headers(init.headers)._h) {
        if (v instanceof SecretHandle) {
          headers.push([k, { secret: v[SECRET].name, template: v[SECRET].template }]);
        } else {
          headers.push([k, { plain: v }]);
        }
      }
      let body = "";
      if (init.body !== undefined && init.body !== null) {
        if (typeof init.body !== "string") {
          throw new TypeError("fetch: body must be a string (use JSON.stringify)");
        }
        body = init.body;
      }
      const reply = native.fetch(JSON.stringify({ method, url, headers, body }));
      if (reply.refused !== undefined) {
        const e = JSON.parse(reply.refused);
        return Promise.reject(new FetchRefused(e.code, e.message));
      }
      const r = JSON.parse(reply.ok);
      return Promise.resolve(
        new Response(r.body, { status: r.status, headers: r.headers, url })
      );
    } catch (e) {
      return Promise.reject(e);
    }
  }

  function show(v) {
    if (typeof v === "string") return v;
    if (v instanceof Error) return v.name + ": " + v.message;
    try {
      const s = JSON.stringify(v);
      return s === undefined ? String(v) : s;
    } catch (_) {
      return String(v);
    }
  }
  function logger(level) {
    return function (...args) {
      native.log(level, args.map(show).join(" "));
    };
  }
  const console = {
    debug: logger(0),
    log: logger(1),
    info: logger(1),
    warn: logger(2),
    error: logger(3),
  };

  function makeCtx(c) {
    const ctx = {
      tenant: c.tenant,
      function: c.function,
      version: c.version,
      runId: c.runId,
      trigger: "call",
      secret(name) {
        return new SecretHandle(name, "{}");
      },
      log: console.log,
    };
    return Object.freeze(ctx);
  }

  // Called by the engine for each call: the module's default export and the
  // envelope. Always resolves to a JSON string, so the host reads one shape.
  async function invoke(def, envText) {
    try {
      const env = JSON.parse(envText);
      const ctx = makeCtx(env.ctx || {});
      let fn = null;
      let self = def;
      if (typeof def === "function") {
        fn = def;
        self = undefined;
      } else if (def && typeof def.call === "function") {
        fn = def.call;
      }
      if (!fn) {
        throw new TypeError("the module's default export has no call(input, ctx)");
      }
      const out = await fn.call(self, env.input, ctx);
      const s = JSON.stringify(out === undefined ? null : out);
      return '{"ok":' + s + "}";
    } catch (e) {
      const err = {
        message: e instanceof Error ? e.message : show(e),
        name: e instanceof Error ? e.name : "Error",
        stack: e instanceof Error && e.stack ? String(e.stack) : "",
      };
      if (e && typeof e.code === "string") err.code = e.code;
      return JSON.stringify({ error: err });
    }
  }

  // ---- Bytes, text and crypto. The byte work is native (`web.rs`); a range
  // crosses as (buffer, offset, length).

  class DOMException extends Error {
    constructor(message, name) {
      super(message === undefined ? "" : String(message));
      Object.defineProperty(this, "name", { value: name === undefined ? "Error" : String(name) });
    }
  }

  // A BufferSource as a Uint8Array over the same bytes.
  function view(x, what) {
    if (x instanceof ArrayBuffer) return new Uint8Array(x);
    if (ArrayBuffer.isView(x)) return new Uint8Array(x.buffer, x.byteOffset, x.byteLength);
    throw new TypeError(what + " must be an ArrayBuffer, a typed array or a DataView");
  }

  class TextEncoder {
    get encoding() {
      return "utf-8";
    }
    encode(input) {
      return new Uint8Array(native.utf8Encode(input === undefined ? "" : String(input)));
    }
    encodeInto(source, dest) {
      if (!(dest instanceof Uint8Array)) {
        throw new TypeError("TextEncoder.encodeInto: the destination must be a Uint8Array");
      }
      const r = native.utf8EncodeInto(String(source), dest.buffer, dest.byteOffset, dest.byteLength);
      return { read: r.read, written: r.written };
    }
  }

  const UTF8_LABELS = ["utf-8", "utf8", "unicode-1-1-utf-8", "unicode11utf8", "unicode20utf8", "x-unicode20utf8"];

  class TextDecoder {
    #fatal;
    #ignoreBOM;
    constructor(label, options) {
      const l = label === undefined ? "utf-8" : String(label).trim().toLowerCase();
      if (!UTF8_LABELS.includes(l)) {
        throw new RangeError('TextDecoder: "' + label + '" is not supported; only utf-8 is');
      }
      this.#fatal = !!(options && options.fatal);
      this.#ignoreBOM = !!(options && options.ignoreBOM);
    }
    get encoding() {
      return "utf-8";
    }
    get fatal() {
      return this.#fatal;
    }
    get ignoreBOM() {
      return this.#ignoreBOM;
    }
    decode(input, options) {
      if (options && options.stream) {
        throw new TypeError("TextDecoder: stream is not supported; decode the whole input at once");
      }
      if (input === undefined) return "";
      const u = view(input, "TextDecoder.decode: the input");
      return native.utf8Decode(u.buffer, u.byteOffset, u.byteLength, this.#fatal, this.#ignoreBOM);
    }
  }

  function notSupported(what) {
    return new DOMException(what, "NotSupportedError");
  }
  function algName(a) {
    return String(typeof a === "string" ? a : a && a.name).toUpperCase();
  }
  // SHA-1 stays for HMAC: some webhook senders still sign with it.
  const HASH_BLOCK_BITS = { "SHA-1": 512, "SHA-256": 512, "SHA-384": 1024, "SHA-512": 1024 };
  function hashName(a) {
    const n = algName(a);
    if (!(n in HASH_BLOCK_BITS)) {
      throw notSupported("unsupported hash " + n + "; SHA-1, SHA-256, SHA-384 and SHA-512 are supported");
    }
    return n;
  }
  function hmacOnly(a) {
    if (algName(a) !== "HMAC") {
      throw notSupported("unsupported algorithm " + algName(a) + "; HMAC is supported");
    }
  }

  // A key's bytes live here, out of the function's reach. A CryptoKey
  // object is only a handle. A key imported from a secret holds only the
  // secret's name: the host keeps the value and computes the HMAC.
  const KEYS = new WeakMap();
  class CryptoKey {
    constructor() {
      throw new TypeError("CryptoKey has no constructor; use crypto.subtle.importKey");
    }
  }
  function makeKey(bytes, hash, extractable, usages) {
    const k = Object.create(CryptoKey.prototype);
    const ro = (value) => ({ value, enumerable: true });
    Object.defineProperties(k, {
      type: ro("secret"),
      extractable: ro(!!extractable),
      algorithm: ro(Object.freeze({ name: "HMAC", hash: Object.freeze({ name: hash }), length: bytes.length * 8 })),
      usages: ro(Object.freeze(usages.slice())),
    });
    KEYS.set(k, { bytes, hash });
    return Object.freeze(k);
  }
  function makeSecretKey(secret, hash, usages) {
    const k = Object.create(CryptoKey.prototype);
    const ro = (value) => ({ value, enumerable: true });
    Object.defineProperties(k, {
      type: ro("secret"),
      extractable: ro(false),
      algorithm: ro(Object.freeze({ name: "HMAC", hash: Object.freeze({ name: hash }) })),
      usages: ro(Object.freeze(usages.slice())),
    });
    KEYS.set(k, { secret, hash });
    return Object.freeze(k);
  }
  // The secret a key names: a name, or a handle from ctx.secret(name).
  function secretName(keyData) {
    if (keyData instanceof SecretHandle) return keyData[SECRET].name;
    if (typeof keyData === "string" && keyData !== "") return keyData;
    throw new TypeError('importKey("secret", ...): name the secret, or pass ctx.secret(name)');
  }
  const hexOf = (u) => Array.from(u, (x) => x.toString(16).padStart(2, "0")).join("");
  // Sign (signature undefined) or verify with the host holding the key.
  function hostMac(k, data, signature) {
    const req = { secret: k.secret, hash: k.hash };
    if (signature !== undefined) req.signature = hexOf(signature);
    const reply = native.hostMac(JSON.stringify(req), data.buffer, data.byteOffset, data.byteLength);
    if (reply.refused !== undefined) {
      const e = JSON.parse(reply.refused);
      throw new SecretRefused(e.code, e.message);
    }
    return reply.ok;
  }
  function checkUsages(usages) {
    if (!Array.isArray(usages) || usages.length === 0) {
      throw new DOMException("key usages must name sign, verify or both", "SyntaxError");
    }
    for (const u of usages) {
      if (u !== "sign" && u !== "verify") {
        throw new DOMException("an HMAC key is for sign and verify, not " + u, "SyntaxError");
      }
    }
  }
  function keyFor(key, usage, algorithm) {
    hmacOnly(algorithm);
    const k = KEYS.get(key);
    if (!k) throw new TypeError("expected a CryptoKey");
    if (!key.usages.includes(usage)) {
      throw new DOMException("this key's usages do not include " + usage, "InvalidAccessError");
    }
    return k;
  }

  const subtle = {
    async digest(algorithm, data) {
      const h = hashName(algorithm);
      const u = view(data, "digest: the data");
      return native.digest(h, u.buffer, u.byteOffset, u.byteLength);
    },
    async importKey(format, keyData, algorithm, extractable, usages) {
      if (format === "secret") {
        hmacOnly(algorithm);
        const h = hashName(algorithm.hash);
        checkUsages(usages);
        if (extractable) {
          throw new DOMException(
            "importKey: a secret's key stays in the host, so it cannot be extractable; pass false",
            "InvalidAccessError");
        }
        if (algorithm.length !== undefined) {
          throw new DOMException("importKey: a secret's key takes no length", "DataError");
        }
        return makeSecretKey(secretName(keyData), h, usages);
      }
      if (format !== "raw") {
        throw notSupported('importKey: the raw and "secret" formats are supported');
      }
      hmacOnly(algorithm);
      const h = hashName(algorithm.hash);
      checkUsages(usages);
      const bytes = view(keyData, "importKey: the key").slice();
      if (bytes.length === 0) throw new DOMException("importKey: the key is empty", "DataError");
      if (algorithm.length !== undefined && algorithm.length !== bytes.length * 8) {
        throw new DOMException("importKey: length does not match the key", "DataError");
      }
      return makeKey(bytes, h, extractable, usages);
    },
    async generateKey(algorithm, extractable, usages) {
      hmacOnly(algorithm);
      const h = hashName(algorithm.hash);
      checkUsages(usages);
      const bits = algorithm.length === undefined ? HASH_BLOCK_BITS[h] : algorithm.length;
      if (!(bits > 0) || bits % 8 !== 0) throw notSupported("generateKey: length must be whole bytes");
      const bytes = new Uint8Array(bits / 8);
      native.fillRandom(bytes.buffer, 0, bytes.length);
      return makeKey(bytes, h, extractable, usages);
    },
    async exportKey(format, key) {
      if (format !== "raw") throw notSupported("exportKey: only the raw format is supported");
      const k = KEYS.get(key);
      if (!k) throw new TypeError("expected a CryptoKey");
      if (k.secret !== undefined) {
        throw new DOMException(
          "the key is secret " + k.secret + ", held by the host; its value never enters the function",
          "InvalidAccessError");
      }
      if (!key.extractable) throw new DOMException("the key is not extractable", "InvalidAccessError");
      return k.bytes.slice().buffer;
    },
    async sign(algorithm, key, data) {
      const k = keyFor(key, "sign", algorithm);
      const u = view(data, "sign: the data");
      if (k.secret !== undefined) return hostMac(k, u);
      return native.hmacSign(k.hash, k.bytes.buffer, 0, k.bytes.length, u.buffer, u.byteOffset, u.byteLength);
    },
    async verify(algorithm, key, signature, data) {
      const k = keyFor(key, "verify", algorithm);
      const s = view(signature, "verify: the signature");
      const u = view(data, "verify: the data");
      if (k.secret !== undefined) return new Uint8Array(hostMac(k, u, s))[0] === 1;
      return native.hmacVerify(
        k.hash, k.bytes.buffer, 0, k.bytes.length,
        s.buffer, s.byteOffset, s.byteLength,
        u.buffer, u.byteOffset, u.byteLength
      );
    },
  };

  function getRandomValues(array) {
    if (
      !ArrayBuffer.isView(array) ||
      array instanceof DataView ||
      array instanceof Float32Array ||
      array instanceof Float64Array
    ) {
      throw new DOMException("getRandomValues takes an integer typed array", "TypeMismatchError");
    }
    if (array.byteLength > 65536) {
      throw new DOMException("getRandomValues fills at most 65536 bytes at once", "QuotaExceededError");
    }
    native.fillRandom(array.buffer, array.byteOffset, array.byteLength);
    return array;
  }

  function randomUUID() {
    const b = new Uint8Array(16);
    native.fillRandom(b.buffer, 0, 16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    const h = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
    return h.slice(0, 8) + "-" + h.slice(8, 12) + "-" + h.slice(12, 16) + "-" + h.slice(16, 20) + "-" + h.slice(20);
  }

  const crypto = Object.freeze({ getRandomValues, randomUUID, subtle: Object.freeze(subtle) });

  const define = (k, v) =>
    Object.defineProperty(globalThis, k, { value: v, writable: false, enumerable: false, configurable: false });
  define("fetch", fetch);
  define("Headers", Headers);
  define("Response", Response);
  define("console", console);
  define("TextEncoder", TextEncoder);
  define("TextDecoder", TextDecoder);
  define("DOMException", DOMException);
  define("CryptoKey", CryptoKey);
  define("crypto", crypto);
  define("__ordo_invoke", invoke);
})();
