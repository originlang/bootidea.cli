package cli

import (
	"fmt"

	"github.com/spf13/cobra"
	"gopkg.in/yaml.v3"

	"bootidea.cli/internal/app"
)

func newConfigCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "config",
		Short: "查看配置",
		Args:  cobra.NoArgs,
	}

	cmd.AddCommand(&cobra.Command{
		Use:   "view",
		Short: "打印最终生效的配置及其来源",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			a, err := app.From(cmd.Context())
			if err != nil {
				return err
			}
			out, err := yaml.Marshal(a.Cfg)
			if err != nil {
				return err
			}
			fmt.Fprintf(a.IO.Out, "# source: %s\n", a.Cfg.Source)
			_, err = a.IO.Out.Write(out)
			return err
		},
	})

	return cmd
}
