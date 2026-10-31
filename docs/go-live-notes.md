# Go-live checklist: notes app on `ordo.inf.smg.icu`

Authority: `examples/notes/README.md` §8 and `docs/deploying.md` §4c.
You run every step. Nothing here was run against the live server.
Replace each `<placeholder>`. Never paste a real secret into a file in git.

**Before you start:** the image from main (step 1) already carries passkey
sign-in, CORS and sign-up provisioning. Custody is minted at the server's first
boot on an empty database, so there is nothing to do for it.

## 1. Deploy the image built from main

1. Open the latest green **Images** run for `main` on GitHub Actions.
2. Copy the line "Identity (deploy this)". It is a `sha256:` digest.
3. Deploy it in place of the current image:
   `ghcr.io/hominemai/ordo/ordo-host@sha256:<digest>`
4. For the CLI used in step 5, use `ghcr.io/hominemai/ordo/ordo-cli@sha256:<cli-digest>` from the same run.

You should see: the container starts on the new digest. Do not use `latest` or `main`; they move.

## 2. Set the environment variables

Paste this block into the server's environment, then fill the placeholder. Do not restart yet.

```
ORDO_AUTH_METHODS=passkey
ORDO_APEX_DOMAIN=inf.smg.icu
ORDO_AUTH_ORIGINS=https://notes.inf.smg.icu
ORDO_CORS_ORIGINS=https://notes.inf.smg.icu
ORDO_AUTH_PROVISIONING=auto-onboard
ORDO_PROVISION_SELF_SERVE=on-signup
ORDO_PROVISION_SIGNUPS_PER_HOUR=60
```

You should see: all 7 lines present in the server's settings.
Everything else stays at its default on purpose (`examples/notes/README.md` §6).

### Optional: more ways in

Skip this for a passkey-only launch. Each way in needs its setting on the server. The page asks the door which ways it serves, so its build needs no matching word. Details: `examples/notes/README.md` §6 and `docs/howto-sign-in.md` §8 to §14.

- **Emailed code.** Add `email` to `ORDO_AUTH_METHODS` (`passkey,email`) and set the mail channel: `ORDO_MAIL_KIND`, `ORDO_MAIL_FROM`, and its host or key file. Why: people without a passkey device can still sign in.
- **Guest.** `ORDO_AUTH_GUEST=on`. Why: a visitor tries the app at once. Set the human check beside it.
- **Outside connection** (Discord, GitHub, Google, Microsoft). Add the name to `ORDO_AUTH_METHODS`, and set `ORDO_PUBLIC_URL`, `ORDO_AUTH_RETURN_URLS=https://notes.inf.smg.icu`, and `ORDO_AUTH_<NAME>_PRESET`, `_CLIENT_ID`, `_CLIENT_SECRET_FILE` (Microsoft also `_TENANT`). Register `<ORDO_PUBLIC_URL>/auth/callback/<name>` at the provider. Why: one-click sign-in.
- **Human check.** `ORDO_AUTH_HUMAN_CHECK=turnstile` and `ORDO_AUTH_HUMAN_CHECK_SECRET_FILE`. Also set `ORDO_AUTH_HUMAN_CHECK_SITE_KEY=<public site key>`, so the page can draw the widget. Why: slows scripts that start sign-ins in bulk.

## 3. Nothing to mount

Every tenant gets a database of its own, with no setting to turn on. It is made on the server's own Postgres, with the password the server already holds in `ORDO_PG_PASSWORD` or `ORDO_PG_PASSWORD_FILE`. No credentials directory is needed.

If the environment still has `ORDO_PG_ROUTING=attached`, remove the line. The boot warns about it. Any other value refuses the boot.

## 4. Restart and check the startup log

1. Restart the server.
2. Read the startup log.

You should see these lines, and no complaint about `$deployment`:
```
passkey relying party inf.smg.icu
provisioning self-serve on-signup
```

## 5. Apply the program once

1. From the repo root, use the CLI image or a built `ordo`, with the same `ORDO_PG_*` and key settings as the server.
2. Run `ordo apply examples/notes --as <you>`

You should see: the apply reports success. If the system space has no program yet, the server log then prints `system space system: founding row written`.

## 6. Build and publish the page

The next step needs the page, so do this first.

1. `cd examples/notes/web && npm ci`
2. `VITE_ORDO_URL=https://ordo.inf.smg.icu npm run build`. Nothing else is needed for the optional ways in: the page reads them from `GET /auth/methods`. If the door cannot answer that route, the page offers the passkey alone.
3. In Cloudflare, create a Pages project by direct upload of `web/dist`.
4. Add the custom domain `notes.inf.smg.icu` to the project.
5. In your DNS, add `CNAME notes <address-Cloudflare-gives-you>`.

You should see: `https://notes.inf.smg.icu` loads the sign-in page. A build without `VITE_ORDO_URL` refuses to start.

## 7. Sign in as the first owner

1. Open `https://notes.inf.smg.icu` and sign up with a passkey.
2. Find **About this account** and copy the account id, `usr_...`.
3. Add `ORDO_DEPLOYMENT_OWNERS=<your-usr-id>` to the environment and restart.
4. From now on you can apply over the door:
   `ordo apply examples/notes --host ordo.inf.smg.icu --space <the-deployment-space> --token <your-passkey-token>`

You should see: the page shows your `usr_...` id, and the restart log shows no error.

## 8. Smoke test

1. Open `https://notes.inf.smg.icu` in a private window.
2. Sign up as a second person (a different passkey).
3. Write a note.
4. Back in your own window, reload and look at your notes.

You should see: the second person's note is in the private window only. Yours never shows it.

## If something goes wrong

- **Sign-up works but the page says no tenant was made.** You signed up before step 5. The page shows your `usr_...` id in its message. Apply (step 5), then continue.
- **No tenant is served at all.** Check that the deployment's Postgres user may `CREATE DATABASE` on the tenant cluster, and that any `ORDO_PG_TENANT_*` setting points where you intend.
