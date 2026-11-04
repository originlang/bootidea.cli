package cli

import (
	"fmt"

	"github.com/spf13/cobra"

	"bootidea.cli/internal/app"
	"bootidea.cli/internal/version"
)

func newVersionCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "version",
		Short: "打印版本信息",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			a, err := app.From(cmd.Context())
			if err != nil {
				return err
			}
			_, err = fmt.Fprintln(a.IO.Out, version.Details())
			return err
		},
	}
}
