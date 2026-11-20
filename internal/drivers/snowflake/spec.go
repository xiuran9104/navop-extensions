package snowflake

import (
	"strings"

	gosnowflake "github.com/snowflakedb/gosnowflake"

	"navop-db-ipc-drivers/internal/dbipc"
)

func ConfigFromWire(raw map[string]any) (dbipc.Config, error) {
	return dbipc.ConfigFromWire(raw, 443)
}

// Spec builds the driver spec for Snowflake. Snowflake's account-level object
// listing cannot be expressed through INFORMATION_SCHEMA (which is scoped to
// the current database), so schema/databases intentionally returns empty SQL
// and the shared server falls back to the connected database.
func Spec() dbipc.DriverSpec {
	return dbipc.DriverSpec{
		ID:                   "snowflake",
		Name:                 "Snowflake",
		SQLDriverName:        "snowflake",
		DefaultPort:          443,
		IdentifierQuoteLeft:  "\"",
		IdentifierQuoteRight: "\"",
		SupportsComments:     true,
		BuildDSN:             BuildDSN,
		SchemaSQL:            SchemaSQL(),
	}
}

// buildDSN uses gosnowflake.Config so every user-supplied value is escaped by
// the official DSN encoder instead of string formatting.
func BuildDSN(cfg dbipc.Config) (string, error) {
	if err := dbipc.RequireConfig(cfg, "host", "username"); err != nil {
		return "", err
	}
	snowflakeCfg := &gosnowflake.Config{
		User:     cfg.Username,
		Password: cfg.Password,
		Host:     cfg.Host,
		Port:     cfg.Port,
		Params:   map[string]*string{},
	}
	if database := strings.TrimSpace(cfg.Database); database != "" {
		// Snowflake connects to <database>/<schema>; the host form's database
		// field carries the Snowflake database, schema defaults to PUBLIC.
		snowflakeCfg.Database = database
		snowflakeCfg.Schema = "PUBLIC"
	}
	for key, value := range dbipc.CopyDriverExtra(cfg.Extra) {
		switch strings.ToLower(strings.TrimSpace(key)) {
		case "account":
			snowflakeCfg.Account = value
		case "warehouse":
			snowflakeCfg.Warehouse = value
		case "role":
			snowflakeCfg.Role = value
		case "schema":
			snowflakeCfg.Schema = value
		case "region":
			snowflakeCfg.Region = value
		}
	}
	return gosnowflake.DSN(snowflakeCfg)
}

func SchemaSQL() dbipc.SchemaSQL {
	// Snowflake's INFORMATION_SCHEMA is scoped to the connected database;
	// every object query filters by bound schema/table parameters.
	return dbipc.SchemaSQL{
		DatabasesQ: func(cfg dbipc.Config) dbipc.SchemaQuery {
			// Deliberately empty: falls back to the connected database.
			return dbipc.SchemaQuery{}
		},
		SchemasQ: func(cfg dbipc.Config, database string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL: "SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY SCHEMA_NAME",
			}
		},
		ObjectsQ: func(cfg dbipc.Config, database, schema string, kinds []string) dbipc.SchemaQuery {
			args := []any{schemaArg(cfg, schema)}
			return dbipc.SchemaQuery{
				SQL:  "SELECT TABLE_NAME, CASE WHEN TABLE_TYPE = 'VIEW' THEN 'view' ELSE 'table' END, '' AS COMMENT, TABLE_SCHEMA FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = ?" + objectsKindFilter(kinds) + " ORDER BY TABLE_NAME",
				Args: args,
			}
		},
		ColumnsQ: func(cfg dbipc.Config, database, schema, table string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT ORDINAL_POSITION, COLUMN_NAME, DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT, '' FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
				Args: []any{schemaArg(cfg, schema), table},
			}
		},
		ViewsQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT TABLE_NAME, TABLE_SCHEMA, '' , 'NO', VIEW_DEFINITION FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		FunctionsQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT FUNCTION_NAME, FUNCTION_SCHEMA, DATA_TYPE, 'FUNCTION', '' FROM INFORMATION_SCHEMA.FUNCTIONS WHERE FUNCTION_SCHEMA = ? ORDER BY FUNCTION_NAME",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		ViewDefinitionQ: func(cfg dbipc.Config, database, schema, view string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT VIEW_DEFINITION, 'NO' FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
				Args: []any{schemaArg(cfg, schema), view},
			}
		},
	}
}

// schemaArg resolves the effective schema for metadata filters: the explicit
// schema, else the database configured on the connection (Snowflake treats
// the host's database as the working database; INFORMATION_SCHEMA queries
// still filter on schema names).
func schemaArg(cfg dbipc.Config, schema string) string {
	if strings.TrimSpace(schema) != "" {
		return schema
	}
	return cfg.Database
}

func objectsKindFilter(kinds []string) string {
	if len(kinds) == 0 {
		return ""
	}
	seen := map[string]bool{}
	for _, kind := range kinds {
		switch strings.ToLower(strings.TrimSpace(kind)) {
		case "table", "base_table":
			seen["'BASE TABLE'"] = true
		case "view":
			seen["'VIEW'"] = true
		}
	}
	if len(seen) == 0 {
		return " AND 1 = 0"
	}
	values := make([]string, 0, len(seen))
	for _, value := range []string{"'BASE TABLE'", "'VIEW'"} {
		if seen[value] {
			values = append(values, value)
		}
	}
	return " AND TABLE_TYPE IN (" + strings.Join(values, ",") + ")"
}
