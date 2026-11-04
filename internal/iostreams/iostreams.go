package iostreams

import (
	"io"
	"os"

	"golang.org/x/term"
)

type Streams struct {
	In  io.Reader
	Out io.Writer
	Err io.Writer

	OutTTY bool
	ErrTTY bool
}

func System() *Streams {
	return &Streams{
		In:     os.Stdin,
		Out:    os.Stdout,
		Err:    os.Stderr,
		OutTTY: isTTY(os.Stdout),
		ErrTTY: isTTY(os.Stderr),
	}
}

func isTTY(f *os.File) bool {
	return term.IsTerminal(int(f.Fd()))
}
