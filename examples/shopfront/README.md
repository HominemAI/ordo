# Shopfront

One organisation's shop, served by one Ordo deployment to three pages. A
customer browses the catalogue and places orders. Staff see every order and
ship them. A third page signs everyone in. There is one program and one
tenant, and the three pages are one build of one web app served at three
origins.

It shows two things Ordo can do without any tenant machinery in the way:

- **A single-tenant deployment.** The shop is the deployment's own tenant.
  Customers join it by the open way in. Staff are the people who hold a level
  on the shop.
- **Two front ends on subdomains, one deployment.** The customer page and the
  staff console call the same ops. One sign-in, at the login page, serves both
  (`docs/plan-subdomain-signin.md`, `docs/howto-sign-in.md` §11).

If you have never seen Ordo, read §1 first. It says what the program is. The
rest says how to run it, how the three pages hand a person to each other, and
what the platform still owes the app.

| Part | Where |
|---|---|
| The program, the whole server side | [`shopfront.ordo`](shopfront.ordo) |
| The pages: React over `@hominemai/ordo-sdk`, one Vite build | [`web/`](web) |
| The local deployment, one command | [`dev/serve.sh`](dev/serve.sh) |
| The synthesized shop (people, products, orders) | [`dev/seed.mjs`](dev/seed.mjs) |
| The proofs that need a server | `crates/ordo-host/tests/shopfront_app.rs` |
| The proofs that need none | `web/test/` (`npx vitest run`) |

## 1 · The program

```
ordo check examples/shopfront/shopfront.ordo
```

Four entities.

| Entity | What it holds | Who reads | Who writes |
|---|---|---|---|
| `shops` | the shop itself, with `acl (staff, admin)` | those who hold a level on it | those who hold a level on it |
| `products` | `sku`, name, category, `price: money(2)`, `stock`, `active`, `attributes: json` | everyone | staff |
| `orders` | the customer, the product, the quantity, when it was placed | the customer, and staff | the customer, and staff |
| `shipments` | one row per order, with when it shipped and the carrier | the customer, and staff | staff only |

Shipping is its own row on purpose. A customer may write their order. If
"shipped" lived on the order, they could write that too.

`acl (staff, admin)` derives the link entity `shopsAccess`, the guards on
`shops`, and the ops `grantShopsAccess`, `revokeShopsAccess` and
`setShopsAccessLevel`. Whoever opens the shop holds `admin` on it from that
moment.

### The ops

| Op | Who uses it | What it does |
|---|---|---|
| `me` | anyone signed in | the caller's own member row: id, display name, role |
| `myShops` | anyone signed in | the shops the caller holds a level on. Empty for a customer, which is how the console turns them away |
| `catalogue` | anyone signed in | the active products, sorted by category and name |
| `productsWhere($color)` | anyone signed in | the active products whose `attributes.color` is `$color` |
| `placeOrder($product, $quantity, $placed, $note)` | customer | writes an order for the caller |
| `myOrders` | customer | the caller's orders, with the product name, price, total and shipment |
| `openShop($name)` | staff, once | makes the shop. The caller becomes its `admin` |
| `allOrders` | staff | every order the caller may read, with the customer and the shipment |
| `addProduct(...)` | staff | adds a product. It converges on `sku`, so adding the same one twice changes it in place |
| `editProduct(...)` | staff | changes a product, asserting the `rev` the page read |
| `restock($product, $by)` | staff | adds to a product's stock |
| `markShipped(...)` | staff | writes the shipment and takes the stock, once per order |
| `staffOf($shop)` | staff | who works in the shop, and at which level |
| `grantShopsAccess`, `revokeShopsAccess`, `setShopsAccessLevel` | admin | derived. Add, remove or change a member of staff |
| `std.space.setAdmission` | deployment owner | a platform op. Opens the tenant's door to customers (§2) |

`markShipped` files the shipment with `rev: null`, which refuses a second
shipment for the same order. `check stock >= 0` on `products` refuses shipping
more than the shop holds.

### What a customer is refused

Every staff op is refused to a customer by a guard. The word depends on
whether she can read the row.

- `restock` and `markShipped` answer `access_guard`. She can read the product,
  so the row exists for her and the guard says no.
- `addProduct` answers `not_found`. She cannot read the shop, so for her it
  does not exist.
- `allOrders` returns her own orders and no one else's.
- `staffOf` and `myShops` return no rows.

### Roles and guards answer different questions

A role answers "may this person call this op at all". A guard answers "which
rows". Ordo has four fixed roles: `owner`, `manager`, `editor` and `viewer`.
A viewer may call only reads, and placing an order is a write. So customers
join as `editor`.

Staff are not a role. They are the people who hold a level on the shop
(`acl (staff, admin)`), granted by an admin with `grantShopsAccess`. The guards
on `products`, `orders` and `shipments` read that level.

- `staff` reads and writes the catalogue and the orders.
- `admin` also adds and removes staff.

### JSON attributes

`attributes` is a JSON object, such as `{"color": "red", "size": "M"}`.
`productsWhere` filters on a path inside it: `attributes.color == $color`. The
database does the matching inside the document.

- A product with no colour does not match.
- A colour that is not text does not match.
- A number `7` in the document does not match the text `"7"`.

## 2 · One tenant, and who joins it

The shop is the deployment's own tenant, `shopfront` (`ORDO_SPACE`). Nobody
gets a tenant of their own (`ORDO_PROVISION_SELF_SERVE=off`). Everyone who
signs in is admitted to the one tenant.

Three decisions make that work.

1. **A tenant may open its door.** `ORDO_AUTH_PROVISIONING=auto-onboard` is the
   ceiling that allows it. Without it, no tenant can admit a stranger.
2. **The owner is named on the server.** The first deployment owner signs up,
   reads the account id (`usr_...`) and puts it in `ORDO_DEPLOYMENT_OWNERS`.
   Then the door restarts. `serve.sh` does this for its seed owner. A shop on a
   public address must not belong to whoever arrives first, so Ordo has no
   first-comer rule. Setup belongs on the server side.
3. **The owner opens the door.** The owner calls
   `std.space.setAdmission {open: true, role: "editor"}`. From then on a
   customer joins by the open way in, as an `editor`. A shop wants strangers,
   so customers are not invited one by one.

The owner then calls `openShop` to make the shop and becomes its `admin`.

## 3 · The three pages

There is one build of the web app. Which page it shows is chosen by
`location.origin`.

| Page | Origin variable | For |
|---|---|---|
| login | `VITE_LOGIN_URL` | signing people in, with any method the deployment serves |
| shop | `VITE_SHOP_URL` | customers: the catalogue, a colour filter, orders |
| admin | `VITE_ADMIN_URL` | staff: all orders, shipping, stock, products, staff |

The other settings the pages read (`web/src/config.ts`):

| Variable | Meaning |
|---|---|
| `VITE_ORDO_URL` | the door. A built page has no default, and refuses to start without it |
| `VITE_ORDO_SPACE` | the deployment's tenant. Default `shopfront` |
| `VITE_ORDO_SIGN_IN` | a fallback only. The login page asks the door which ways in it serves (`GET /auth/methods`). If the door cannot answer, the page reads this list, in the words of `ORDO_AUTH_METHODS`. This page offers `passkey` and `email`. Unset, it is `passkey` |

### The hand-off

A person who opens the shop or the console is not signed in there. The page
sends them to the login page, and they come back signed in. There is no shared
cookie and no token in a URL.

1. The shop page makes a random binder and keeps it in its own
   `sessionStorage`. It sends the person to the login page with `return_to`
   and the binder's SHA-256.
2. The login page signs the person in, if they are not already. It then calls
   `POST /auth/handoff` with its bearer token.
3. The door checks the session is live and `return_to` is an allowed origin.
   It files a one-time code and answers with `return_to` plus the code.
4. The login page sends the browser there.
5. The shop page calls `POST /auth/handoff/redeem` with the code and the binder
   it kept. It gets its own session.

What to know about it:

- The code is useless without the binder, which only the starting page holds.
- The code lives 60 seconds (`ORDO_AUTH_HANDOFF_SECS`, 1 to 300) and works
  once.
- The new session has its own id and ends at the same ceiling as the login
  session. A hand-off cannot make a sign-in last longer.
- It carries no presence. A step-up on the shop page runs its own passkey
  touch.
- Its `via` is `handoff`, so the audit log can tell how it began.
- Logging out on the login page does not end it. "Sign out everywhere" does.
- Each page keeps its own session in its own origin's `sessionStorage`.

## 4 · Try it on this machine

You need PostgreSQL 15 or later, Node 18 or later, and a browser with
passkeys: Safari and Chrome on a Mac, Chrome or Edge on Windows or Android. In
Chrome without a hardware authenticator, DevTools, WebAuthn, "Enable virtual
authenticator environment" gives you one.

```
cargo build -p ordo-cli -p ordo-host
sh examples/shopfront/dev/serve.sh                          # terminal 1: the door
cd examples/shopfront/web && npm install && npm run dev     # terminal 2: the pages
```

`serve.sh` starts a throwaway Postgres on port 55494 under `dev/.data`, and
boots `ordo-host` on `127.0.0.1:4010` with the settings in §5. On the first run
it also does the following.

1. Registers a seed owner with a software passkey.
2. Restarts the door with that person in `ORDO_DEPLOYMENT_OWNERS`.
3. Runs `seed.mjs`: 45 products and 30 orders across four customers (Ada, Bo,
   Cy and Dee). About half the orders are shipped, by a staff member named
   Sam.

Then open the shop at `http://localhost:5181` or the console at
`http://localhost:5182`. Both send you to `http://localhost:5180` to sign in.

`npm run dev` starts all three pages. `npm run dev:login`, `dev:shop` and
`dev:admin` start one each.

### Local subdomains or three ports

Locally the three pages can be `login.localhost`, `shop.localhost` and
`admin.localhost`, or three ports on `localhost`: 5180 login, 5181 shop, 5182
admin. Both work. Browsers treat `localhost` and `*.localhost` as secure
contexts, so `http://` is fine.

- Set `ORDO_AUTH_ORIGINS` and `ORDO_AUTH_RETURN_URLS` to the origins you use,
  for example `http://login.localhost:5180`, `http://shop.localhost:5181` and
  `http://admin.localhost:5182`.
- Ordo accepts exactly one label under `localhost` (`shop.localhost`). It
  refuses `a.b.localhost` and any other `http://` host
  (`crates/ordo-identity/src/origin.rs`, `is_loopback_host`).
- The passkey relying party stays `localhost` (`ORDO_APEX_DOMAIN`). It covers
  every one of these origins.
- Three ports are three origins, so the hand-off runs exactly as it does on
  real subdomains.

### A walk through it

1. Open `http://localhost:5181` and press **Sign in**. It sends you to the
   login page.
2. Sign up with a passkey. You land back on the shop, signed in. That was the
   hand-off.
3. Place an order. It shows under **My orders**, waiting to ship.
4. Open `http://localhost:5182`. The console says you hold no level on the
   shop. That is correct, because you are a customer.

To use the console yourself:

1. Sign up on the shop page.
2. Copy your member id from the top of the page.
3. Run `node examples/shopfront/dev/seed.mjs make-staff <memberId> [staff|admin]`.
4. Reload the console. Your orders and everyone else's are there.

To be a deployment owner too, start `serve.sh` with `SHOPFRONT_OWNERS=usr_...`
and restart.

Delete `examples/shopfront/dev/.data` to start again. After you change
`shopfront.ordo`, run `npm run gen` in `web/` to regenerate the typed client.

## 5 · The settings

`serve.sh` sets the local column. A real deployment sets the live column, with
its own domain in place of `example.com`.

| Variable | Locally | Live | Why |
|---|---|---|---|
| `ORDO_SPACE` | `shopfront` | `shopfront` | the deployment's own tenant, which is the shop |
| `ORDO_AUTH_METHODS` | `passkey` | `passkey` (add `email` if wanted) | the ways in the login page serves |
| `ORDO_APEX_DOMAIN` | `localhost` | `example.com` | the passkey relying party. Changing it strands every passkey, so it is set once |
| `ORDO_AUTH_ORIGINS` | the three `http://localhost:518x` origins | `https://login.example.com`, `https://shop.example.com`, `https://admin.example.com` | which pages may run a ceremony. `http://` only on loopback |
| `ORDO_CORS_ORIGINS` | the same three | the same three | which pages may call the door |
| `ORDO_AUTH_RETURN_URLS` | shop and admin | `https://shop.example.com`, `https://admin.example.com` | where the login page may send a person back. Any path on a listed origin |
| `ORDO_AUTH_PROVISIONING` | `auto-onboard` | `auto-onboard` | lets the tenant open its door |
| `ORDO_PROVISION_SELF_SERVE` | `off` | `off` | nobody gets a tenant of their own |
| `ORDO_DEPLOYMENT_OWNERS` | the seed owner | your account id | who may apply the program and open admission |
| `ORDO_AUTH_HANDOFF_SECS` | not set (60) | `60` | how long a hand-off code lives |

`serve.sh` also reads:

| Variable | Meaning |
|---|---|
| `ORDO_AUTH_METHODS` | the ways in the login page offers. Default `passkey` |
| `SHOPFRONT_OWNERS` | more deployment owners, as account ids (`usr_...`) |
| `SHOPFRONT_SEED=off` | skip the synthesized data |
| `SHOPFRONT_LOGIN_PAGE`, `SHOPFRONT_SHOP_PAGE`, `SHOPFRONT_ADMIN_PAGE` | the three page origins |

### Wildcards

Instead of listing `shop` and `admin` in `ORDO_AUTH_RETURN_URLS`, you may set
`ORDO_WILDCARD_ORIGINS=on` and write `https://*.example.com`. It is off by
default.

- The `*` is one whole label, and only `https` is accepted.
- The cost: a forgotten DNS record on a subdomain lets whoever claims it serve
  a page the lists accept.
- That page still holds no binder and no token.
- The app developer who turns it on accepts that every subdomain is theirs to
  serve.

With three fixed pages, listing them is the tighter choice.

## 6 · Putting it live

Nothing here was run against a real domain.

1. Build the web app once with the origins and the door:
   `VITE_ORDO_URL=https://ordo.example.com VITE_LOGIN_URL=https://login.example.com VITE_SHOP_URL=https://shop.example.com VITE_ADMIN_URL=https://admin.example.com npm run build`.
2. Serve `web/dist` at all three origins. It is a static site, and the page
   shown is chosen by `location.origin`.
3. Set the live column of §5 on the server and make the tenant's keys
   (`ordo keys init --space shopfront`).
4. Apply the program through the database for the first deployment
   (`docs/deploying.md` §4c).
5. Open the login page and sign up. Read your account id (`usr_...`).
6. Put it in `ORDO_DEPLOYMENT_OWNERS` and restart.
7. As the owner, call `std.space.setAdmission {open: true, role: "editor"}` and
   `openShop`.
8. Grant your staff a level with `grantShopsAccess`.

### What to watch

- **The relying party.** `ORDO_APEX_DOMAIN` is the passkey relying party. Set
  it once.
- **The dev keys are constants.** `serve.sh` uses the development key provider,
  fixed secrets and a database with no password. None of that is fit for a real
  deployment. Use a real key provider (`docs/deploying.md`).
- **Each page keeps its own session**, in its own origin's `sessionStorage`. A
  second tab at the same origin does not share it.
- **A refused hand-off** means `ORDO_AUTH_RETURN_URLS` does not hold the
  origin. The refusal names the setting.
- **A page the browser blocks** means the origin is missing from
  `ORDO_CORS_ORIGINS`. The page's message names the setting.

## 7 · The tests

```
cd examples/shopfront/web && npm install && npx vitest run   # the pages, no server
bash scripts/conformance-pg.sh start                         # a Postgres for the rest
cargo test -p ordo-host --test shopfront_app                 # the real binary
```

`shopfront_app.rs` boots the real `ordo-host` with the settings in §5 and acts
as the pages and `seed.mjs` do, with the software authenticator standing in
for the browser. It has four tests.

- `a_customer_sees_only_their_own_orders_and_staff_see_all_and_ship_them`
- `a_customer_cannot_reach_the_staff_ops`
- `the_hand_off_carries_a_session_from_the_login_page_to_the_shop`
- `a_json_attribute_filter_returns_the_products_of_that_colour`

The web tests are `web/test/pages.test.ts` (which page an address is, and where
a hand-off may go) and `web/test/subdomain.test.ts`.

## 8 · Planned, and known gaps

1. **Names.** A member admitted by a sign-in is shown by the name the
   directory holds (25ba, fixed). The passkey's display name is shown by the
   authenticator only.
2. **The order total and the stock.**
   - The total is read at the catalogue's current price, because a write
     cannot read another row's price yet.
   - Stock is taken when an order ships, not when it is placed, because a
     customer may not write products.
3. **`space` on every sign-in.** A single-tenant deployment still sends `space`
   with every sign-in. Defaulting it is planned
   (`docs/plan-auth-federated.md` §2.3).
4. **Fixed while building this example.** Create guards that walk through a ref
   refused minted ids longer than 40 characters (commit `229001b8`).
5. **Not yet driven in a browser.** The web pages have not been driven in a
   browser against a running door. The live test and the seed exercise the
   same calls.
