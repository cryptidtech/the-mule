use anyhow::{Context, Result};
use redis::AsyncCommands;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::config::TestConfig;
use crate::peer_monitor::PeerState;

/// Wait for all peers to report "started" with a timeout.
pub async fn wait_for_peers_started(
    state: &Arc<Mutex<PeerState>>,
    peer_names: &[String],
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;

    loop {
        if Instant::now() >= deadline {
            let state = state.lock().await;
            let missing: Vec<&String> = peer_names
                .iter()
                .filter(|name| {
                    state
                        .statuses
                        .get(*name)
                        .map(|s| s != "started")
                        .unwrap_or(true)
                })
                .collect();
            anyhow::bail!(
                "timeout waiting for peers to start. Missing: {:?}",
                missing
            );
        }

        {
            let state = state.lock().await;
            let all_started = peer_names.iter().all(|name| {
                state
                    .statuses
                    .get(name)
                    .map(|s| s == "started")
                    .unwrap_or(false)
            });
            if all_started {
                return Ok(());
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Wait for all peers to report "stopped" with a timeout.
pub async fn wait_for_peers_stopped(
    state: &Arc<Mutex<PeerState>>,
    peer_names: &[String],
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;

    loop {
        if Instant::now() >= deadline {
            let state = state.lock().await;
            let still_running: Vec<&String> = peer_names
                .iter()
                .filter(|n| {
                    state
                        .statuses
                        .get(*n)
                        .map(|s| s != "stopped")
                        .unwrap_or(true)
                })
                .collect();
            anyhow::bail!(
                "timeout waiting for peers to stop. Still running: {:?}",
                still_running
            );
        }

        {
            let state = state.lock().await;
            if peer_names
                .iter()
                .all(|n| state.statuses.get(n).map(|s| s == "stopped").unwrap_or(false))
            {
                return Ok(());
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Send bootstrap peer commands to each peer based on config.
pub async fn send_bootstrap_commands(
    config: &TestConfig,
    state: &Arc<Mutex<PeerState>>,
    redis_conn: &mut redis::aio::MultiplexedConnection,
) -> Result<()> {
    let state = state.lock().await;

    for peer in &config.peers {
        for bootstrap_name in &peer.bootstrap {
            if let Some((vlad, multiaddr)) = state.peer_info.get(bootstrap_name) {
                let cmd = format!("peer|{vlad}|{multiaddr}");
                let key = format!("{}_command", peer.name);
                redis_conn
                    .lpush::<_, _, ()>(&key, &cmd)
                    .await
                    .context(format!(
                        "failed to send bootstrap command to {}",
                        peer.name
                    ))?;
                tracing::info!(
                    "sent bootstrap: {} -> {} ({})",
                    peer.name,
                    bootstrap_name,
                    multiaddr
                );
            } else {
                tracing::warn!(
                    "bootstrap peer '{}' not found in peer_info for '{}'",
                    bootstrap_name,
                    peer.name
                );
            }
        }
    }

    Ok(())
}

/// Send shutdown to all peers and clean up.
pub async fn shutdown_all_peers(
    peer_names: &[String],
    redis_conn: &mut redis::aio::MultiplexedConnection,
) {
    for name in peer_names {
        let key = format!("{name}_command");
        let _ = redis_conn.lpush::<_, _, ()>(&key, "shutdown").await;
    }
    tracing::info!("sent shutdown to all peers");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_for_peers_started_immediate_success() {
        let state = Arc::new(Mutex::new(PeerState::new()));
        {
            let mut s = state.lock().await;
            s.statuses.insert("alice".to_string(), "started".to_string());
            s.statuses.insert("bob".to_string(), "started".to_string());
        }
        let names = vec!["alice".to_string(), "bob".to_string()];
        let result =
            wait_for_peers_started(&state, &names, Duration::from_secs(1)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn wait_for_peers_started_timeout_with_missing() {
        let state = Arc::new(Mutex::new(PeerState::new()));
        {
            let mut s = state.lock().await;
            s.statuses.insert("alice".to_string(), "started".to_string());
            // bob is missing
        }
        let names = vec!["alice".to_string(), "bob".to_string()];
        let result =
            wait_for_peers_started(&state, &names, Duration::from_millis(100)).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("bob"));
    }
}
