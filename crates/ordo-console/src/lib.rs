//! The Ordo console: its own server, its own Postgres database and its own
//! sign-in (`docs/plan-console.md`, checklist item 18o).
//!
//! Step 1a, built here: the server with health and strict headers
//! ([`server`]), the console's own database and migrations ([`store`]), the
//! owner's passkey sign-in with step-up, recovery codes, a second owner, the
//! first-run setup code and `ordo console recover` ([`recover`]), and the
//! audit log. Step 1b adds the page ([`page`]), served from
//! `ORDO_CONSOLE_PAGE_DIR`. Every failure is one [`ConsoleError`], turned into an HTTP
//! answer in one place.
//!
//! Step 2a added the supervisor: the console ran `ordo-host` as a child,
//! with runtime revisions, apply and automatic rollback, and managed
//! secrets sealed with the console's key ([`sealing`]). Identity step 7
//! moved the supervisor out to `ordo-supervisor`, the one image's
//! entrypoint: the console now manages the deployment through the
//! supervisor's socket ([`runtime`]), and keeps the revision history.
//!
//! Step 3a adds the link to the deployment ([`link`]): the console key
//! (G14) the console signs in to the door with, the owner's touch relayed
//! for writes (G15), and the claim, all through one client ([`door`]).
//!
//! Without `ORDO_CONSOLE_SUPERVISOR_SOCKET` the console is in door-only
//! mode: it reaches the deployment through its door alone. The console
//! binary holds no code that starts a process in either mode.
//!
//! Identity service step 6 (`plan-identity-service.md` 5.4): with
//! `ORDO_IDENTITY_URL` set, people sign in at the identity service, every
//! call carries its token as a bearer, verified through one client
//! ([`identity`]), and the console calls the door as that person
//! ([`link::DoorAs`]). The console's own sign-in and the console key routes
//! then answer `moved_to_identity_service`.
//!
//! The console never connects to the deployment's database: boot refuses a
//! console database URL that names it ([`config`]).

pub mod codes;
pub mod config;
pub mod door;
pub mod error;
pub mod harden;
pub mod identity;
pub mod link;
pub mod page;
pub mod passkeys;
pub mod recover;
pub mod runtime;
pub mod sealing;
pub mod server;
pub mod store;

pub use codes::Clock;
pub use config::{ConsoleConfig, DatabaseSettings, Mode};
pub use error::{ConsoleError, SessionEnd};
pub use runtime::{ChildState, Phase, Runtime};
pub use server::{BootOptions, Console};
