package hana

import (
	"strings"
	"testing"

	"navop-db-ipc-drivers/internal/dbipc"
)

func TestSpecCarriesIdentity(t *testing.T) {
	spec := Spec()
	if spec.ID != "hana" || spec.DefaultPort != 30015 || spec.SQLDriverName != "hdb" {
		t.Fatalf("spec = %#v", spec)
	}
}

func TestBuildDSNIsHdbURL(t *testing.T) {
	dsn, err := BuildDSN(dbipc.Config{
		Host: "h.local", Port: 30015, Username: "u", Password: "p@ss/word",
		Extra: map[string]string{"ssl_mode": "disable"},
	})
	if err != nil {
		t.Fatalf("BuildDSN: %v", err)
	}
	if !strings.HasPrefix(dsn, "hdb://") {
		t.Fatalf("dsn scheme = %q", dsn)
	}
	if strings.Contains(dsn, "p@ss/word") {
		t.Fatalf("credentials must be percent-encoded: %q", dsn)
	}
	if !strings.Contains(dsn, "tls=false") {
		t.Fatalf("ssl_mode disable must map to tls=false: %q", dsn)
	}
}

func TestMetadataQueriesBindHostValues(t *testing.T) {
	hostile := "x' OR '1'='1"
	sql := SchemaSQL()
	q := sql.ObjectsQ(dbipc.Config{}, "", hostile, nil)
	if strings.Contains(q.SQL, hostile) || len(q.Args) != 1 {
		t.Fatalf("objects query = %q args = %#v", q.SQL, q.Args)
	}
	// union 两个分支共享同一 schema 绑定参数。
	if strings.Count(q.SQL, "?") != 1 && strings.Count(q.SQL, "?") != 2 {
		t.Fatalf("objects placeholders = %q", q.SQL)
	}
	if q := sql.DatabasesQ(dbipc.Config{}); q.SQL != "" {
		t.Fatalf("hana is single-tenant-database; expected empty SQL, got %q", q.SQL)
	}
}
