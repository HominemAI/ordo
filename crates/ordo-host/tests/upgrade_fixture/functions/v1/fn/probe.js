// probe, version 1. It answers `one`, so a call shows which code ran.
//
// The fetch carries probeKey toward the test's listener. `reason` is the
// word the fetch failed with: `secret_not_set` when the value did not open,
// and a network word when it opened. The listener closes before TLS.
export default {
  async call(input, ctx) {
    let reason = "answered";
    try {
      await fetch("https://upgrade.localhost:" + input.port + "/upgrade", {
        headers: { authorization: ctx.secret("probeKey").bearer() },
      });
    } catch (e) {
      reason = e.code || String(e);
    }
    return { code: "one", reason };
  },
};
