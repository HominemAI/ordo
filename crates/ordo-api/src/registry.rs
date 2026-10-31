//! The hot-swap seam. `RegistryHandle` is the one place a registry is looked
//! up; `swap()` replaces the whole registry atomically. Requests clone their
//! `OpSpec` at dispatch, so in-flight work finishes on the version it looked
//! up — no drain, no restart; rollback is the same swap, backwards.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::spec::{OpRegistry, OpSpec, SpecError};

pub struct RegistryHandle {
    inner: RwLock<Arc<dyn OpRegistry>>,
}

impl RegistryHandle {
    pub fn new(initial: Arc<dyn OpRegistry>) -> Self {
        RegistryHandle {
            inner: RwLock::new(initial),
        }
    }

    /// Clone the current registry `Arc`. The critical section is a pointer
    /// clone — nothing awaits under the lock.
    pub fn current(&self) -> Arc<dyn OpRegistry> {
        self.inner.read().expect("registry lock").clone()
    }

    pub fn swap(&self, next: Arc<dyn OpRegistry>) {
        *self.inner.write().expect("registry lock") = next;
    }
}

/// Map-backed registry — tests and single-process embeddings; the
/// Postgres-backed artifact registry fulfills the same trait later.
pub struct MapRegistry {
    ops: HashMap<String, OpSpec>,
}

impl MapRegistry {
    /// Accept a set of specs, **validating each** ([`OpSpec::validate`]).
    /// Registration is where a malformed op is refused: a spec declaring a
    /// `$`-prefixed parameter would put a second, caller-controlled `$space`
    /// in the parameter map (SH2-1), and the request path is far too late to
    /// notice. A rejected registry never serves a request.
    pub fn new(ops: impl IntoIterator<Item = OpSpec>) -> Result<Self, SpecError> {
        let mut map = HashMap::new();
        for op in ops {
            op.validate()?;
            map.insert(op.name.clone(), op);
        }
        Ok(MapRegistry { ops: map })
    }
}

impl OpRegistry for MapRegistry {
    fn get(&self, name: &str) -> Option<OpSpec> {
        self.ops.get(name).cloned()
    }
}
