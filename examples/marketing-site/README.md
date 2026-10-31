# Marketing site

A landing page that reads its products, pricing and testimonials from Ordo.
Visitors never sign in and the page holds no token. A contact form lets a
visitor leave a message.

## What it shows

- `site.ordo`: three public entities (`products`, `plans`, `testimonials`), one
  private entity (`enquiries`), and these ops:
  - Public reads: `catalogue`, `product` (by slug), `pricing`, `reviews`.
  - One public write: `contact`.
  - For the site owner: `addProduct`, `addPlan`, `addTestimonial`, `enquiryList`.
- `dev/serve.sh` starts a throwaway Postgres and the Ordo server.
- `dev/seed` signs in as the site owner and writes synthesized content (10
  products, 3 plans, 8 testimonials).
- `web/` is a small React page: hero, product grid, pricing table,
  testimonials, contact form. It reads `VITE_ORDO_URL` (the server) and
  optionally `VITE_TURNSTILE_SITE_KEY`.

## What is public, and how it is declared

- A `public` line inside an entity marks every field of that entity.
- `public` at the end of a field line marks that one field.
- `op pricing public = ...` marks an op as callable with no token.
- A public read may read public fields only. That covers its output, its
  conditions and its sort keys.
- An entity that a public op uses has no `acl` and no `access` line.
- Public data lives in the deployment's own space, `system`.
- A visitor calls `GET <door>/op/pricing`. There is no token and no space header.
- Answers are cached, so the database sees only the misses.

The site owner adds content with the ordinary add ops. They carry the owner's
token and `X-Ordo-Space: system`.

## The contact form is a public write

- `contact` only creates a row in `enquiries`. It takes no `id`, no condition and no `$caller`.
- The visitor POSTs JSON to `<door>/op/contact` with no token.
- The answer holds the new row's id, `{"rows":[{"created":"<id>"}]}`.
- The visitor can write and cannot read. `enquiries` has no `public` line.
- The site owner reads enquiries with `enquiryList`, signed in.

## The human check

A public write must prove a person sent it. This is on by default.

- The page loads the Cloudflare Turnstile widget and sends its token in the
  `X-Ordo-Human` header.
- Build the page with `VITE_TURNSTILE_SITE_KEY=<site key>`. The site key is public.
- The deployment owner puts the Turnstile secret key in a file and sets
  `ORDO_PUBLIC_HUMAN_SECRET_FILE` to its path.
- `ORDO_PUBLIC_HUMAN_PROVIDER` picks `turnstile` (the default), `hcaptcha` or `recaptcha`.
- With neither a secret file nor `ORDO_PUBLIC_HUMAN_CHECK=off`, `contact` answers
  403 `human_check_failed`, and the message names these settings.
- The page shows a plain message for `human_check_failed` and `quota_exceeded`.

## The hourly cap

`ORDO_PUBLIC_WRITES_PER_HOUR` caps each public write op, per node. The default
is 10000. `0` turns the cap off. Past it, `contact` answers 429
`quota_exceeded`, and the message names the setting.

## Run it

You need PostgreSQL 15 or later, Node 18 or later, and the two binaries built:
`cargo build -p ordo-cli -p ordo-host`.

```
sh examples/marketing-site/dev/serve.sh     # terminal 1: database and server
sh examples/marketing-site/dev/seed         # terminal 2: content
cd examples/marketing-site/web && npm install
npm run dev                                 # open http://localhost:5183
```

The server listens on `127.0.0.1:4010` and the page on port 5183. The page
origin is fixed because the server's CORS setting names it.

Two things in `dev/` are for local use only:

- `serve.sh` sets `ORDO_PUBLIC_HUMAN_CHECK=off`, so the form works with no
  Turnstile account. A real deployment leaves it unset and sets a secret file.
- The site owner must own `system` to write there. The first `seed` run saves
  the owner's principal id in `dev/.data/owner-principal`. If it says the
  write was refused, stop `serve.sh`, start it again (it reads that file into
  `ORDO_DEPLOYMENT_OWNERS`) and run `seed` again.

`seed` keeps the site owner's guest secret in `dev/.data`, so a second run is
the same owner. It does not add products twice, because a product converges on
its `slug`.

## Notes

- A visitor can create a row and can change nothing else.
- Nothing limits one visitor apart from another. Put a CDN or a proxy limit in front.
- Share links still exist for sharing private rows with a person who has no
  account. See `docs/howto-sharing.md`.

Read next: `docs/howto-public.md`.
