package main

import (
	"fmt"
	"os"

	"github.com/azusachino/ochna/cmd"
	"github.com/azusachino/ochna/pkg/seed"
	"github.com/sirupsen/logrus"
	"github.com/urfave/cli/v2"
)

func init() {
	seed.WithTimeAndRand()
}

func New() *cli.App {
	app := cli.NewApp()
	app.Name = "ochna"
	app.Description = `ochna helps you to test lots of stuffs`
	app.EnableBashCompletion = false
	app.Flags = []cli.Flag{}

	app.Commands = []*cli.Command{
		cmd.HelpCommand,
		cmd.ProgressCommand,
	}

	app.Before = func(context *cli.Context) error {
		if context.Bool("debug") {
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
