package trino

import (
	"fmt"
	"net/url"
	"strings"

	"navop-db-ipc-drivers/internal/dbipc"
)

func ConfigFromWire(raw map[string]any) (dbipc.Config, error) {
	return dbipc.ConfigFromWire(raw, 8080)
}

func Spec() dbipc.DriverSpec {
	return dbipc.DriverSpec{
		ID:                   "trino",
		Name:                 "Trino",
		SQLDriverName:        "trino",
		DefaultPort:          8080,
		IdentifierQuoteLeft:  "\"",
		IdentifierQuoteRight: "\"",
		SupportsComments:     true,
		BuildDSN:             BuildDSN,
		SchemaSQL:            SchemaSQL(),
	}
}

// buildDSN assembles the trino-go-client URL. The host form's database field
// maps to the Trino schema; the catalog comes from the catalog extra
// parameter. Credentials are percent-encoded through net/url.
func BuildDSN(cfg dbipc.Config) (string, error) {
	if err := dbipc.RequireConfig(cfg, "host", "username"); err != nil {
		return "", err
	}
	scheme := "https"
	if strings.EqualFold(strings.TrimSpace(cfg.Extra["ssl_mode"]), "disable") {
		scheme = "http"
	}
	endpoint := url.URL{
		Scheme: scheme,
		Host:   fmt.Sprintf("%s:%d", cfg.Host, cfg.Port),
		User:   url.User(cfg.Username),
	}
	if cfg.Password != "" {
		endpoint.User = url.UserPassword(cfg.Username, cfg.Password)
	}
	params := url.Values{}
	if catalog := strings.TrimSpace(cfg.Extra["catalog"]); catalog != "" {
		params.Set("catalog", catalog)
	}
	if schema := strings.TrimSpace(cfg.Database); schema != "" {
		params.Set("schema", schema)
	}
	for key, value := range dbipc.CopyDriverExtra(cfg.Extra) {
		switch strings.ToLower(strings.TrimSpace(key)) {
		case "source":
			params.Set("source", value)
		case "session_properties":
			params.Set("sessionProperties", value)
		}
	}
	endpoint.RawQuery = params.Encode()
	return endpoint.String(), nil
}

// qualifiedCatalog renders `<catalog>.information_schema.<view>` for metadata
// reads. The catalog is an identifier (from the connection form), not a bind
// value in Trino SQL, so it passes through validateIdentifier before any
// interpolation.
func qualifiedCatalog(cfg dbipc.Config, view string) (string, error) {
	catalog := strings.TrimSpace(cfg.Extra["catalog"])
	if catalog == "" {
		return "", fmt.Errorf("missing required extra parameter catalog")
	}
	if err := validateIdentifier(catalog); err != nil {
		return "", err
	}
	return catalog + ".information_schema." + view, nil
}

func validateIdentifier(identifier string) error {
	if identifier == "" {
		return fmt.Errorf("empty identifier")
	}
	for _, ch := range identifier {
		if ch == '"' || ch == '\'' || ch == '`' || ch == '\000' || ch == '.' || ch == ';' {
			return fmt.Errorf("identifier %q contains forbidden characters", identifier)
		}
	}
	return nil
}

func SchemaSQL() dbipc.SchemaSQL {
	return dbipc.SchemaSQL{
		// SHOW CATALOGS has no bindable SELECT equivalent; its single output
		// column is the catalog name, so the shared positional mapper reads
		// it correctly.
		DatabasesQ: func(cfg dbipc.Config) dbipc.SchemaQuery {
			return dbipc.SchemaQuery{SQL: "SHOW CATALOGS"}
		},
		SchemasQ: func(cfg dbipc.Config, database string) dbipc.SchemaQuery {
			source, err := qualifiedCatalog(cfg, "schemata")
			if err != nil {
				return dbipc.SchemaQuery{}
			}
			return dbipc.SchemaQuery{
				SQL: "SELECT schema_name FROM " + source + " ORDER BY schema_name",
			}
		},
		ObjectsQ: func(cfg dbipc.Config, database, schema string, kinds []string) dbipc.SchemaQuery {
			source, err := qualifiedCatalog(cfg, "tables")
			if err != nil {
				return dbipc.SchemaQuery{}
			}
			args := []any{schemaArg(cfg, schema)}
			return dbipc.SchemaQuery{
				SQL:  "SELECT table_name, CASE WHEN table_type = 'VIEW' THEN 'view' ELSE 'table' END, '' , table_schema FROM " + source + " WHERE table_schema = ?" + objectsKindFilter(kinds) + " ORDER BY table_name",
				Args: args,
			}
		},
		ColumnsQ: func(cfg dbipc.Config, database, schema, table string) dbipc.SchemaQuery {
			source, err := qualifiedCatalog(cfg, "columns")
			if err != nil {
				return dbipc.SchemaQuery{}
			}
			return dbipc.SchemaQuery{
				SQL:  "SELECT ordinal_position, column_name, data_type, is_nullable, NULL, '' FROM " + source + " WHERE table_schema = ? AND table_name = ? ORDER BY ordinal_position",
				Args: []any{schemaArg(cfg, schema), table},
			}
		},
		ViewsQ: func(cfg dbipc.Config, database, schema string) dbipc.SchemaQuery {
			source, err := qualifiedCatalog(cfg, "tables")
			if err != nil {
				return dbipc.SchemaQuery{}
			}
			return dbipc.SchemaQuery{
				SQL:  "SELECT table_name, table_schema, '', 'NO', '' FROM " + source + " WHERE table_type = 'VIEW' AND table_schema = ? ORDER BY table_name",
				Args: []any{schemaArg(cfg, schema)},
			}
		},
		ViewDefinitionQ: func(cfg dbipc.Config, database, schema, view string) dbipc.SchemaQuery {
			// SHOW CREATE VIEW takes identifiers, not bind values; both are
			// validated before formatting.
			schemaName := schemaArg(cfg, schema)
			if err := validateIdentifier(schemaName); err != nil {
				return dbipc.SchemaQuery{}
			}
			if err := validateIdentifier(view); err != nil {
				return dbipc.SchemaQuery{}
			}
			return dbipc.SchemaQuery{
				SQL: fmt.Sprintf("SHOW CREATE VIEW %s.%s", quoteIdentifier(schemaName), quoteIdentifier(view)),
			}
		},
	}
}

func quoteIdentifier(identifier string) string {
	return "\"" + strings.ReplaceAll(identifier, "\"", "\"\"") + "\""
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
	return " AND table_type IN (" + strings.Join(values, ",") + ")"
}
