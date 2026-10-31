//! A tenant's own CORS origins (`design-public-path.md` §8; ruled
//! `public-data` asks 14 and 26).
//!
//! A page embedded on another site calls Ordo from that site's origin. The
//! deployment's list (`ORDO_CORS_ORIGINS`) is one list for every tenant, so a
//! tenant with an embed of its own needs a list of its own.
//!
//! - **Layered.** The effective list for a space is the deployment's list
//!   plus that tenant's own. The layer is `ordo_api::cors`.
//! - **Off until the deployment allows it.** `ORDO_CORS_TENANT_ORIGINS_MAX`
//!   is how many origins one tenant may list. The default, 0, lets no
//!   tenant list any, and the refusal names the setting.
//! - **Exact origins only.** `*` and `https://*.<domain>` are refused on a
//!   tenant's list, whatever `ORDO_WILDCARD_ORIGINS` says: a wildcard is the
//!   deployment's choice to make, not a tenant's.
//! - **The owner's.** `std.space.setOrigins` and `std.space.origins` are
//!   owner-only, like the admission policy.
//!
//! Each node keeps the lists in memory. The node that took the change has it
//! at once. The others re-read it on the access refresh period
//! (`tenant-origins.refresh`).

use std::collections::HashMap;
use std::sync::RwLock;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_exec::{sql_lit, Pool, PoolError};
use serde_json::json;

pub const SET_ORIGINS: &str = "std.space.setOrigins";
pub const ORIGINS: &str = "std.space.origins";
pub const OPS: [&str; 2] = [SET_ORIGINS, ORIGINS];
pub const MAX_ENV: &str = "ORDO_CORS_TENANT_ORIGINS_MAX";
/// The largest value `ORDO_CORS_TENANT_ORIGINS_MAX` takes.
pub const MAX_CEILING: usize = 1000;

pub const DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$spaceorigins" (
  "space"   text PRIMARY KEY,
  "origins" text NOT NULL DEFAULT '',
  "by"      text NOT NULL DEFAULT '',
  "at"      bigint NOT NULL DEFAULT 0
)"#;

/// `ORDO_CORS_TENANT_ORIGINS_MAX`, 0 to 1000, default 0.
pub fn max_from_vars(
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<usize, (&'static str, String)> {
    match var(MAX_ENV).map(|v| v.trim().to_string()) {
        None => Ok(0),
        Some(v) if v.is_empty() => Ok(0),
        Some(v) => v
            .parse::<usize>()
            .ok()
            .filter(|n| *n <= MAX_CEILING)
            .ok_or_else(|| {
                (
                    MAX_ENV,
                    format!(
                        "`{v}` is not a whole number from 0 to {MAX_CEILING}; unset it for the \
                         default, 0 (no tenant may list origins)"
                    ),
                )
            }),
    }
}

/// Parse a tenant's comma list: exact origins only, at most `max`.
pub fn parse_list(text: &str, max: usize) -> Result<Vec<String>, String> {
    if text.contains('*') {
        return Err(
            "a tenant's list takes exact origins only, such as https://shop.example.com; a \
             wildcard is the deployment's to set in ORDO_CORS_ORIGINS"
                .into(),
        );
    }
    let origins = ordo_api::cors::CorsPolicy::parse(text)?
        .map(|p| p.origins().to_vec())
        .unwrap_or_default();
    if origins.len() > max {
        return Err(if max == 0 {
            format!(
                "this deployment lets no tenant list its own origins. The deployment owner \
                 allows it with {MAX_ENV}=<how many>, for example {MAX_ENV}=10"
            )
        } else {
            format!(
                "{} origins is more than this deployment allows a tenant, {max}. The \
                 deployment owner can raise {MAX_ENV}",
                origins.len()
            )
        });
    }
    Ok(origins)
}

/// The tenants' lists, in memory, with the table they are kept in.
pub struct TenantOriginMap {
    max: usize,
    pool: Pool,
    lists: RwLock<HashMap<String, Vec<String>>>,
}

impl TenantOriginMap {
    pub fn new(max: usize, pool: Pool) -> TenantOriginMap {
        TenantOriginMap {
            max,
            pool,
            lists: RwLock::new(HashMap::new()),
        }
    }

    pub fn max(&self) -> usize {
        self.max
    }

    pub async fn ensure_table(&self) -> Result<(), PoolError> {
        self.pool.rows(DDL).await.map(|_| ())
    }

    /// Re-read every list. A list stored when the setting was higher keeps
    /// only as many origins as the setting allows now, so lowering it takes
    /// effect without anyone editing a tenant.
    pub async fn load(&self) -> Result<usize, String> {
        let rows = self
            .pool
            .rows(r#"SELECT "space", "origins" FROM "$spaceorigins" WHERE "origins" <> ''"#)
            .await
            .map_err(|e| e.to_string())?;
        let mut next = HashMap::new();
        for row in rows {
            let (Some(space), Some(text)) = (
                row.first().cloned().flatten(),
                row.get(1).cloned().flatten(),
            ) else {
                continue;
            };
            let mut list = ordo_api::cors::CorsPolicy::parse(&text)
                .ok()
                .flatten()
                .map(|p| p.origins().to_vec())
                .unwrap_or_default();
            list.truncate(self.max);
            if !list.is_empty() {
                next.insert(space, list);
            }
        }
        let n = next.len();
        *self.lists.write().unwrap_or_else(|p| p.into_inner()) = next;
        Ok(n)
    }

    pub fn get(&self, space: &str) -> Vec<String> {
        self.lists
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(space)
            .cloned()
            .unwrap_or_default()
    }

    async fn set(
        &self,
        space: &str,
        text: &str,
        by: &str,
        at: u64,
    ) -> Result<Vec<String>, ExecError> {
        let list =
            parse_list(text, self.max).map_err(|e| ExecError::new(ErrorCode::TenantRefused, e))?;
        let stored = list.join(",");
        self.pool
            .rows(&format!(
                r#"INSERT INTO "$spaceorigins" ("space", "origins", "by", "at")
                   VALUES ({}, {}, {}, {at})
                   ON CONFLICT ("space") DO UPDATE SET
                     "origins" = EXCLUDED."origins", "by" = EXCLUDED."by", "at" = EXCLUDED."at""#,
                sql_lit(space),
                sql_lit(&stored),
                sql_lit(by),
            ))
            .await
            .map_err(|e| {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
            })?;
        let mut lists = self.lists.write().unwrap_or_else(|p| p.into_inner());
        if list.is_empty() {
            lists.remove(space);
        } else {
            lists.insert(space.to_string(), list.clone());
        }
        Ok(list)
    }

    /// Run `std.space.setOrigins` or `std.space.origins`.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
        at: u64,
    ) -> Result<ExecResult, ExecError> {
        let list = match op {
            SET_ORIGINS => {
                let text = bound
                    .0
                    .get("origins")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                self.set(space, text, caller, at).await?
            }
            ORIGINS => self.get(space),
            other => {
                return Err(ExecError::internal(format!(
                    "`{other}` is not a tenant origins op"
                )))
            }
        };
        Ok(ExecResult {
            position: None,
            rows: vec![json!({ "origins": list.join(","), "max": self.max })],
            next_cursor: None,
        })
    }
}

impl ordo_api::cors::TenantOrigins for TenantOriginMap {
    fn allows(&self, space: &str, origin: &str) -> bool {
        self.lists
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(space)
            .is_some_and(|l| l.iter().any(|o| o == origin))
    }

    fn any_allows(&self, origin: &str) -> bool {
        self.lists
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .any(|l| l.iter().any(|o| o == origin))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let m: Vec<_> = pairs.to_vec();
        move |k: &str| m.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_setting_defaults_to_zero_and_is_bounded() {
        assert_eq!(max_from_vars(&vars(&[])).unwrap(), 0);
        assert_eq!(max_from_vars(&vars(&[(MAX_ENV, "10")])).unwrap(), 10);
        let (var, detail) = max_from_vars(&vars(&[(MAX_ENV, "1001")])).unwrap_err();
        assert_eq!(var, MAX_ENV);
        assert!(detail.contains("from 0 to 1000"), "{detail}");
    }

    #[test]
    fn a_list_at_zero_names_the_setting() {
        let e = parse_list("https://shop.example.com", 0).unwrap_err();
        assert!(e.contains("ORDO_CORS_TENANT_ORIGINS_MAX=10"), "{e}");
        assert_eq!(parse_list("", 0).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn a_list_takes_exact_origins_up_to_the_max() {
        assert_eq!(
            parse_list("https://Shop.example.com, http://localhost:5173", 2).unwrap(),
            ["http://localhost:5173", "https://shop.example.com"]
        );
        let e = parse_list("https://a.example,https://b.example,https://c.example", 2).unwrap_err();
        assert!(e.contains(MAX_ENV), "{e}");
        for bad in [
            "*",
            "https://*.example.com",
            "https://a.example/path",
            "a.example",
        ] {
            assert!(parse_list(bad, 5).is_err(), "{bad}");
        }
    }
}
