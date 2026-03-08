use anyhow::{Context, Result};
use std::process::Command;

use crate::config::RedisConfig;

/// Enable Redis keyspace notifications for string commands (SET).
/// This allows subscribers to receive notifications on `__keyspace@0__:<key>` channels.
pub async fn enable_keyspace_notifications(conn: &mut redis::aio::MultiplexedConnection) {
    let result: redis::RedisResult<()> = redis::cmd("CONFIG")
        .arg("SET")
        .arg("notify-keyspace-events")
        .arg("K$")
        .query_async(conn)
        .await;
    match result {
        Ok(()) => tracing::info!("enabled Redis keyspace notifications (K$)"),
        Err(e) => tracing::warn!("failed to enable keyspace notifications: {e}"),
    }
}

/// Manages a local Redis Docker container.
pub struct RedisManager {
    container_name: String,
}

impl RedisManager {
    /// Start a local Redis container.
    pub fn start(config: &RedisConfig) -> Result<Self> {
        let container_name = "tm-redis".to_string();

        // Remove any existing container with the same name
        let _ = Command::new("docker")
            .args(["rm", "-f", &container_name])
            .output();

        let output = Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                &container_name,
                "-p",
                &format!("{}:6379", config.port),
                &config.image,
            ])
            .output()
            .context("failed to run docker")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("failed to start Redis container: {stderr}");
        }

        tracing::info!(
            "started Redis container '{}' on port {}",
            container_name,
            config.port
        );

        Ok(Self { container_name })
    }

    /// Create a Redis client connected to this container.
    pub fn client(&self, port: u16) -> Result<redis::Client> {
        let url = format!("redis://127.0.0.1:{port}");
        redis::Client::open(url.as_str()).context("failed to create Redis client")
    }

    /// Stop and remove the Redis container.
    pub fn stop(&self) -> Result<()> {
        let output = Command::new("docker")
            .args(["rm", "-f", &self.container_name])
            .output()
            .context("failed to stop Redis container")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("failed to stop Redis container: {stderr}");
        } else {
            tracing::info!("stopped Redis container '{}'", self.container_name);
        }

        Ok(())
    }
}

impl Drop for RedisManager {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.container_name])
            .output();
    }
}
