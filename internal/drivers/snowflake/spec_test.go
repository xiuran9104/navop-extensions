package snowflake

import (
	"strings"
	"testing"

	"navop-db-ipc-drivers/internal/dbipc"
)

func TestSpecCarriesIdentity(t *testing.T) {
	spec := Spec()
	if spec.ID != "snowflake" || spec.DefaultPort != 443 || spec.SQLDriverName != "snowflake" {
		t.Fatalf("spec = %#v", spec)
	}
}

func TestBuildDSNUsesOfficialEncoder(t *testing.T) {
	dsn, err := BuildDSN(dbipc.Config{
		Host: "acct.snowflakecomputing.com", Port: 443, Username: "u", Password: "p@ss word",
		Database: "ANALYTICS", Extra: map[string]string{"account": "acct", "warehouse": "WH"},
	})
	if err != nil {
		t.Fatalf("BuildDSN: %v", err)
	}
	if strings.Contains(dsn, "p@ss word") {
		t.Fatalf("password must be DSN-encoded by gosnowflake: %q", dsn)
	}
	if !strings.Contains(dsn, "warehouse=WH") || !strings.Contains(dsn, "ANALYTICS") {
		t.Fatalf("dsn must carry warehouse and database: %q", dsn)
	}
}

func TestMetadataQueriesBindHostValues(t *testing.T) {
	hostile := "x' OR '1'='1"
	sql := SchemaSQL()
	q := sql.ObjectsQ(dbipc.Config{Database: "ANALYTICS"}, "", hostile, []string{"table"})
	if strings.Contains(q.SQL, hostile) {
		t.Fatalf("objects SQL interpolates schema: %q", q.SQL)
	}
	if len(q.Args) != 1 || q.Args[0] != hostile {
		t.Fatalf("objects args = %#v", q.Args)
	}
	// schema 为空时回退连接的 database（Snowflake database 即工作库）。
	q = sql.ColumnsQ(dbipc.Config{Database: "ANALYTICS"}, "", "", "users")
	if len(q.Args) != 2 || q.Args[0] != "ANALYTICS" {
		t.Fatalf("columns args must fall back to database: %#v", q.Args)
	}
}

func TestDatabasesFallsBackToConnectedDatabase(t *testing.T) {
	if q := SchemaSQL().DatabasesQ(dbipc.Config{}); q.SQL != "" {
		t.Fatalf("snowflake has no bindable cross-database listing; expected empty SQL, got %q", q.SQL)
	}
}
