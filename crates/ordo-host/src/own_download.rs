//! A person's own download: the platform's half of `GET /download`
//! (`ordo_api::download`, owner ruling 2026-10-22).
//!
//! Two things the door cannot know by itself:
//!
//! - **which ops are marked `download`** in the program it serves now. Read
//!   from the live version on every request, so an `ordo apply` that marks
//!   or unmarks an op changes the download at once.
//! - **where a download is recorded**: the tenant's own security-event
//!   stream, as a `CustodyAction` named `own_download`, beside the export of
//!   the whole space (`exit_ops`), which is the act it is a smaller sibling
//!   of.
//!
//! The download is the person's own. A delegated token and a share guest are
//! refused by default, naming the setting. The app developer opens it to
//! delegated tokens by naming `download` in `ORDO_DELEGABLE_OPS`; no setting
//! opens it to a guest.
//!
//! | Setting | Default | Means |
//! |---|---|---|
//! | `ORDO_DELEGABLE_OPS` | none | name `download` to let a delegated token download |
//! | `ORDO_OWN_DOWNLOAD_PER_HOUR` | 6 | downloads one person may start in one tenant in an hour, per node |
//! | `ORDO_OWN_DOWNLOAD_MAX_BYTES` | 1073741824 (1 GiB) | the most bytes of files one download holds; at most 4000000000 |

use std::sync::Arc;

use ordo_api::download::{DownloadOp, DownloadSource};

use crate::deploy::Deployment;

/// The security record's op name for a download.
pub const RECORD_OP: &str = "std.space.download";

/// [`DownloadSource`] over the deployment the door serves.
pub struct ProgramDownload {
    pub deployment: Arc<Deployment>,
    /// The security recorder. `None` records nothing, for a rig with none.
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

/// The marked ops of a fixture, by name, with the entity each reads.
pub fn marked_ops(f: &ordo_conformance::Fixture) -> Vec<DownloadOp> {
    let mut names: Vec<&String> = f.registry.names().collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let op = f.registry.get(name)?;
            let entity = ordo_plan::download::download_entity(&f.schema, op)?;
            Some(DownloadOp {
                op: name.clone(),
                entity,
            })
        })
        .collect()
}

impl DownloadSource for ProgramDownload {
    fn ops(&self) -> Vec<DownloadOp> {
        marked_ops(&self.deployment.current().fixture)
    }

    fn record(&self, space: &str, principal: &str, agent: Option<&str>, entities: &[String]) {
        let Some(stream) = &self.stream else {
            return;
        };
        let at = (self.now)();
        // A delegated download names the agent too; the subject stays the
        // person whose rows they are.
        let action = match agent {
            None => format!("own_download:{}", entities.join(",")),
            Some(a) => format!("own_download:{};agent={a}", entities.join(",")),
        };
        stream.observe(
            ordo_secevent::Observation {
                space: Some(ordo_commandlog::SpaceId(space.to_string())),
                subject: ordo_secevent::Subject::member(principal),
                op: RECORD_OP.to_string(),
                kind: ordo_secevent::EventKind::CustodyAction { action },
                at: ordo_commandlog::Timestamp(at),
                // A fresh id: a download joins no command group.
                correlation: ordo_secevent::correlation::mint(),
            },
            ordo_secevent::Relationship::Member,
        );
    }
}
