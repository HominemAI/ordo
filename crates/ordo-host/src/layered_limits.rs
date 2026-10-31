//! Numbers in three layers: the deployment's defaults and absolute limits,
//! the app developer's values and optional ranges, and a tenant's own values
//! inside them. Written for a share link's password limits
//! ([`crate::link_password`]) and used again for the password slowdown
//! ([`crate::guess_limits`]), so both read, check and answer the same way.
//!
//! | Layer | Sets | How |
//! |---|---|---|
//! | Deployment | a default and an absolute limit per value | one variable each, and a `_LIMIT` each |
//! | App developer | its own values, and optionally a narrower range per value for tenants | a `std.deployment.*` setter |
//! | Tenant | its own values, looser or tighter, inside the app developer's range, or inside the deployment's limits when the app developer set none | a `std.space.*` setter |
//!
//! The value in force is the tenant's, else the app developer's, else the
//! deployment's. A stored value that a later change left outside its range
//! is held to the nearest end of it.
//!
//! A value that may be **off** is the number 0, written `off`. Where
//! [`Def::tenant_off`] is false, a tenant may choose off only when the app
//! developer's range for it starts at 0: off is the looser mode, and the
//! looser mode is opted into by the layer above.

use std::marker::PhantomData;

use ordo_api::{BoundParams, ErrorCode, ExecError};
use serde_json::{json, Value};

/// The most values one family holds.
pub const MAX: usize = 8;

/// The number that means off.
pub const OFF: u64 = 0;

/// Which way is looser, and so what the deployment's `_LIMIT` bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Looser {
    /// A larger number is looser: the limit is the largest any layer may
    /// choose. Off (0) is allowed when [`Def::off`].
    More,
    /// A smaller number is looser: the limit is the smallest any layer may
    /// choose, up to `ceiling`.
    Less { ceiling: u64 },
}

/// One value of a family.
#[derive(Debug)]
pub struct Def {
    /// The parameter and answer word.
    pub word: &'static str,
    /// The app developer's range parameter, and its answer word.
    pub range_word: &'static str,
    /// The answer word saying whose the value is.
    pub from_word: &'static str,
    /// The answer word saying whose the range is.
    pub range_from_word: &'static str,
    pub env: &'static str,
    pub limit_env: &'static str,
    /// The tenant's column on `$admissionpolicy`.
    pub column: &'static str,
    pub default: u64,
    pub default_limit: u64,
    /// What the `_LIMIT` may itself be.
    pub limit_span: (u64, u64),
    pub looser: Looser,
    /// The value may be `off` (0).
    pub off: bool,
    /// A tenant may choose off when the app developer set no range.
    pub tenant_off: bool,
}

/// A family of values: its definitions and the names its refusals use.
pub trait Family: Copy + Eq + std::fmt::Debug + 'static {
    /// Every value, in order.
    const ALL: &'static [Self];
    /// The refusal code for a value that is not understood.
    const BAD: &'static str;
    /// The refusal code for a value outside its range.
    const OUT_OF_RANGE: &'static str;
    /// The app developer's setter, named in a tenant's refusal.
    const APP_OP: &'static str;
    fn def(self) -> &'static Def;
    fn i(self) -> usize;
}

fn refused(why: impl Into<String>) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, why.into())
}

/// The deployment's layer: a default and an absolute limit per value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deployment<F: Family> {
    pub values: [u64; MAX],
    pub limits: [u64; MAX],
    _f: PhantomData<F>,
}

impl<F: Family> Default for Deployment<F> {
    fn default() -> Deployment<F> {
        let mut d = Deployment {
            values: [0; MAX],
            limits: [0; MAX],
            _f: PhantomData,
        };
        for f in F::ALL {
            d.values[f.i()] = f.def().default;
            d.limits[f.i()] = f.def().default_limit;
        }
        d
    }
}

fn env_number(var: &'static str, off_word: bool) -> Result<Option<u64>, (&'static str, String)> {
    match std::env::var(var) {
        Err(_) => Ok(None),
        Ok(v) if v.trim().is_empty() => Ok(None),
        Ok(v) if off_word && v.trim().eq_ignore_ascii_case("off") => Ok(Some(OFF)),
        Ok(v) => v.trim().parse::<u64>().map(Some).map_err(|_| {
            let or_off = if off_word { ", or `off`" } else { "" };
            (var, format!("`{v}` is not a whole number{or_off}"))
        }),
    }
}

impl<F: Family> Deployment<F> {
    pub fn value(&self, f: F) -> u64 {
        self.values[f.i()]
    }

    pub fn with_value(mut self, f: F, v: u64) -> Deployment<F> {
        self.values[f.i()] = v;
        self
    }

    pub fn with_limit(mut self, f: F, v: u64) -> Deployment<F> {
        self.limits[f.i()] = v;
        self
    }

    /// The range any layer may choose `f` from.
    pub fn bounds(&self, f: F) -> (u64, u64) {
        let limit = self.limits[f.i()];
        let d = f.def();
        match d.looser {
            Looser::More => (if d.off { OFF } else { 1 }, limit),
            Looser::Less { ceiling } => (limit, ceiling),
        }
    }

    /// The range a tenant may choose `f` from when the app developer set
    /// none: the deployment's, without off unless [`Def::tenant_off`].
    pub fn tenant_bounds(&self, f: F) -> (u64, u64) {
        let (lo, hi) = self.bounds(f);
        let d = f.def();
        if d.off && !d.tenant_off {
            (lo.max(1), hi)
        } else {
            (lo, hi)
        }
    }

    /// Read the variables. A limit outside what it may be, or a default
    /// outside its limit, refuses the boot naming the variable.
    pub fn from_env() -> Result<Deployment<F>, (&'static str, String)> {
        let mut d = Deployment::<F>::default();
        for f in F::ALL {
            let def = f.def();
            if let Some(n) = env_number(def.limit_env, false)? {
                let (lo, hi) = def.limit_span;
                if n < lo || n > hi {
                    return Err((def.limit_env, format!("{n} is not from {lo} to {hi}")));
                }
                d.limits[f.i()] = n;
            }
        }
        for f in F::ALL {
            let def = f.def();
            let (lo, hi) = d.bounds(*f);
            match env_number(def.env, def.off)? {
                Some(n) if n < lo || n > hi => {
                    return Err((
                        def.env,
                        format!(
                            "{n} is outside {lo} to {hi}, the deployment's limits ({})",
                            def.limit_env
                        ),
                    ))
                }
                Some(n) => d.values[f.i()] = n,
                // An unset default is held inside a limit that was set.
                None => d.values[f.i()] = d.values[f.i()].clamp(lo, hi),
            }
        }
        Ok(d)
    }
}

/// One stored layer. A tenant's has values only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer<F: Family> {
    pub values: [Option<u64>; MAX],
    pub ranges: [Option<(u64, u64)>; MAX],
    _f: PhantomData<F>,
}

impl<F: Family> Default for Layer<F> {
    fn default() -> Layer<F> {
        Layer {
            values: [None; MAX],
            ranges: [None; MAX],
            _f: PhantomData,
        }
    }
}

impl<F: Family> Layer<F> {
    pub fn value(&self, f: F) -> Option<u64> {
        self.values[f.i()]
    }

    pub fn set_value(&mut self, f: F, v: Option<u64>) {
        self.values[f.i()] = v;
    }

    pub fn set_range(&mut self, f: F, r: Option<(u64, u64)>) {
        self.ranges[f.i()] = r;
    }

    /// The app developer's layer as stored in `$deploymentsetting`.
    pub fn to_json(&self) -> Value {
        let mut o = serde_json::Map::new();
        for f in F::ALL {
            if let Some(v) = self.values[f.i()] {
                o.insert(f.def().word.into(), json!(v));
            }
            if let Some((lo, hi)) = self.ranges[f.i()] {
                o.insert(f.def().range_word.into(), json!([lo, hi]));
            }
        }
        Value::Object(o)
    }

    pub fn from_json(v: &Value) -> Layer<F> {
        let mut l = Layer::default();
        for f in F::ALL {
            let d = f.def();
            l.values[f.i()] = v[d.word].as_u64();
            l.ranges[f.i()] = match (v[d.range_word][0].as_u64(), v[d.range_word][1].as_u64()) {
                (Some(lo), Some(hi)) => Some((lo, hi)),
                _ => None,
            };
        }
        l
    }

    /// The tenant's values from a row of its columns, in [`Family::ALL`]
    /// order.
    pub fn from_columns(row: Option<&Vec<Option<String>>>) -> Layer<F> {
        let mut l = Layer::default();
        for (n, f) in F::ALL.iter().enumerate() {
            l.values[f.i()] = row
                .and_then(|r| r.get(n))
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse::<u64>().ok());
        }
        l
    }
}

/// The values in force for a tenant, where each came from (`tenant`, `app`,
/// `deployment`), and the range a tenant may choose from and whose it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective<F: Family> {
    pub values: [u64; MAX],
    pub from: [&'static str; MAX],
    pub tenant_range: [(u64, u64); MAX],
    pub range_from: [&'static str; MAX],
    _f: PhantomData<F>,
}

impl<F: Family> Effective<F> {
    pub fn get(&self, f: F) -> u64 {
        self.values[f.i()]
    }

    pub fn from_of(&self, f: F) -> &'static str {
        self.from[f.i()]
    }

    /// What the setters answer.
    pub fn row(&self) -> Value {
        let mut o = serde_json::Map::new();
        for f in F::ALL {
            let d = f.def();
            let (lo, hi) = self.tenant_range[f.i()];
            o.insert(d.word.into(), json!(self.values[f.i()]));
            o.insert(d.from_word.into(), json!(self.from[f.i()]));
            o.insert(d.range_word.into(), json!([lo, hi]));
            o.insert(d.range_from_word.into(), json!(self.range_from[f.i()]));
        }
        Value::Object(o)
    }
}

/// The answer fields of a family's setters.
pub fn answer_fields<F: Family>() -> Vec<&'static str> {
    F::ALL
        .iter()
        .flat_map(|f| {
            let d = f.def();
            [d.word, d.from_word, d.range_word, d.range_from_word]
        })
        .collect()
}

/// The app developer's range for `f`, held inside the deployment's limits.
fn app_range<F: Family>(d: &Deployment<F>, app: &Layer<F>, f: F) -> Option<(u64, u64)> {
    let (lo, hi) = d.bounds(f);
    app.ranges[f.i()]
        .map(|(a, b)| (a.max(lo), b.min(hi)))
        .filter(|(a, b)| a <= b)
}

pub fn resolve<F: Family>(d: &Deployment<F>, app: &Layer<F>, tenant: &Layer<F>) -> Effective<F> {
    let mut e = Effective {
        values: [0; MAX],
        from: ["deployment"; MAX],
        tenant_range: [(0, 0); MAX],
        range_from: ["deployment"; MAX],
        _f: PhantomData,
    };
    for f in F::ALL {
        let f = *f;
        let bounds = d.bounds(f);
        let range = match app_range(d, app, f) {
            Some(r) => {
                e.range_from[f.i()] = "app";
                r
            }
            None => d.tenant_bounds(f),
        };
        e.tenant_range[f.i()] = range;
        let (v, from) = if let Some(t) = tenant.values[f.i()] {
            (t.clamp(range.0, range.1), "tenant")
        } else if let Some(a) = app.values[f.i()] {
            (a.clamp(bounds.0, bounds.1), "app")
        } else {
            (d.values[f.i()].clamp(bounds.0, bounds.1), "deployment")
        };
        e.values[f.i()] = v;
        e.from[f.i()] = from;
    }
    e
}

/// A value's change: `None` leaves it, `Some(None)` clears it (`default`).
pub fn value_change<F: Family>(
    bound: &BoundParams,
    f: F,
) -> Result<Option<Option<u64>>, ExecError> {
    let d = f.def();
    let Some(word) = word_of(bound, d.word) else {
        return Ok(None);
    };
    if word.eq_ignore_ascii_case("default") {
        return Ok(Some(None));
    }
    if d.off && word.eq_ignore_ascii_case("off") {
        return Ok(Some(Some(OFF)));
    }
    word.parse::<u64>().map(|n| Some(Some(n))).map_err(|_| {
        let or_off = if d.off { ", `off`" } else { "" };
        refused(format!(
            "{}: `{}` is a whole number{or_off}, or `default`",
            F::BAD,
            d.word
        ))
    })
}

/// A range's change, `lo-hi`: `None` leaves it, `Some(None)` clears it
/// (`none`).
pub fn range_change<F: Family>(
    bound: &BoundParams,
    f: F,
) -> Result<Option<Option<(u64, u64)>>, ExecError> {
    let d = f.def();
    let Some(word) = word_of(bound, d.range_word) else {
        return Ok(None);
    };
    if word.eq_ignore_ascii_case("none") {
        return Ok(Some(None));
    }
    let bad = || {
        refused(format!(
            "{}: `{}` is a range like `1-10`, or `none`",
            F::BAD,
            d.range_word
        ))
    };
    let (a, b) = word.split_once('-').ok_or_else(bad)?;
    let lo = a.trim().parse::<u64>().map_err(|_| bad())?;
    let hi = b.trim().parse::<u64>().map_err(|_| bad())?;
    if lo > hi {
        return Err(bad());
    }
    Ok(Some(Some((lo, hi))))
}

fn word_of<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The app developer's values and ranges sit inside the deployment's
/// limits, or the refusal names the limit.
pub fn check_app<F: Family>(d: &Deployment<F>, app: &Layer<F>) -> Result<(), ExecError> {
    for f in F::ALL {
        let f = *f;
        let (lo, hi) = d.bounds(f);
        let def = f.def();
        let why = |what: String| {
            refused(format!(
                "{}: {what} is outside {lo} to {hi}, the deployment's limits. The deployment \
                 owner changes them with {}",
                F::OUT_OF_RANGE,
                def.limit_env
            ))
        };
        if let Some(v) = app.values[f.i()].filter(|v| *v < lo || *v > hi) {
            return Err(why(format!("`{}` {v}", def.word)));
        }
        if let Some((a, b)) = app.ranges[f.i()].filter(|(a, b)| *a < lo || *b > hi) {
            return Err(why(format!("`{}` {a}-{b}", def.range_word)));
        }
    }
    Ok(())
}

/// A tenant's values sit inside the app developer's range, or the
/// deployment's limits when there is none. Looser or tighter is the
/// tenant's choice. The refusal names the range and who sets it.
pub fn check_tenant<F: Family>(
    d: &Deployment<F>,
    app: &Layer<F>,
    tenant: &Layer<F>,
) -> Result<(), ExecError> {
    for f in F::ALL {
        let f = *f;
        let def = f.def();
        let Some(v) = tenant.values[f.i()] else {
            continue;
        };
        let (lo, hi, whose) = match app_range(d, app, f) {
            Some((lo, hi)) => (
                lo,
                hi,
                format!(
                    "the app developer's range. The app developer changes it with {} {}",
                    F::APP_OP,
                    def.range_word
                ),
            ),
            None => {
                let (lo, hi) = d.tenant_bounds(f);
                let whose = if lo > d.bounds(f).0 {
                    format!(
                        "the deployment's limits; off needs the app developer's range to start \
                         at 0 ({} {}=0-{hi})",
                        F::APP_OP,
                        def.range_word
                    )
                } else {
                    format!(
                        "the deployment's limits. The deployment owner changes them with {}",
                        def.limit_env
                    )
                };
                (lo, hi, whose)
            }
        };
        if v < lo || v > hi {
            return Err(refused(format!(
                "{}: `{}` {v} is outside {lo} to {hi}, {whose}",
                F::OUT_OF_RANGE,
                def.word
            )));
        }
    }
    Ok(())
}
