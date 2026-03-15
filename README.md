![The Mule](the_mule.jpeg)

# The Mule (`tm`)

A standalone test orchestration tool for running distributed peer integration
tests across multiple hosts. Manages SSH connections, Docker image distribution,
container lifecycle, Redis coordination, and provides both TUI and console
interfaces for monitoring test execution.

## Build

```sh
cargo build --release
```

The binary is `target/release/tm`.

## Usage

```sh
tm <config.yaml>                    # console mode (default)
tm <config.yaml> --tui              # ratatui TUI mode
tm <config.yaml> --verbose          # console mode with tracing output on stderr
tm <config.yaml> --redis-url <url>  # use an external Redis instance
```

### Config file

Tests are defined in YAML. See [docs/test-schema.md](docs/test-schema.md) for
the full schema reference and [examples/smoke-test-5peer.yaml](examples/smoke-test-5peer.yaml)
for a working example.

### How it works

1. Starts a local Redis container for peer coordination (or connects to an
   external instance via `--redis-url`)
2. Enables Redis keyspace notifications (`CONFIG SET notify-keyspace-events K$`)
   so the orchestrator reacts instantly to peer status changes via pub/sub
3. Connects to remote hosts via SSH
4. Distributes Docker test images to all hosts (skips if already present)
5. Starts peer containers with environment variables for Redis, listen address, etc.
6. Monitors each peer via two channels:
   - **Status**: subscribes to `__keyspace@0__:{peer}_status` for instant
     notification when a peer SETs its status key (no polling)
   - **Logs**: drains `{peer}_log` via `BLPOP` with timeout 0 (truly blocking)
7. Executes a timed command sequence (connect, push, pull, rotate-key, etc.)
8. Shuts down all peers and cleans up containers

### Architecture

Each peer monitor task creates three dedicated Redis connections:

| Connection | Purpose |
|------------|---------|
| **PubSub** | Subscribes to `__keyspace@0__:{peer}_status` keyspace notifications |
| **GET** | Fetches the status value when a notification fires |
| **BLPOP** | Drains `{peer}_log` entries (blocking, timeout 0) |

The GET and BLPOP connections are separate `MultiplexedConnection` instances
(separate TCP sockets) so that a blocking BLPOP cannot starve status GET
requests.

### Signals

- `SIGINT` (Ctrl+C) and `SIGTERM` trigger graceful shutdown — peers are told to
  stop and the tool waits for them to report "stopped" before cleaning up.

### Client libraries

Peer applications communicate with the orchestrator using one of the provided
client libraries:

- **[Rust](lib/rust/README.md)** — `the_mule` crate with async Stream-based API
- **[Python](lib/python/README.md)** — `the_mule` package with async iterator API
- **[Go](lib/go/README.md)** — `the_mule` package with channel-based API

See the [client libraries README](lib/README.md) for the full Redis protocol
specification, and the per-language guides:

- [Write a Rust test app](docs/write-a-rust-test-app.md)
- [Write a Go test app](docs/write-a-go-test-app.md)
- [Write a Python test app](docs/write-a-python-test-app.md)

### Getting started

See [QUICKSTART.md](QUICKSTART.md) for a step-by-step guide to running your
first test on localhost.

## License

Apache 2.0 — see [LICENSE](LICENSE).
