#![allow(clippy::result_large_err)]

//! MySQL 协议系会话层：mysql_async 客户端 + INFORMATION_SCHEMA 元数据。
//!
//! 所有元数据过滤值（schema/table/view 名）一律以 `?` 位置参数绑定，
//! 不做 SQL 文本拼接；标识符只能出现在 `SHOW CREATE TABLE` 的位置，
//! 那里以 `validate_identifier` 白名单校验后反引号引用。

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use extension_protocol::row::{CellValue, ColumnSpec, ColumnTypeKind, Row};
use extension_protocol::schema::{
    CheckInfo, ColumnInfo, DatabaseInfo, DumpDdlOptions, ForeignKeyInfo, FunctionInfo, IndexInfo,
    ObjectInfo, ObjectKind, ObjectRef, SchemaInfo, TriggerInfo, ViewDefinitionResult, ViewInfo,
};
use mysql_async::consts::ColumnType;
use mysql_async::prelude::Queryable;
use mysql_async::{Column, Conn, Params, Value};
use serde_json::json;
use tokio::runtime::Runtime;

use crate::config::MyCompatConnectionConfig;

pub struct MyCompatSession {
    runtime: Runtime,
    conn: Option<Conn>,
}

impl MyCompatSession {
    pub fn connect(cfg: MyCompatConnectionConfig) -> Result<Self> {
        let runtime = Runtime::new().context("failed to create driver Tokio runtime")?;
        let endpoint = cfg.endpoint();
        let builder = cfg
            .to_opts_builder()
            .map_err(anyhow::Error::msg)
            .context("invalid MySQL connection config")?;
        let conn = runtime.block_on(Conn::new(builder)).with_context(|| {
            format!("failed to connect to MySQL-compatible server at {endpoint}")
        })?;
        Ok(Self {
            runtime,
            conn: Some(conn),
        })
    }

    pub fn close(&mut self) {
        // disconnect 按值拿走 Conn；归还一个断开后的实例即可释放连接。
        if let Some(conn) = self.conn.take() {
            let _ = self.runtime.block_on(conn.disconnect());
        }
    }

    pub fn ping(&mut self) -> Result<()> {
        let conn = self.conn.as_mut().context("connection already closed")?;
        self.runtime
            .block_on(conn.query_drop("SELECT 1"))
            .context("failed to ping server with SELECT 1")
    }

    pub fn server_version(&mut self) -> Result<String> {
        self.query_single_string("SELECT VERSION()")
    }

    pub fn current_database(&mut self) -> Result<String> {
        self.query_single_string("SELECT DATABASE()")
    }

    pub fn query(&mut self, sql: &str, params: &[CellValue]) -> Result<QueryResult> {
        let values = bind_params(params)?;
        let result = self
            .runtime
            .block_on(async {
                let conn = self.conn.as_mut().context("connection already closed")?;
                let mut result = conn
                    .exec_iter::<_, _>(sql, Params::Positional(values))
                    .await?;
                let columns: Vec<Column> = result
                    .columns()
                    .as_deref()
                    .map(|cols| cols.to_vec())
                    .unwrap_or_default();
                let rows: Vec<mysql_async::Row> = result.collect().await?;
                Ok::<_, anyhow::Error>((columns, rows))
            })
            .with_context(|| format!("failed to execute query `{sql}`"))?;
        Ok(rows_to_query_result(result.0, result.1))
    }

    pub fn execute(&mut self, sql: &str, params: &[CellValue]) -> Result<u64> {
        let values = bind_params(params)?;
        self.runtime
            .block_on(async {
                let conn = self.conn.as_mut().context("connection already closed")?;
                let result = conn
                    .exec_iter::<_, _>(sql, Params::Positional(values))
                    .await?;
                let affected = result.affected_rows();
                result.drop_result().await?;
                Ok::<_, anyhow::Error>(affected)
            })
            .with_context(|| format!("failed to execute statement `{sql}`"))
    }

    pub fn execute_batch(&mut self, statements: &[String]) -> Result<Vec<u64>> {
        statements
            .iter()
            .map(|statement| self.execute(statement, &[]))
            .collect()
    }

    pub fn simple_execute(&mut self, sql: &str) -> Result<()> {
        let conn = self.conn.as_mut().context("connection already closed")?;
        self.runtime
            .block_on(conn.query_drop(sql))
            .with_context(|| format!("failed to execute batch `{sql}`"))
    }

    pub fn list_databases(&mut self) -> Result<Vec<DatabaseInfo>> {
        let result = self.query(
            "SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY SCHEMA_NAME",
            &[],
        )?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| DatabaseInfo {
                name: cell_as_text(&row[0]).unwrap_or_default(),
                owner: None,
                ..Default::default()
            })
            .collect())
    }

    pub fn list_schemas(&mut self, _database: Option<&str>) -> Result<Vec<SchemaInfo>> {
        let result = self.query(
            "SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY SCHEMA_NAME",
            &[],
        )?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| SchemaInfo {
                name: cell_as_text(&row[0]).unwrap_or_default(),
                owner: None,
                ..Default::default()
            })
            .collect())
    }

    pub fn list_objects(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
        kinds: &[ObjectKind],
    ) -> Result<Vec<ObjectInfo>> {
        let include_tables = kinds.is_empty() || kinds.contains(&ObjectKind::Table);
        let include_views = kinds.is_empty() || kinds.contains(&ObjectKind::View);
        let include_functions = kinds.is_empty() || kinds.contains(&ObjectKind::Function);
        let include_triggers = kinds.is_empty() || kinds.contains(&ObjectKind::Trigger);
        let schema = schema
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        let mut objects = Vec::new();

        if include_tables || include_views {
            // 表与视图同源 INFORMATION_SCHEMA.TABLES，仅 TABLE_TYPE 不同。
            let mut sql = "SELECT TABLE_NAME, TABLE_SCHEMA, TABLE_COMMENT, TABLE_TYPE \
                           FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE IN ('BASE TABLE', 'VIEW')"
                .to_string();
            let mut params: Vec<CellValue> = Vec::new();
            if let Some(schema) = schema.as_deref() {
                sql.push_str(" AND TABLE_SCHEMA = ?");
                params.push(CellValue::Text {
                    value: schema.to_string(),
                });
            }
            sql.push_str(" ORDER BY TABLE_SCHEMA, TABLE_NAME");
            let result = self.query(&sql, &params)?;
            for row in result.rows {
                let name = cell_as_text(&row[0]).unwrap_or_default();
                let schema_name = cell_as_text(&row[1]).unwrap_or_default();
                let comment = cell_as_text(&row[2]).unwrap_or_default();
                let table_type = cell_as_text(&row[3]).unwrap_or_default();
                let kind = if table_type.eq_ignore_ascii_case("VIEW") {
                    if !include_views {
                        continue;
                    }
                    ObjectKind::View
                } else {
                    if !include_tables {
                        continue;
                    }
                    ObjectKind::Table
                };
                objects.push(ObjectInfo {
                    name,
                    kind,
                    schema: schema_name,
                    comment,
                    row_count_estimate: None,
                    size_bytes: None,
                    created_at: None,
                    updated_at: None,
                    extra: serde_json::Value::Null,
                });
            }
        }
        if include_functions {
            for function in self.list_functions(_database, schema.as_deref())? {
                let function_schema = function
                    .extra
                    .get("schema")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                objects.push(ObjectInfo {
                    name: function.name,
                    kind: ObjectKind::Function,
                    schema: function_schema,
                    comment: function.comment,
                    row_count_estimate: None,
                    size_bytes: None,
                    created_at: None,
                    updated_at: None,
                    extra: function.extra,
                });
            }
        }
        if include_triggers {
            for trigger in self.list_triggers(_database, schema.as_deref(), None)? {
                let trigger_schema = trigger
                    .extra
                    .get("schema")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                objects.push(ObjectInfo {
                    name: trigger.name,
                    kind: ObjectKind::Trigger,
                    schema: trigger_schema,
                    comment: String::new(),
                    row_count_estimate: None,
                    size_bytes: None,
                    created_at: None,
                    updated_at: None,
                    extra: trigger.extra,
                });
            }
        }

        Ok(objects)
    }

    pub fn list_columns(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<ColumnInfo>> {
        let schema = match schema.filter(|value| !value.trim().is_empty()) {
            Some(schema) => schema.to_string(),
            None => self.current_database()?,
        };
        let result = self.query(
            "SELECT ORDINAL_POSITION, COLUMN_NAME, DATA_TYPE, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, \
                    CHARACTER_MAXIMUM_LENGTH, NUMERIC_PRECISION, NUMERIC_SCALE, COLUMN_COMMENT \
             FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
            &[
                CellValue::Text { value: schema },
                CellValue::Text {
                    value: table.to_string(),
                },
            ],
        )?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| {
                let type_str = cell_as_text(&row[3]).unwrap_or_default();
                ColumnInfo {
                    ordinal: cell_as_i64(&row[0]).unwrap_or(0).max(0) as u32,
                    name: cell_as_text(&row[1]).unwrap_or_default(),
                    raw_type: Some(type_str.clone()),
                    type_str: cell_as_text(&row[2]).unwrap_or_default(),
                    nullable: cell_as_text(&row[4])
                        .map(|value| value.eq_ignore_ascii_case("YES"))
                        .unwrap_or(true),
                    default: cell_as_text(&row[5]),
                    max_length: cell_as_i64(&row[6]).map(|v| v.max(0) as u32),
                    precision: cell_as_i64(&row[7]).map(|v| v.max(0) as u32),
                    scale: cell_as_i64(&row[8]).map(|v| v as i32),
                    comment: cell_as_text(&row[9]).unwrap_or_default(),
                    ..Default::default()
                }
            })
            .collect())
    }

    pub fn list_views(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<ViewInfo>> {
        let mut sql =
            "SELECT TABLE_SCHEMA, TABLE_NAME, VIEW_DEFINITION FROM INFORMATION_SCHEMA.VIEWS WHERE 1 = 1"
                .to_string();
        let mut params: Vec<CellValue> = Vec::new();
        if let Some(schema) = schema.filter(|value| !value.trim().is_empty()) {
            sql.push_str(" AND TABLE_SCHEMA = ?");
            params.push(CellValue::Text {
                value: schema.to_string(),
            });
        }
        sql.push_str(" ORDER BY TABLE_SCHEMA, TABLE_NAME");
        let result = self.query(&sql, &params)?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| ViewInfo {
                name: cell_as_text(&row[1]).unwrap_or_default(),
                kind: ObjectKind::View,
                definition_sql: cell_as_text(&row[2]).unwrap_or_default(),
                comment: String::new(),
                extra: json!({ "schema": cell_as_text(&row[0]).unwrap_or_default() }),
            })
            .collect())
    }

    pub fn list_indexes(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
        table: Option<&str>,
    ) -> Result<Vec<IndexInfo>> {
        let schema = match schema.filter(|value| !value.trim().is_empty()) {
            Some(schema) => schema.to_string(),
            None => self.current_database()?,
        };
        let mut sql = "SELECT INDEX_NAME, TABLE_NAME, TABLE_SCHEMA, \
                              GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ','), \
                              CASE WHEN NON_UNIQUE = 0 THEN 'YES' ELSE 'NO' END, INDEX_TYPE \
                       FROM INFORMATION_SCHEMA.STATISTICS WHERE TABLE_SCHEMA = ?"
            .to_string();
        let mut params: Vec<CellValue> = vec![CellValue::Text { value: schema }];
        if let Some(table) = table.filter(|value| !value.trim().is_empty()) {
            sql.push_str(" AND TABLE_NAME = ?");
            params.push(CellValue::Text {
                value: table.to_string(),
            });
        }
        sql.push_str(
            " GROUP BY INDEX_NAME, TABLE_NAME, TABLE_SCHEMA, NON_UNIQUE, INDEX_TYPE \
             ORDER BY INDEX_NAME",
        );
        let result = self.query(&sql, &params)?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| IndexInfo {
                name: cell_as_text(&row[0]).unwrap_or_default(),
                table: cell_as_text(&row[1]).unwrap_or_default(),
                columns: cell_as_text(&row[3])
                    .map(|value| value.split(',').map(str::to_string).collect())
                    .unwrap_or_default(),
                is_unique: cell_as_text(&row[4])
                    .map(|value| value.eq_ignore_ascii_case("YES"))
                    .unwrap_or(false),
                extra: json!({
                    "schema": cell_as_text(&row[2]).unwrap_or_default(),
                    "index_type": cell_as_text(&row[5]).unwrap_or_default(),
                }),
                ..Default::default()
            })
            .collect())
    }

    pub fn list_foreign_keys(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<ForeignKeyInfo>> {
        let schema = match schema.filter(|value| !value.trim().is_empty()) {
            Some(schema) => schema.to_string(),
            None => self.current_database()?,
        };
        let result = self.query(
            "SELECT kcu.CONSTRAINT_NAME, kcu.TABLE_NAME, \
                    GROUP_CONCAT(kcu.COLUMN_NAME ORDER BY kcu.ORDINAL_POSITION SEPARATOR ','), \
                    kcu.REFERENCED_TABLE_NAME, \
                    GROUP_CONCAT(kcu.REFERENCED_COLUMN_NAME ORDER BY kcu.ORDINAL_POSITION SEPARATOR ','), \
                    rc.UPDATE_RULE, rc.DELETE_RULE, kcu.REFERENCED_TABLE_SCHEMA \
             FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu \
             LEFT JOIN INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS rc \
               ON rc.CONSTRAINT_NAME = kcu.CONSTRAINT_NAME AND rc.CONSTRAINT_SCHEMA = kcu.TABLE_SCHEMA \
             WHERE kcu.TABLE_SCHEMA = ? AND kcu.TABLE_NAME = ? AND kcu.REFERENCED_TABLE_NAME IS NOT NULL \
             GROUP BY kcu.CONSTRAINT_NAME, kcu.TABLE_NAME, kcu.REFERENCED_TABLE_NAME, \
                      kcu.REFERENCED_TABLE_SCHEMA, rc.UPDATE_RULE, rc.DELETE_RULE \
             ORDER BY kcu.CONSTRAINT_NAME",
            &[
                CellValue::Text { value: schema },
                CellValue::Text {
                    value: table.to_string(),
                },
            ],
        )?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| ForeignKeyInfo {
                name: cell_as_text(&row[0]).unwrap_or_default(),
                from_table: cell_as_text(&row[1]).unwrap_or_default(),
                from_columns: cell_as_text(&row[2])
                    .map(|value| value.split(',').map(str::to_string).collect())
                    .unwrap_or_default(),
                to_table: cell_as_text(&row[3]).unwrap_or_default(),
                to_columns: cell_as_text(&row[4])
                    .map(|value| value.split(',').map(str::to_string).collect())
                    .unwrap_or_default(),
                to_schema: cell_as_text(&row[7]),
                on_update: cell_as_text(&row[5]),
                on_delete: cell_as_text(&row[6]),
                comment: String::new(),
            })
            .collect())
    }

    pub fn list_checks(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<CheckInfo>> {
        // MySQL 8.0.16+ 才有 INFORMATION_SCHEMA.CHECK_CONSTRAINTS，旧库/部分
        // 兼容实现没有：查询失败时按无 CHECK 约束处理而不是让对象浏览报错。
        let schema = match schema.filter(|value| !value.trim().is_empty()) {
            Some(schema) => schema.to_string(),
            None => self.current_database()?,
        };
        let result = self.query(
            "SELECT cc.CONSTRAINT_NAME, tc.TABLE_NAME, cc.CHECK_CLAUSE \
             FROM INFORMATION_SCHEMA.CHECK_CONSTRAINTS cc \
             JOIN INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc \
               ON tc.CONSTRAINT_NAME = cc.CONSTRAINT_NAME AND tc.CONSTRAINT_SCHEMA = cc.CONSTRAINT_SCHEMA \
             WHERE tc.CONSTRAINT_TYPE = 'CHECK' AND tc.TABLE_SCHEMA = ? AND tc.TABLE_NAME = ? \
             ORDER BY cc.CONSTRAINT_NAME",
            &[
                CellValue::Text { value: schema },
                CellValue::Text {
                    value: table.to_string(),
                },
            ],
        );
        let rows = match result {
            Ok(result) => result.rows,
            Err(error) => {
                tracing::warn!(%error, "server has no INFORMATION_SCHEMA.CHECK_CONSTRAINTS");
                Vec::new()
            }
        };
        Ok(rows
            .into_iter()
            .map(|row| CheckInfo {
                name: cell_as_text(&row[0]).unwrap_or_default(),
                table: cell_as_text(&row[1]).unwrap_or_default(),
                definition: cell_as_text(&row[2]),
                ..Default::default()
            })
            .collect())
    }

    pub fn list_functions(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<FunctionInfo>> {
        let mut sql = "SELECT ROUTINE_SCHEMA, ROUTINE_NAME, DTD_IDENTIFIER \
                       FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_TYPE = 'FUNCTION'"
            .to_string();
        let mut params: Vec<CellValue> = Vec::new();
        if let Some(schema) = schema.filter(|value| !value.trim().is_empty()) {
            sql.push_str(" AND ROUTINE_SCHEMA = ?");
            params.push(CellValue::Text {
                value: schema.to_string(),
            });
        }
        sql.push_str(" ORDER BY ROUTINE_SCHEMA, ROUTINE_NAME");
        let result = self.query(&sql, &params)?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| FunctionInfo {
                name: cell_as_text(&row[1]).unwrap_or_default(),
                return_type: cell_as_text(&row[2]),
                language: None,
                definition: None,
                extra: json!({ "schema": cell_as_text(&row[0]).unwrap_or_default() }),
                ..Default::default()
            })
            .collect())
    }

    pub fn list_procedures(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<FunctionInfo>> {
        let mut sql = "SELECT ROUTINE_SCHEMA, ROUTINE_NAME, DTD_IDENTIFIER \
                       FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_TYPE = 'PROCEDURE'"
            .to_string();
        let mut params: Vec<CellValue> = Vec::new();
        if let Some(schema) = schema.filter(|value| !value.trim().is_empty()) {
            sql.push_str(" AND ROUTINE_SCHEMA = ?");
            params.push(CellValue::Text {
                value: schema.to_string(),
            });
        }
        sql.push_str(" ORDER BY ROUTINE_SCHEMA, ROUTINE_NAME");
        let result = self.query(&sql, &params)?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| FunctionInfo {
                name: cell_as_text(&row[1]).unwrap_or_default(),
                return_type: cell_as_text(&row[2]),
                language: None,
                definition: None,
                extra: json!({ "schema": cell_as_text(&row[0]).unwrap_or_default() }),
                ..Default::default()
            })
            .collect())
    }

    pub fn list_triggers(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
        table: Option<&str>,
    ) -> Result<Vec<TriggerInfo>> {
        let mut sql = "SELECT TRIGGER_SCHEMA, TRIGGER_NAME, EVENT_OBJECT_TABLE, \
                              ACTION_TIMING, EVENT_MANIPULATION, ACTION_STATEMENT \
                       FROM INFORMATION_SCHEMA.TRIGGERS WHERE 1 = 1"
            .to_string();
        let mut params: Vec<CellValue> = Vec::new();
        if let Some(schema) = schema.filter(|value| !value.trim().is_empty()) {
            sql.push_str(" AND TRIGGER_SCHEMA = ?");
            params.push(CellValue::Text {
                value: schema.to_string(),
            });
        }
        if let Some(table) = table.filter(|value| !value.trim().is_empty()) {
            sql.push_str(" AND EVENT_OBJECT_TABLE = ?");
            params.push(CellValue::Text {
                value: table.to_string(),
            });
        }
        sql.push_str(" ORDER BY TRIGGER_SCHEMA, TRIGGER_NAME");
        let result = self.query(&sql, &params)?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| TriggerInfo {
                name: cell_as_text(&row[1]).unwrap_or_default(),
                table: cell_as_text(&row[2]).unwrap_or_default(),
                timing: cell_as_text(&row[3]).unwrap_or_default(),
                event: cell_as_text(&row[4]).unwrap_or_default(),
                definition: cell_as_text(&row[5]),
                extra: json!({ "schema": cell_as_text(&row[0]).unwrap_or_default() }),
                ..Default::default()
            })
            .collect())
    }

    pub fn list_sequences(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<extension_protocol::schema::SequenceInfo>> {
        // MySQL 家族基线没有 SEQUENCE 对象（TiDB 有但通过 SHOW SEQUENCES，
        // 不在 INFORMATION_SCHEMA 基线内），统一返回空。
        let _ = (_database, schema);
        Ok(Vec::new())
    }

    pub fn list_types(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<extension_protocol::schema::TypeInfo>> {
        let _ = (_database, schema);
        Ok(Vec::new())
    }

    pub fn view_definition(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
        view: &str,
    ) -> Result<ViewDefinitionResult> {
        let schema = match schema.filter(|value| !value.trim().is_empty()) {
            Some(schema) => schema.to_string(),
            None => self.current_database()?,
        };
        let result = self.query(
            "SELECT VIEW_DEFINITION FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
            &[
                CellValue::Text { value: schema },
                CellValue::Text {
                    value: view.to_string(),
                },
            ],
        )?;
        Ok(ViewDefinitionResult {
            sql: result
                .rows
                .first()
                .and_then(|row| cell_as_text(&row[0]))
                .unwrap_or_default(),
            is_materialized: false,
        })
    }

    pub fn dump_ddl(
        &mut self,
        objects: &[ObjectRef],
        _options: &DumpDdlOptions,
    ) -> Result<Vec<String>> {
        let mut statements = Vec::new();
        for object in objects {
            match object.kind {
                ObjectKind::View | ObjectKind::MaterializedView => {
                    let def = self.view_definition(
                        object.database.as_deref(),
                        object.schema.as_deref(),
                        &object.name,
                    )?;
                    if !def.sql.trim().is_empty() {
                        statements.push(format!(
                            "CREATE OR REPLACE VIEW `{}` AS {}",
                            object.name, def.sql
                        ));
                    }
                }
                _ => {
                    if let Some(ddl) = self.table_definition(
                        object.database.as_deref(),
                        object.schema.as_deref(),
                        &object.name,
                    )? {
                        statements.push(ddl);
                    }
                }
            }
        }
        Ok(statements)
    }

    /// `SHOW CREATE TABLE` 的服务端建表语句；表不存在时返回 None。
    /// 标识符无法用参数绑定，先经 `validate_identifier` 白名单校验。
    fn table_definition(
        &mut self,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Option<String>> {
        let schema = match schema.filter(|value| !value.trim().is_empty()) {
            Some(schema) => schema.to_string(),
            None => self.current_database()?,
        };
        let result = self.query(
            &format!(
                "SHOW CREATE TABLE `{}`.`{}`",
                escape_identifier(&schema)?,
                escape_identifier(table)?
            ),
            &[],
        )?;
        Ok(result
            .rows
            .first()
            .and_then(|row| cell_as_text(&row[1]))
            .filter(|ddl| !ddl.trim().is_empty()))
    }

    fn query_single_string(&mut self, sql: &str) -> Result<String> {
        let result = self.query(sql, &[])?;
        Ok(result
            .rows
            .first()
            .and_then(|row| cell_as_text(&row[0]))
            .unwrap_or_default())
    }

}

pub struct QueryResult {
    pub columns: Vec<ColumnSpec>,
    pub rows: Vec<Row>,
}

fn rows_to_query_result(columns: Vec<Column>, rows: Vec<mysql_async::Row>) -> QueryResult {
    let specs = columns.iter().map(column_to_spec).collect::<Vec<_>>();
    let converted = rows
        .into_iter()
        .map(|row| {
            let values = row_values(row);
            specs
                .iter()
                .zip(values)
                .map(|(spec, value)| value_to_cell(value, spec.type_kind))
                .collect::<Vec<_>>()
        })
        .collect();
    QueryResult {
        columns: specs,
        rows: converted,
    }
}

/// 以 Value 原生形态取出行内全部值（identity FromValue）。
fn row_values(row: mysql_async::Row) -> Vec<Value> {
    let columns = row.columns_ref().len();
    (0..columns)
        .map(|idx| row.get::<Value, usize>(idx).unwrap_or(Value::NULL))
        .collect()
}

fn bind_params(params: &[CellValue]) -> Result<Vec<Value>> {
    params.iter().map(bind_param).collect()
}

fn bind_param(param: &CellValue) -> Result<Value> {
    Ok(match param {
        CellValue::Null => Value::NULL,
        CellValue::Bool { value } => Value::Int(if *value { 1 } else { 0 }),
        CellValue::I64 { value } => Value::Int(*value),
        CellValue::U64 { value } => {
            if *value <= i64::MAX as u64 {
                Value::Int(*value as i64)
            } else {
                Value::Bytes(value.to_string().into_bytes())
            }
        }
        CellValue::F64 { value } => Value::Double(*value),
        CellValue::Bytes { value } => Value::Bytes(
            base64::engine::general_purpose::STANDARD
                .decode(value)
                .context("invalid base64 bytes parameter")?,
        ),
        CellValue::Json { value } => Value::Bytes(value.to_string().into_bytes()),
        CellValue::Date { value } => Value::Bytes(value.clone().into_bytes()),
        CellValue::Time { value } => Value::Bytes(value.clone().into_bytes()),
        CellValue::Datetime { value } => Value::Bytes(value.clone().into_bytes()),
        CellValue::Decimal { value }
        | CellValue::Text { value }
        | CellValue::Uuid { value }
        | CellValue::Duration { value }
        | CellValue::Geo { value, .. } => Value::Bytes(value.clone().into_bytes()),
        CellValue::Array { value, .. } => Value::Bytes(
            serde_json::to_string(value)
                .unwrap_or_default()
                .into_bytes(),
        ),
        CellValue::Map { value } => Value::Bytes(
            serde_json::Value::Object(value.clone())
                .to_string()
                .into_bytes(),
        ),
        CellValue::Custom { raw, .. } => Value::Bytes(raw.clone().into_bytes()),
    })
}

fn column_to_spec(column: &Column) -> ColumnSpec {
    ColumnSpec::new(column.name_str(), "", column_type_kind(column)).nullable(true)
}

/// 文本协议列的 kind 判定：BLOB/字节列依赖列元数据而非值内容——协议上
/// TEXT 与 BLOB 都可能以 Bytes 到达，二进制列必须无损 base64，不能猜 UTF-8。
fn column_type_kind(column: &Column) -> ColumnTypeKind {
    match column.column_type() {
        ColumnType::MYSQL_TYPE_TINY
        | ColumnType::MYSQL_TYPE_SHORT
        | ColumnType::MYSQL_TYPE_LONG
        | ColumnType::MYSQL_TYPE_LONGLONG
        | ColumnType::MYSQL_TYPE_INT24
        | ColumnType::MYSQL_TYPE_YEAR => ColumnTypeKind::I64,
        ColumnType::MYSQL_TYPE_FLOAT | ColumnType::MYSQL_TYPE_DOUBLE => ColumnTypeKind::F64,
        ColumnType::MYSQL_TYPE_NEWDECIMAL | ColumnType::MYSQL_TYPE_DECIMAL => {
            ColumnTypeKind::Decimal
        }
        ColumnType::MYSQL_TYPE_DATE => ColumnTypeKind::Date,
        ColumnType::MYSQL_TYPE_TIME => ColumnTypeKind::Time,
        ColumnType::MYSQL_TYPE_TIMESTAMP
        | ColumnType::MYSQL_TYPE_DATETIME
        | ColumnType::MYSQL_TYPE_NEWDATE => ColumnTypeKind::Datetime,
        ColumnType::MYSQL_TYPE_JSON => ColumnTypeKind::Json,
        ColumnType::MYSQL_TYPE_BIT
        | ColumnType::MYSQL_TYPE_BLOB
        | ColumnType::MYSQL_TYPE_TINY_BLOB
        | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
        | ColumnType::MYSQL_TYPE_LONG_BLOB
        | ColumnType::MYSQL_TYPE_GEOMETRY => ColumnTypeKind::Bytes,
        _ => ColumnTypeKind::Text,
    }
}

fn value_to_cell(value: Value, kind: ColumnTypeKind) -> CellValue {
    match value {
        Value::NULL => CellValue::Null,
        Value::Int(value) => CellValue::I64 { value },
        Value::UInt(value) => CellValue::U64 { value },
        Value::Float(value) => CellValue::F64 {
            value: value as f64,
        },
        Value::Double(value) => CellValue::F64 { value },
        Value::Bytes(bytes) => match kind {
            ColumnTypeKind::Bytes => CellValue::Bytes {
                value: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
            ColumnTypeKind::I64 => match String::from_utf8(bytes) {
                Ok(text) => match text.trim().parse::<i64>() {
                    Ok(value) => CellValue::I64 { value },
                    Err(_) => CellValue::Text { value: text },
                },
                Err(error) => CellValue::Bytes {
                    value: base64::engine::general_purpose::STANDARD.encode(error.into_bytes()),
                },
            },
            ColumnTypeKind::Json => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(value) => CellValue::Json { value },
                Err(_) => text_or_bytes(bytes),
            },
            _ => text_or_bytes(bytes),
        },
        Value::Date(year, month, day, hour, minute, second, _micros) => {
            if hour == 0 && minute == 0 && second == 0 {
                CellValue::Date {
                    value: format!("{year:04}-{month:02}-{day:02}"),
                }
            } else {
                CellValue::Datetime {
                    value: format!(
                        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
                    ),
                }
            }
        }
        Value::Time(neg, _days, hour, minute, second, _micros) => CellValue::Time {
            value: format!(
                "{}{hour:02}:{minute:02}:{second:02}",
                if neg { "-" } else { "" }
            ),
        },
    }
}

fn text_or_bytes(bytes: Vec<u8>) -> CellValue {
    match String::from_utf8(bytes) {
        Ok(text) => CellValue::Text { value: text },
        Err(error) => CellValue::Bytes {
            value: base64::engine::general_purpose::STANDARD.encode(error.into_bytes()),
        },
    }
}

fn cell_as_text(cell: &CellValue) -> Option<String> {
    match cell {
        CellValue::Null => None,
        CellValue::Text { value } => Some(value.clone()),
        CellValue::Decimal { value } => Some(value.clone()),
        CellValue::I64 { value } => Some(value.to_string()),
        CellValue::U64 { value } => Some(value.to_string()),
        CellValue::F64 { value } => Some(value.to_string()),
        CellValue::Bool { value } => Some(value.to_string()),
        CellValue::Date { value }
        | CellValue::Time { value }
        | CellValue::Datetime { value }
        | CellValue::Uuid { value }
        | CellValue::Duration { value } => Some(value.clone()),
        _ => None,
    }
}

fn cell_as_i64(cell: &CellValue) -> Option<i64> {
    match cell {
        CellValue::I64 { value } => Some(*value),
        CellValue::U64 { value } => i64::try_from(*value).ok(),
        CellValue::Text { value } => value.trim().parse().ok(),
        CellValue::Decimal { value } => value.trim().parse().ok(),
        _ => None,
    }
}

/// 标识符白名单：字母/数字/下划线/美元符/连字符与 CJK 等非 ASCII 字符；
/// 反引号、引号、NUL、点号一律拒绝（防 `SHOW CREATE TABLE` 注入）。
fn escape_identifier(identifier: &str) -> Result<String> {
    let trimmed = identifier.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("empty identifier"));
    }
    for ch in trimmed.chars() {
        if ch == '`' || ch == '\'' || ch == '"' || ch == '\0' || ch == '.' || ch == ';' {
            return Err(anyhow!(
                "identifier {trimmed:?} contains forbidden characters"
            ));
        }
    }
    Ok(trimmed.replace('`', "``"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_identifier_rejects_injection() {
        assert!(escape_identifier("t`ble").is_err());
        assert!(escape_identifier("a.b").is_err());
        assert!(escape_identifier("").is_err());
        assert_eq!(escape_identifier("users").unwrap(), "users");
        assert_eq!(escape_identifier("订单表").unwrap(), "订单表");
    }

    #[test]
    fn bind_params_map_wire_cells_to_driver_values() {
        let params = bind_params(&[
            CellValue::Null,
            CellValue::Bool { value: true },
            CellValue::I64 { value: -3 },
            CellValue::U64 { value: u64::MAX },
            CellValue::Text { value: "x".into() },
        ])
        .unwrap();
        assert_eq!(params[0], Value::NULL);
        assert_eq!(params[1], Value::Int(1));
        assert_eq!(params[2], Value::Int(-3));
        assert_eq!(params[3], Value::Bytes(u64::MAX.to_string().into_bytes()));
        assert_eq!(params[4], Value::Bytes(b"x".to_vec()));
    }

    #[test]
    fn value_to_cell_respects_declared_kind() {
        let bytes = vec![0xac, 0xed, 0x00, 0x05];
        let cell = value_to_cell(Value::Bytes(bytes.clone()), ColumnTypeKind::Bytes);
        assert!(matches!(cell, CellValue::Bytes { .. }));

        // 同样的字节若声明为文本且非法 UTF-8：回退 base64 bytes，绝不 lossy
        let cell = value_to_cell(Value::Bytes(bytes), ColumnTypeKind::Text);
        assert!(matches!(cell, CellValue::Bytes { .. }));

        let cell = value_to_cell(Value::Bytes(b"123".to_vec()), ColumnTypeKind::I64);
        assert_eq!(cell, CellValue::I64 { value: 123 });
    }
}
