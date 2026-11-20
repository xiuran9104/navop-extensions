package hana

import (
	"fmt"
	"net/url"
	"strings"

	"navop-db-ipc-drivers/internal/dbipc"
)

func ConfigFromWire(raw map[string]any) (dbipc.Config, error) {
	return dbipc.ConfigFromWire(raw, 30015)
}

func Spec() dbipc.DriverSpec {
	return dbipc.DriverSpec{
		ID:                   "hana",
		Name:                 "SAP HANA",
		SQLDriverName:        "hdb",
		DefaultPort:          30015,
		IdentifierQuoteLeft:  "\"",
		IdentifierQuoteRight: "\"",
		SupportsComments:     true,
		BuildDSN:             BuildDSN,
		SchemaSQL:            SchemaSQL(),
	}
}

// buildDSN renders the go-hdb URL form (`hdb://user:password@host:port`);
// credentials are percent-encoded by net/url. TLS selection follows the
// ssl_mode extra parameter (go-hdb's `?tls=` flag).
func BuildDSN(cfg dbipc.Config) (string, error) {
	if err := dbipc.RequireConfig(cfg, "host", "username"); err != nil {
		return "", err
	}
	endpoint := url.URL{
		Scheme: "hdb",
		Host:   fmt.Sprintf("%s:%d", cfg.Host, cfg.Port),
		User:   url.UserPassword(cfg.Username, cfg.Password),
	}
	params := url.Values{}
	for key, value := range dbipc.CopyDriverExtra(cfg.Extra) {
		switch strings.ToLower(strings.TrimSpace(key)) {
		case "tls", "ssl_mode", "sslmode":
			// go-hdb exposes TLS as the `tls` query flag; accept both spellings.
			if strings.EqualFold(strings.TrimSpace(key), "ssl_mode") || strings.EqualFold(strings.TrimSpace(key), "sslmode") {
				if strings.EqualFold(strings.TrimSpace(value), "disable") {
					params.Set("tls", "false")
				} else if !strings.EqualFold(strings.TrimSpace(value), "") {
					params.Set("tls", "true")
				}
				continue
			}
			params.Set("tls", value)
		case "certificate":
			params.Set("certificate", value)
		case "database":
			params.Set("databaseName", value)
		}
	}
	endpoint.RawQuery = params.Encode()
	return endpoint.String(), nil
}

// schemaSQL reads SAP HANA's SYS system views. HANA is single-database per
// tenant, so schema/databases falls back to the connected database.
func SchemaSQL() dbipc.SchemaSQL {
	return dbipc.SchemaSQL{
		DatabasesQ: func(cfg dbipc.Config) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{}
		},
		SchemasQ: func(cfg dbipc.Config, database string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL: "SELECT SCHEMA_NAME, SCHEMA_OWNER FROM SYS.SCHEMAS WHERE SCHEMA_NAME NOT LIKE 'SYS%' AND SCHEMA_NAME NOT LIKE 'SAP%' AND SCHEMA_NAME NOT LIKE '_SYS%' ORDER BY SCHEMA_NAME",
			}
		},
		ObjectsQ: func(cfg dbipc.Config, database, schema string, kinds []string) dbipc.SchemaQuery {
			args := []any{schemaArg(cfg, schema)}
			sql := "SELECT TABLE_NAME, 'table', '', SCHEMA_NAME FROM SYS.TABLES WHERE SCHEMA_NAME = ?" +
				" UNION ALL SELECT VIEW_NAME, 'view', '', SCHEMA_NAME FROM SYS.VIEWS WHERE IS_VALID = 'TRUE' AND SCHEMA_NAME = ?"
			if len(kinds) > 0 {
				includeTables := false
				includeViews := false
				for _, kind := range kinds {
					switch strings.ToLower(strings.TrimSpace(kind)) {
					case "table", "base_table":
						includeTables = true
					case "view":
						includeViews = true
					}
				}
				switch {
				case includeTables && !includeViews:
					sql = "SELECT TABLE_NAME, 'table', '', SCHEMA_NAME FROM SYS.TABLES WHERE SCHEMA_NAME = ?"
				case includeViews && !includeTables:
					sql = "SELECT VIEW_NAME, 'view', '', SCHEMA_NAME FROM SYS.VIEWS WHERE IS_VALID = 'TRUE' AND SCHEMA_NAME = ?"
				case !includeTables && !includeViews:
					sql = "SELECT TABLE_NAME, 'table', '', SCHEMA_NAME FROM SYS.TABLES WHERE 1 = 0"
				}
			}
			return dbipc.SchemaQuery{SQL: sql, Args: args}
		},
		ColumnsQ: func(cfg dbipc.Config, database, schema, table string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT POSITION, COLUMN_NAME, DATA_TYPE_NAME, IS_NULLABLE, DEFAULT_VALUE, COALESCE(COMMENTS, '') FROM SYS.TABLE_COLUMNS WHERE SCHEMA_NAME = ? AND TABLE_NAME = ? ORDER BY POSITION",
				Args: []any{schemaArg(cfg, schema), table},
			}
		},
		ViewsQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT VIEW_NAME, SCHEMA_NAME, COALESCE(COMMENTS, ''), 'NO', '' FROM SYS.VIEWS WHERE IS_VALID = 'TRUE' AND SCHEMA_NAME = ? ORDER BY VIEW_NAME",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		SequencesQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT SEQUENCE_NAME, SCHEMA_NAME, '' FROM SYS.SEQUENCES WHERE SCHEMA_NAME = ? ORDER BY SEQUENCE_NAME",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		FunctionsQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT FUNCTION_NAME, SCHEMA_NAME, '', 'FUNCTION', COALESCE(DEFINITION, '') FROM SYS.FUNCTIONS WHERE SCHEMA_NAME = ? ORDER BY FUNCTION_NAME",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		ProceduresQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{
				SQL:  "SELECT PROCEDURE_NAME, SCHEMA_NAME, '', 'PROCEDURE', COALESCE(DEFINITION, '') FROM SYS.PROCEDURES WHERE SCHEMA_NAME = ? ORDER BY PROCEDURE_NAME",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		ViewDefinitionQ: func(cfg dbipc.Config, database, schema, view string) dbipc.SchemaQuery {
			// SYS.VIEWS does not expose the view definition text; leave the
			// provider unset so the host falls back to its shared builder.
			return dbipc.SchemaQuery{}
		},
	}
}

func schemaArg(cfg dbipc.Config, schema string) string {
	if strings.TrimSpace(schema) != "" {
		return schema
	}
	return "PUBLIC"
}
