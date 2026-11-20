//! 真库集成测试：默认 `#[ignore]`，用 `MYSQLCOMPAT_LIVE=1` 环境变量启用。
//!
//! ```sh
//! docker run -d --name mariadb -e MARIADB_ROOT_PASSWORD=navop123 \
//!   -p 127.0.0.1:13306:3306 mariadb:11
//! MYSQLCOMPAT_LIVE=1 MYSQLCOMPAT_URL="root:navop123@tcp(127.0.0.1:13306)/mysql" \
//!   cargo test -p mysqlcompat-driver --lib live -- --nocapture
//! ```

#![allow(clippy::result_large_err)]

use base64::Engine;
use serde_json::json;

use crate::config::MyCompatConnectionConfig;
use crate::session::MyCompatSession;

fn live_enabled() -> bool {
    std::env::var("MYSQLCOMPAT_LIVE").ok().as_deref() == Some("1")
}

fn live_config() -> MyCompatConnectionConfig {
    MyCompatConnectionConfig::from_wire_config(json!({
        "host": "127.0.0.1",
        "port": std::env::var("MYSQLCOMPAT_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(13306),
        "username": "root",
        "password": std::env::var("MYSQLCOMPAT_PASSWORD").unwrap_or_else(|_| "navop123".to_string()),
        "database": std::env::var("MYSQLCOMPAT_DB").unwrap_or_else(|_| "mysql".to_string()),
    }))
    .expect("live config")
}

#[test]
#[ignore]
fn live_session_roundtrip() {
    if !live_enabled() {
        return;
    }
    let mut session = MyCompatSession::connect(live_config()).expect("connect");
    assert!(session.ping().is_ok(), "ping");
    assert!(!session.server_version().unwrap().is_empty(), "server version");
    assert_eq!(session.current_database().unwrap(), "mysql");

    // DDL + 参数化写入/读取 + 注入对抗：值一律走绑定参数。
    session
        .simple_execute(
            "DROP DATABASE IF EXISTS navop_e2e; CREATE DATABASE navop_e2e; USE navop_e2e",
        )
        .expect("prepare database");
    session
        .simple_execute(
            "CREATE TABLE users (id BIGINT PRIMARY KEY AUTO_INCREMENT, name VARCHAR(64) NOT NULL, \
             payload VARBINARY(64) NULL, score DECIMAL(10,2) DEFAULT 0)",
        )
        .expect("create table");
    let hostile = "x' OR '1'='1";
    session
        .execute(
            "INSERT INTO users (name, payload, score) VALUES (?, ?, ?)",
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
    let affected = session.execute(
        "DELETE FROM users WHERE name = ?",
        &[extension_protocol::row::CellValue::Text {
            value: "nonexistent".to_string(),
        }],
    );
    assert_eq!(affected.unwrap(), 0, "delete with bound param");

    // 元数据：对象/列，过滤值同样走绑定。
    let objects = session
        .list_objects(Some("navop_e2e"), None, &[])
        .expect("list objects");
    assert!(
        objects.iter().any(|o| o.name == "users"),
        "objects = {objects:?}"
    );
    let columns = session
        .list_columns(Some("navop_e2e"), Some("navop_e2e"), "users")
        .expect("list columns");
    assert!(
        columns.iter().any(|c| c.name == "payload"),
        "columns = {columns:?}"
    );

    // 查询结果按声明类型编码：二进制列必须 base64，文本注入对抗值原样返回。
    let result = session
        .query(
            "SELECT name, payload FROM users WHERE name = ?",
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
        "binary payload must stay base64 bytes: {:?}",
        result.rows[0][1]
    );

    // 事务回滚。
    session.simple_execute("BEGIN").expect("begin");
    session
        .execute("INSERT INTO users (name) VALUES ('tx')", &[])
        .expect("insert in tx");
    session.simple_execute("ROLLBACK").expect("rollback");
    let result = session
        .query("SELECT COUNT(*) FROM users WHERE name = 'tx'", &[])
        .expect("count");
    assert_eq!(result.rows.len(), 1);

    // dump DDL（SHOW CREATE TABLE，标识符校验路径）。
    let ddls = session
        .dump_ddl(
            &[extension_protocol::schema::ObjectRef {
                kind: extension_protocol::schema::ObjectKind::Table,
                database: Some("navop_e2e".to_string()),
                schema: Some("navop_e2e".to_string()),
                name: "users".to_string(),
            }],
            &Default::default(),
        )
        .expect("dump ddl");
    assert!(
        ddls.len() == 1 && ddls[0].contains("CREATE TABLE"),
        "{ddls:?}"
    );
    // 敌意标识符：dump_ddl 必须拒绝而不是拼进 SHOW CREATE TABLE。
    assert!(
        session
            .dump_ddl(
                &[extension_protocol::schema::ObjectRef {
                    kind: extension_protocol::schema::ObjectKind::Table,
                    database: Some("navop_e2e".to_string()),
                    schema: Some("navop_e2e".to_string()),
                    name: "t`ble".to_string(),
                }],
                &Default::default(),
            )
            .is_err()
    );

    session.close();
}
