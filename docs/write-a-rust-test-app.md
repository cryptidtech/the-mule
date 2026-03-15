# Write a Rust Test App

This guide walks through building a peer application in Rust that runs under
The Mule test orchestrator.

## Prerequisites

- Rust toolchain (stable)
- Docker (for building the container image)
- A working `tm` setup (see [QUICKSTART.md](../QUICKSTART.md))

## 1. Create a new crate

```bash
cargo new my-peer --name my-peer
cd my-peer
```

Add the `the_mule` client library as a dependency. It lives in the `lib/rust/`
directory of The Mule repository, so use a path or git dependency:

```toml
# Cargo.toml
[dependencies]
the_mule = { path = "../the-mule/lib/rust" }
tokio = { version = "1", features = ["full"] }
tokio-stream = "0.1"
tracing = "0.1"
```

## 2. Write the peer binary

The client library handles Redis connection, command parsing, log forwarding,
and tracing setup. Your code just needs to:

1. Build the client
2. Send a `"started"` status (with optional VLAD and multiaddr)
3. Loop over incoming commands
4. Send a `"stopped"` status before exiting

```rust
// src/main.rs
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() {
    let mut client = the_mule::MuleClientBuilder::new()
        .build()
        .await
        .expect("failed to build mule client");

    // Tell the orchestrator we are ready.
    // Use "started|<vlad>|<multiaddr>" if your app has identity/network info.
    client
        .send_status("started")
        .await
        .expect("failed to send started status");

    // Process commands from the orchestrator.
    // The client implements futures::Stream<Item = Command>.
    loop {
        let cmd = {
            use std::pin::Pin;
            let mut pinned = Pin::new(&mut client);
            pinned.next().await
        };
        match cmd {
            Some(the_mule::Command::Connect) => {
                tracing::info!("connecting to network...");
                // ... your connect logic ...
                client.send_status("connected").await.ok();
            }
            Some(the_mule::Command::Disconnect) => {
                tracing::info!("disconnecting...");
                // ... your disconnect logic ...
                client.send_status("disconnected").await.ok();
            }
            Some(the_mule::Command::Push { peer, message }) => {
                tracing::info!("push to {peer}: {message}");
                // ... your push logic ...
            }
            Some(the_mule::Command::Pull) => {
                tracing::info!("pulling messages...");
                // ... your pull logic ...
            }
            Some(the_mule::Command::RotateKey) => {
                tracing::info!("rotating key...");
                // ... your key rotation logic ...
            }
            Some(the_mule::Command::Track { peer }) => {
                tracing::info!("tracking {peer}");
                // ... your tracking logic ...
            }
            Some(the_mule::Command::Peer { vlad, multiaddr }) => {
                tracing::info!("adding bootstrap peer: {vlad} at {multiaddr}");
                // ... add peer to your DHT routing table ...
            }
            Some(the_mule::Command::Restart { delay_secs }) => {
                client.send_status("restarting").await.ok();
                let _ = std::fs::write("/tmp/delay", delay_secs.to_string());
                std::process::exit(42);
            }
            Some(the_mule::Command::Shutdown) => {
                client.send_status("stopped").await.ok();
                break;
            }
            Some(the_mule::Command::Unknown(raw)) => {
                tracing::warn!("unknown command: {raw}");
            }
            None => break, // command stream closed
        }
    }
}
```

### How commands are received

The client library spawns a background tokio task that calls
`BLPOP {peer}_command 0` in a loop — this blocks on the Redis server until a
command is available, then parses it into the `Command` enum and sends it
through an `mpsc` channel. Your code consumes commands via the `Stream` trait.

The `BLPOP` uses timeout 0 (truly blocking), so there is no polling overhead.
Because it runs in a separate tokio task, it does not block your application's
other async work.

### How logs are forwarded

The client installs a `tracing` subscriber layer that captures all log events
and forwards them to Redis via `LPUSH {peer}_log "level|message"`. You use
`tracing::info!()`, `tracing::error!()`, etc. as normal and they appear in the
orchestrator's log output and TUI automatically.

## 3. Create a Dockerfile

```dockerfile
FROM rust:1.82-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release --bin my-peer

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/my-peer /usr/local/bin/my-peer
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
docker build -t my-peer:latest .
```

## 5. Write a test config

```yaml
# my-test.yaml
name: "my-peer-test"

timeout:
  startup: 60
  shutdown: 30

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
    image: "my-peer:latest"
    bootstrap: [bob]
  - name: bob
    image: "my-peer:latest"
    bootstrap: [alice]

commands:
  - { time: 0,  peer: alice, command: "connect" }
  - { time: 0,  peer: bob,   command: "connect" }
  - { time: 10, peer: alice, command: "push|bob|hello" }
  - { time: 15, peer: bob,   command: "pull" }
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
| `RUST_LOG` | Tracing filter level (from peer config `environment`) |

## API reference

| Item | Description |
|------|-------------|
| `MuleClientBuilder::new()` | Create builder, reads `REDIS_URL` and `PEER_NAME` from env |
| `.redis_url(url)` / `.peer_name(name)` | Override env var values |
| `.build().await` | Connect to Redis, install tracing, start background tasks |
| `client.send_status(status)` | SET the status key (triggers orchestrator notification) |
| `Stream<Item = Command>` | Yields parsed commands from the orchestrator |
| `Command` enum | `Connect`, `Disconnect`, `Shutdown`, `Restart { delay_secs }`, `Push { peer, message }`, `Pull`, `RotateKey`, `Track { peer }`, `Peer { vlad, multiaddr }`, `Unknown(String)` |
