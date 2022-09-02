package cmd

import (
	"errors"
	 "github.com/azusachino/ribes/yandere"
	 
	"github.com/urfave/cli/v2"
	"github.com/sirupsen/logrus"
)

var YandereCommand = &cli.Command{
	Name:  "yandere",
	Usage: "print out progress",
	Action: func(ctx *cli.Context) error {
		var (
			id   = ctx.Args().First()
			args = ctx.Args().Tail()
		)

		if id == "" {
			return errors.New("failed to get id")
		}
		yandere.DownloadByShowId(id, ".")
		logrus.Printf("current id is %s\n", id)

		logrus.Println(args)
		return nil
	},
}
