# Write a Go Test App

This guide walks through building a peer application in Go that runs under
The Mule test orchestrator.

## Prerequisites

- Go 1.22+
- Docker (for building the container image)
- A working `tm` setup (see [QUICKSTART.md](../QUICKSTART.md))

## 1. Set up your module

```bash
mkdir my-peer && cd my-peer
go mod init github.com/myorg/my-peer
go get github.com/cryptidtech/the-mule/lib/go
```

Or if using a local copy, add a `replace` directive in `go.mod`:

```
replace github.com/cryptidtech/the-mule/lib/go => ../the-mule/lib/go
```

## 2. Write the peer application

The client library handles Redis connection, log forwarding via `slog`, and
command delivery through a Go channel. Your code needs to:

1. Build the client
2. Send a `"started"` status (with optional VLAD and multiaddr)
3. Range over the commands channel
4. Send a `"stopped"` status before exiting

```go
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

	// Tell the orchestrator we are ready.
	// Use "started|<vlad>|<multiaddr>" if your app has identity/network info.
	if err := client.SendStatus(ctx, "started"); err != nil {
		slog.Error("failed to send started status", "error", err)
		os.Exit(1)
	}

	// Process commands from the orchestrator.
	// client.Commands() returns a <-chan string that yields commands.
	for cmd := range client.Commands() {
		slog.Info("received command", "command", cmd)

		switch {
		case cmd == "connect":
			slog.Info("connecting to network...")
			// ... your connect logic ...
			_ = client.SendStatus(ctx, "connected")

		case cmd == "disconnect":
			slog.Info("disconnecting...")
			// ... your disconnect logic ...
			_ = client.SendStatus(ctx, "disconnected")

		case strings.HasPrefix(cmd, "push|"):
			parts := strings.SplitN(cmd, "|", 3)
			peer, message := parts[1], parts[2]
			slog.Info("push", "peer", peer, "message", message)
			// ... your push logic ...

		case cmd == "pull":
			slog.Info("pulling messages...")
			// ... your pull logic ...

		case cmd == "rotate-key":
			slog.Info("rotating key...")
			// ... your key rotation logic ...

		case strings.HasPrefix(cmd, "track|"):
			peer := strings.SplitN(cmd, "|", 2)[1]
			slog.Info("tracking", "peer", peer)
			// ... your tracking logic ...

		case strings.HasPrefix(cmd, "peer|"):
			parts := strings.SplitN(cmd, "|", 3)
			vlad, multiaddr := parts[1], parts[2]
			slog.Info("adding bootstrap peer", "vlad", vlad, "multiaddr", multiaddr)
			// ... add peer to your routing table ...

		case strings.HasPrefix(cmd, "restart|"):
			delay := strings.SplitN(cmd, "|", 2)[1]
			_ = client.SendStatus(ctx, "restarting")
			_ = os.WriteFile("/tmp/delay", []byte(delay), 0644)
			os.Exit(42)

		case cmd == "shutdown":
			_ = client.SendStatus(ctx, "stopped")
			return
		}
	}
}
```

### How commands are received

The client library spawns a goroutine that calls `BLPOP {peer}_command 0` in a
loop — this blocks on the Redis server until a command is available, then sends
the raw string to a buffered Go channel. Your code reads commands via
`range client.Commands()`.

The `BLPOP` uses timeout 0 (truly blocking), so there is no polling overhead.
The goroutine is managed by a `context.Context` — when the context is cancelled
(or `client.Close()` is called), the goroutine exits and the channel is closed.
Go's scheduler ensures that blocking the goroutine does not block your other
goroutines.

### How logs are forwarded

The client installs a `slog.Handler` that captures all log records and forwards
them to Redis via `LPUSH {peer}_log "level|message"`. Use `slog.Info()`,
`slog.Error()`, etc. as normal and they appear in the orchestrator's log output
and TUI automatically.

## 3. Create a Dockerfile

```dockerfile
FROM golang:1.22 AS builder
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 go build -o /my-peer .

FROM alpine:3.19
RUN apk add --no-cache bash
COPY --from=builder /my-peer /usr/local/bin/my-peer
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
```

### entrypoint.sh

The entrypoint script supports the restart protocol (exit code 42):

```bash
#!/bin/bash
set -e
while true; do
    /usr/local/bin/my-peer "$@" && break
    EXIT_CODE=$?
    if [ "$EXIT_CODE" -eq 42 ] && [ -f /tmp/delay ]; then
        DELAY=$(cat /tmp/delay)
        rm -f /tmp/delay
        echo "restarting in ${DELAY}s..."
        sleep "$DELAY"
    else
        exit $EXIT_CODE
    fi
done
```

## 4. Build the image

```bash
docker build -t my-go-peer:latest .
```

## 5. Write a test config

```yaml
# my-test.yaml
test_name: "my-go-peer-test"

redis:
  port: 6399
  image: "redis:7-alpine"

hosts:
  - address: localhost
    ssh_user: user
    ssh_auth: agent
    base_port: 11984

peers:
  - name: alice
    image: "my-go-peer:latest"
    bootstrap: [bob]
  - name: bob
    image: "my-go-peer:latest"
    bootstrap: [alice]

commands:
  - { time: 0,  peer: alice, command: "connect" }
  - { time: 0,  peer: bob,   command: "connect" }
  - { time: 10, peer: alice, command: "push|bob|hello" }
  - { time: 15, peer: bob,   command: "pull" }

timeout:
  startup: 60
  shutdown: 30
```

## 6. Run the test

```bash
RUST_LOG=info tm my-test.yaml --verbose
```

## Environment variables

The orchestrator sets these environment variables on your container:

| Variable | Description |
|----------|-------------|
| `REDIS_URL` | Redis connection URL (e.g., `redis://192.168.1.10:6399`) |
| `PEER_NAME` | This peer's name (e.g., `alice`) |
| `LISTEN_ADDR` | Multiaddr to listen on (e.g., `/ip4/0.0.0.0/udp/11984/quic-v1`) |

## API reference

| Item | Description |
|------|-------------|
| `NewBuilder()` | Create builder, reads `REDIS_URL` and `PEER_NAME` from env |
| `.RedisURL(url)` / `.PeerName(name)` | Override env var values |
| `.Build(ctx)` | Connect to Redis, install `slog` handler, start command goroutine |
| `client.SendStatus(ctx, status)` | SET the status key (triggers orchestrator notification) |
| `client.Commands()` | Returns `<-chan string` yielding raw command strings via `BLPOP` |
| `client.Close()` | Cancel command goroutine and close Redis connection |
