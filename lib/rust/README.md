# The Mule — Rust Client Library

Rust client library for peer applications running under The Mule test orchestrator.

## Usage

```rust
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() {
    let mut client = the_mule::MuleClientBuilder::new()
        .build()
        .await
        .expect("failed to build client");

    client.send_status("started").await.unwrap();

    loop {
        let cmd = {
            use std::pin::Pin;
            let mut pinned = Pin::new(&mut client);
            pinned.next().await
        };
        match cmd {
            Some(the_mule::Command::Shutdown) => {
                client.send_status("stopped").await.ok();
                break;
            }
            Some(cmd) => tracing::info!("received: {:?}", cmd),
            None => break,
        }
    }
}
```

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `REDIS_URL` | yes | Redis connection URL |
| `PEER_NAME` | yes | This peer's name |
| `RUST_LOG` | no | tracing filter (e.g., `info`) |

## How it works

- **Commands**: a background tokio task calls `BLPOP {peer}_command 0` (truly
  blocking, no polling) and sends parsed `Command` values through an `mpsc`
  channel. Your code consumes them via the `Stream` trait.
- **Logs**: a `tracing` subscriber layer captures log events and forwards them
  to Redis via `LPUSH {peer}_log "level|message"` in a background task.
- **Status**: `send_status()` calls `SET {peer}_status <value>`, which triggers
  a keyspace notification on the orchestrator side.

## API

- `MuleClientBuilder::new()` — reads env vars
- `.redis_url(url)` / `.peer_name(name)` — override
- `.build().await` — connect, install tracing, start background tasks
- `MuleClient::send_status(status)` — push status to orchestrator
- `impl Stream<Item = Command>` — yields parsed commands
- `Command` enum: `Connect`, `Disconnect`, `Shutdown`, `Restart`, `Push`, `Pull`, `RotateKey`, `Track`, `Peer`, `Unknown`
