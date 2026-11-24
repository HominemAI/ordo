// GENERATED — do not edit; regenerate with `ordo gen ts`.
//
// One corpus program, rebuilt through the builder.
//
// Source of truth: the program's own parsed AST, translated by `ordo_gen::emit`
//
// This file is the GENERATED half of this package. The hand-written
// half is under `src/core/` and is versioned by hand; the boundary
// between them is documented in the package README. An edit here is
// lost the next time the generator runs.

import { auth, domain, entity, synth, t } from '../../src/index.js';

const notes = entity('notes', {
  body: t.text(),
});

export const ordo: string = synth(
  domain('hal', '1.0.0'),
  auth({
    methods: ['passkey', 'email', 'google', 'work'],
    providers: { google: { preset: 'google' }, work: { kind: 'oidc', label: 'Work account' } },
    passkeyName: 'Human Agent Link',
    provisioning: { domains: ['example.com', 'my-co.example'] },
    selfServe: false,
    guest: { on: true, keep: '7d' },
    linking: { on: true, window: '5m' },
    secondFactor: 'required-without-passkey',
    userVerification: 'preferred',
    session: { token: '1h', max: '12h', tenants: ['15m', '4h'] },
    remember: { on: true, idle: '30d', max: 'none' },
  }),
  notes,
);
