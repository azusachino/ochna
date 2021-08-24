package cmd

import "github.com/spf13/cobra"

var Root = &cobra.Command{
	Use:   "ochna",
	Short: "Show help for ochna commands, flags and backends",
	PersistentPostRun: func(cmd *cobra.Command, args []string) {

	},
	DisableAutoGenTag: true,
}
