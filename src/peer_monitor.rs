use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub enum PeerEvent {
    StatusChange { peer: String, status: String },
    LogEntry { peer: String, level: String, message: String },
}

/// Shared state between monitor tasks and the UI/orchestrator.
pub struct PeerState {
    /// peer_name -> current status string (e.g. "started", "connecting", "connected")
    pub statuses: BTreeMap<String, String>,
    /// peer_name -> (VLAD string, multiaddr string) — populated from "started|<VLAD>|<multiaddr>"
    pub peer_info: BTreeMap<String, (String, String)>,
    /// Aggregated log lines: (peer_name, level, message)
    pub logs: Vec<(String, String, String)>,
}

impl PeerState {
    pub fn new() -> Self {
        Self {
            statuses: BTreeMap::new(),
            peer_info: BTreeMap::new(),
            logs: Vec::new(),
        }
    }
}

/// BLPOP helper that avoids temporary lifetime issues in tokio::select!
async fn blpop(
    conn: &mut redis::aio::MultiplexedConnection,
    key: &str,
    timeout: u32,
) -> redis::RedisResult<Option<(String, String)>> {
    let mut cmd = redis::cmd("BLPOP");
    cmd.arg(key).arg(timeout);
    cmd.query_async(conn).await
}

/// Process a raw status string, updating shared state and broadcasting events.
async fn process_status(
    peer_name: &str,
    raw_status: &str,
    last_status: &mut Option<String>,
    state: &Arc<Mutex<PeerState>>,
    event_tx: &Option<tokio::sync::broadcast::Sender<PeerEvent>>,
) {
    if last_status.as_deref() == Some(raw_status) {
        return;
    }
    *last_status = Some(raw_status.to_string());
    let mut state = state.lock().await;
    let parts: Vec<&str> = raw_status.splitn(3, '|').collect();
    let status = parts[0].to_string();
    state.statuses.insert(peer_name.to_string(), status.clone());
    if parts.len() == 3 && parts[0] == "started" {
        state.peer_info.insert(
            peer_name.to_string(),
            (parts[1].to_string(), parts[2].to_string()),
        );
    }
    if let Some(ref tx) = event_tx {
        let _ = tx.send(PeerEvent::StatusChange {
            peer: peer_name.to_string(),
            status,
        });
    }
}

/// Runs a monitor loop for a single peer.
/// Subscribes to keyspace notifications for `<name>_status` and drains `<name>_log` via BLPOP.
/// If `event_tx` is `Some`, broadcasts `PeerEvent`s for console mode.
pub async fn monitor_peer(
    peer_name: String,
    redis_client: redis::Client,
    state: Arc<Mutex<PeerState>>,
    cancel: CancellationToken,
    event_tx: Option<tokio::sync::broadcast::Sender<PeerEvent>>,
) {
    let status_key = format!("{peer_name}_status");
    let log_key = format!("{peer_name}_log");
    let channel = format!("__keyspace@0__:{status_key}");

    // Create dedicated connections:
    // - pubsub: for keyspace notification subscription
    // - get_conn: for GET commands (triggered by notifications)
    // - blpop_conn: for BLPOP log draining (blocks its TCP socket, must be separate)
    let mut pubsub = redis_client
        .get_async_pubsub()
        .await
        .expect("failed to create PubSub connection");
    let mut get_conn = redis_client
        .get_multiplexed_async_connection()
        .await
        .expect("failed to create GET connection");
    let mut blpop_conn = redis_client
        .get_multiplexed_async_connection()
        .await
        .expect("failed to create BLPOP connection");

    // Subscribe FIRST (before initial GET) to avoid missing a SET
    pubsub
        .subscribe(&channel)
        .await
        .expect("failed to subscribe to keyspace notifications");

    // Initial GET to catch status set before subscription
    let mut last_status: Option<String> = None;
    let initial: redis::RedisResult<Option<String>> =
        redis::AsyncCommands::get(&mut get_conn, &status_key).await;
    if let Ok(Some(raw_status)) = initial {
        process_status(&peer_name, &raw_status, &mut last_status, &state, &event_tx).await;
    }

    // Use into_on_message() to get an owned Stream (avoids borrow issues in select!)
    let mut pubsub_stream = pubsub.into_on_message();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,

            // Branch 1: keyspace notification — the status key was SET
            msg = futures_util::StreamExt::next(&mut pubsub_stream) => {
                match msg {
                    Some(msg) => {
                        let payload: String = msg.get_payload().unwrap_or_default();
                        if payload == "set" {
                            let result: redis::RedisResult<Option<String>> =
                                redis::AsyncCommands::get(&mut get_conn, &status_key).await;
                            if let Ok(Some(raw_status)) = result {
                                process_status(&peer_name, &raw_status, &mut last_status, &state, &event_tx).await;
                            }
                        }
                    }
                    None => {
                        tracing::error!(peer = %peer_name, "keyspace subscription stream ended");
                        break;
                    }
                }
            }

            // Branch 2: drain log entries via BLPOP with timeout 0 (truly blocking)
            result = blpop(&mut blpop_conn, &log_key, 0) => {
                if let Ok(Some((_key, log_entry))) = result {
                    if let Some((level, message)) = log_entry.split_once('|') {
                        let parsed_level = match level.to_ascii_lowercase().as_str() {
                            "error" => tracing::Level::ERROR,
                            "warn"  => tracing::Level::WARN,
                            "info"  => tracing::Level::INFO,
                            "debug" => tracing::Level::DEBUG,
                            "trace" => tracing::Level::TRACE,
                            _       => tracing::Level::TRACE,
                        };
                        match parsed_level {
                            tracing::Level::ERROR => tracing::error!(peer = %peer_name, "{message}"),
                            tracing::Level::WARN  => tracing::warn!(peer = %peer_name, "{message}"),
                            tracing::Level::INFO  => tracing::info!(peer = %peer_name, "{message}"),
                            tracing::Level::DEBUG => tracing::debug!(peer = %peer_name, "{message}"),
                            tracing::Level::TRACE => tracing::trace!(peer = %peer_name, "{message}"),
                        }

                        let canonical_level = parsed_level.as_str().to_ascii_lowercase();
                        let mut state = state.lock().await;
                        state.logs.push((
                            peer_name.clone(),
                            canonical_level.clone(),
                            message.to_string(),
                        ));

                        if let Some(ref tx) = event_tx {
                            let _ = tx.send(PeerEvent::LogEntry {
                                peer: peer_name.clone(),
                                level: canonical_level,
                                message: message.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_state_new_is_empty() {
        let state = PeerState::new();
        assert!(state.statuses.is_empty());
        assert!(state.peer_info.is_empty());
        assert!(state.logs.is_empty());
    }

    #[test]
    fn peer_state_insert_and_retrieve() {
        let mut state = PeerState::new();
        state
            .statuses
            .insert("alice".to_string(), "started".to_string());
        assert_eq!(state.statuses.get("alice").unwrap(), "started");
        state.peer_info.insert(
            "alice".to_string(),
            ("vlad123".to_string(), "/ip4/1.2.3.4".to_string()),
        );
        let (vlad, addr) = state.peer_info.get("alice").unwrap();
        assert_eq!(vlad, "vlad123");
        assert_eq!(addr, "/ip4/1.2.3.4");
    }

    #[test]
    fn peer_event_clone() {
        let event = PeerEvent::StatusChange {
            peer: "bob".to_string(),
            status: "connected".to_string(),
        };
        let cloned = event.clone();
        if let PeerEvent::StatusChange { peer, status } = cloned {
            assert_eq!(peer, "bob");
            assert_eq!(status, "connected");
        } else {
            panic!("expected StatusChange");
        }
    }
}
