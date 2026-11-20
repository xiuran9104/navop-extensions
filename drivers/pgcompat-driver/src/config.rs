#![allow(clippy::result_large_err)]

use std::fs;

use serde::{Deserialize, Deserializer};
use serde_json::Value;
use tokio_postgres::config::SslMode;

#[derive(Debug, Clone, Deserialize)]
pub struct PgCompatConnectionConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_username", alias = "user")]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_database", alias = "dbname")]
    pub database: String,
    #[serde(default)]
    pub ssl_mode: Option<String>,
    #[serde(default, alias = "ssl_root_cert", alias = "sslrootcert")]
    pub ssl_root_cert_path: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub ssl_accept_invalid_certs: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub ssl_accept_invalid_hostnames: Option<bool>,
    #[serde(default)]
    pub connect_timeout_ms: Option<u64>,
}

impl PgCompatConnectionConfig {
    pub fn from_wire_config(mut value: Value) -> Result<Self, serde_json::Error> {
        if let Some(config) = value.as_object_mut() {
            let extra_params = config
                .get("extra_params")
                .and_then(Value::as_object)
                .map(|params| {
                    params
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            for (key, extra_value) in extra_params {
                config.entry(key).or_insert(extra_value);
            }
        }

        serde_json::from_value(value)
    }

    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn database_name(&self) -> &str {
        if self.database.trim().is_empty() {
            "postgres"
        } else {
            self.database.trim()
        }
    }

    pub fn to_client_config(&self) -> Result<tokio_postgres::Config, String> {
        let mut cfg = tokio_postgres::Config::new();
        cfg.host(&self.host);
        cfg.port(self.port);
        cfg.user(&self.username);
        cfg.dbname(self.database_name());
        if !self.password.is_empty() {
            cfg.password(&self.password);
        }
        if let Some(timeout_ms) = self.connect_timeout_ms {
            cfg.connect_timeout(std::time::Duration::from_millis(timeout_ms));
        }
        cfg.ssl_mode(parse_ssl_mode(self.ssl_mode.as_deref())?);
        Ok(cfg)
    }

    pub fn requires_tls_connector(&self) -> Result<bool, String> {
        Ok(parse_ssl_mode(self.ssl_mode.as_deref())? != SslMode::Disable)
    }

    /// 构建 rustls TLS 连接器：显式 root cert 优先，否则回退 webpki 内置根。
    /// `ssl_accept_invalid_certs/hostnames` 尚不支持，显式开启时报错而不是静默降级。
    pub fn to_rustls_make_tls(&self) -> Result<tokio_postgres_rustls::MakeRustlsConnect, String> {
        if self.ssl_accept_invalid_certs == Some(true)
            || self.ssl_accept_invalid_hostnames == Some(true)
        {
            return Err(
                "ssl_accept_invalid_certs/hostnames is not supported by the pgcompat driver yet"
                    .to_string(),
            );
        }
        let mut roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        if let Some(root_cert_path) = non_empty(self.ssl_root_cert_path.as_deref()) {
            let cert_bytes = read_tls_file(root_cert_path, "ssl_root_cert_path")?;
            let mut reader = std::io::BufReader::new(cert_bytes.as_slice());
            let certificates: Vec<rustls::pki_types::CertificateDer> =
                rustls_pemfile::certs(&mut reader)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        format!("failed to parse ssl_root_cert_path `{root_cert_path}`: {error}")
                    })?;
            if certificates.is_empty() {
                return Err(format!(
                    "ssl_root_cert_path `{root_cert_path}` contains no PEM certificates"
                ));
            }
            for certificate in certificates {
                roots
                    .add(certificate)
                    .map_err(|error| format!("failed to add root certificate: {error}"))?;
            }
        }
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(tokio_postgres_rustls::MakeRustlsConnect::new(client_config))
    }
}

fn parse_ssl_mode(value: Option<&str>) -> Result<SslMode, String> {
    match value.unwrap_or("disable").to_ascii_lowercase().as_str() {
        "disable" | "disabled" | "false" | "0" => Ok(SslMode::Disable),
        "prefer" => Ok(SslMode::Prefer),
        "require" | "true" | "1" => Ok(SslMode::Require),
        other => Err(format!("unsupported ssl_mode `{other}`")),
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn read_tls_file(path: &str, field_name: &str) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("failed to read {field_name} `{path}`: {error}"))
}

fn deserialize_optional_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolValue {
        Bool(bool),
        Number(i64),
        String(String),
    }

    let value = Option::<BoolValue>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(BoolValue::Bool(value)) => Ok(Some(value)),
        Some(BoolValue::Number(0)) => Ok(Some(false)),
        Some(BoolValue::Number(1)) => Ok(Some(true)),
        Some(BoolValue::Number(value)) => Err(serde::de::Error::custom(format!(
            "unsupported boolean value `{value}`"
        ))),
        Some(BoolValue::String(value)) => parse_bool_string(&value)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

fn parse_bool_string(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "y" | "on" => Ok(true),
        "false" | "0" | "no" | "n" | "off" | "" => Ok(false),
        other => Err(format!("unsupported boolean value `{other}`")),
    }
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    5432
}

fn default_username() -> String {
    "gaussdb".to_string()
}

fn default_database() -> String {
    "postgres".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_postgres_compatible_endpoint() {
        let cfg: PgCompatConnectionConfig = serde_json::from_value(serde_json::json!({})).unwrap();

        assert_eq!("127.0.0.1:5432", cfg.endpoint());
        assert_eq!("postgres", cfg.database_name());
        assert!(cfg.to_client_config().is_ok());
        assert!(!cfg.requires_tls_connector().unwrap());
    }

    #[test]
    fn require_ssl_mode_requests_tls_connector() {
        let cfg: PgCompatConnectionConfig =
            serde_json::from_value(serde_json::json!({ "ssl_mode": "require" })).unwrap();

        assert!(cfg.requires_tls_connector().unwrap());
    }

    #[test]
    fn parses_host_postgresql_tls_options() {
        let cfg: PgCompatConnectionConfig = serde_json::from_value(serde_json::json!({
            "ssl_mode": "require",
            "ssl_root_cert_path": "/tmp/ca.pem",
            "ssl_accept_invalid_certs": "true",
            "ssl_accept_invalid_hostnames": "1"
        }))
        .unwrap();

        assert_eq!(Some("/tmp/ca.pem"), cfg.ssl_root_cert_path.as_deref());
        assert_eq!(Some(true), cfg.ssl_accept_invalid_certs);
        assert_eq!(Some(true), cfg.ssl_accept_invalid_hostnames);
    }

    #[test]
    fn parses_host_postgresql_tls_options_from_extra_params() {
        let cfg = PgCompatConnectionConfig::from_wire_config(serde_json::json!({
            "host": "db.example.com",
            "port": 5432,
            "username": "gaussdb",
            "database": "postgres",
            "extra_params": {
                "ssl_mode": "require",
                "ssl_root_cert_path": "/tmp/ca.pem",
                "ssl_accept_invalid_certs": "true",
                "ssl_accept_invalid_hostnames": "1"
            }
        }))
        .unwrap();

        assert_eq!("db.example.com:5432", cfg.endpoint());
        assert!(cfg.requires_tls_connector().unwrap());
        assert_eq!(Some("/tmp/ca.pem"), cfg.ssl_root_cert_path.as_deref());
        assert_eq!(Some(true), cfg.ssl_accept_invalid_certs);
        assert_eq!(Some(true), cfg.ssl_accept_invalid_hostnames);
    }
}
