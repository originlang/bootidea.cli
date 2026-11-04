package app

import (
	"context"
	"fmt"
	"log/slog"

	"bootidea.cli/internal/config"
	"bootidea.cli/internal/iostreams"
)

type App struct {
	Cfg *config.Config
	Log *slog.Logger
	IO  *iostreams.Streams
}

type contextKey struct{}

func Into(ctx context.Context, a *App) context.Context {
	return context.WithValue(ctx, contextKey{}, a)
}

func From(ctx context.Context) (*App, error) {
	a, ok := ctx.Value(contextKey{}).(*App)
	if !ok {
		return nil, fmt.Errorf("内部错误：运行时上下文未初始化")
	}
	return a, nil
}
