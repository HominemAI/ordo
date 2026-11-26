# Application findings for the board handoff

2026-11-26. The example-test plan reserves board and roadmap changes for
Claude. These are findings and evidence to transfer, not recorded board state.
All changes remain in PR #59.

## Browser reload served acknowledged edits late

- Seen: an editor saved a product. Studio read revision 2, but the public page
  reload displayed revision 1. CI 45894957979 retained the network trace.
- Cause: `stale-while-revalidate` in `Cache-Control` permits a browser to reuse
  stale data while fetching a replacement. `max-age=0` does not prevent this.
- Fix: keep immediate browser revalidation in `Cache-Control`; target the CDN
  stale allowance through `CDN-Cache-Control` (RFC 9213). Public blob answers
  carry the same split. Authenticated immutable file policy is unchanged.
- Regression: the existing real-browser public editor journey keeps normal
  browser caching enabled and must display the acknowledged edit after reload.
  It also checks the split headers. Existing host header assertions track it.
- Status: implementation 90878598; browser journey passed in CI 45897056095.
- Source: https://www.rfc-editor.org/rfc/rfc9213.html

## Real console could not create a tenant

- Seen: console creation completed both passkey touches, then the real door
  refused `400 missing_param`, naming the required `Idempotency-Key` header.
  CI 45897056095 retained the page and network trace.
- Cause: the console's shared door client had no way to carry this key.
  Stand-in door fixtures did not enforce the host's side-effect contract.
- Fix: the page creates one key per action, retained across the touch retry.
  The tenant route passes it through the shared door client on a request-scoped
  clone. Existing API clients without a key receive a generated single-attempt
  key; supplying the key enables an explicit replay.
- Regression: the real-console browser journey creates a tenant and replays
  the exact keyed request. Both responses must name the same tenant.
- Status: implemented, awaiting CI.

## Fixture corrections, not product failures

- Reminder: the fixture scheduled in `system`, which the routine scheduler
  explicitly excludes. The execution journey now uses the owner's tenant.
- Version apply: the default rolling strategy acknowledged a rollout start,
  not completed activation. The cache journey selects the supported `all`
  strategy through an authenticated application call before applying.
- Console rotation: rotation consumes the old key's deployment touch. Tenant
  creation must answer the next real passkey prompt.
- Machine administration: CLI redemption returns a session secret. Exchange
  it at `/token` for the deployment token before calling administration APIs.
- Tenant creation requires an idempotency key; the lifecycle fixture now sends it.

## Existing unresolved contract checks remain strict

- Unknown/forbidden refusals echo different caller-supplied `error.op` values.
  The plan requires identical bodies except request ID; existing product tests
  explicitly allow the op echo. No normalization has been added here.
- Public calls naming `system` and an unknown space answer differently. The
  runtime documents that distinction; the example plan expects uniformity.
- A comment-only apply is byte-identical and already active. It does not cause
  a version swap, while the example plan calls it a new version and expects a
  cache miss. The assertion is retained for the existing board handoff.

These rows need the existing owner contract questions resolved through the
board before changing their acceptance rules. They must not be silently
counted as passing or excluded from release evidence.

## Controlled HTTPS function receiver needs supported trust configuration

- Source finding: `runtime.rs` constructs the function fetch transport with
  `TlsRoots::webpki()`. The configured `ORDO_AUTH_PROVIDER_CA_FILE` is passed
  to identity-provider transports, but not to function fetch.
- Impact on the requested journey: a throwaway local HTTPS receiver signed by
  the fixture CA cannot exercise successful function egress under that wiring.
  `ORDO_EGRESS_PRIVATE_HOSTS` permits its address but does not trust its CA.
- Reproduction to add: call an egress-authorized function against a fixture-CA
  HTTPS endpoint. Identity-provider trust configuration alone cannot make the
  function transport trust it.
- Status: source-level limitation, not yet an executed failure. No runtime fix
  made. Successful local HTTPS secret-consumer coverage remains unresolved.
