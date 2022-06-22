package cmd

import (
	"fmt"

	"github.com/urfave/cli/v2"
)

var HelpCommand = &cli.Command{
	Name:        "help",
	Usage:       "print out ochna help",
	ArgsUsage:   "<ref>",
	Description: "print out ochna help",
	Action: func(context *cli.Context) error {
		fmt.Println("ochna helps you do the work")
		return nil
	},
}
