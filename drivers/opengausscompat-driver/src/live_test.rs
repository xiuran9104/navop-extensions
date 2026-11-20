//! 真库集成测试：默认 `#[ignore]`，用 `OGCOMPAT_LIVE=1` 环境变量启用。
//!
//! 验证 gaussdb/vastbase 驱动的核心假设：tokio-opengauss 客户端完成
//! openGauss 5.x 的 sha256 认证并读取 information_schema / pg_catalog 元数据。
//!
//! ```sh
//! docker run -d --name og -e GS_PASSWORD=Navop@123 -p 127.0.0.1:15400:5432 \
//!   opengauss/opengauss:5.0.0
//! OGCOMPAT_LIVE=1 cargo test -p opengausscompat-driver --lib live -- --nocapture --ignored
//! ```

#![allow(clippy::result_large_err)]

use serde_json::json;

use crate::config::OgCompatConnectionConfig;
use crate::session::OgCompatSession;

fn live_enabled() -> bool {
    std::env::var("OGCOMPAT_LIVE").ok().as_deref() == Some("1")
}

fn live_config() -> OgCompatConnectionConfig {
    OgCompatConnectionConfig::from_wire_config(json!({
        "host": "127.0.0.1",
        "port": std::env::var("OGCOMPAT_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(15400),
        "username": std::env::var("OGCOMPAT_USER").unwrap_or_else(|_| "gaussdb".to_string()),
        "password": std::env::var("OGCOMPAT_PASSWORD").unwrap_or_else(|_| "Navop@123".to_string()),
        "database": std::env::var("OGCOMPAT_DB").unwrap_or_else(|_| "postgres".to_string()),
        "ssl_mode": "disable",
    }))
    .expect("live config")
}

#[test]
#[ignore]
fn live_session_roundtrip_opengauss() {
    if !live_enabled() {
        return;
    }
    let mut session = OgCompatSession::connect(live_config()).expect("connect to openGauss");
    assert!(session.ping().is_ok(), "ping");
    assert!(
        !session.server_version().unwrap().is_empty(),
        "server version"
    );
    assert_eq!(session.current_database().unwrap(), "postgres");

    session
        .simple_execute("DROP SCHEMA IF EXISTS navop_e2e CASCADE; CREATE SCHEMA navop_e2e")
        .expect("prepare schema");
    session
        .simple_execute(
            "CREATE TABLE navop_e2e.users (id BIGSERIAL PRIMARY KEY, name TEXT NOT NULL)",
        )
        .expect("create table");
    let hostile = "x'; DROP TABLE users; --";
    session
        .execute(
            "INSERT INTO navop_e2e.users (name) VALUES ($1)",
            &[extension_protocol::row::CellValue::Text {
                value: hostile.to_string(),
            }],
        )
        .expect("parameterized insert");

    let objects = session
        .list_objects(Some("postgres"), None, &[])
        .expect("list objects");
    assert!(
        objects.iter().any(|o| o.name == "users"),
        "objects = {objects:?}"
    );

    let result = session
        .query(
            "SELECT name FROM navop_e2e.users WHERE name = $1",
            &[extension_protocol::row::CellValue::Text {
                value: hostile.to_string(),
            }],
        )
        .expect("select");
    assert_eq!(result.rows.len(), 1, "rows = {:?}", result.rows);

    // 事务回滚。
    session.simple_execute("BEGIN").expect("begin");
    session
        .execute("INSERT INTO navop_e2e.users (name) VALUES ('tx')", &[])
        .expect("insert in tx");
    session.simple_execute("ROLLBACK").expect("rollback");

    session.close();
}
