//! Hardening the console process, in both builds (`plan-console.md` 10).

/// Mark the console not dumpable: the kernel then refuses any other
/// process, even one of the console's own unix user, the console's memory,
/// environment and open files under `/proc`, and refuses to attach a
/// debugger. This is the part of `hidepid` a container can set for itself;
/// the mount option needs the platform. Best effort; Linux only.
pub fn not_dumpable() {
    #[cfg(target_os = "linux")]
    if let Err(e) =
        rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
    {
        eprintln!("ordo-console: could not mark itself not dumpable: {e}");
    }
}
