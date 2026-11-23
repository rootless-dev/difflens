package cli

import "github.com/spf13/cobra"

func runRefund(cmd *cobra.Command, args []string) error { return nil }

func Root() *cobra.Command {
	root := &cobra.Command{Use: "payctl"}
	root.AddCommand(&cobra.Command{
		Use:  "refund <id>",
		RunE: runRefund,
	})
	root.AddCommand(&cobra.Command{
		Use: "status",
		Run: func(cmd *cobra.Command, args []string) {},
	})
	return root
}
