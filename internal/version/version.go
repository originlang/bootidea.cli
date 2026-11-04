package version

import (
	"fmt"
	"runtime"
)

var (
	Version = "dev"
	Commit  = "none"
	Date    = "unknown"
)

func String() string {
	return fmt.Sprintf("%s (commit %s, built %s, %s/%s)",
		Version, Commit, Date, runtime.GOOS, runtime.GOARCH)
}

func Details() string {
	return fmt.Sprintf("bootidea %s\n  commit:   %s\n  built:    %s\n  platform: %s/%s",
		Version, Commit, Date, runtime.GOOS, runtime.GOARCH)
}
