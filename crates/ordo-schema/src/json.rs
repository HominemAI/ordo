//! The door's limits on a `json` value (checklist 28b).
//!
//! A `json` field holds any JSON document, so the type itself bounds
//! nothing. Two deployment settings do, each with a shipped default:
//!
//! - `ORDO_JSON_MAX_BYTES`, default 262,144 (256 KiB): the document's size
//!   in its compact serialization, the form it is stored and logged in.
//! - `ORDO_JSON_MAX_DEPTH`, default 64: how deeply arrays and objects nest.
//!   `{}` and `[]` are depth 1, a scalar is depth 0.
//!
//! Over either one, the door answers `bound_violation`, the class a
//! parameter over its declared bound already gets. The deployment owner
//! raises a limit when an app needs larger documents; the refusal names
//! the variable so the caller knows who can.
//!
//! A string holding U+0000, in a value or a key, is `invalid_param`:
//! Postgres `jsonb` cannot store it, and the statement would otherwise
//! fail as a 500.

use std::sync::OnceLock;

use serde_json::Value;

/// The shipped default for [`JsonLimits::max_bytes`].
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024;
/// The shipped default for [`JsonLimits::max_depth`].
pub const DEFAULT_MAX_DEPTH: usize = 64;

/// How large and how deep one `json` value may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonLimits {
    /// `ORDO_JSON_MAX_BYTES`: the compact serialization's length in bytes.
    pub max_bytes: usize,
    /// `ORDO_JSON_MAX_DEPTH`: the deepest nesting of arrays and objects.
    pub max_depth: usize,
}

impl Default for JsonLimits {
    fn default() -> Self {
        JsonLimits {
            max_bytes: DEFAULT_MAX_BYTES,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }
}

impl JsonLimits {
    /// The environment's values, the defaults for what it leaves unset.
    ///
    /// A value that is not a number, or zero, reads as unset here. The
    /// host's `RuntimeConfig::from_env` refuses the boot on the same
    /// variables, so a deployment never runs on a default it did not
    /// choose, while a tool or test that sets nothing still works.
    pub fn from_env() -> JsonLimits {
        let var = |k: &str| std::env::var(k).ok();
        JsonLimits::parse(
            var("ORDO_JSON_MAX_BYTES").as_deref(),
            var("ORDO_JSON_MAX_DEPTH").as_deref(),
        )
    }

    /// [`JsonLimits::from_env`] over given values, testable without
    /// touching the process environment.
    pub fn parse(max_bytes: Option<&str>, max_depth: Option<&str>) -> JsonLimits {
        let num = |v: Option<&str>, d: usize| {
            v.and_then(|s| s.trim().parse::<usize>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(d)
        };
        JsonLimits {
            max_bytes: num(max_bytes, DEFAULT_MAX_BYTES),
            max_depth: num(max_depth, DEFAULT_MAX_DEPTH),
        }
    }
}

/// The deployment's limits, read from the environment once per process.
///
/// Read once and kept, like every other door setting: the process does
/// not change its limits under a running request.
pub fn limits() -> JsonLimits {
    static LIMITS: OnceLock<JsonLimits> = OnceLock::new();
    *LIMITS.get_or_init(JsonLimits::from_env)
}

/// Why a `json` value was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum JsonRefusal {
    /// Larger than `ORDO_JSON_MAX_BYTES`. `bound_violation`.
    TooLarge { bytes: usize, max: usize },
    /// Deeper than `ORDO_JSON_MAX_DEPTH`. `bound_violation`.
    TooDeep { depth: usize, max: usize },
    /// A string or key holds U+0000. `invalid_param`.
    Nul,
}

impl std::fmt::Display for JsonRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonRefusal::TooLarge { bytes, max } => write!(
                f,
                "is {bytes} bytes, and the limit is {max}. The limit is the deployment \
                 setting `ORDO_JSON_MAX_BYTES` (default {DEFAULT_MAX_BYTES}); ask the \
                 deployment owner to raise it"
            ),
            JsonRefusal::TooDeep { depth, max } => write!(
                f,
                "nests {depth} levels deep, and the limit is {max}. The limit is the \
                 deployment setting `ORDO_JSON_MAX_DEPTH` (default {DEFAULT_MAX_DEPTH}); ask \
                 the deployment owner to raise it"
            ),
            JsonRefusal::Nul => write!(
                f,
                "holds a NUL character (U+0000) in a string or key, which a stored \
                 document cannot carry"
            ),
        }
    }
}

/// Check one document against the limits.
///
/// Depth is checked first and without recursion, so a hostile document
/// cannot spend the stack proving it is too deep.
pub fn check(v: &Value, limits: &JsonLimits) -> Result<(), JsonRefusal> {
    let mut stack: Vec<(&Value, usize)> = vec![(v, 0)];
    while let Some((node, depth)) = stack.pop() {
        match node {
            Value::Array(items) => {
                let d = depth + 1;
                if d > limits.max_depth {
                    return Err(JsonRefusal::TooDeep {
                        depth: d,
                        max: limits.max_depth,
                    });
                }
                stack.extend(items.iter().map(|i| (i, d)));
            }
            Value::Object(map) => {
                let d = depth + 1;
                if d > limits.max_depth {
                    return Err(JsonRefusal::TooDeep {
                        depth: d,
                        max: limits.max_depth,
                    });
                }
                for (k, i) in map {
                    if k.contains('\0') {
                        return Err(JsonRefusal::Nul);
                    }
                    stack.push((i, d));
                }
            }
            Value::String(s) if s.contains('\0') => return Err(JsonRefusal::Nul),
            _ => {}
        }
    }
    let bytes = serde_json::to_string(v).map(|s| s.len()).unwrap_or(0);
    if bytes > limits.max_bytes {
        return Err(JsonRefusal::TooLarge {
            bytes,
            max: limits.max_bytes,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_scalar_is_depth_zero_and_an_empty_container_is_depth_one() {
        let one = JsonLimits {
            max_bytes: 100,
            max_depth: 1,
        };
        assert!(check(&json!("x"), &one).is_ok());
        assert!(check(&json!({}), &one).is_ok());
        assert!(check(&json!([1, 2]), &one).is_ok());
        assert_eq!(
            check(&json!({"a": []}), &one),
            Err(JsonRefusal::TooDeep { depth: 2, max: 1 })
        );
    }

    #[test]
    fn size_counts_the_compact_serialization() {
        let l = JsonLimits {
            max_bytes: 7,
            max_depth: 8,
        };
        assert!(
            check(&json!({"a": 1}), &l).is_ok(),
            "`{{\"a\":1}}` is 7 bytes"
        );
        assert!(matches!(
            check(&json!({"a": 10}), &l),
            Err(JsonRefusal::TooLarge { bytes: 8, max: 7 })
        ));
    }

    #[test]
    fn a_nul_anywhere_is_refused() {
        let l = JsonLimits::default();
        assert_eq!(check(&json!({"a": "x\u{0}"}), &l), Err(JsonRefusal::Nul));
        assert_eq!(check(&json!({"a\u{0}": 1}), &l), Err(JsonRefusal::Nul));
    }

    #[test]
    fn unset_zero_or_garbage_settings_read_as_the_defaults() {
        assert_eq!(JsonLimits::parse(None, None), JsonLimits::default());
        assert_eq!(
            JsonLimits::parse(Some("0"), Some("deep")),
            JsonLimits::default()
        );
        assert_eq!(
            JsonLimits::parse(Some("1024"), Some("8")),
            JsonLimits {
                max_bytes: 1024,
                max_depth: 8
            }
        );
    }
}
