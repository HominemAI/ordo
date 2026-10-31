//! The deployment's runtime, managed through the supervisor
//! (`plan-identity-service.md` 3.2, step 7).
//!
//! The console no longer runs the deployment. `ordo-supervisor` does, as a
//! sibling of the console, and holds the live revision on its own volume.
//! The console reaches it over the supervisor's socket
//! (`ORDO_CONSOLE_SUPERVISOR_SOCKET`), through the one client the
//! supervisor's crate provides ([`ordo_supervisor::SupervisorClient`]).
//! The console keeps what needs a person: the revision history, the diff,
//! the touch, and the secret values a person typed, sealed with the
//! console's key ([`crate::sealing`]).
//!
//! - [`spec`]: a revision's settings and secret references, a requested
//!   change, and what it would change.
//! - [`Runtime`]: status, logs, apply, rollback and restart, each a call
//!   on the socket, and at boot the upgrade's carry-over of the console's
//!   own live revision.
//!
//! With no socket set the console is in door-only mode: it holds no
//! [`Runtime`], and every runtime route answers that, naming the setting.

mod remote;
pub mod spec;

pub use ordo_supervisor::state::{ChildState, Phase, Privilege, Status};
pub use remote::{Runtime, Synced, CARRY_OVER_ACTION};

/// What a successful apply or rollback did.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Applied {
    pub revision: u64,
    pub diff: spec::Diff,
}
