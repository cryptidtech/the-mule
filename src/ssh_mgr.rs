use anyhow::{Context, Result};
use ssh2::Session;
use std::io::{Read, Write, BufReader};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

use crate::config::HostConfig;

/// Result of executing a remote command via SSH.
#[derive(Debug)]
pub enum ExitStatus {
    /// Command exited with status 0; contains stdout.
    Success(String),
    /// Command exited with non-zero status; contains stderr (or stdout if stderr empty).
    Failed(String),
}

/// Manages SSH connections and remote Docker container operations on a single host.
pub struct SshManager {
    session: Session,
    host_address: String,
}

impl SshManager {
    /// Create a new SSH session to the given host.
    pub fn new(host: &HostConfig) -> Result<Self> {
        let tcp = TcpStream::connect(format!("{}:22", host.address))
            .context(format!("failed to connect to {}:22", host.address))?;

        let mut session = Session::new().context("failed to create SSH session")?;
        session.set_tcp_stream(tcp);
        session
            .handshake()
            .context(format!("SSH handshake failed for {}", host.address))?;

        if host.ssh_auth.to_lowercase() == "agent" {
            let mut agent = session.agent().context("failed to init SSH agent")?;
            agent.connect().context(format!(
                "failed to connect to SSH agent (is SSH_AUTH_SOCK set?)",
            ))?;
            agent.list_identities().context("failed to list SSH agent identities")?;
            let identities = agent.identities().context("failed to get SSH agent identities")?;

            if identities.is_empty() {
                anyhow::bail!("SSH agent has no identities loaded for {}@{}", host.ssh_user, host.address);
            }

            let mut authed = false;
            for identity in &identities {
                if agent.userauth(&host.ssh_user, identity).is_ok() {
                    tracing::info!(
                        "SSH connected to {}@{} (agent, key: {})",
                        host.ssh_user, host.address, identity.comment()
                    );
                    authed = true;
                    break;
                }
            }

            if !authed {
                anyhow::bail!(
                    "SSH agent auth failed for {}@{}: none of {} loaded identities were accepted",
                    host.ssh_user, host.address, identities.len()
                );
            }
        } else {
            let key_path = shellexpand(&host.ssh_auth);
            session
                .userauth_pubkey_file(&host.ssh_user, None, &key_path, None)
                .context(format!(
                    "SSH key auth failed for {}@{} with key {}",
                    host.ssh_user, host.address, key_path.display()
                ))?;
            tracing::info!("SSH connected to {}@{} (key file)", host.ssh_user, host.address);
        }

        Ok(Self {
            session,
            host_address: host.address.clone(),
        })
    }

    /// Execute a command via SSH and return its exit status.
    pub fn exec(&self, cmd: &str) -> Result<ExitStatus> {
        let mut channel = self
            .session
            .channel_session()
            .context("failed to open SSH channel")?;
        channel.exec(cmd).context("failed to execute SSH command")?;

        let mut output = String::new();
        channel
            .read_to_string(&mut output)
            .context("failed to read SSH output")?;
        channel.wait_close().context("failed to close SSH channel")?;

        let exit_status = channel.exit_status()?;
        if exit_status != 0 {
            let mut stderr = String::new();
            let _ = channel.stderr().read_to_string(&mut stderr);
            tracing::debug!(
                "SSH command on {} exited with {}: cmd='{}' stderr='{}'",
                self.host_address,
                exit_status,
                cmd,
                stderr.trim()
            );
            let err_msg = if stderr.trim().is_empty() { output } else { stderr };
            Ok(ExitStatus::Failed(err_msg))
        } else {
            Ok(ExitStatus::Success(output))
        }
    }

    /// Execute a command via SSH and return (stdout, exit_code).
    pub fn exec_with_status(&self, cmd: &str) -> Result<(String, i32)> {
        let mut channel = self
            .session
            .channel_session()
            .context("failed to open SSH channel")?;
        channel.exec(cmd).context("failed to execute SSH command")?;
        let mut output = String::new();
        channel
            .read_to_string(&mut output)
            .context("failed to read SSH output")?;
        channel.wait_close().context("failed to close SSH channel")?;
        let exit_status = channel.exit_status()?;
        Ok((output, exit_status))
    }

    /// Send a local file to a remote host via SCP, streaming in 256KB chunks.
    pub fn scp_send_file(&self, local_path: &Path, remote_path: &Path) -> Result<()> {
        let local_file = std::fs::File::open(local_path)
            .context(format!("failed to open {}", local_path.display()))?;
        let file_size = local_file.metadata()?.len();

        let mut channel = self
            .session
            .scp_send(remote_path, 0o644, file_size, None)
            .context(format!("SCP init failed to {}", self.host_address))?;

        let mut reader = BufReader::with_capacity(256 * 1024, local_file);
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            channel.write_all(&buf[..n])?;
        }

        channel.send_eof()?;
        channel.wait_eof()?;
        channel.close()?;
        channel.wait_close()?;

        tracing::info!(
            "SCP complete: {} -> {}:{} ({:.1} MB)",
            local_path.display(),
            self.host_address,
            remote_path.display(),
            file_size as f64 / (1024.0 * 1024.0)
        );
        Ok(())
    }
}

/// Expand ~ to the home directory.
pub fn shellexpand(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Some(home) = dirs_home() {
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shellexpand_tilde() {
        let expanded = shellexpand("~/foo/bar");
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expanded, PathBuf::from(format!("{home}/foo/bar")));
    }

    #[test]
    fn shellexpand_absolute() {
        let expanded = shellexpand("/etc/ssh/config");
        assert_eq!(expanded, PathBuf::from("/etc/ssh/config"));
    }

    #[test]
    fn shellexpand_relative() {
        let expanded = shellexpand("relative/path");
        assert_eq!(expanded, PathBuf::from("relative/path"));
    }
}
