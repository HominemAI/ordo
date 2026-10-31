//! `ordo-supervisor`: the one image's entrypoint
//! (`docs/plan-identity-service.md` 3, step 7).
//!
//! It runs 3 children as siblings, none another's parent: the identity
//! service (unix user 65531), the console (65532) and the deployment,
//! `ordo-host` (65533). The supervisor runs as 65530 and carries the 4
//! file capabilities that let it switch users ([`child`]). It moved here
//! from the console, because "if the thing that manages the thing depends
//! on the thing its managing its no bueno" (owner, 2026-10-27).
//!
//! - [`config`]: its own settings, `ORDO_SUPERVISOR_*`.
//! - [`children`]: the container's variables split among the children, and
//!   the identity defaults filled from the console's settings.
//! - [`spec`]: the deployment's live and last good revision, sealed on the
//!   supervisor's volume.
//! - [`run`]: one loop per child: restart, backoff, crash loop, apply.
//! - [`socket`] and [`client`]: how the console still manages the
//!   deployment.
//! - [`health`]: `/health/live` on 127.0.0.1:8079.

pub mod child;
pub mod children;
pub mod client;
pub mod config;
pub mod error;
pub mod health;
pub mod run;
pub mod socket;
pub mod spec;
pub mod state;

pub use client::SupervisorClient;
pub use config::{ChildName, SupervisorConfig};
pub use error::{Kind, SupervisorError};
pub use run::Supervisor;
pub use state::{Applied, ChildState, Phase, Status, Timings};
