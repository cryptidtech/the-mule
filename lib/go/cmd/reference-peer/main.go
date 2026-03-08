package main

import (
	"context"
	"log/slog"
	"os"
	"strings"

	the_mule "github.com/cryptidtech/the-mule/lib/go"
)

func main() {
	ctx := context.Background()

	client, err := the_mule.NewBuilder().Build(ctx)
	if err != nil {
		slog.Error("failed to build mule client", "error", err)
		os.Exit(1)
	}
	defer client.Close()

	if err := client.SendStatus(ctx, "started"); err != nil {
		slog.Error("failed to send started status", "error", err)
		os.Exit(1)
	}

	for cmd := range client.Commands() {
		slog.Info("received command", "command", cmd)

		if cmd == "shutdown" {
			_ = client.SendStatus(ctx, "stopped")
			break
		}

		if strings.HasPrefix(cmd, "restart|") {
			_ = client.SendStatus(ctx, "restarting")
			delay := strings.SplitN(cmd, "|", 2)[1]
			_ = os.WriteFile("/tmp/delay", []byte(delay), 0644)
			os.Exit(42)
		}
	}
}
