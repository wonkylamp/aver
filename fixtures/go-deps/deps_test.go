package deps

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestPair(t *testing.T) {
	if diff := cmp.Diff([]string{"a", "b"}, Pair("a", "b")); diff != "" {
		t.Fatalf("Pair mismatch (-want +got):\n%s", diff)
	}
}
