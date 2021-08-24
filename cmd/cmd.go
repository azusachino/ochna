package cmd

import (
	"github.com/AzusaChino/ochna/lib/random"
	"log"
	"strings"
)

func Run() {
	if err := random.Seed(); err != nil {
		log.Fatalf("Fatal error: %v", err)
	}
	if err := Root.Execute(); err != nil {
		if strings.HasPrefix(err.Error(), "unknown command") {
			Root.PrintErrf("You could use '%s selfupdate' to get latest features. \n\n", Root.CommandPath())
		}
		log.Fatalf("Fatal error: %v", err)
	}
}
