// Package calc is an Aver test fixture: a Go module with no dependencies
// and no aver.toml, so generic mode detects it.
package calc

import "errors"

// ErrNotImplemented is what Divide returns until it is written.
var ErrNotImplemented = errors.New("not implemented")

func Add(a, b int) int { return a + b }

func Sub(a, b int) int { return a - b }

func Mul(a, b int) int { return a * b }

func Divide(a, b int) (int, error) { return 0, ErrNotImplemented }
