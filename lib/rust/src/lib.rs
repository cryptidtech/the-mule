use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use tokio::sync::mpsc;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Errors returned by the Mule client.
#[derive(Debug)]
pub enum MuleError {
    MissingConfig(String),
    Redis(redis::RedisError),
}

impl std::fmt::Display for MuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MuleError::MissingConfig(msg) => write!(f, "missing config: {msg}"),
            MuleError::Redis(e) => write!(f, "redis error: {e}"),
        }
    }
}

impl std::error::Error for MuleError {}

impl From<redis::RedisError> for MuleError {
    fn from(e: redis::RedisError) -> Self {
        MuleError::Redis(e)
    }
}

/// A parsed command from the orchestrator.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Connect,
    Disconnect,
    Shutdown,
    Restart { delay_secs: u64 },
    Peer { multiaddr: String },
    Test(String),
}

impl Command {
    /// Parse a raw pipe-delimited command string from Redis.
    pub fn parse(raw: &str) -> Self {
        let parts: Vec<&str> = raw.splitn(2, '|').collect();
        match parts[0] {
            "connect" => Command::Connect,
            "disconnect" => Command::Disconnect,
            "shutdown" => Command::Shutdown,
            "restart" if parts.len() >= 2 => match parts[1].parse::<u64>() {
                Ok(delay) => Command::Restart { delay_secs: delay },
                Err(_) => Command::Test(raw.to_string()),
            },
            "peer" if parts.len() >= 2 => Command::Peer {
                multiaddr: parts[1].to_string(),
            },
            _ => Command::Test(raw.to_string()),
        }
    }
}

/// Builder for constructing a `MuleClient`.
pub struct MuleClientBuilder {
    redis_url: Option<String>,
    peer_name: Option<String>,
}

impl MuleClientBuilder {
    /// Create a new builder, reading defaults from environment variables:
    /// `REDIS_URL`, `PEER_NAME`, `RUST_LOG`.
    pub fn new() -> Self {
        Self {
            redis_url: std::env::var("REDIS_URL").ok(),
            peer_name: std::env::var("PEER_NAME").ok(),
        }
    }

    pub fn redis_url(mut self, url: &str) -> Self {
        self.redis_url = Some(url.to_string());
        self
    }

    pub fn peer_name(mut self, name: &str) -> Self {
        self.peer_name = Some(name.to_string());
        self
    }

    /// Build the client, connecting to Redis and installing the tracing subscriber.
    pub async fn build(self) -> Result<MuleClient, MuleError> {
        let redis_url = self
            .redis_url
            .ok_or_else(|| MuleError::MissingConfig("REDIS_URL not set".into()))?;
        let peer_name = self
            .peer_name
            .ok_or_else(|| MuleError::MissingConfig("PEER_NAME not set".into()))?;

        let client = redis::Client::open(redis_url.as_str()).map_err(MuleError::Redis)?;
        let conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(MuleError::Redis)?;

        let status_key = format!("{peer_name}_status");
        let command_key = format!("{peer_name}_command");
        let log_key = format!("{peer_name}_log");

        // Set up log forwarding via tracing
        let (log_tx, mut log_rx) = mpsc::channel::<String>(1024);
        let log_layer = RedisLogLayer {
            sender: log_tx,
            peer_name: peer_name.clone(),
        };

        let env_filter = tracing_subscriber::EnvFilter::from_default_env();
        tracing_subscriber::registry()
            .with(log_layer)
            .with(env_filter)
            .init();

        // Spawn log flusher
        let mut log_conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(MuleError::Redis)?;
        let log_key_clone = log_key.clone();
        tokio::spawn(async move {
            while let Some(entry) = log_rx.recv().await {
                let _: Result<(), _> =
                    redis::AsyncCommands::lpush(&mut log_conn, &log_key_clone, &entry).await;
            }
        });

        // Spawn command poller
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(256);
        let mut cmd_conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(MuleError::Redis)?;
        let cmd_key = command_key.clone();
        tokio::spawn(async move {
            loop {
                let result: redis::RedisResult<Option<(String, String)>> = {
                    let mut cmd = redis::cmd("BLPOP");
                    cmd.arg(&cmd_key).arg(0u32);
                    cmd.query_async(&mut cmd_conn).await
                };
                match result {
                    Ok(Some((_key, raw))) => {
                        let command = Command::parse(&raw);
                        if cmd_tx.send(command).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        });

        Ok(MuleClient {
            conn,
            status_key,
            cmd_rx,
        })
    }
}

impl Default for MuleClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Client for communicating with The Mule orchestrator via Redis.
#[derive(Debug)]
pub struct MuleClient {
    conn: redis::aio::MultiplexedConnection,
    status_key: String,
    cmd_rx: mpsc::Receiver<Command>,
}

impl MuleClient {
    /// Send a status update to the orchestrator.
    pub async fn send_status(&mut self, status: &str) -> Result<(), MuleError> {
        redis::AsyncCommands::set::<_, _, ()>(&mut self.conn, &self.status_key, status)
            .await
            .map_err(MuleError::Redis)
    }
}

impl Stream for MuleClient {
    type Item = Command;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.cmd_rx.poll_recv(cx)
    }
}

/// A tracing layer that forwards log events to Redis.
struct RedisLogLayer {
    sender: mpsc::Sender<String>,
    peer_name: String,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for RedisLogLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => "error",
            tracing::Level::WARN => "warn",
            tracing::Level::INFO => "info",
            tracing::Level::DEBUG => "debug",
            tracing::Level::TRACE => "debug",
        };

        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let message = if visitor.0.is_empty() {
            event.metadata().name().to_string()
        } else {
            visitor.0
        };

        let entry = format!("{level}|[{}] {message}", self.peer_name);
        let _ = self.sender.try_send(entry);
    }
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connect() {
        assert_eq!(Command::parse("connect"), Command::Connect);
    }

    #[test]
    fn parse_disconnect() {
        assert_eq!(Command::parse("disconnect"), Command::Disconnect);
    }

    #[test]
    fn parse_shutdown() {
        assert_eq!(Command::parse("shutdown"), Command::Shutdown);
    }

    #[test]
    fn parse_restart() {
        assert_eq!(
            Command::parse("restart|5"),
            Command::Restart { delay_secs: 5 }
        );
    }

    #[test]
    fn parse_restart_invalid_delay() {
        assert_eq!(
            Command::parse("restart|abc"),
            Command::Test("restart|abc".to_string())
        );
    }

    #[test]
    fn parse_peer() {
        assert_eq!(
            Command::parse("peer|/ip4/1.2.3.4/udp/10000/quic-v1"),
            Command::Peer {
                multiaddr: "/ip4/1.2.3.4/udp/10000/quic-v1".to_string(),
            }
        );
    }

    #[test]
    fn parse_test() {
        assert_eq!(
            Command::parse("foobar"),
            Command::Test("foobar".to_string())
        );
    }

    #[test]
    fn builder_missing_redis_url() {
        let builder = MuleClientBuilder {
            redis_url: None,
            peer_name: Some("test".into()),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(builder.build());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("REDIS_URL"));
    }

    #[test]
    fn builder_missing_peer_name() {
        let builder = MuleClientBuilder {
            redis_url: Some("redis://localhost:6379".into()),
            peer_name: None,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(builder.build());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("PEER_NAME"));
    }
}
