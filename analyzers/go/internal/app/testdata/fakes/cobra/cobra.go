// Package cobra is a tiny stand-in for github.com/spf13/cobra, for tests.
package cobra

type Command struct {
	Use  string
	Run  func(cmd *Command, args []string)
	RunE func(cmd *Command, args []string) error
}

func (c *Command) AddCommand(cmds ...*Command) {}

func (c *Command) Execute() error { return nil }
