//! A database of a service's own, named by one connection string.
//!
//! The console (`ORDO_CONSOLE_DATABASE_URL`) and the identity service
//! (`ORDO_IDENTITY_DATABASE_URL`) each name their database this way, in
//! either form libpq reads: a URL or `key=value` pairs. This is the one
//! reader, so both take the same forms and refuse the same mistakes. Each
//! caller turns a [`DatabaseUrlError`] into its own refusal naming its own
//! setting.

use std::str::FromStr;
use std::time::Duration;

use crate::PoolConfig;

/// What is wrong with a connection string.
#[derive(Debug)]
pub enum DatabaseUrlError {
    /// Neither a URL nor `key=value` pairs.
    NotAConnectionString(tokio_postgres::Error),
    NoHost,
    NoDatabase,
    NoUser,
}

impl std::fmt::Display for DatabaseUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DatabaseUrlError::NotAConnectionString(_) => "is not a Postgres connection string",
            DatabaseUrlError::NoHost => "names no host",
            DatabaseUrlError::NoDatabase => "names no database",
            DatabaseUrlError::NoUser => "names no user",
        })
    }
}

impl std::error::Error for DatabaseUrlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DatabaseUrlError::NotAConnectionString(e) => Some(e),
            _ => None,
        }
    }
}

/// Read a connection string into a pool's settings, `size` connections.
pub fn parse_database_url(url: &str, size: usize) -> Result<PoolConfig, DatabaseUrlError> {
    let parsed = tokio_postgres::Config::from_str(url.trim())
        .map_err(DatabaseUrlError::NotAConnectionString)?;
    let host = match parsed.get_hosts().first() {
        Some(tokio_postgres::config::Host::Tcp(h)) => h.clone(),
        #[cfg(unix)]
        Some(tokio_postgres::config::Host::Unix(p)) => p.display().to_string(),
        None => return Err(DatabaseUrlError::NoHost),
    };
    let db = parsed
        .get_dbname()
        .map(str::to_string)
        .ok_or(DatabaseUrlError::NoDatabase)?;
    let user = parsed
        .get_user()
        .map(str::to_string)
        .ok_or(DatabaseUrlError::NoUser)?;
    Ok(PoolConfig {
        host,
        port: parsed.get_ports().first().copied().unwrap_or(5432),
        user,
        db,
        password: parsed
            .get_password()
            .map(|p| String::from_utf8_lossy(p).into_owned()),
        size,
        acquire_timeout: Duration::from_secs(2),
    })
}

/// Whether two settings name the same database: the same name, on the same
/// host and port. A loopback host is one host whatever it is called.
pub fn same_database(a_db: &str, a_host: &str, a_port: u16, b: &PoolConfig) -> bool {
    a_db.trim() == b.db && same_host(a_host, &b.host) && a_port == b.port
}

fn same_host(a: &str, b: &str) -> bool {
    fn norm(h: &str) -> String {
        let h = h.trim().trim_end_matches('/').to_ascii_lowercase();
        match h.as_str() {
            "localhost" | "127.0.0.1" | "::1" | "[::1]" => "loopback".to_string(),
            _ => h,
        }
    }
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_forms_read_and_each_gap_is_named() {
        let a = parse_database_url("postgres://svc:pw@db:5433/ordo_x", 3).unwrap();
        assert_eq!(
            (a.host.as_str(), a.port, a.user.as_str(), a.db.as_str()),
            ("db", 5433, "svc", "ordo_x")
        );
        assert_eq!(a.password.as_deref(), Some("pw"));
        let b = parse_database_url("host=/tmp user=svc dbname=x", 3).unwrap();
        assert_eq!((b.host.as_str(), b.port), ("/tmp", 5432));
        assert!(matches!(
            parse_database_url("host=/tmp user=svc", 1),
            Err(DatabaseUrlError::NoDatabase)
        ));
        assert!(matches!(
            parse_database_url("host=/tmp dbname=x", 1),
            Err(DatabaseUrlError::NoUser)
        ));
        assert!(matches!(
            parse_database_url("user=svc dbname=x", 1),
            Err(DatabaseUrlError::NoHost)
        ));
        assert!(same_database("ordo_x", "db", 5433, &a));
        assert!(!same_database("ordo_x", "db", 5432, &a));
        let local = parse_database_url("postgres://svc@localhost/x", 1).unwrap();
        assert!(same_database("x", "127.0.0.1", 5432, &local));
    }
}
