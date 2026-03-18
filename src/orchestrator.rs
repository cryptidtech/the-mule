use anyhow::{Context, Result};
use redis::AsyncCommands;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::config::{PeerName, TestConfig};
use crate::peer_monitor::PeerState;

/// Wait for all peers to report "started" with a timeout.
pub async fn wait_for_peers_started(
    state: &Arc<Mutex<PeerState>>,
    peer_names: &[PeerName],
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;

    loop {
        if Instant::now() >= deadline {
            let state = state.lock().await;
            let missing: Vec<String> = peer_names
                .iter()
                .filter(|name| {
                    !state
                        .statuses
                        .get(*name)
                        .map(|s| s == "started")
                        .unwrap_or(false)
                })
                .map(|name| {
                    let status = state
                        .statuses
                        .get(name)
                        .map(|s| s.as_str())
                        .unwrap_or("unknown");
                    format!("{} (status={})", name.as_str(), status)
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
    peer_names: &[PeerName],
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;

    loop {
        if Instant::now() >= deadline {
            let state = state.lock().await;
            let still_running: Vec<&PeerName> = peer_names
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
/// Waits up to 10 seconds for needed bootstrap peers to report their multiaddr.
pub async fn send_bootstrap_commands(
    config: &TestConfig,
    state: &Arc<Mutex<PeerState>>,
    redis_conn: &mut redis::aio::MultiplexedConnection,
) -> Result<()> {
    // Collect all bootstrap peer names we need info for
    let needed: std::collections::BTreeSet<&PeerName> = config
        .peers
        .iter()
        .flat_map(|p| p.bootstrap.iter())
        .collect();

    // Wait up to 10s for needed peers' multiaddr info
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = state.lock().await;
        let all_present = needed.iter().all(|name| state.peer_info.contains_key(*name));
        drop(state);
        if all_present || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let state = state.lock().await;

    for peer in &config.peers {
        for bootstrap_name in &peer.bootstrap {
            if let Some(addr) = state.peer_info.get(bootstrap_name) {
                let cmd = format!("peer|{addr}");
                let key = format!("{}_command", peer.name.as_str());
                redis_conn
                    .lpush::<_, _, ()>(&key, &cmd)
                    .await
                    .context(format!(
                        "failed to send bootstrap command to {}",
                        peer.name.as_str()
                    ))?;
                tracing::info!(
                    "sent bootstrap: {} -> {} ({})",
                    peer.name.as_str(),
                    bootstrap_name.as_str(),
                    addr
                );
            } else {
                tracing::warn!(
                    "bootstrap peer '{}' not found in peer_info for '{}'",
                    bootstrap_name.as_str(),
                    peer.name.as_str()
                );
            }
        }
    }

    Ok(())
}

/// Send shutdown to all peers and clean up.
pub async fn shutdown_all_peers(
    peer_names: &[PeerName],
    redis_conn: &mut redis::aio::MultiplexedConnection,
) {
    for name in peer_names {
        let key = format!("{}_command", name.as_str());
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
        let alice = PeerName::new("alice");
        let bob = PeerName::new("bob");
        {
            let mut s = state.lock().await;
            s.statuses.insert(alice.clone(), "started".to_string());
            s.statuses.insert(bob.clone(), "started".to_string());
        }
        let names = vec![alice, bob];
        let result =
            wait_for_peers_started(&state, &names, Duration::from_secs(1)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn wait_for_peers_started_timeout_with_missing() {
        let state = Arc::new(Mutex::new(PeerState::new()));
        let alice = PeerName::new("alice");
        let bob = PeerName::new("bob");
        {
            let mut s = state.lock().await;
            s.statuses.insert(alice.clone(), "started".to_string());
            // bob is missing
        }
        let names = vec![alice, bob];
        let result =
            wait_for_peers_started(&state, &names, Duration::from_millis(100)).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("bob"));
    }
}
