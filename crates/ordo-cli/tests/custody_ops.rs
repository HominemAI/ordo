//! Reported fix 25o: every op a CLI custody record carries must survive the
//! recorder's op rule unchanged.
//!
//! The recorder keeps an op only when it is `[A-Za-z0-9._-]` and at most 64
//! bytes, and records anything else as `<unprintable>`. `ordo keys init`,
//! `rotate`, `reseal`, `destroy` and `ordo restore --record` once passed
//! their command line with spaces, so every one of their records lost the
//! name of the command that made it. A regression reads as `<unprintable>`
//! in the security stream, and fails here first.

#[test]
fn every_custody_op_is_recorded_as_named() {
    let ops = [
        ordo_cli::keys::OP_KEYS_INIT,
        ordo_cli::keys::OP_KEYS_ROTATE,
        ordo_cli::keys::OP_KEYS_RESEAL,
        ordo_cli::keys::OP_KEYS_DESTROY,
        ordo_cli::triggers::OP,
        ordo_exec::undo::OP_RESTORE_RECORD,
    ];
    for op in ops {
        assert_eq!(
            ordo_secevent::sanitize_op(op),
            op,
            "{op} would be recorded as <unprintable>"
        );
    }
}
