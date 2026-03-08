# The Mule — Go Client Library

Go client library for peer applications running under The Mule test orchestrator.

## Usage

```go
package main

import (
    "context"
    "log/slog"
    the_mule "github.com/cryptidtech/the-mule/lib/go"
)

func main() {
    ctx := context.Background()
    client, err := the_mule.NewBuilder().Build(ctx)
    if err != nil {
        panic(err)
    }
    defer client.Close()

    client.SendStatus(ctx, "started")

    for cmd := range client.Commands() {
        slog.Info("received", "command", cmd)
        if cmd == "shutdown" {
            client.SendStatus(ctx, "stopped")
            break
        }
    }
}
```

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `REDIS_URL` | yes | Redis connection URL |
| `PEER_NAME` | yes | This peer's name |

## How it works

- **Commands**: a goroutine calls `BLPOP {peer}_command 0` (truly blocking, no
  polling) and sends raw strings to a buffered Go channel. Context cancellation
  is respected — when the context is cancelled or `Close()` is called, the
  goroutine exits and the channel is closed. Go's M:N scheduler ensures the
  blocking goroutine does not block your other goroutines.
- **Logs**: a `slog.Handler` captures log records and forwards them to Redis
  via `LPUSH {peer}_log "level|message"`.
- **Status**: `SendStatus()` calls `SET {peer}_status <value>`, which triggers
  a keyspace notification on the orchestrator side.

## API

- `NewBuilder()` — reads env vars
- `.RedisURL(url)` / `.PeerName(name)` — override
- `.Build(ctx)` — connect, install slog handler, start command goroutine
- `MuleClient.SendStatus(ctx, status)` — push status to orchestrator
- `MuleClient.Commands()` — returns `<-chan string` yielding commands
- `MuleClient.Close()` — stop listener and close Redis
