//! `KeyError` classifies through methods and keeps its cause.

use ordo_crypto::KeyError;
use ordo_fault::FaultClass;
use std::error::Error;

#[test]
fn a_store_fault_keeps_its_class_and_cause() {
    let busy = KeyError::caused_by(
        FaultClass::Busy,
        "pool full",
        std::io::Error::other("no connection"),
    );
    assert!(busy.is_busy() && busy.is_store_fault());
    assert_eq!(busy.class(), FaultClass::Busy);
    assert!(!busy.is_destroyed(), "a busy store says nothing of the key");
    let cause = busy.source().and_then(|s| s.source());
    assert_eq!(
        cause.map(|c| c.to_string()).as_deref(),
        Some("no connection")
    );

    let broken = KeyError::broken("gone");
    assert_eq!(broken.class(), FaultClass::Broken);
    assert!(!broken.is_busy() && !broken.is_destroyed());
    assert_eq!(KeyError::refused("bad").class(), FaultClass::Refused);
}

#[test]
fn only_a_missing_epoch_is_certainly_destroyed() {
    assert!(KeyError::UnknownEpoch(3).is_destroyed());
    for e in [KeyError::UnknownSpace, KeyError::NotLeased, KeyError::Seal] {
        assert!(!e.is_destroyed() && !e.is_store_fault(), "{e}");
        assert!(e.source().is_none());
    }
    // An envelope that will not open with its key present is unreadable,
    // possibly tampered (owner, 2026-10-23), and never retryable.
    assert_eq!(KeyError::Seal.class(), FaultClass::Unreadable);
    assert!(KeyError::Seal.is_unreadable() && !KeyError::UnknownEpoch(3).is_unreadable());
    assert_eq!(KeyError::UnknownSpace.class(), FaultClass::Refused);
    assert_eq!(KeyError::NotLeased.class(), FaultClass::Refused);
}
