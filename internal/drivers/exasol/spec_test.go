package exasol

import (
	"strings"
	"testing"

	"navop-db-ipc-drivers/internal/dbipc"
)

func TestSpecCarriesIdentity(t *testing.T) {
	spec := Spec()
	if spec.ID != "exasol" || spec.DefaultPort != 8563 || spec.SQLDriverName != "exasol" {
		t.Fatalf("spec = %#v", spec)
	}
}

func TestBuildDSNEncodesCredentials(t *testing.T) {
	dsn, err := BuildDSN(dbipc.Config{Host: "e.local", Port: 8563, Username: "u", Password: "p@ss:word"})
	if err != nil {
		t.Fatalf("BuildDSN: %v", err)
	}
	if !strings.HasSuffix(dsn, "e.local:8563") {
		t.Fatalf("dsn endpoint = %q", dsn)
	}
	if strings.Contains(dsn, "p@ss:word") {
		t.Fatalf("credentials must be url-encoded: %q", dsn)
	}
}

func TestMetadataQueriesBindHostValues(t *testing.T) {
	hostile := "x' OR '1'='1"
	sql := SchemaSQL()
	q := sql.ObjectsQ(dbipc.Config{}, "", hostile, nil)
	if strings.Contains(q.SQL, hostile) || len(q.Args) != 1 {
		t.Fatalf("objects query = %q args = %#v", q.SQL, q.Args)
	}
	q = sql.SequencesQ(dbipc.Config{}, "", "public")
	if len(q.Args) != 1 || q.Args[0] != "public" {
		t.Fatalf("sequences args = %#v", q.Args)
	}
}
