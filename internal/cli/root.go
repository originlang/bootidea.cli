package cli

import (
	"context"
	"fmt"

	"github.com/spf13/cobra"

	"bootidea.cli/internal/app"
	"bootidea.cli/internal/config"
	"bootidea.cli/internal/iostreams"
	"bootidea.cli/internal/log"
	"bootidea.cli/internal/version"
)

const programName = "bootidea"

func Execute(ctx context.Context, args []string, streams *iostreams.Streams) int {
	root := newRootCmd(ctx, streams)
	root.SetArgs(args)

	if err := root.Execute(); err != nil {
		fmt.Fprintln(streams.Err, "错误:", err)
		return 1
	}
	return 0
}

func newRootCmd(ctx context.Context, streams *iostreams.Streams) *cobra.Command {
	root := &cobra.Command{
		Use:           programName,
		Short:         "BootIDEA 命令行工具",
		Long:          "BootIDEA 命令行工具。核心只提供配置、日志与命令框架，具体能力由各命令模块提供。",
		Version:       version.String(),
		SilenceUsage:  true,
		SilenceErrors: true,
		PersistentPreRunE: func(cmd *cobra.Command, _ []string) error {
			return bootstrap(cmd, streams)
		},
	}

	root.SetContext(ctx)
	root.SetIn(streams.In)
	root.SetOut(streams.Out)
	root.SetErr(streams.Err)

	bindGlobalFlags(root)
	root.AddCommand(builtinCommands()...)

	return root
}

// builtinCommands 是核心命令的注册缝：后续功能模块/插件在这里追加自己的 *cobra.Command。
func builtinCommands() []*cobra.Command {
	return []*cobra.Command{
		newConfigCmd(),
		newVersionCmd(),
	}
}

func bootstrap(cmd *cobra.Command, streams *iostreams.Streams) error {
	cfg, err := config.Load(overridesFrom(cmd))
	if err != nil {
		return fmt.Errorf("加载配置: %w", err)
	}

	logger, err := log.New(cfg, streams.Err, useColor(cfg, streams))
	if err != nil {
		return fmt.Errorf("初始化日志: %w", err)
	}

	cmd.SetContext(app.Into(cmd.Context(), &app.App{Cfg: cfg, Log: logger, IO: streams}))
	logger.Debug("配置已解析", "source", cfg.Source, "home", cfg.Home)
	return nil
}

func useColor(cfg *config.Config, streams *iostreams.Streams) bool {
	switch cfg.Color {
	case config.ColorAlways:
		return true
	case config.ColorNever:
		return false
	default:
		return streams.ErrTTY
	}
}
