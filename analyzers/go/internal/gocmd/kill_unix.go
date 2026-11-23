//go:build unix

package gocmd

import (
	"os/exec"
	"syscall"
)

// killTree starts the command in its own process group and makes
// cancellation kill the whole group: the go command does not forward a kill
// to the test binaries it started.
func killTree(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error {
		return syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
	}
}
