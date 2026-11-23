//go:build !unix

package gocmd

import "os/exec"

// killTree leaves the default cancellation, which kills the command only;
// WaitDelay keeps a surviving child from blocking Wait.
func killTree(cmd *exec.Cmd) {}
