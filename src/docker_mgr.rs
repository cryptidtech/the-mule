use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::PeerAssignment;
use crate::ssh_mgr::{ExitStatus, SshManager};

/// Print a message to stdout when in console mode.
fn console_print(console_mode: bool, msg: &str) {
    if console_mode {
        println!("{msg}");
    }
}

/// Sanitize a Docker image name into a safe filename component.
pub fn sanitize_image_name(image: &str) -> String {
    let sanitized: String = image
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse consecutive hyphens and trim leading/trailing hyphens
    let mut result = String::new();
    let mut prev_hyphen = false;
    for c in sanitized.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push(c);
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }
    result.trim_matches('-').to_string()
}

/// Build a temp archive path for the exported Docker image.
pub fn temp_archive_path(image: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/tm-image-{}.tar.gz", sanitize_image_name(image)))
}

/// Verify the Docker image exists locally and return its ID (sha256 hash).
fn check_local_image(image: &str) -> Result<String> {
    tracing::info!("checking local Docker image: {image}");
    let output = Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", image])
        .output()
        .context("failed to run 'docker image inspect'")?;

    if !output.status.success() {
        anyhow::bail!(
            "Docker image '{}' not found locally. Build it with:\n  docker build -t {} <path-to-build-context>",
            image,
            image
        );
    }
    let image_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    tracing::info!("local image ID: {image_id}");
    Ok(image_id)
}

/// Check which remote hosts are missing or have a stale Docker image.
/// Compares the local image ID against each remote host's image ID.
/// Returns a list of host addresses that need the image.
fn check_remote_hosts(
    image: &str,
    local_image_id: &str,
    ssh_managers: &HashMap<String, SshManager>,
    console_mode: bool,
) -> Result<Vec<String>> {
    tracing::info!("checking Docker image on {} remote host(s)", ssh_managers.len());
    let mut missing = Vec::new();

    for (host_addr, mgr) in ssh_managers {
        let cmd = format!("docker image inspect --format '{{{{.Id}}}}' '{}'", image);
        let (remote_id, exit_code) = mgr
            .exec_with_status(&cmd)
            .context(format!("failed to check image on {host_addr}"))?;

        if exit_code != 0 {
            tracing::info!("image '{}' missing on {}", image, host_addr);
            console_print(console_mode, &format!("  {host_addr}: image missing"));
            missing.push(host_addr.clone());
        } else {
            let remote_id = remote_id.trim();
            if remote_id == local_image_id {
                tracing::info!("image '{}' present on {} (ID matches)", image, host_addr);
                console_print(console_mode, &format!("  {host_addr}: present ({remote_id})"));
            } else {
                tracing::info!(
                    "image '{}' stale on {} (local={}, remote={})",
                    image, host_addr, local_image_id, remote_id
                );
                console_print(console_mode, &format!("  {host_addr}: stale ({remote_id}), needs update"));
                missing.push(host_addr.clone());
            }
        }
    }

    // Ensure ~/peer-config directory exists on all remote hosts
    for (host_addr, mgr) in ssh_managers {
        match mgr.exec("mkdir -p ~/peer-config")? {
            ExitStatus::Success(_) => {}
            ExitStatus::Failed(err) => {
                tracing::error!("failed to create ~/peer-config on {host_addr}: {err}");
                anyhow::bail!("failed to create ~/peer-config on {host_addr}: {err}");
            }
        }
    }

    Ok(missing)
}

/// Export a Docker image to a gzipped tar archive.
fn export_image(image: &str, archive_path: &Path, console_mode: bool) -> Result<()> {
    tracing::info!("exporting Docker image '{}' to {}", image, archive_path.display());
    console_print(
        console_mode,
        &format!("exporting Docker image '{image}' to local archive..."),
    );

    let cmd = format!(
        "docker save '{}' | gzip > '{}'",
        image,
        archive_path.display()
    );
    let output = Command::new("sh")
        .args(["-c", &cmd])
        .output()
        .context("failed to run docker save")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("docker save failed: {}", stderr.trim());
    }

    let size_mb = std::fs::metadata(archive_path)?.len() as f64 / (1024.0 * 1024.0);
    tracing::info!("image archive size: {:.1} MB", size_mb);
    console_print(console_mode, &format!("  archive size: {size_mb:.1} MB"));

    Ok(())
}

/// Transfer the image archive to a remote host and load it.
fn transfer_and_load(
    mgr: &SshManager,
    host_addr: &str,
    local_archive: &Path,
    remote_archive: &Path,
    console_mode: bool,
) -> Result<()> {
    tracing::info!("transferring image to {host_addr}...");
    console_print(console_mode, &format!("  transferring image to {host_addr}..."));

    mgr.scp_send_file(local_archive, remote_archive)
        .context(format!("failed to SCP image to {host_addr}"))?;

    tracing::info!("loading image on {host_addr}...");
    console_print(console_mode, &format!("  loading image on {host_addr}..."));

    let load_cmd = format!("docker load -i '{}'", remote_archive.display());
    match mgr.exec(&load_cmd)? {
        ExitStatus::Success(_) => {}
        ExitStatus::Failed(err) => {
            tracing::error!("failed to load image on {host_addr}: {err}");
            anyhow::bail!("failed to load image on {host_addr}: {err}");
        }
    }

    let rm_cmd = format!("rm -f '{}'", remote_archive.display());
    match mgr.exec(&rm_cmd)? {
        ExitStatus::Success(_) => {}
        ExitStatus::Failed(err) => {
            tracing::error!("failed to cleanup archive on {host_addr}: {err}");
            anyhow::bail!("failed to cleanup archive on {host_addr}: {err}");
        }
    }

    tracing::info!("image loaded on {host_addr}");
    console_print(console_mode, &format!("  image loaded on {host_addr}"));

    Ok(())
}

/// Ensure the Docker image is available on all remote hosts.
///
/// 1. Verifies image exists locally (bails with build instructions if not)
/// 2. Checks each remote host for the image
/// 3. If any hosts are missing it, exports locally and SCPs to each
pub fn ensure_image_on_hosts(
    docker_image: &str,
    ssh_managers: &HashMap<String, SshManager>,
    console_mode: bool,
) -> Result<()> {
    tracing::info!("ensuring Docker image '{}' is available on all hosts", docker_image);
    console_print(console_mode, &format!("checking Docker image '{docker_image}'..."));

    // 1. Check local and get image ID
    let local_image_id = check_local_image(docker_image)?;
    console_print(console_mode, &format!("  local image: present ({local_image_id})"));

    // 2. Check remote hosts (compare image IDs)
    let missing_hosts = check_remote_hosts(docker_image, &local_image_id, ssh_managers, console_mode)?;

    if missing_hosts.is_empty() {
        tracing::info!("Docker image present on all remote hosts");
        console_print(console_mode, "Docker image present on all remote hosts");
        return Ok(());
    }

    tracing::info!(
        "image missing on {} host(s), distributing...",
        missing_hosts.len()
    );
    console_print(
        console_mode,
        &format!(
            "image missing on {} host(s), distributing...",
            missing_hosts.len()
        ),
    );

    // 3. Export image locally
    let archive_path = temp_archive_path(docker_image);
    export_image(docker_image, &archive_path, console_mode)?;

    // 4. Transfer to each missing host, always cleaning up archive afterward
    let remote_archive = PathBuf::from(format!(
        "/tmp/tm-image-{}.tar.gz",
        sanitize_image_name(docker_image)
    ));

    let result = (|| -> Result<()> {
        for host_addr in &missing_hosts {
            let mgr = ssh_managers
                .get(host_addr)
                .expect("SSH manager must exist for missing host");
            transfer_and_load(mgr, host_addr, &archive_path, &remote_archive, console_mode)?;
        }
        Ok(())
    })();

    // Always clean up local archive
    if archive_path.exists() {
        let _ = std::fs::remove_file(&archive_path);
    }

    result?;

    tracing::info!("Docker image distributed to all hosts");
    console_print(console_mode, "Docker image distributed to all hosts");

    Ok(())
}

/// Pull a list of Docker images locally. Bails on the first failure.
pub fn pull_images(images: &[String], console_mode: bool) -> Result<()> {
    for image in images {
        console_print(console_mode, &format!("pulling Docker image '{image}'..."));
        tracing::info!("pulling Docker image: {image}");

        let output = Command::new("docker")
            .args(["pull", image])
            .output()
            .context(format!("failed to run 'docker pull {image}'"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker pull '{image}' failed: {}", stderr.trim());
        }

        console_print(console_mode, &format!("  pulled '{image}'"));
        tracing::info!("pulled Docker image: {image}");
    }
    Ok(())
}

/// Start a peer container on its assigned remote host via SSH.
/// Returns the docker run command that was executed.
pub fn start_peer(
    assignment: &PeerAssignment,
    redis_url: &str,
    ssh_managers: &HashMap<String, SshManager>,
) -> Result<String> {
    let mgr = ssh_managers
        .get(&assignment.host.address)
        .context(format!("no SSH session for {}", assignment.host.address))?;

    let name = &assignment.peer_name;
    let port = assignment.port;
    let container_name = format!("tm-{name}");

    // Remove any existing container
    match mgr.exec(&format!("docker rm -f {container_name}")) {
        Ok(ExitStatus::Failed(err)) => {
            tracing::warn!("failed to remove container {container_name}: {err}");
        }
        Err(e) => {
            tracing::warn!("failed to remove container {container_name}: {e}");
        }
        _ => {}
    }

    // Ensure per-peer config directory exists on the remote host
    let config_dir = format!("~/peer-config/{container_name}-config");
    match mgr.exec(&format!("mkdir -p {config_dir}"))? {
        ExitStatus::Success(_) => {}
        ExitStatus::Failed(err) => {
            tracing::error!("failed to create {config_dir} on {}: {err}", assignment.host.address);
            anyhow::bail!("failed to create {config_dir} on {}: {err}", assignment.host.address);
        }
    }

    // Build env var args
    let mut env_args = format!(
        "-e REDIS_URL={redis_url} -e PEER_NAME={name} -e LISTEN_ADDR={listen}",
        listen = assignment.listen_addr
    );
    for (k, v) in &assignment.extra_env {
        env_args.push_str(&format!(" -e {k}={v}"));
    }

    let image = &assignment.docker_image;
    let cmd = format!(
        "docker run -d --name {container_name} \
         -v {config_dir}:/config \
         -p {port}:{port}/udp \
         {env_args} \
         {image}"
    );

    let output = match mgr.exec(&cmd)? {
        ExitStatus::Success(out) => out,
        ExitStatus::Failed(err) => {
            tracing::error!("failed to start peer {name} on {}: {err}", assignment.host.address);
            anyhow::bail!("failed to start peer {name} on {}: {err}", assignment.host.address);
        }
    };

    tracing::info!(
        "started peer '{}' on {}:{} (container: {})",
        name,
        assignment.host.address,
        port,
        output.trim()
    );

    Ok(cmd)
}

/// Stop and remove a peer container on its assigned remote host.
pub fn stop_peer(
    peer_name: &str,
    host_address: &str,
    ssh_managers: &HashMap<String, SshManager>,
) -> Result<()> {
    let mgr = ssh_managers
        .get(host_address)
        .context(format!("no SSH session for {host_address}"))?;

    let container_name = format!("tm-{peer_name}");
    match mgr.exec(&format!("docker rm -f {container_name}")) {
        Ok(ExitStatus::Failed(err)) => {
            tracing::warn!("failed to remove container {container_name}: {err}");
        }
        Err(e) => {
            tracing::warn!("failed to remove container {container_name}: {e}");
        }
        _ => {}
    }
    tracing::info!("stopped peer '{}' on {}", peer_name, host_address);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_simple_name() {
        assert_eq!(sanitize_image_name("alpine"), "alpine");
    }

    #[test]
    fn sanitize_registry_name() {
        assert_eq!(
            sanitize_image_name("registry.example.com/my-image:latest"),
            "registry-example-com-my-image-latest"
        );
    }

    #[test]
    fn sanitize_collapse_hyphens() {
        assert_eq!(sanitize_image_name("a//b::c"), "a-b-c");
    }

    #[test]
    fn sanitize_trim_hyphens() {
        assert_eq!(sanitize_image_name(":leading:"), "leading");
    }

    #[test]
    fn temp_archive_path_prefix() {
        let path = temp_archive_path("alpine:latest");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("tm-image-"));
        assert!(filename.ends_with(".tar.gz"));
    }
}
