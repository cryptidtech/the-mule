package main

import (
	"context"
	"fmt"
	"log/slog"
	"net"
	"os"
	"strings"

	the_mule "github.com/cryptidtech/the-mule/lib/go"
)

func localIP() string {
	conn, err := net.Dial("udp", "8.8.8.8:80")
	if err != nil {
		return "0.0.0.0"
	}
	defer conn.Close()
	return conn.LocalAddr().(*net.UDPAddr).IP.String()
}

func extractPort(listenAddr string) string {
	parts := strings.Split(listenAddr, "/")
	for i, p := range parts {
		if p == "udp" && i+1 < len(parts) {
			return parts[i+1]
		}
	}
	return "0"
}

func main() {
	ctx := context.Background()

	client, err := the_mule.NewBuilder().Build(ctx)
	if err != nil {
		slog.Error("failed to build mule client", "error", err)
		os.Exit(1)
	}
	defer client.Close()

	ip := localIP()
	port := extractPort(os.Getenv("LISTEN_ADDR"))
	status := fmt.Sprintf("started|/ip4/%s/udp/%s/quic-v1", ip, port)
	if err := client.SendStatus(ctx, status); err != nil {
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
