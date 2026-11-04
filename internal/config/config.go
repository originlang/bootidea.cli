package config

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const (
	EnvPrefix = "BOOTIDEA_"

	dirName   = ".bootidea"
	fileName  = "config.yaml"
	xdgSubDir = "bootidea"

	LevelDebug = "debug"
	LevelInfo  = "info"
	LevelWarn  = "warn"
	LevelError = "error"

	FormatText = "text"
	FormatJSON = "json"

	ColorAuto   = "auto"
	ColorAlways = "always"
	ColorNever  = "never"
)

type Config struct {
	Home      string `yaml:"home"`
	LogLevel  string `yaml:"log_level"`
	LogFormat string `yaml:"log_format"`
	Color     string `yaml:"color"`

	Source string `yaml:"-"`
}

func Default() *Config {
	return &Config{
		Home:      defaultHome(),
		LogLevel:  LevelInfo,
		LogFormat: FormatText,
		Color:     ColorAuto,
	}
}

func (c *Config) Normalize() {
	c.LogLevel = normalizeLevel(c.LogLevel)
	c.LogFormat = strings.ToLower(strings.TrimSpace(c.LogFormat))
	c.Color = strings.ToLower(strings.TrimSpace(c.Color))
}

func (c *Config) Validate() error {
	if err := oneOf("日志级别", c.LogLevel, LevelDebug, LevelInfo, LevelWarn, LevelError); err != nil {
		return err
	}
	if err := oneOf("日志格式", c.LogFormat, FormatText, FormatJSON); err != nil {
		return err
	}
	if err := oneOf("颜色输出", c.Color, ColorAuto, ColorAlways, ColorNever); err != nil {
		return err
	}
	if strings.TrimSpace(c.Home) == "" {
		return fmt.Errorf("数据目录不能为空")
	}
	return nil
}

func normalizeLevel(v string) string {
	v = strings.ToLower(strings.TrimSpace(v))
	if v == "warning" {
		return LevelWarn
	}
	return v
}

func oneOf(kind, v string, allowed ...string) error {
	for _, a := range allowed {
		if a == v {
			return nil
		}
	}
	return fmt.Errorf("%s %q 无效，可选值：%s", kind, v, strings.Join(allowed, "|"))
}

func defaultHome() string {
	base, err := os.UserHomeDir()
	if err != nil {
		return filepath.Join(".", dirName)
	}
	return filepath.Join(base, dirName)
}
