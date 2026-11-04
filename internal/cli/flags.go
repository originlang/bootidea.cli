package cli

import (
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"

	"bootidea.cli/internal/config"
)

func bindGlobalFlags(root *cobra.Command) {
	pf := root.PersistentFlags()
	pf.StringP("config", "c", "", "配置文件路径（默认按 $BOOTIDEA_CONFIG、~/.bootidea/config.yaml 顺序查找）")
	pf.String("home", "", "BootIDEA 数据目录")
	pf.String("log-level", "", "日志级别: debug|info|warn|error")
	pf.String("log-format", "", "日志格式: text|json")
	pf.String("color", "", "颜色输出: auto|always|never")
}

func overridesFrom(cmd *cobra.Command) config.Overrides {
	f := cmd.Flags()
	return config.Overrides{
		ConfigPath: optString(f, "config"),
		Home:       optString(f, "home"),
		LogLevel:   optString(f, "log-level"),
		LogFormat:  optString(f, "log-format"),
		Color:      optString(f, "color"),
	}
}

// optString 只把用户显式传入的 flag 变成覆盖项，未传入时交由下层（环境变量、配置文件、默认值）决定。
func optString(f *pflag.FlagSet, name string) *string {
	if !f.Changed(name) {
		return nil
	}
	v := f.Lookup(name).Value.String()
	return &v
}
