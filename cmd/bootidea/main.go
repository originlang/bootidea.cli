package main

import (
	"context"
	"os"
	"os/signal"
	"syscall"

	"bootidea.cli/internal/cli"
	"bootidea.cli/internal/iostreams"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	os.Exit(cli.Execute(ctx, os.Args[1:], iostreams.System()))
}
