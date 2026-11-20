//! Typed settings loaded from environment variables. Secrets never have defaults.
//!
//! Every config struct exposes `from_env()` (reads `std::env`) and `from_lookup()`
//! (reads through a caller-supplied closure, used by tests and any non-env source).

use std::fmt::{self, Display};
use std::str::FromStr;

use thiserror::Error;

/// Errors produced while reading configuration.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    /// A required key is unset or empty.
    #[error("missing required config key {0}")]
    Missing(String),
    /// A key is set but its value is unusable.
    #[error("invalid config key {key}: {reason}")]
    Invalid { key: String, reason: String },
}

const REDACTED: &str = "***";
const MIN_JWT_SECRET_BYTES: usize = 32;

/// Reads a required env var (empty counts as missing).
pub fn required(key: &str) -> Result<String, ConfigError> {
    required_from(&env_lookup, key)
}

/// Reads an optional env var (empty counts as unset).
pub fn optional(key: &str) -> Option<String> {
    optional_from(&env_lookup, key)
}

/// Reads an env var and parses it, falling back to `default` when unset or empty.
pub fn parse_or<T>(key: &str, default: T) -> Result<T, ConfigError>
where
    T: FromStr,
    T::Err: Display,
{
    parse_or_from(&env_lookup, key, default)
}

fn env_lookup(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

fn value_from<F>(lookup: &F, key: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    lookup(key).filter(|v| !v.is_empty())
}

fn required_from<F>(lookup: &F, key: &str) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    value_from(lookup, key).ok_or_else(|| ConfigError::Missing(key.to_owned()))
}

fn optional_from<F>(lookup: &F, key: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    value_from(lookup, key)
}

fn parse_or_from<F, T>(lookup: &F, key: &str, default: T) -> Result<T, ConfigError>
where
    F: Fn(&str) -> Option<String>,
    T: FromStr,
    T::Err: Display,
{
    match value_from(lookup, key) {
        None => Ok(default),
        Some(raw) => raw.trim().parse::<T>().map_err(|e| ConfigError::Invalid {
            key: key.to_owned(),
            reason: e.to_string(),
        }),
    }
}

/// PostgreSQL connection settings.
#[derive(Clone)]
pub struct DatabaseConfig {
    /// `DATABASE_URL` (required). May contain a password; never printed by `Debug`.
    pub url: String,
    /// `DATABASE_MAX_CONNECTIONS` (default 10).
    pub max_connections: u32,
}

impl DatabaseConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(env_lookup)
    }

    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        Ok(Self {
            url: required_from(&lookup, "DATABASE_URL")?,
            max_connections: parse_or_from(&lookup, "DATABASE_MAX_CONNECTIONS", 10)?,
        })
    }
}

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("url", &REDACTED)
            .field("max_connections", &self.max_connections)
            .finish()
    }
}

/// Redis connection and stream retention settings.
#[derive(Debug, Clone)]
pub struct RedisConfig {
    /// `REDIS_URL` (required).
    pub url: String,
    /// `REDIS_STREAM_MAXLEN` (default 200).
    pub stream_maxlen: usize,
    /// `REDIS_STREAM_TTL_SECONDS` (default 3600).
    pub stream_ttl_seconds: u64,
}

impl RedisConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(env_lookup)
    }

    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        Ok(Self {
            url: required_from(&lookup, "REDIS_URL")?,
            stream_maxlen: parse_or_from(&lookup, "REDIS_STREAM_MAXLEN", 200)?,
            stream_ttl_seconds: parse_or_from(&lookup, "REDIS_STREAM_TTL_SECONDS", 3600)?,
        })
    }
}

/// JWT signing and token lifetime settings.
#[derive(Clone)]
pub struct JwtConfig {
    /// `JWT_SECRET` (required, at least 32 bytes). Never printed by `Debug`.
    pub secret: String,
    /// `JWT_ISSUER` (default "WarpTalk.AuthService").
    pub issuer: String,
    /// `JWT_AUDIENCE` (default "WarpTalk").
    pub audience: String,
    /// `JWT_ACCESS_TTL_SECONDS` (default 900).
    pub access_ttl_seconds: u64,
    /// `JWT_REFRESH_TTL_SECONDS` (default 2592000).
    pub refresh_ttl_seconds: u64,
}

impl JwtConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(env_lookup)
    }

    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let secret = required_from(&lookup, "JWT_SECRET")?;
        if secret.len() < MIN_JWT_SECRET_BYTES {
            return Err(ConfigError::Invalid {
                key: "JWT_SECRET".to_owned(),
                reason: format!("must be at least {MIN_JWT_SECRET_BYTES} bytes"),
            });
        }
        Ok(Self {
            secret,
            issuer: optional_from(&lookup, "JWT_ISSUER")
                .unwrap_or_else(|| "WarpTalk.AuthService".to_owned()),
            audience: optional_from(&lookup, "JWT_AUDIENCE")
                .unwrap_or_else(|| "WarpTalk".to_owned()),
            access_ttl_seconds: parse_or_from(&lookup, "JWT_ACCESS_TTL_SECONDS", 900)?,
            refresh_ttl_seconds: parse_or_from(&lookup, "JWT_REFRESH_TTL_SECONDS", 2_592_000)?,
        })
    }
}

impl fmt::Debug for JwtConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtConfig")
            .field("secret", &REDACTED)
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("access_ttl_seconds", &self.access_ttl_seconds)
            .field("refresh_ttl_seconds", &self.refresh_ttl_seconds)
            .finish()
    }
}

/// HTTP server settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConfig {
    /// `HTTP_BIND` (default "0.0.0.0:8080").
    pub bind: String,
    /// `CORS_ORIGINS`, comma-separated (default empty). Blank entries are dropped.
    pub cors_origins: Vec<String>,
}

impl HttpConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(env_lookup)
    }

    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let cors_origins = optional_from(&lookup, "CORS_ORIGINS")
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            bind: optional_from(&lookup, "HTTP_BIND").unwrap_or_else(|| "0.0.0.0:8080".to_owned()),
            cors_origins,
        })
    }
}

/// Log output format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogFormat {
    #[default]
    Pretty,
    Json,
}

/// Error returned when a string is not a known [`LogFormat`].
#[derive(Debug, Error, PartialEq, Eq)]
#[error("expected `pretty` or `json`, got `{0}`")]
pub struct ParseLogFormatError(String);

impl FromStr for LogFormat {
    type Err = ParseLogFormatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("pretty") {
            Ok(Self::Pretty)
        } else if s.eq_ignore_ascii_case("json") {
            Ok(Self::Json)
        } else {
            Err(ParseLogFormatError(s.to_owned()))
        }
    }
}

/// Logging and metrics settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryConfig {
    /// `LOG_FORMAT` (`pretty` or `json`, default pretty).
    pub log_format: LogFormat,
    /// `RUST_LOG` (default "info").
    pub log_filter: String,
    /// `METRICS_BIND` (optional; metrics endpoint disabled when unset).
    pub metrics_bind: Option<String>,
}

impl TelemetryConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(env_lookup)
    }

    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        Ok(Self {
            log_format: parse_or_from(&lookup, "LOG_FORMAT", LogFormat::Pretty)?,
            log_filter: optional_from(&lookup, "RUST_LOG").unwrap_or_else(|| "info".to_owned()),
            metrics_bind: optional_from(&lookup, "METRICS_BIND"),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| map.get(k).cloned()
    }

    fn secret(len: usize) -> String {
        "s".repeat(len)
    }

    #[test]
    fn database_defaults_and_overrides() {
        let cfg = DatabaseConfig::from_lookup(env(&[("DATABASE_URL", "postgres://h/db")])).unwrap();
        assert_eq!(cfg.url, "postgres://h/db");
        assert_eq!(cfg.max_connections, 10);

        let cfg = DatabaseConfig::from_lookup(env(&[
            ("DATABASE_URL", "postgres://h/db"),
            ("DATABASE_MAX_CONNECTIONS", " 25 "),
        ]))
        .unwrap();
        assert_eq!(cfg.max_connections, 25);
    }

    #[test]
    fn database_missing_url_is_error() {
        let err = DatabaseConfig::from_lookup(env(&[])).unwrap_err();
        assert_eq!(err, ConfigError::Missing("DATABASE_URL".to_owned()));
    }

    #[test]
    fn empty_required_counts_as_missing() {
        let err = RedisConfig::from_lookup(env(&[("REDIS_URL", "")])).unwrap_err();
        assert_eq!(err, ConfigError::Missing("REDIS_URL".to_owned()));
    }

    #[test]
    fn invalid_number_is_error() {
        let err = DatabaseConfig::from_lookup(env(&[
            ("DATABASE_URL", "postgres://h/db"),
            ("DATABASE_MAX_CONNECTIONS", "lots"),
        ]))
        .unwrap_err();
        match err {
            ConfigError::Invalid { key, .. } => assert_eq!(key, "DATABASE_MAX_CONNECTIONS"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn negative_unsigned_is_invalid() {
        let err = RedisConfig::from_lookup(env(&[
            ("REDIS_URL", "redis://h"),
            ("REDIS_STREAM_TTL_SECONDS", "-1"),
        ]))
        .unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid { ref key, .. } if key == "REDIS_STREAM_TTL_SECONDS")
        );
    }

    #[test]
    fn redis_defaults() {
        let cfg =
            RedisConfig::from_lookup(env(&[("REDIS_URL", "redis://localhost:6379")])).unwrap();
        assert_eq!(cfg.url, "redis://localhost:6379");
        assert_eq!(cfg.stream_maxlen, 200);
        assert_eq!(cfg.stream_ttl_seconds, 3600);
    }

    #[test]
    fn jwt_short_secret_is_invalid() {
        let s = secret(31);
        let err = JwtConfig::from_lookup(env(&[("JWT_SECRET", s.as_str())])).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { ref key, .. } if key == "JWT_SECRET"));
    }

    #[test]
    fn jwt_minimum_secret_accepted_with_defaults() {
        let s = secret(32);
        let cfg = JwtConfig::from_lookup(env(&[("JWT_SECRET", s.as_str())])).unwrap();
        assert_eq!(cfg.secret, s);
        assert_eq!(cfg.issuer, "WarpTalk.AuthService");
        assert_eq!(cfg.audience, "WarpTalk");
        assert_eq!(cfg.access_ttl_seconds, 900);
        assert_eq!(cfg.refresh_ttl_seconds, 2_592_000);
    }

    #[test]
    fn jwt_missing_secret_is_error() {
        let err = JwtConfig::from_lookup(env(&[])).unwrap_err();
        assert_eq!(err, ConfigError::Missing("JWT_SECRET".to_owned()));
    }

    #[test]
    fn jwt_debug_redacts_secret() {
        let s = format!("hunter2-{}", secret(32));
        let cfg = JwtConfig::from_lookup(env(&[("JWT_SECRET", s.as_str())])).unwrap();
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("hunter2"), "secret leaked: {dbg}");
        assert!(dbg.contains("***"));
    }

    #[test]
    fn database_debug_redacts_url_password() {
        let cfg =
            DatabaseConfig::from_lookup(env(&[("DATABASE_URL", "postgres://user:topsecret@h/db")]))
                .unwrap();
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("topsecret"), "password leaked: {dbg}");
        assert!(dbg.contains("***"));
    }

    #[test]
    fn http_defaults() {
        let cfg = HttpConfig::from_lookup(env(&[])).unwrap();
        assert_eq!(cfg.bind, "0.0.0.0:8080");
        assert!(cfg.cors_origins.is_empty());
    }

    #[test]
    fn cors_origins_comma_list_is_trimmed() {
        let cfg = HttpConfig::from_lookup(env(&[(
            "CORS_ORIGINS",
            " https://a.test , https://b.test,,",
        )]))
        .unwrap();
        assert_eq!(cfg.cors_origins, vec!["https://a.test", "https://b.test"]);
    }

    #[test]
    fn telemetry_defaults() {
        let cfg = TelemetryConfig::from_lookup(env(&[])).unwrap();
        assert_eq!(cfg.log_format, LogFormat::Pretty);
        assert_eq!(cfg.log_filter, "info");
        assert_eq!(cfg.metrics_bind, None);
    }

    #[test]
    fn telemetry_json_and_metrics_bind() {
        let cfg = TelemetryConfig::from_lookup(env(&[
            ("LOG_FORMAT", "JSON"),
            ("RUST_LOG", "debug,hyper=warn"),
            ("METRICS_BIND", "127.0.0.1:9100"),
        ]))
        .unwrap();
        assert_eq!(cfg.log_format, LogFormat::Json);
        assert_eq!(cfg.log_filter, "debug,hyper=warn");
        assert_eq!(cfg.metrics_bind.as_deref(), Some("127.0.0.1:9100"));
    }

    #[test]
    fn telemetry_invalid_log_format_is_error() {
        let err = TelemetryConfig::from_lookup(env(&[("LOG_FORMAT", "xml")])).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { ref key, .. } if key == "LOG_FORMAT"));
    }

    #[test]
    fn parse_or_uses_default_when_unset_or_empty() {
        let v: u16 = parse_or_from(&env(&[]), "PORT_X", 7).unwrap();
        assert_eq!(v, 7);
        let v: u16 = parse_or_from(&env(&[("PORT_X", "")]), "PORT_X", 7).unwrap();
        assert_eq!(v, 7);
    }

    #[test]
    fn config_error_display() {
        assert_eq!(
            ConfigError::Missing("X".into()).to_string(),
            "missing required config key X"
        );
        assert_eq!(
            ConfigError::Invalid {
                key: "X".into(),
                reason: "bad".into()
            }
            .to_string(),
            "invalid config key X: bad"
        );
    }
}
