package main

import (
	"fmt"
	"github.com/AzusaChino/ochna/cmd"
	"github.com/AzusaChino/ochna/pkg/seed"
	"github.com/sirupsen/logrus"
	"github.com/urfave/cli"
	"os"
)

func init() {
	seed.WithTimeAndRand()
}

func New() *cli.App {
	app := cli.NewApp()
	app.Name = "ochna"
	app.Description = `
ochna helps you do whatever you want(joke)`
	app.EnableBashCompletion = false
	app.Flags = []cli.Flag{}

	app.Commands = []cli.Command{
		cmd.HelpCommand,
		cmd.ProgressCommand,
	}

	app.Before = func(context *cli.Context) error {
		if context.GlobalBool("debug") {
			logrus.SetLevel(logrus.DebugLevel)
		}
		return nil
	}
	return app
}

func main() {
	app := New()
	if err := app.Run(os.Args); err != nil {
		_, _ = fmt.Fprintf(os.Stderr, "ochna: %s\n", err)
		os.Exit(1)
	}
}
