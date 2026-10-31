//! The identity service for deployment people
//! (`docs/plan-identity-service.md`, checklist item 18o).
//!
//! A sibling of the console and the deployment in the one image. It holds
//! the people and machines that run a deployment, in its own database, and
//! signs the 10 minute tokens the door and the console verify with its
//! published keys. The app realm's users never reach it.
//!
//! Step 1, built so far:
//!
//! - settings ([`config`]), the one error ([`error`]), the database
//!   ([`store`]), signing keys and the JWKS ([`keys`]), the token
//!   ([`token`]), the audit chain ([`audit`]);
//! - people ([`people`]), sessions ([`sessions`]), passkeys moved from the
//!   console ([`passkeys`]), recovery codes ([`recovery`]), invites
//!   ([`invites`]), CLI grants ([`cli`]), machine keys ([`machine`]),
//!   guessing limits ([`throttle`]);
//! - what each call does ([`flows`]), its HTTP shape ([`routes`]), boot,
//!   first start and serving ([`server`]), and the host commands
//!   ([`commands`]).
//!
//! The console import (6.1) and its RP ID warning (6.2): [`import`].
//!
//! Step 2: the service's own page ([`page`]): sign-up by link, sign-in,
//! CLI approval, and the account (passkeys, recovery codes).

pub mod audit;
pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod flows;
pub mod import;
pub mod invites;
pub mod keys;
pub mod machine;
pub mod page;
pub mod passkeys;
pub mod people;
pub mod recovery;
mod routes;
pub mod secrets;
pub mod server;
pub mod sessions;
pub mod store;
pub mod throttle;
pub mod token;

pub use config::IdentityConfig;
pub use error::{IdentityError, SessionEnd};
pub use server::{BootOptions, FirstStart, Service};
