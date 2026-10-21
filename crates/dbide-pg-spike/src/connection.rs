//! Credential-safe connection errors. Raw driver/server error strings never leave this module.
use dbide_profiles::ConnectionProfile;
use std::error::Error;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionTarget {
    pub host: String,
    pub port: u16,
    pub database: String,
}
impl From<&ConnectionProfile> for ConnectionTarget {
    fn from(p: &ConnectionProfile) -> Self {
        Self {
            host: if p.host.len() <= 253
                && !p.host.is_empty()
                && !p.host.chars().any(char::is_whitespace)
                && !p.host.contains(['/', '?', '#', '@'])
                && !p.host.chars().any(char::is_control)
            {
                p.host.clone()
            } else {
                "<invalid host>".into()
            },
            port: p.port,
            database: if p.database.len() <= 256 && !p.database.chars().any(char::is_control) {
                p.database.clone()
            } else {
                "<invalid database>".into()
            },
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionErrorKind {
    Network,
    Refused,
    Dns,
    Timeout,
    Authentication,
    DatabaseMissing,
    Tls,
    Configuration,
}
#[derive(Debug, Clone)]
pub struct ConnectionDiagnostic {
    pub kind: ConnectionErrorKind,
    pub target: Box<ConnectionTarget>,
    pub message: &'static str,
    pub hint: &'static str,
    pub sqlstate: Option<String>,
    pub os_code: Option<i32>,
}
impl ConnectionDiagnostic {
    pub fn configuration(target: ConnectionTarget) -> Self {
        Self::new(ConnectionErrorKind::Configuration, target)
    }
    pub fn timeout(target: ConnectionTarget) -> Self {
        Self::new(ConnectionErrorKind::Timeout, target)
    }
    pub fn tls(target: ConnectionTarget) -> Self {
        Self::new(ConnectionErrorKind::Tls, target)
    }
    fn new(kind: ConnectionErrorKind, target: ConnectionTarget) -> Self {
        let (message, hint) = match kind {
            ConnectionErrorKind::Network => (
                "Database network connection failed",
                "Check network access, host, port, firewall, and whether PostgreSQL is running.",
            ),
            ConnectionErrorKind::Refused => (
                "PostgreSQL refused the connection",
                "Start PostgreSQL and check its listening address and port.",
            ),
            ConnectionErrorKind::Dns => (
                "Database hostname could not be resolved",
                "Check the hostname and DNS or VPN connectivity.",
            ),
            ConnectionErrorKind::Timeout => (
                "Database connection timed out",
                "Check host, port, firewall, and VPN. Increase the timeout only if this endpoint is expected to respond slowly.",
            ),
            ConnectionErrorKind::Authentication => (
                "PostgreSQL authentication failed",
                "Check the username, session password, and server authentication rules. Update the password and retry.",
            ),
            ConnectionErrorKind::DatabaseMissing => (
                "Requested database does not exist",
                "Check the database name or create it using an authorized administrative connection.",
            ),
            ConnectionErrorKind::Tls => (
                "TLS connection or certificate verification failed",
                "Check server TLS, hostname, certificate expiry, and trusted root certificate. TLS verification is never disabled automatically.",
            ),
            ConnectionErrorKind::Configuration => (
                "Connection profile is invalid",
                "Check required host, database, username, port, timeout, and TLS certificate settings.",
            ),
        };
        Self {
            kind,
            target: Box::new(target),
            message,
            hint,
            sqlstate: None,
            os_code: None,
        }
    }
    pub fn from_pg(error: &tokio_postgres::Error, target: ConnectionTarget) -> Self {
        let state = error.code().map(|c| c.code().to_owned());
        let mut kind = match state.as_deref() {
            Some("28P01" | "28000") => ConnectionErrorKind::Authentication,
            Some("3D000") => ConnectionErrorKind::DatabaseMissing,
            _ => ConnectionErrorKind::Network,
        };
        let mut os_code = None;
        let mut current: Option<&(dyn Error + 'static)> = Some(error);
        while let Some(cause) = current {
            if cause.downcast_ref::<native_tls::Error>().is_some() {
                kind = ConnectionErrorKind::Tls;
            }
            if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                os_code = io.raw_os_error();
                kind = match io.kind() {
                    std::io::ErrorKind::ConnectionRefused => ConnectionErrorKind::Refused,
                    std::io::ErrorKind::TimedOut => ConnectionErrorKind::Timeout,
                    std::io::ErrorKind::NotFound => ConnectionErrorKind::Dns,
                    _ if kind == ConnectionErrorKind::Tls => ConnectionErrorKind::Tls,
                    _ => ConnectionErrorKind::Network,
                };
                // Resolver errors are platform specific; inspect only for classification,
                // never expose arbitrary source strings (which may contain credentials).
                let text = io.to_string().to_ascii_lowercase();
                if text.contains("name or service not known")
                    || text.contains("nodename nor servname")
                    || text.contains("failed to lookup")
                {
                    kind = ConnectionErrorKind::Dns;
                }
            }
            current = cause.source();
        }
        // tokio-postgres represents a server refusing SSL as a TLS kind with a
        // plain source string; use its bounded top-level label only for classification.
        if error.to_string() == "error performing TLS handshake" {
            kind = ConnectionErrorKind::Tls;
        }
        let mut diagnostic = Self::new(kind, target);
        diagnostic.sqlstate = state;
        diagnostic.os_code = os_code;
        diagnostic
    }
}
impl std::fmt::Display for ConnectionDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.message, self.hint)
    }
}
impl Error for ConnectionDiagnostic {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_credential_url_cannot_escape_through_target() {
        let mut profile = ConnectionProfile::fixture();
        profile.host = "postgresql://user:never-leak-this@localhost/db".into();
        let diagnostic = ConnectionDiagnostic::configuration(ConnectionTarget::from(&profile));
        assert_eq!(diagnostic.target.host, "<invalid host>");
        assert!(!format!("{diagnostic:?}").contains("never-leak-this"));
    }
}
