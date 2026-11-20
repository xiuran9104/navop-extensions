#![allow(clippy::result_large_err)]

//! 宿主 wire 配置 → mysql_async 连接参数。
//!
//! 与 Go 版 mysqlcompat 语义对齐：`protocol` 等驱动控制键不进入连接参数；
//! `sslmode` 额外参数（disable/prefer 默认关闭，require 及以上启用 rustls
//! TLS）决定 TLS。

use mysql_async::SslOpts;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

fn default_host() -> String {
    "localhost".to_string()
}

fn default_port() -> u16 {
    3306
}

fn default_database() -> String {
    String::new()
}

fn deserialize_optional_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(flag)),
        Some(Value::String(text)) => parse_bool_string(&text)
            .map(Some)
            .map_err(serde::de::Error::custom),
        Some(other) => Err(serde::de::Error::custom(format!(
            "invalid boolean value `{other}`"
        ))),
    }
}

fn parse_bool_string(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" | "" => Ok(false),
        other => Err(format!("invalid boolean value `{other}`")),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MyCompatConnectionConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default, alias = "user")]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_database", alias = "dbname")]
    pub database: String,
    #[serde(default, alias = "sslmode", alias = "ssl-mode")]
    pub ssl_mode: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub ssl_accept_invalid_certs: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub ssl_accept_invalid_hostnames: Option<bool>,
    #[serde(default)]
    pub connect_timeout_ms: Option<u64>,
}

impl MyCompatConnectionConfig {
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
        &self.database
    }

    /// TLS 选项：sslmode 缺省或 disable/prefer 时不用 TLS，require 及以上启用。
    pub fn ssl_opts(&self) -> Result<Option<SslOpts>, String> {
        let mode = self
            .ssl_mode
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let mut opts = match mode.as_str() {
            "" | "disable" | "prefer" | "preferred" => return Ok(None),
            "require" | "required" | "verify_ca" | "verify-ca" | "verify_identity"
            | "verify-identity" => SslOpts::default(),
            other => return Err(format!("unsupported sslmode `{other}`")),
        };
        if self.ssl_accept_invalid_certs == Some(true) {
            opts = opts.with_danger_accept_invalid_certs(true);
        }
        if self.ssl_accept_invalid_hostnames == Some(true) {
            opts = opts.with_danger_skip_domain_validation(true);
        }
        Ok(Some(opts))
    }

    pub fn to_opts_builder(&self) -> Result<mysql_async::OptsBuilder, String> {
        if self.host.trim().is_empty() {
            return Err("missing required config field host".to_string());
        }
        let mut builder = mysql_async::OptsBuilder::default()
            .ip_or_hostname(self.host.clone())
            .tcp_port(self.port)
            .user(Some(self.username.clone()))
            .pass(Some(self.password.clone()));
        if !self.database.trim().is_empty() {
            builder = builder.db_name(Some(self.database.clone()));
        }
        if let Some(ssl) = self.ssl_opts()? {
            builder = builder.ssl_opts(ssl);
        }
        Ok(builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_extra_params_into_top_level_fields() {
        let cfg = MyCompatConnectionConfig::from_wire_config(serde_json::json!({
            "host": "db.local",
            "port": 4000,
            "username": "root",
            "extra_params": {"sslmode": "require"}
        }))
        .unwrap();
        assert_eq!(cfg.host, "db.local");
        assert_eq!(cfg.port, 4000);
        assert_eq!(cfg.ssl_mode.as_deref(), Some("require"));
        assert_eq!(cfg.endpoint(), "db.local:4000");
    }

    #[test]
    fn defaults_apply_without_optional_fields() {
        let cfg = MyCompatConnectionConfig::from_wire_config(serde_json::json!({"username": "u"}))
            .unwrap();
        assert_eq!(cfg.host, "localhost");
        assert_eq!(cfg.port, 3306);
        assert!(cfg.ssl_opts().unwrap().is_none());
    }

    #[test]
    fn ssl_mode_mapping() {
        let mut cfg =
            MyCompatConnectionConfig::from_wire_config(serde_json::json!({"username": "u"}))
                .unwrap();
        assert!(cfg.ssl_opts().unwrap().is_none());
        cfg.ssl_mode = Some("disable".into());
        assert!(cfg.ssl_opts().unwrap().is_none());
        cfg.ssl_mode = Some("require".into());
        assert!(cfg.ssl_opts().unwrap().is_some());
        cfg.ssl_mode = Some("bogus".into());
        assert!(cfg.ssl_opts().is_err());
    }

    #[test]
    fn opts_builder_requires_host() {
        let mut cfg =
            MyCompatConnectionConfig::from_wire_config(serde_json::json!({"username": "u"}))
                .unwrap();
        cfg.host = String::new();
        assert!(cfg.to_opts_builder().is_err());
    }
}
