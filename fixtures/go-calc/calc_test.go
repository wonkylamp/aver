package calc

import "testing"

func TestAdd(t *testing.T) {
	if got := Add(2, 3); got != 5 {
		t.Fatalf("Add(2, 3) = %d, want 5", got)
	}
}

func TestSub(t *testing.T) {
	if got := Sub(5, 3); got != 2 {
		t.Fatalf("Sub(5, 3) = %d, want 2", got)
	}
}

func TestMul(t *testing.T) {
	for _, c := range []struct{ a, b, want int }{{4, 3, 12}, {0, 7, 0}} {
		t.Run("", func(t *testing.T) {
			if got := Mul(c.a, c.b); got != c.want {
				t.Fatalf("Mul(%d, %d) = %d, want %d", c.a, c.b, got, c.want)
			}
		})
	}
}
