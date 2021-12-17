//go:build !linux
// +build !linux

package seed

import (
	"crypto/rand"
	"io"
)

func tryReadRandom(p []byte) {
	_, _ = io.ReadFull(rand.Reader, p)
}
