package trino

import (
	"strings"
	"testing"

	"navop-db-ipc-drivers/internal/dbipc"
)

func TestSpecCarriesIdentity(t *testing.T) {
	spec := Spec()
	if spec.ID != "trino" || spec.DefaultPort != 8080 || spec.SQLDriverName != "trino" {
		t.Fatalf("spec = %#v", spec)
	}
}

func TestBuildDSNEncodesCredentials(t *testing.T) {
	dsn, err := BuildDSN(dbipc.Config{
		Host: "trino.local", Port: 8443, Username: "u", Password: "p@ss:word", Database: "sales",
		Extra: map[string]string{"catalog": "hive", "ssl_mode": "require"},
	})
	if err != nil {
		t.Fatalf("BuildDSN: %v", err)
	}
	if !strings.HasPrefix(dsn, "https://") || !strings.Contains(dsn, "trino.local:8443") {
		t.Fatalf("dsn scheme/host = %q", dsn)
	}
	if strings.Contains(dsn, "p@ss:word") || strings.Contains(dsn, "catalog=hive") == false {
		t.Fatalf("dsn must escape password and carry catalog: %q", dsn)
	}
}

func TestMetadataQueriesBindHostValues(t *testing.T) {
	cfg := dbipc.Config{Database: "sales", Extra: map[string]string{"catalog": "hive"}}
	hostile := "x' OR '1'='1"
	sql := SchemaSQL()
	q := sql.ObjectsQ(cfg, "", hostile, nil)
	if strings.Contains(q.SQL, hostile) {
		t.Fatalf("objects SQL interpolates schema: %q", q.SQL)
	}
	if len(q.Args) != 1 || q.Args[0] != hostile {
		t.Fatalf("objects args = %#v", q.Args)
	}
	q = sql.ColumnsQ(cfg, "", hostile, "users")
	if strings.Contains(q.SQL, hostile) || len(q.Args) != 2 {
		t.Fatalf("columns query = %q args = %#v", q.SQL, q.Args)
	}
}

func TestViewDefinitionValidatesIdentifiers(t *testing.T) {
	cfg := dbipc.Config{Database: "sales", Extra: map[string]string{"catalog": "hive"}}
	sql := SchemaSQL()
	if q := sql.ViewDefinitionQ(cfg, "", "v;drop", "users"); q.SQL != "" {
		t.Fatalf("hostile schema must be rejected: %q", q.SQL)
	}
	if q := sql.ViewDefinitionQ(cfg, "", "sales", "users"); !strings.HasPrefix(q.SQL, "SHOW CREATE VIEW") {
		t.Fatalf("valid view definition SQL = %q", q.SQL)
	}
}
