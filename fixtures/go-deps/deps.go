// Package deps is an Aver test fixture: a Go module whose tests need a
// module from go.sum, installed into a module cache layer.
package deps

// Pair returns its arguments as a slice.
func Pair(a, b string) []string { return []string{a, b} }
