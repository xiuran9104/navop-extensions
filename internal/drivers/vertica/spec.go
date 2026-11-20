package vertica

import (
	"fmt"
	"net/url"
	"strings"

	"navop-db-ipc-drivers/internal/dbipc"
)

func ConfigFromWire(raw map[string]any) (dbipc.Config, error) {
	return dbipc.ConfigFromWire(raw, 5433)
}

func Spec() dbipc.DriverSpec {
	return dbipc.DriverSpec{
		ID:                   "vertica",
		Name:                 "Vertica",
		SQLDriverName:        "vertica",
		DefaultPort:          5433,
		IdentifierQuoteLeft:  "\"",
		IdentifierQuoteRight: "\"",
		SupportsComments:     true,
		BuildDSN:             BuildDSN,
		SchemaSQL:            SchemaSQL(),
	}
}

// buildDSN renders the vertica:// URL form; credential and database values
// are percent-encoded by net/url rather than raw-formatted.
func BuildDSN(cfg dbipc.Config) (string, error) {
	if err := dbipc.RequireConfig(cfg, "host", "username", "database"); err != nil {
		return "", err
	}
	endpoint := url.URL{
		Scheme: "vertica",
		Host:   fmt.Sprintf("%s:%d", cfg.Host, cfg.Port),
		User:   url.UserPassword(cfg.Username, cfg.Password),
		Path:   "/" + strings.TrimPrefix(cfg.Database, "/"),
	}
	params := url.Values{}
	for key, value := range dbipc.CopyDriverExtra(cfg.Extra) {
		switch strings.ToLower(strings.TrimSpace(key)) {
		case "connection_load_balance":
			params.Set("connectionLoadBalance", value)
		case "backup_server_node":
			params.Set("BackupServerNode", value)
		}
	}
	endpoint.RawQuery = params.Encode()
	return endpoint.String(), nil
}

// schemaSQL reads Vertica's INFORMATION_SCHEMA / v_catalog. Vertica runs a
// single database per connection, so schema/databases falls back to the
// connected database.
func SchemaSQL() dbipc.SchemaSQL {
	return dbipc.SchemaSQL{
		DatabasesQ: func(cfg dbipc.Config) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{}
		},
		SchemasQ: func(cfg dbipc.Config, database string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL: "SELECT schema_name FROM v_catalog.schemata WHERE NOT is_system ORDER BY schema_name",
			}
		},
		ObjectsQ: func(cfg dbipc.Config, database, schema string, kinds []string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT table_name, CASE WHEN table_type = 'VIEW' THEN 'view' ELSE 'table' END, '' , table_schema FROM v_catalog.tables WHERE table_schema = ?" + objectsKindFilter(kinds) + " ORDER BY table_name",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		ColumnsQ: func(cfg dbipc.Config, database, schema, table string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT ordinal_position, column_name, data_type, is_nullable, column_default, '' FROM v_catalog.columns WHERE table_schema = ? AND table_name = ? ORDER BY ordinal_position",
				Args: []any{schemaArg(cfg, schema), table},
			}
		},
		ViewsQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT view_name, view_schema, '', 'NO', COALESCE(definition, '') FROM v_catalog.views WHERE view_schema = ? ORDER BY view_name",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		SequencesQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT sequence_name, sequence_schema, '' FROM v_catalog.sequences WHERE sequence_schema = ? ORDER BY sequence_name",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		ViewDefinitionQ: func(cfg dbipc.Config, database, schema, view string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT COALESCE(definition, ''), 'NO' FROM v_catalog.views WHERE view_schema = ? AND view_name = ?",
				Args: []any{schemaArg(cfg, schema), view},
			}
		},
	}
}

func schemaArg(cfg dbipc.Config, schema string) string {
	if strings.TrimSpace(schema) != "" {
		return schema
	}
	return "public"
}

func objectsKindFilter(kinds []string) string {
	if len(kinds) == 0 {
		return ""
	}
	seen := map[string]bool{}
	for _, kind := range kinds {
		switch strings.ToLower(strings.TrimSpace(kind)) {
		case "table", "base_table":
			seen["'table'"] = true
		case "view":
			seen["'view'"] = true
		}
	}
	if len(seen) == 0 {
		return " AND 1 = 0"
	}
	values := make([]string, 0, len(seen))
	for _, value := range []string{"'table'", "'view'"} {
		if seen[value] {
			values = append(values, value)
		}
	}
	return " AND table_type IN (" + strings.Join(values, ",") + ")"
}
