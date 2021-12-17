package cmd

import (
	"fmt"
	"github.com/urfave/cli"
)

var HelpCommand = cli.Command{
	Name:        "help",
	Usage:       "print out ochna help",
	ArgsUsage:   "<ref>",
	Description: "print out ochna help",
	Action: func(context *cli.Context) {
		fmt.Println("ochna helps you do the work")
	},
}
