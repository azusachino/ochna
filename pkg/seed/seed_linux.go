package seed

import "golang.org/x/sys/unix"

func tryReadRandom(p []byte) {
	// Ignore errors, just decreases uniqueness of seed
	unix.Getrandom(p, unix.GRND_NONBLOCK)
}
