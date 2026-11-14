/**
 * The `auth` block: how people sign in to the app
 * (`plan-configuration-in-the-file.md` §2, §5.1).
 *
 * **Skeleton half**, like `release.ts`. The closed word sets (presets,
 * provider kinds, second factor levels) come from `vocabulary.ts`, which the
 * generator writes from the parser's own tables.
 *
 * ```ts
 * const signIn = auth({
 *   methods: ['passkey', 'email', 'google'],
 *   providers: { google: { preset: 'google' } },
 *   provisioning: { domains: ['example.com'] },
 *   guest: { on: true, keep: '7d' },
 *   secondFactor: 'optional',
 *   session: { token: '1h', max: '12h' },
 * });
 * synth(domain('hal', '1.0.0'), signIn, ...rest);
 * ```
 *
 * A method that is not built in must name a key of `providers`, or the call
 * does not typecheck. A provider no method names is refused when the block
 * is built, as the compiler refuses it.
 *
 * The file sets the app developer's value. The deployment owner's ceilings
 * narrow it at run time. Values that cannot change once used, such as the
 * relying party id, are never in the block.
 */

import type {
  AUTH_BUILT_IN_METHODS,
  AUTH_PRESETS,
  AUTH_PROVIDER_KINDS,
  SECOND_FACTOR_LEVELS,
  USER_VERIFICATION_LEVELS,
} from '../generated/vocabulary.js';
import type { AuthNode, AuthProviderNode } from './ir.js';

/** `passkey`, `password` or `email`. */
export type BuiltInMethod = (typeof AUTH_BUILT_IN_METHODS)[number];
/** A provider preset the host ships. */
export type AuthPreset = (typeof AUTH_PRESETS)[number];
export type AuthProviderKind = (typeof AUTH_PROVIDER_KINDS)[number];
export type SecondFactorLevel = (typeof SECOND_FACTOR_LEVELS)[number];
export type UserVerificationLevel = (typeof USER_VERIFICATION_LEVELS)[number];

/** A duration in seconds, minutes, hours, days or weeks: `'5m'`, `'7d'`. */
export type AuthDuration = `${number}${'s' | 'm' | 'h' | 'd' | 'w'}`;

/** `provider <name> preset <p>` or `kind oauth2|oidc`, with an optional label. */
export type ProviderSpec =
  | { readonly preset: AuthPreset; readonly label?: string }
  | { readonly kind: AuthProviderKind; readonly label?: string };

export interface AuthSpec<P extends Record<string, ProviderSpec>> {
  /** The methods a sign-in page offers, in order. */
  readonly methods?: readonly NoInfer<BuiltInMethod | (keyof P & string)>[];
  readonly providers?: P;
  /** The name a person sees on their passkey. */
  readonly passkeyName?: string;
  readonly provisioning?: 'invite-only' | 'open' | { readonly domains: readonly string[] };
  readonly selfServe?: boolean;
  readonly guest?: { readonly on: false } | { readonly on: true; readonly keep?: AuthDuration };
  readonly linking?:
    | { readonly on: false }
    | { readonly on: true; readonly window?: AuthDuration };
  readonly secondFactor?: SecondFactorLevel;
  readonly userVerification?: UserVerificationLevel;
  readonly session?: {
    readonly token: AuthDuration;
    readonly max?: AuthDuration;
    readonly tenants?: readonly [AuthDuration, AuthDuration];
  };
  readonly remember?:
    | { readonly on: false }
    | {
        readonly on: true;
        readonly idle?: AuthDuration;
        /** `'none'`: a remembered sign-in lasts while it is used. */
        readonly max?: AuthDuration | 'none';
      };
}

const BUILT_IN: readonly string[] = ['passkey', 'password', 'email'];
const DURATION = /^[0-9]+[smhdw]$/;
/** A provider or method name: lowerCamel, as the parser reads a name. */
const NAME = /^[a-z][a-zA-Z0-9]*$/;

function duration(what: string, d: string): string {
  if (!DURATION.test(d)) {
    throw new Error(`auth: \`${what}\` is \`<n><s|m|h|d|w>\` such as \`5m\`, found \`${d}\``);
  }
  return d;
}

/** `auth { … }`, the one sign-in block a program may carry. */
export function auth<const P extends Record<string, ProviderSpec> = Record<never, ProviderSpec>>(
  spec: AuthSpec<P>,
): AuthNode {
  const methods = [...(spec.methods ?? [])] as string[];
  const providers: AuthProviderNode[] = Object.entries(spec.providers ?? {}).map(([name, p]) => {
    if (!NAME.test(name) || BUILT_IN.includes(name)) {
      throw new Error(`auth: \`${name}\` cannot name a provider`);
    }
    const source = 'preset' in p ? { preset: p.preset } : { kind: p.kind };
    return { name, ...source, label: p.label ?? null };
  });
  for (const p of providers) {
    if (!methods.includes(p.name)) {
      throw new Error(
        `auth: provider \`${p.name}\` is in no method, so nobody could use it; add it to \`methods\``,
      );
    }
  }
  for (const m of methods) {
    if (!BUILT_IN.includes(m) && !providers.some((p) => p.name === m)) {
      throw new Error(`auth: method \`${m}\` names no provider`);
    }
  }
  const prov = spec.provisioning;
  return {
    kind: 'auth',
    methods,
    providers,
    passkeyName: spec.passkeyName ?? null,
    provisioning:
      prov === undefined
        ? null
        : typeof prov === 'string'
          ? { mode: prov, domains: [] }
          : { mode: 'domains', domains: [...prov.domains] },
    selfServe: spec.selfServe ?? null,
    guest:
      spec.guest === undefined
        ? null
        : {
            on: spec.guest.on,
            keep: spec.guest.on && spec.guest.keep ? duration('keep', spec.guest.keep) : null,
          },
    linking:
      spec.linking === undefined
        ? null
        : {
            on: spec.linking.on,
            window:
              spec.linking.on && spec.linking.window
                ? duration('window', spec.linking.window)
                : null,
          },
    secondFactor: spec.secondFactor ?? null,
    userVerification: spec.userVerification ?? null,
    session:
      spec.session === undefined
        ? null
        : {
            token: duration('token', spec.session.token),
            max: spec.session.max ? duration('max', spec.session.max) : null,
            tenants: spec.session.tenants
              ? [
                  duration('tenants', spec.session.tenants[0]),
                  duration('tenants', spec.session.tenants[1]),
                ]
              : null,
          },
    remember:
      spec.remember === undefined
        ? null
        : {
            on: spec.remember.on,
            idle:
              spec.remember.on && spec.remember.idle ? duration('idle', spec.remember.idle) : null,
            max:
              spec.remember.on && spec.remember.max
                ? spec.remember.max === 'none'
                  ? 'none'
                  : duration('max', spec.remember.max)
                : null,
          },
  };
}
