package dbipc

import (
	"context"
	"database/sql/driver"
	"encoding/json"
	"fmt"
	"testing"

	"navop-db-ipc-drivers/internal/ipc"
)

func TestSchemaObjectsBindsArgsFromArgsVariant(t *testing.T) {
	driverName, state := registerStreamingDriver(t, [][]driver.Value{
		{"orders", "table", "order rows", "sales"},
	})
	state.columns = []string{"object_name", "kind", "comment", "schema_name"}
	spec := testSpecWithSQLDriver(driverName)
	spec.SchemaSQL.ObjectsQ = func(cfg Config, database, schema string, kinds []string) SchemaQuery {
		return SchemaQuery{
			SQL:  "SELECT object_name, kind, comment, schema_name FROM all_objects WHERE schema_name = ?",
			Args: []any{schema},
		}
	}
	server := NewServer(spec, nil)
	server.initialized = true

	connID := openTestConn(t, server)
	resp := server.Handle(context.Background(), ipc.Message{
		JSONRPC: "2.0",
		ID:      json.RawMessage(`2`),
		Method:  "schema/objects",
		Params:  []byte(fmt.Sprintf(`{"conn_id":%d,"database":"main","schema":"sales","kinds":["table"]}`, connID)),
	})
	if resp.Error != nil {
		t.Fatalf("schema/objects returned error: %#v", resp.Error)
	}

	if state.lastQuerySQL != "SELECT object_name, kind, comment, schema_name FROM all_objects WHERE schema_name = ?" {
		t.Fatalf("query SQL = %q, want placeholder form", state.lastQuerySQL)
	}
	if len(state.lastQueryArgs) != 1 || state.lastQueryArgs[0].Value != "sales" {
		t.Fatalf("query args = %#v, want bound schema value", state.lastQueryArgs)
	}

	var result []map[string]any
	decodeResult(t, resp, &result)
	if len(result) != 1 || result[0]["name"] != "orders" {
		t.Fatalf("objects result = %#v", result)
	}
}

// TestSchemaObjectsGatesOnArgsVariantAlone proves a driver that only fills the
// args-bound builders (no legacy string functions) is still treated as
// capable: the handler must not take the "unsupported" fallback path.
func TestSchemaObjectsGatesOnArgsVariantAlone(t *testing.T) {
	driverName, state := registerStreamingDriver(t, [][]driver.Value{
		{"demo", "table", ""},
	})
	state.columns = []string{"object_name", "kind", "comment"}
	spec := testSpecWithSQLDriver(driverName)
	spec.SchemaSQL.Objects = nil
	spec.SchemaSQL.ObjectsQ = func(cfg Config, database, schema string, kinds []string) SchemaQuery {
		return SchemaQuery{SQL: "SELECT object_name, kind, comment FROM all_objects"}
	}
	server := NewServer(spec, nil)
	server.initialized = true

	connID := openTestConn(t, server)
	resp := server.Handle(context.Background(), ipc.Message{
		JSONRPC: "2.0",
		ID:      json.RawMessage(`2`),
		Method:  "schema/objects",
		Params:  []byte(fmt.Sprintf(`{"conn_id":%d,"database":"main","schema":"","kinds":["table"]}`, connID)),
	})
	if resp.Error != nil {
		t.Fatalf("schema/objects returned error: %#v", resp.Error)
	}
	if state.lastQuerySQL == "" {
		t.Fatal("args-only spec must still execute the objects query")
	}
}

func TestSchemaDatabasesUsesArgsVariantAndDefaultFallback(t *testing.T) {
	driverName, state := registerStreamingDriver(t, [][]driver.Value{
		{"main"},
	})
	state.columns = []string{"datname"}
	spec := testSpecWithSQLDriver(driverName)
	spec.SchemaSQL.Databases = nil
	spec.SchemaSQL.DatabasesQ = func(cfg Config) SchemaQuery {
		return SchemaQuery{SQL: "SELECT datname FROM sys_database WHERE datallowconn", Args: nil}
	}
	server := NewServer(spec, nil)
	server.initialized = true

	connID := openTestConn(t, server)

	resp := server.Handle(context.Background(), ipc.Message{
		JSONRPC: "2.0",
		ID:      json.RawMessage(`2`),
		Method:  "schema/databases",
		Params:  []byte(fmt.Sprintf(`{"conn_id":%d}`, connID)),
	})
	if resp.Error != nil {
		t.Fatalf("schema/databases returned error: %#v", resp.Error)
	}
	if state.lastQuerySQL != "SELECT datname FROM sys_database WHERE datallowconn" {
		t.Fatalf("databases SQL = %q", state.lastQuerySQL)
	}

	var result []map[string]any
	decodeResult(t, resp, &result)
	if len(result) != 1 || result[0]["name"] != "main" {
		t.Fatalf("databases result = %#v", result)
	}
}

func TestSchemaQueryResolversPreferArgsVariant(t *testing.T) {
	schemaSQL := SchemaSQL{
		Databases: func(Config) string { return "legacy-db" },
		DatabasesQ: func(Config) SchemaQuery {
			return SchemaQuery{SQL: "args-db", Args: []any{1}}
		},
		Schemas: func(Config, string) string { return "legacy-schema" },
	}
	if q := schemaSQL.databasesQuery(Config{}); q.SQL != "args-db" || len(q.Args) != 1 {
		t.Fatalf("databasesQuery = %#v, want args variant", q)
	}
	if q := schemaSQL.schemasQuery(Config{}, "db"); q.SQL != "legacy-schema" || q.Args != nil {
		t.Fatalf("schemasQuery = %#v, want legacy fallback", q)
	}
	if q := (SchemaSQL{}).columnsQuery(Config{}, "d", "s", "t"); q.SQL != "" || q.Args != nil {
		t.Fatalf("empty SchemaSQL columnsQuery = %#v, want zero query", q)
	}
}

func TestSchemaSQLHasHelpersCoverArgsVariants(t *testing.T) {
	onlyArgs := SchemaSQL{
		DumpDDLQ:        func(Config, string, string, string) SchemaQuery { return SchemaQuery{SQL: "x"} },
		ViewDefinitionQ: func(Config, string, string, string) SchemaQuery { return SchemaQuery{SQL: "x"} },
	}
	if !onlyArgs.hasDumpDDL() || !onlyArgs.hasViewDefinition() {
		t.Fatal("has helpers must accept args-only specs")
	}
	if onlyArgs.hasObjects() || onlyArgs.hasDatabases() {
		t.Fatal("has helpers must not report unset builders")
	}
}
