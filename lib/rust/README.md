# The Mule — Rust Client Library

Rust client library for peer applications running under The Mule test orchestrator.

## Usage

```rust
use futures_core::Stream;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() {
    let mut client = the_mule::MuleClientBuilder::new()
        .build()
        .await
        .expect("failed to build client");

    client.send_status("started").await.unwrap();

    tokio::pin!(client);
    while let Some(cmd) = client.next().await {
        match cmd {
            the_mule::Command::Shutdown => break,
            _ => tracing::info!("received: {:?}", cmd),
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

## API

- `MuleClientBuilder::new()` — reads env vars
- `.redis_url(url)` / `.peer_name(name)` — override
- `.build().await` — connect, install tracing, start background tasks
- `MuleClient::send_status(status)` — push status to orchestrator
- `impl Stream<Item = Command>` — yields parsed commands
- `Command` enum: `Connect`, `Disconnect`, `Shutdown`, `Restart`, `Push`, `Pull`, `RotateKey`, `Track`, `Peer`, `Unknown`
