//! 真库集成测试：默认 `#[ignore]`，用 `PGCOMPAT_LIVE=1` 环境变量启用。
//!
//! 验证 pgcompat 的核心假设：tokio-postgres 客户端（openGauss 分叉）可以
//! 连接标准 PostgreSQL 并完成 scram 认证与 information_schema 元数据访问。
//!
//! ```sh
//! docker run -d --name pg -e POSTGRES_PASSWORD=navop123 -e POSTGRES_DB=app \
//!   -p 127.0.0.1:15432:5432 postgres:16
//! PGCOMPAT_LIVE=1 cargo test -p pgcompat-driver --lib live -- --nocapture --ignored
//! ```

#![allow(clippy::result_large_err)]

use base64::Engine;
use serde_json::json;

use crate::config::PgCompatConnectionConfig;
use crate::session::PgCompatSession;

fn live_enabled() -> bool {
    std::env::var("PGCOMPAT_LIVE").ok().as_deref() == Some("1")
}

fn live_config() -> PgCompatConnectionConfig {
    PgCompatConnectionConfig::from_wire_config(json!({
        "host": "127.0.0.1",
        "port": std::env::var("PGCOMPAT_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(15432),
        "username": std::env::var("PGCOMPAT_USER").unwrap_or_else(|_| "postgres".to_string()),
        "password": std::env::var("PGCOMPAT_PASSWORD").unwrap_or_else(|_| "navop123".to_string()),
        "database": std::env::var("PGCOMPAT_DB").unwrap_or_else(|_| "app".to_string()),
        "ssl_mode": "disable",
    }))
    .expect("live config")
}

#[test]
#[ignore]
fn live_session_roundtrip_vanilla_postgres() {
    if !live_enabled() {
        return;
    }
    let mut session = PgCompatSession::connect(live_config()).expect("connect to vanilla postgres");
    assert!(session.ping().is_ok(), "ping");
    assert!(
        session.server_version().unwrap().contains("PostgreSQL"),
        "version"
    );
    assert_eq!(session.current_database().unwrap(), "app");

    session
        .simple_execute("DROP SCHEMA IF EXISTS navop_e2e CASCADE; CREATE SCHEMA navop_e2e")
        .expect("prepare schema");
    session
        .simple_execute(
            "CREATE TABLE navop_e2e.users (id BIGSERIAL PRIMARY KEY, name TEXT NOT NULL, \
             payload BYTEA NULL, score NUMERIC(10,2) DEFAULT 0)",
        )
        .expect("create table");
    let hostile = "x'; DROP TABLE users; --";
    session
        .execute(
            "INSERT INTO navop_e2e.users (name, payload, score) VALUES ($1, $2::bytea, ($3::text)::numeric)",
            &[
                extension_protocol::row::CellValue::Text {
                    value: hostile.to_string(),
                },
                extension_protocol::row::CellValue::Bytes {
                    value: base64::engine::general_purpose::STANDARD
                        .encode([0xacu8, 0xed, 0x00, 0x05]),
                },
                extension_protocol::row::CellValue::Decimal {
                    value: "12.34".to_string(),
                },
            ],
        )
        .expect("parameterized insert");

    // 元数据走 information_schema；当前库是 app，库过滤匹配。
    let objects = session
        .list_objects(Some("app"), None, &[])
        .expect("list objects");
    assert!(
        objects.iter().any(|o| o.name == "users"),
        "objects = {objects:?}"
    );
    let columns = session
        .list_columns(Some("app"), Some("navop_e2e"), "users")
        .expect("list columns");
    assert!(columns.iter().any(|c| c.name == "payload"), "columns");

    let result = session
        .query(
            "SELECT name, payload FROM navop_e2e.users WHERE name = $1",
            &[extension_protocol::row::CellValue::Text {
                value: hostile.to_string(),
            }],
        )
        .expect("select");
    assert_eq!(result.rows.len(), 1, "rows = {:?}", result.rows);
    assert!(
        matches!(&result.rows[0][0], extension_protocol::row::CellValue::Text { value } if value == hostile),
        "hostile value roundtrip: {:?}",
        result.rows[0][0]
    );
    assert!(
        matches!(
            &result.rows[0][1],
            extension_protocol::row::CellValue::Bytes { .. }
        ),
        "bytea must stay base64 bytes"
    );

    // 事务回滚。
    session.simple_execute("BEGIN").expect("begin");
    session
        .execute("INSERT INTO navop_e2e.users (name) VALUES ('tx')", &[])
        .expect("insert in tx");
    session.simple_execute("ROLLBACK").expect("rollback");

    session.close();
}
