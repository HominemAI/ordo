/**
 * The `auth` builder prints the block the parser reads
 * (`plan-configuration-in-the-file.md` §2.2, §5.1).
 *
 * The full block's round trip through the parser is the corpus case
 * `auth-block` (`crates/ordo-gen/tests/isomorphism.rs`). This file covers
 * the plan's example, the type errors and the refusals.
 */

import { describe, expect, it } from 'vitest';

import { auth, domain, synth } from '../src/index.js';
import {
  AUTH_BUILT_IN_METHODS,
  AUTH_PRESETS,
  AUTH_PROVIDER_KINDS,
  SECOND_FACTOR_LEVELS,
  USER_VERIFICATION_LEVELS,
} from '../src/generated/vocabulary.js';

describe('auth', () => {
  it("prints the plan's example", () => {
    const signIn = auth({
      methods: ['passkey', 'email', 'google'],
      providers: { google: { preset: 'google' } },
      provisioning: { domains: ['example.com'] },
      guest: { on: true, keep: '7d' },
      secondFactor: 'optional',
      session: { token: '1h', max: '12h' },
    });
    expect(synth(domain('hal', '1.0.0'), signIn)).toBe(
      [
        'domain hal @ 1.0.0',
        '',
        'auth {',
        '  methods passkey, email, google',
        '  provider google preset google',
        '  provisioning domains example.com',
        '  guest on, keep 7d',
        '  secondFactor optional',
        '  session token 1h, max 12h',
        '}',
        '',
      ].join('\n'),
    );
  });

  it('prints every clause in canonical order', () => {
    const a = auth({
      remember: { on: true, idle: '30d', max: 'none' },
      session: { token: '1h', max: '12h', tenants: ['15m', '4h'] },
      userVerification: 'required',
      secondFactor: 'required-without-passkey',
      linking: { on: true, window: '5m' },
      guest: { on: false },
      selfServe: true,
      provisioning: 'open',
      passkeyName: 'Human Agent Link',
      providers: { work: { kind: 'oidc', label: 'Work "SSO"' } },
      methods: ['work', 'passkey'],
    });
    expect(synth(a)).toBe(
      [
        'auth {',
        '  methods work, passkey',
        '  provider work kind oidc, label "Work \\"SSO\\""',
        '  passkeyName "Human Agent Link"',
        '  provisioning open',
        '  selfServe on',
        '  guest off',
        '  linking on, window 5m',
        '  secondFactor required-without-passkey',
        '  userVerification required',
        '  session token 1h, max 12h, tenants 15m to 4h',
        '  remember on, idle 30d, max none',
        '}',
        '',
      ].join('\n'),
    );
  });

  it('prints an empty block, and quotes a domain the parser cannot read bare', () => {
    expect(synth(auth({}))).toBe('auth {\n}\n');
    expect(synth(auth({ provisioning: { domains: ['my-co.example', '1password.example'] } }))).toBe(
      'auth {\n  provisioning domains "my-co.example", "1password.example"\n}\n',
    );
  });

  it('refuses a provider no method names, as the compiler does', () => {
    expect(() => auth({ methods: ['passkey'], providers: { gh: { preset: 'github' } } })).toThrow(
      /provider `gh` is in no method/,
    );
  });

  it('refuses a bad duration and a provider named like a built-in method', () => {
    expect(() => auth({ session: { token: '1 hour' as '1h' } })).toThrow(/`token` is/);
    expect(() => auth({ methods: ['email'], providers: { email: { kind: 'oidc' } } })).toThrow(
      /cannot name a provider/,
    );
  });

  it('types an undeclared method, preset or level as an error', () => {
    // Each line below fails to typecheck; `npm run typecheck` holds the claim.
    // The runtime refusal backs the first one for a caller without types.
    expect(() =>
      auth({
        // @ts-expect-error -- `discord` is not built in and no provider has that name
        methods: ['passkey', 'discord'],
      }),
    ).toThrow(/method `discord` names no provider/);
    // @ts-expect-error -- `okta` is not a preset the host ships
    auth({ methods: ['okta'], providers: { okta: { preset: 'okta' } } });
    // @ts-expect-error -- `sometimes` is not a second factor level
    auth({ secondFactor: 'sometimes' });
    // @ts-expect-error -- `keep` is written only with `on: true`
    auth({ guest: { on: false, keep: '7d' } });
  });

  it('takes its word sets from the generated vocabulary', () => {
    expect(AUTH_BUILT_IN_METHODS).toEqual(['passkey', 'password', 'email']);
    expect(AUTH_PRESETS).toEqual(['discord', 'github', 'google', 'microsoft']);
    expect(AUTH_PROVIDER_KINDS).toEqual(['oauth2', 'oidc']);
    expect(SECOND_FACTOR_LEVELS).toEqual(['off', 'optional', 'required-without-passkey', 'required']);
    expect(USER_VERIFICATION_LEVELS).toEqual(['required', 'preferred']);
  });
});
