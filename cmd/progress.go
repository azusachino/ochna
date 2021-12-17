package cmd

import (
	"errors"
	"github.com/sirupsen/logrus"
	"github.com/urfave/cli"
)

var ProgressCommand = cli.Command{
	Name:  "progress",
	Usage: "print out progress",
	Action: func(ctx *cli.Context) error {
		var (
			id   = ctx.Args().First()
			args = ctx.Args().Tail()
		)

		if id == "" {
			return errors.New("failed to get id")
		}
		logrus.Printf("current id is %s\n", id)

		logrus.Println(args)
		return nil
	},
}
