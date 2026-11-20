package vertica

import (
	"strings"
	"testing"

	"navop-db-ipc-drivers/internal/dbipc"
)

func TestSpecCarriesIdentity(t *testing.T) {
	spec := Spec()
	if spec.ID != "vertica" || spec.DefaultPort != 5433 || spec.SQLDriverName != "vertica" {
		t.Fatalf("spec = %#v", spec)
	}
}

func TestBuildDSNIsVerticaURL(t *testing.T) {
	dsn, err := BuildDSN(dbipc.Config{
		Host: "v.local", Port: 5433, Username: "u", Password: "p@ss/word", Database: "vmart",
	})
	if err != nil {
		t.Fatalf("BuildDSN: %v", err)
	}
	if !strings.HasPrefix(dsn, "vertica://") {
		t.Fatalf("dsn scheme = %q", dsn)
	}
	if strings.Contains(dsn, "p@ss/word") {
		t.Fatalf("credentials must be percent-encoded: %q", dsn)
	}
	if !strings.Contains(dsn, "vmart") {
		t.Fatalf("dsn must carry database: %q", dsn)
	}
}

func TestMetadataQueriesBindHostValues(t *testing.T) {
	hostile := "x' OR '1'='1"
	sql := SchemaSQL()
	q := sql.ObjectsQ(dbipc.Config{}, "", hostile, nil)
	if strings.Contains(q.SQL, hostile) {
		t.Fatalf("objects SQL interpolates schema: %q", q.SQL)
	}
	if len(q.Args) != 1 || q.Args[0] != hostile {
		t.Fatalf("objects args = %#v", q.Args)
	}
	if q := sql.DatabasesQ(dbipc.Config{}); q.SQL != "" {
		t.Fatalf("vertica is single-database; expected empty SQL, got %q", q.SQL)
	}
	if q := sql.SequencesQ(dbipc.Config{}, "", "public"); len(q.Args) != 1 || q.Args[0] != "public" {
		t.Fatalf("sequences args = %#v", q.Args)
	}
}
