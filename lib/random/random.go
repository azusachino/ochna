package random

import (
	cryptoRand "crypto/rand"
	"encoding/binary"
	"github.com/pkg/errors"
	mathRand "math/rand"
)

func Seed() error {
	var seed int64

	err := binary.Read(cryptoRand.Reader, binary.LittleEndian, &seed)
	if err != nil {
		return errors.Wrap(err, "failed to read random seed")
	}
	mathRand.Seed(seed)
	return nil
}
