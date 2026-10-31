//! The on-disk cache of the compiled, snapshotted engine.
//!
//! Compiling and snapshotting the engine takes about a second at every
//! process start. The result is one wasmtime [`Module`], so it is written to
//! disk with [`Module::serialize`] and mapped back with
//! [`Module::deserialize_file`] on the next start. A start that finds a good file skips the instrument,
//! both compiles and the boot.
//!
//! # The key
//!
//! A file's name is a SHA-256 over: this cache's format tag, the engine
//! module's bytes, and wasmtime's [`Engine::precompile_compatibility_hash`]
//! (the target, the compiler flags, the engine config and the wasmtime
//! version). A new engine build, a new wasmtime or a changed config is a new
//! name, so a stale file is never opened. On a miss the older files are
//! removed. wasmtime checks the same things again when it loads a file.
//!
//! Loading from a file (not from bytes) lets wasmtime map the snapshot's
//! memory copy-on-write from the file on every platform. From bytes, macOS
//! copies it into each instance.
//!
//! # Why loading it is sound
//!
//! `Module::deserialize_file` is `unsafe`: the bytes become executable code with
//! no validation of the code itself, so a file someone else wrote could run
//! anything in this process. The cache only loads what Ordo wrote:
//!
//! - The directory is the deployment owner's setting. Ordo creates it with
//!   mode 0700 and refuses to use it unless this process's user owns it and
//!   no other user can write to it.
//! - A file is read only if this user owns it, it is a regular file (never a
//!   symlink), and no other user can write to it.
//! - A file is written to a temporary name in the same directory, synced, and
//!   renamed into place, so a reader never sees half of one.
//! - Beside each file is a `.sha256` file holding its SHA-256, written
//!   first. A torn or bit-rotted file fails the check and is never handed
//!   to wasmtime. A file is never changed in place, so the mapped bytes
//!   stay the checked bytes.
//!
//! Anyone who can write as this user can already change the binary itself,
//! so the cache adds no new way in.
//!
//! Every failure (no directory, an untrusted one, a corrupt file, a full
//! disk) falls back to compiling. The cache never fails a call.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use wasmtime::{Engine, Module};

/// Changes when what goes into the snapshot changes in a way the engine
/// bytes do not show: the init function, the imports, this file's layout.
const FORMAT: &str = "ordo-fn-engine-cache 1";
const MAGIC: &[u8; 16] = b"ordo-fn-cache-1\n";
const PREFIX: &str = "engine-";
const SUFFIX: &str = ".cwasm";
const SUM_SUFFIX: &str = ".sha256";

/// How the engine was found at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheOutcome {
    /// No cache directory is set.
    Off,
    /// Loaded from this file.
    Hit(PathBuf),
    /// No file for this key; compiled.
    Miss,
    /// A file was there and was not used; compiled. Says why.
    Rejected(String),
    /// The directory cannot be used; compiled. Says why.
    Unusable(String),
}

/// What happened when the compiled engine was written back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheWrite {
    /// Nothing written: a hit, or no directory.
    Skipped,
    Written(PathBuf),
    /// The write failed; the next start compiles again. Says why.
    Failed(String),
}

/// The cache directory and the key for this engine and config.
pub(crate) struct EngineCache {
    dir: PathBuf,
    file: PathBuf,
    sum: PathBuf,
}

impl EngineCache {
    pub(crate) fn new(dir: &Path, engine: &Engine, wasm: &[u8]) -> EngineCache {
        let key = key(engine, wasm);
        EngineCache {
            dir: dir.to_owned(),
            file: dir.join(format!("{PREFIX}{key}{SUFFIX}")),
            sum: dir.join(format!("{PREFIX}{key}{SUM_SUFFIX}")),
        }
    }

    /// The file this engine and config are kept in.
    pub(crate) fn path(&self) -> &Path {
        &self.file
    }

    /// The module from disk, or why there is none.
    pub(crate) fn load(&self, engine: &Engine) -> Result<Module, CacheOutcome> {
        if !self.dir.exists() {
            return Err(CacheOutcome::Miss);
        }
        trusted_dir(&self.dir, false).map_err(CacheOutcome::Unusable)?;
        for path in [&self.file, &self.sum] {
            let meta = match fs::symlink_metadata(path) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(CacheOutcome::Miss)
                }
                Err(e) => return Err(CacheOutcome::Rejected(format!("cannot read it: {e}"))),
            };
            if let Err(why) = trusted_file(&meta) {
                // Not ours to delete either; leave it and compile.
                return Err(CacheOutcome::Rejected(format!("{}: {why}", path.display())));
            }
        }
        let reject = |why: String| {
            let _ = fs::remove_file(&self.file);
            let _ = fs::remove_file(&self.sum);
            CacheOutcome::Rejected(why)
        };
        let sum = fs::read(&self.sum).map_err(|e| reject(format!("cannot read it: {e}")))?;
        let bytes = fs::read(&self.file).map_err(|e| reject(format!("cannot read it: {e}")))?;
        if sum_line(&bytes) != sum {
            return Err(reject("its checksum is wrong".into()));
        }
        drop(bytes);
        // SAFETY: the file is a regular file this user owns, in a directory
        // this user owns that no other user can write to, and its bytes
        // match the SHA-256 Ordo wrote beside it. Ordo never changes a file
        // in place. So these are bytes `Module::serialize` produced in this
        // process or an earlier one. See the module comment.
        #[allow(unsafe_code)]
        let module = unsafe { Module::deserialize_file(engine, &self.file) };
        module.map_err(|e| reject(format!("wasmtime refused it: {e:#}")))
    }

    /// Writes `module` for the next start, then removes files with other
    /// keys. Never fails the caller.
    pub(crate) fn store(&self, module: &Module) -> CacheWrite {
        match self.try_store(module) {
            Ok(()) => {
                self.prune();
                CacheWrite::Written(self.file.clone())
            }
            Err(e) => CacheWrite::Failed(e),
        }
    }

    fn try_store(&self, module: &Module) -> Result<(), String> {
        trusted_dir(&self.dir, true)?;
        let payload = module
            .serialize()
            .map_err(|e| format!("serialize: {e:#}"))?;
        // The checksum first: a crash between the two leaves a pair that
        // does not match, which the next start compiles over.
        write_atomic(&self.dir, &self.sum, &sum_line(&payload))?;
        write_atomic(&self.dir, &self.file, &payload)
    }

    /// Removes engine files for other keys, and temporary files a crashed
    /// start left over an hour ago.
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let old = SystemTime::now() - Duration::from_secs(3600);
        for e in entries.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let ours = name.starts_with(PREFIX)
                && (name.ends_with(SUFFIX) || name.ends_with(SUM_SUFFIX))
                && path != self.file
                && path != self.sum;
            let stale_tmp = name.starts_with(&format!(".{PREFIX}"))
                && name.ends_with(".tmp")
                && e.metadata()
                    .and_then(|m| m.modified())
                    .is_ok_and(|t| t < old);
            if ours || stale_tmp {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn key(engine: &Engine, wasm: &[u8]) -> String {
    let mut compat = DefaultHasher::new();
    engine.precompile_compatibility_hash().hash(&mut compat);
    let mut h = Sha256::new();
    h.update(FORMAT.as_bytes());
    h.update((wasm.len() as u64).to_le_bytes());
    h.update(wasm);
    h.update(compat.finish().to_le_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// What the `.sha256` file holds for `bytes`.
fn sum_line(bytes: &[u8]) -> Vec<u8> {
    let mut v = MAGIC.to_vec();
    v.extend_from_slice(&Sha256::digest(bytes));
    v
}

/// Writes `bytes` to a temporary name in `dir`, syncs, and renames it to
/// `to`, so a reader sees the old file or the whole new one.
fn write_atomic(dir: &Path, to: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = dir.join(format!(
        ".{PREFIX}{}-{}.tmp",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let write = || -> std::io::Result<()> {
        let mut f = create_private(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, to)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("write {}: {e}", to.display())
    })
}

/// Makes `dir` (mode 0700) when `create`, then checks this user owns it and
/// no one else can write to it.
fn trusted_dir(dir: &Path, create: bool) -> Result<(), String> {
    if create {
        let mut b = fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let meta = match fs::symlink_metadata(dir) {
        Ok(m) => m,
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    if !meta.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    owned_and_private(&meta).map_err(|why| format!("{}: {why}", dir.display()))
}

fn trusted_file(meta: &fs::Metadata) -> Result<(), String> {
    if !meta.file_type().is_file() {
        return Err("it is not a regular file".into());
    }
    owned_and_private(meta)
}

#[cfg(unix)]
fn owned_and_private(meta: &fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let me = rustix::process::geteuid().as_raw();
    if meta.uid() != me {
        return Err(format!(
            "owned by uid {}, not this process's {me}",
            meta.uid()
        ));
    }
    if meta.mode() & 0o022 != 0 {
        return Err("another user can write to it".into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn owned_and_private(_meta: &fs::Metadata) -> Result<(), String> {
    Err("the engine cache is supported on Unix only".into())
}

fn create_private(path: &Path) -> std::io::Result<fs::File> {
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest valid module: a header and nothing else.
    const EMPTY: &[u8] = b"\0asm\x01\0\0\0";

    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("ordo-fn-cache-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn compiled(engine: &Engine) -> Module {
        Module::new(engine, EMPTY).expect("compile")
    }

    #[test]
    fn a_miss_writes_and_the_next_start_hits() {
        let dir = scratch("hit");
        let engine = Engine::default();
        let c = EngineCache::new(&dir, &engine, EMPTY);
        assert_eq!(c.load(&engine).err(), Some(CacheOutcome::Miss));
        assert!(matches!(
            c.store(&compiled(&engine)),
            CacheWrite::Written(_)
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
            let mode = fs::metadata(&c.file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert!(c.load(&engine).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_engine_or_config_is_a_new_key_and_the_old_file_goes() {
        let dir = scratch("stale");
        let engine = Engine::default();
        let old = EngineCache::new(&dir, &engine, EMPTY);
        old.store(&compiled(&engine));
        // Another engine build.
        let other = b"\0asm\x01\0\0\0\0\x01\x00";
        let new = EngineCache::new(&dir, &engine, other);
        assert_ne!(old.file, new.file);
        assert_eq!(new.load(&engine).err(), Some(CacheOutcome::Miss));
        // Another config.
        let mut cfg = wasmtime::Config::new();
        cfg.consume_fuel(true);
        let fueled = Engine::new(&cfg).unwrap();
        let by_config = EngineCache::new(&dir, &fueled, EMPTY);
        assert_ne!(old.file, by_config.file);
        assert_eq!(by_config.load(&fueled).err(), Some(CacheOutcome::Miss));
        // Writing the new key removes the old file.
        new.store(&compiled(&engine));
        assert!(!old.file.exists());
        assert!(new.file.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_rejected_removed_and_rewritten() {
        let dir = scratch("corrupt");
        let engine = Engine::default();
        let c = EngineCache::new(&dir, &engine, EMPTY);
        c.store(&compiled(&engine));
        // One flipped bit in the payload.
        let mut bytes = fs::read(&c.file).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&c.file, &bytes).unwrap();
        match c.load(&engine) {
            Err(CacheOutcome::Rejected(why)) => assert!(why.contains("checksum"), "{why}"),
            other => panic!("{:?}", other.map(|_| ())),
        }
        assert!(!c.file.exists(), "a corrupt file is removed");
        assert!(!c.sum.exists(), "and its checksum");
        // Garbage with a good checksum: wasmtime's own check refuses it.
        let junk = b"not a module".to_vec();
        fs::write(&c.file, &junk).unwrap();
        fs::write(&c.sum, sum_line(&junk)).unwrap();
        match c.load(&engine) {
            Err(CacheOutcome::Rejected(why)) => assert!(why.contains("wasmtime"), "{why}"),
            other => panic!("{:?}", other.map(|_| ())),
        }
        // Truncated to nothing.
        fs::write(&c.file, b"").unwrap();
        fs::write(&c.sum, sum_line(b"x")).unwrap();
        assert!(matches!(c.load(&engine), Err(CacheOutcome::Rejected(_))));
        // The next store puts a good one back.
        assert!(matches!(
            c.store(&compiled(&engine)),
            CacheWrite::Written(_)
        ));
        assert!(c.load(&engine).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_others_can_write_is_not_used() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("open");
        let engine = Engine::default();
        let c = EngineCache::new(&dir, &engine, EMPTY);
        c.store(&compiled(&engine));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(matches!(c.load(&engine), Err(CacheOutcome::Unusable(_))));
        assert!(matches!(c.store(&compiled(&engine)), CacheWrite::Failed(_)));
        // A file others can write is not loaded either.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&c.file, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(matches!(c.load(&engine), Err(CacheOutcome::Rejected(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_in_place_of_the_file_is_not_followed() {
        let dir = scratch("link");
        let engine = Engine::default();
        let c = EngineCache::new(&dir, &engine, EMPTY);
        c.store(&compiled(&engine));
        let real = dir.join("elsewhere");
        fs::rename(&c.file, &real).unwrap();
        std::os::unix::fs::symlink(&real, &c.file).unwrap();
        assert!(matches!(c.load(&engine), Err(CacheOutcome::Rejected(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_directory_fails_the_write_and_nothing_else() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("readonly");
        let engine = Engine::default();
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let c = EngineCache::new(&dir, &engine, EMPTY);
        assert_eq!(c.load(&engine).err(), Some(CacheOutcome::Miss));
        assert!(matches!(c.store(&compiled(&engine)), CacheWrite::Failed(_)));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
