package log

import (
	"fmt"
	"io"
	"log/slog"

	"bootidea.cli/internal/config"
)

func New(cfg *config.Config, w io.Writer, colored bool) (*slog.Logger, error) {
	level, err := parseLevel(cfg.LogLevel)
	if err != nil {
		return nil, err
	}

	opts := &slog.HandlerOptions{
		Level:     level,
		AddSource: level == slog.LevelDebug,
	}
	if colored {
		opts.ReplaceAttr = func(_ []string, a slog.Attr) slog.Attr {
			if a.Key == slog.LevelKey {
				if lv, ok := a.Value.Any().(slog.Level); ok {
					a.Value = slog.StringValue(colorLevel(lv))
				}
			}
			return a
		}
	}
	var h slog.Handler
	switch cfg.LogFormat {
	case config.FormatJSON:
		h = slog.NewJSONHandler(w, opts)
	default:
		h = slog.NewTextHandler(w, opts)
	}

	logger := slog.New(h)
	slog.SetDefault(logger)
	return logger, nil
}

func colorLevel(l slog.Level) string {
	switch {
	case l >= slog.LevelError:
		return "\x1b[31m" + l.String() + "\x1b[0m"
	case l >= slog.LevelWarn:
		return "\x1b[33m" + l.String() + "\x1b[0m"
	case l >= slog.LevelInfo:
		return "\x1b[32m" + l.String() + "\x1b[0m"
	default:
		return "\x1b[36m" + l.String() + "\x1b[0m"
	}
}

func parseLevel(v string) (slog.Level, error) {
	switch v {
	case config.LevelDebug:
		return slog.LevelDebug, nil
	case config.LevelInfo:
		return slog.LevelInfo, nil
	case config.LevelWarn:
		return slog.LevelWarn, nil
	case config.LevelError:
		return slog.LevelError, nil
	default:
		return 0, fmt.Errorf("未知的日志级别 %q", v)
	}
}
