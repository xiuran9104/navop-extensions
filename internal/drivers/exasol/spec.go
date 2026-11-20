package exasol

import (
	"fmt"
	"net/url"
	"strings"

	"navop-db-ipc-drivers/internal/dbipc"
)

func ConfigFromWire(raw map[string]any) (dbipc.Config, error) {
	return dbipc.ConfigFromWire(raw, 8563)
}

func Spec() dbipc.DriverSpec {
	return dbipc.DriverSpec{
		ID:                   "exasol",
		Name:                 "Exasol",
		SQLDriverName:        "exasol",
		DefaultPort:          8563,
		IdentifierQuoteLeft:  "\"",
		IdentifierQuoteRight: "\"",
		SupportsComments:     true,
		BuildDSN:             BuildDSN,
		SchemaSQL:            SchemaSQL(),
	}
}

// buildDSN renders `user:password@host:port` with credentials encoded through
// net/url; exasol-driver-go parses this host[:port]-with-auth form natively.
func BuildDSN(cfg dbipc.Config) (string, error) {
	if err := dbipc.RequireConfig(cfg, "host", "username"); err != nil {
		return "", err
	}
	user := url.UserPassword(cfg.Username, cfg.Password)
	return fmt.Sprintf("%s@%s:%d", user.String(), cfg.Host, cfg.Port), nil
}

// schemaSQL reads Exasol's INFORMATION_SCHEMA (7.1+) and EXA_ALL data
// dictionary. Every schema/table filter value is bound as a parameter.
func SchemaSQL() dbipc.SchemaSQL {
	return dbipc.SchemaSQL{
		// Exasol runs one database per instance; fall back to the connected
		// database name.
		DatabasesQ: func(cfg dbipc.Config) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{}
		},
		SchemasQ: func(cfg dbipc.Config, database string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL: "SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY SCHEMA_NAME",
			}
		},
		ObjectsQ: func(cfg dbipc.Config, database, schema string, kinds []string) dbipc.SchemaQuery {
			args := []any{schemaArg(cfg, schema)}
			// Exasol's information_schema.tables carries BASE TABLE / VIEW;
			// keep the kind filter on INFORMATION_SCHEMA.TABLES so tables and
			// views come from one bound query.
			return dbipc.SchemaQuery{
				SQL:  "SELECT TABLE_NAME, CASE WHEN TABLE_TYPE = 'VIEW' THEN 'view' ELSE 'table' END, '' , TABLE_SCHEMA FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = ?" + objectsKindFilter(kinds) + " ORDER BY TABLE_NAME",
				Args: args,
			}
		},
		ColumnsQ: func(cfg dbipc.Config, database, schema, table string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT ORDINAL_POSITION, COLUMN_NAME, TYPE_ATTRIBUTE, IS_NULLABLE, COLUMN_DEFAULT, COALESCE(COMMENT, '') FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
				Args: []any{schemaArg(cfg, schema), table},
			}
		},
		ViewsQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT VIEW_NAME, SCHEMA_NAME, '', 'NO', '' FROM SYS.EXA_ALL_VIEWS WHERE SCHEMA_NAME = ? ORDER BY VIEW_NAME",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		SequencesQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT SEQUENCE_NAME, SCHEMA_NAME, '' FROM SYS.EXA_ALL_SEQUENCES WHERE SCHEMA_NAME = ? ORDER BY SEQUENCE_NAME",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		FunctionsQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT FUNCTION_NAME, SCHEMA_NAME, '', 'FUNCTION', '' FROM SYS.EXA_ALL_FUNCTIONS WHERE SCHEMA_NAME = ? ORDER BY FUNCTION_NAME",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
	}
}

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
