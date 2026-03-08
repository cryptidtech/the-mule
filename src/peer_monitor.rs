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

/// Runs a monitor loop for a single peer.
/// Polls <name>_status via GET every 200ms. Drains <name>_log via BLPOP with a short timeout.
/// If `event_tx` is `Some`, broadcasts `PeerEvent`s for console mode.
pub async fn monitor_peer(
    peer_name: String,
    mut redis_conn: redis::aio::MultiplexedConnection,
    state: Arc<Mutex<PeerState>>,
    cancel: CancellationToken,
    event_tx: Option<tokio::sync::broadcast::Sender<PeerEvent>>,
) {
    let status_key = format!("{peer_name}_status");
    let log_key = format!("{peer_name}_log");

    let mut last_status: Option<String> = None;
    let mut poll_interval = tokio::time::interval(std::time::Duration::from_millis(200));

    loop {
        if cancel.is_cancelled() {
            break;
        }

        tokio::select! {
            _ = cancel.cancelled() => break,

            // Branch 1: poll status key via GET on interval
            _ = poll_interval.tick() => {
                let result: redis::RedisResult<Option<String>> =
                    redis::AsyncCommands::get(&mut redis_conn, &status_key).await;
                if let Ok(Some(raw_status)) = result {
                    // Only process if the status value has changed
                    if last_status.as_deref() != Some(&raw_status) {
                        last_status = Some(raw_status.clone());

                        let mut state = state.lock().await;
                        let parts: Vec<&str> = raw_status.splitn(3, '|').collect();
                        let status = parts[0].to_string();
                        state.statuses.insert(peer_name.clone(), status.clone());

                        if parts.len() == 3 && parts[0] == "started" {
                            state.peer_info.insert(
                                peer_name.clone(),
                                (parts[1].to_string(), parts[2].to_string()),
                            );
                        }

                        if let Some(ref tx) = event_tx {
                            let _ = tx.send(PeerEvent::StatusChange {
                                peer: peer_name.clone(),
                                status,
                            });
                        }
                    }
                }
            }

            // Branch 2: drain log entries via BLPOP with 1s timeout
            result = blpop(&mut redis_conn, &log_key, 1) => {
                if let Ok(Some((_key, log_entry))) = result {
                    if let Some((level, message)) = log_entry.split_once('|') {
                        match level {
                            "error" => tracing::error!(peer = %peer_name, "{message}"),
                            "warn"  => tracing::warn!(peer = %peer_name, "{message}"),
                            "info"  => tracing::info!(peer = %peer_name, "{message}"),
                            "debug" => tracing::debug!(peer = %peer_name, "{message}"),
                            _       => tracing::trace!(peer = %peer_name, "{message}"),
                        }

                        let mut state = state.lock().await;
                        state.logs.push((
                            peer_name.clone(),
                            level.to_string(),
                            message.to_string(),
                        ));

                        if let Some(ref tx) = event_tx {
                            let _ = tx.send(PeerEvent::LogEntry {
                                peer: peer_name.clone(),
                                level: level.to_string(),
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
