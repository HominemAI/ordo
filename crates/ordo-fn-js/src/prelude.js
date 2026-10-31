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

  const define = (k, v) =>
    Object.defineProperty(globalThis, k, { value: v, writable: false, enumerable: false, configurable: false });
  define("fetch", fetch);
  define("Headers", Headers);
  define("Response", Response);
  define("console", console);
  define("__ordo_invoke", invoke);
})();
