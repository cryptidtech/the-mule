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
tm <config.yaml>          # console mode (default)
tm <config.yaml> --tui    # ratatui TUI mode
tm <config.yaml> --verbose  # console mode with tracing output on stderr
```

### Config file

Tests are defined in YAML. See [docs/test-schema.md](docs/test-schema.md) for
the full schema reference and [examples/smoke-test-5peer.yaml](examples/smoke-test-5peer.yaml)
for a working example.

### How it works

1. Starts a local Redis container for peer coordination
2. Connects to remote hosts via SSH
3. Distributes the Docker test image to all hosts (skips if already present)
4. Starts peer containers with environment variables for Redis, listen address, etc.
5. Monitors peer status and logs via Redis pub/sub
6. Executes a timed command sequence (connect, push, pull, rotate-key, etc.)
7. Shuts down all peers and cleans up containers

### Signals

- `SIGINT` (Ctrl+C) and `SIGTERM` trigger graceful shutdown — peers are told to
  stop and the tool waits for them to report "stopped" before cleaning up.

## License

Apache 2.0 — see [LICENSE](LICENSE).
