package config

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"gopkg.in/yaml.v3"
)

type Overrides struct {
	ConfigPath *string
	Home       *string
	LogLevel   *string
	LogFormat  *string
	Color      *string
}

func Load(ov Overrides) (*Config, error) {
	cfg := Default()

	path, explicit, err := resolvePath(ov)
	if err != nil {
		return nil, err
	}
	if path != "" {
		if err := applyFile(cfg, path, explicit); err != nil {
			return nil, err
		}
		cfg.Source = path
	}

	applyEnv(cfg)
	applyOverrides(cfg, ov)

	cfg.Normalize()
	if err := cfg.Validate(); err != nil {
		return nil, err
	}
	return cfg, nil
}

func resolvePath(ov Overrides) (path string, explicit bool, err error) {
	if ov.ConfigPath != nil {
		if *ov.ConfigPath == "" {
			return "", true, fmt.Errorf("--config 需要非空路径")
		}
		return *ov.ConfigPath, true, nil
	}
	if v := os.Getenv(EnvPrefix + "CONFIG"); v != "" {
		return v, true, nil
	}
	for _, p := range searchPaths() {
		if fileExists(p) {
			return p, false, nil
		}
	}
	return "", false, nil
}

func searchPaths() []string {
	paths := make([]string, 0, 2)
	if xdg := os.Getenv("XDG_CONFIG_HOME"); xdg != "" {
		paths = append(paths, filepath.Join(xdg, xdgSubDir, fileName))
	}
	if home, err := os.UserHomeDir(); err == nil {
		paths = append(paths, filepath.Join(home, dirName, fileName))
	}
	return paths
}

type fileConfig struct {
	Home      *string `yaml:"home"`
	LogLevel  *string `yaml:"log_level"`
	LogFormat *string `yaml:"log_format"`
	Color     *string `yaml:"color"`
}

func applyFile(cfg *Config, path string, explicit bool) error {
	data, err := os.ReadFile(path)
	if err != nil {
		if explicit && errors.Is(err, os.ErrNotExist) {
			return fmt.Errorf("配置文件不存在: %s", path)
		}
		return fmt.Errorf("读取配置文件 %s: %w", path, err)
	}

	var fc fileConfig
	dec := yaml.NewDecoder(bytes.NewReader(data))
	dec.KnownFields(true)
	if err := dec.Decode(&fc); err != nil && !errors.Is(err, io.EOF) {
		return fmt.Errorf("解析配置文件 %s: %w", path, err)
	}

	for _, f := range []struct {
		dst *string
		src *string
		key string
	}{
		{&cfg.Home, fc.Home, "home"},
		{&cfg.LogLevel, fc.LogLevel, "log_level"},
		{&cfg.LogFormat, fc.LogFormat, "log_format"},
		{&cfg.Color, fc.Color, "color"},
	} {
		if f.src != nil {
			*f.dst = *f.src
		}
	}
	return nil
}

func applyEnv(cfg *Config) {
	set := func(dst *string, key string) {
		if v, ok := os.LookupEnv(EnvPrefix + key); ok {
			*dst = v
		}
	}
	set(&cfg.Home, "HOME")
	set(&cfg.LogLevel, "LOG_LEVEL")
	set(&cfg.LogFormat, "LOG_FORMAT")
	set(&cfg.Color, "COLOR")

	if os.Getenv("NO_COLOR") != "" {
		cfg.Color = ColorNever
	}
}

func applyOverrides(cfg *Config, ov Overrides) {
	set := func(dst *string, src *string) {
		if src != nil {
			*dst = *src
		}
	}
	set(&cfg.Home, ov.Home)
	set(&cfg.LogLevel, ov.LogLevel)
	set(&cfg.LogFormat, ov.LogFormat)
	set(&cfg.Color, ov.Color)
}

func fileExists(p string) bool {
	st, err := os.Stat(p)
	return err == nil && !st.IsDir()
}
