use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::PeerAssignment;
use crate::ssh_mgr::{ExitStatus, SshConnectInfo, SshManager};

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

/// Per-image distribution plan: which hosts are missing this image.
struct ImagePlan {
    image: String,
    missing_hosts: Vec<String>,
    archive_path: PathBuf,
}

/// Check if a remote host is missing or has a stale Docker image.
/// Returns `Some(host_addr)` if the host needs the image, `None` otherwise.
fn check_remote_image_blocking(
    connect_info: &SshConnectInfo,
    image: &str,
    local_id: &str,
) -> Result<Option<String>> {
    let host_addr = &connect_info.host.address;
    let mgr = connect_info.connect()
        .context(format!("failed to connect to {host_addr} for image check"))?;

    let cmd = format!("docker image inspect --format '{{{{.Id}}}}' '{}'", image);
    let (remote_id, exit_code) = mgr
        .exec_with_status(&cmd)
        .context(format!("failed to check image on {host_addr}"))?;

    if exit_code != 0 {
        tracing::info!("image '{}' missing on {}", image, host_addr);
        Ok(Some(host_addr.clone()))
    } else {
        let remote_id = remote_id.trim();
        if remote_id == local_id {
            tracing::info!("image '{}' present on {} (ID matches)", image, host_addr);
            Ok(None)
        } else {
            tracing::info!(
                "image '{}' stale on {} (local={}, remote={})",
                image, host_addr, local_id, remote_id
            );
            Ok(Some(host_addr.clone()))
        }
    }
}

/// Export a Docker image to a gzipped tar archive (blocking).
fn export_image_blocking(image: &str, archive_path: &Path) -> Result<()> {
    tracing::info!("exporting Docker image '{}' to {}", image, archive_path.display());

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
        anyhow::bail!("docker save failed for '{}': {}", image, stderr.trim());
    }

    let size_mb = std::fs::metadata(archive_path)?.len() as f64 / (1024.0 * 1024.0);
    tracing::info!("image '{}' archive size: {:.1} MB", image, size_mb);

    Ok(())
}

/// Transfer an image archive to a remote host via SCP (blocking).
fn transfer_blocking(
    connect_info: &SshConnectInfo,
    local_archive: &Path,
    remote_archive: &Path,
) -> Result<()> {
    let host_addr = &connect_info.host.address;
    tracing::info!("transferring image to {host_addr}...");

    let mgr = connect_info.connect()
        .context(format!("failed to connect to {host_addr} for transfer"))?;
    mgr.scp_send_file(local_archive, remote_archive)
        .context(format!("failed to SCP image to {host_addr}"))?;

    Ok(())
}

/// Load an image archive on a remote host and clean up (blocking).
fn import_and_cleanup_blocking(
    connect_info: &SshConnectInfo,
    remote_archive: &Path,
) -> Result<()> {
    let host_addr = &connect_info.host.address;
    tracing::info!("loading image on {host_addr}...");

    let mgr = connect_info.connect()
        .context(format!("failed to connect to {host_addr} for import"))?;

    let load_cmd = format!("docker load -i '{}'", remote_archive.display());
    match mgr.exec(&load_cmd)? {
        ExitStatus::Success(_) => {}
        ExitStatus::Failed(err) => {
            anyhow::bail!("failed to load image on {host_addr}: {err}");
        }
    }

    let rm_cmd = format!("rm -f '{}'", remote_archive.display());
    match mgr.exec(&rm_cmd)? {
        ExitStatus::Success(_) => {}
        ExitStatus::Failed(err) => {
            anyhow::bail!("failed to cleanup archive on {host_addr}: {err}");
        }
    }

    tracing::info!("image loaded on {host_addr}");
    Ok(())
}

/// Ensure ~/peer-config directory exists on all remote hosts, in parallel.
async fn ensure_peer_config_dirs(
    ssh_connect_infos: &HashMap<String, SshConnectInfo>,
) -> Result<()> {
    let mut join_set = tokio::task::JoinSet::new();

    for (host_addr, info) in ssh_connect_infos {
        let info = info.clone();
        let host_addr = host_addr.clone();
        join_set.spawn(tokio::task::spawn_blocking(move || -> Result<()> {
            let mgr = info.connect()
                .context(format!("failed to connect to {host_addr} for mkdir"))?;
            match mgr.exec("mkdir -p ~/peer-config")? {
                ExitStatus::Success(_) => Ok(()),
                ExitStatus::Failed(err) => {
                    anyhow::bail!("failed to create ~/peer-config on {host_addr}: {err}");
                }
            }
        }));
    }

    let mut errors = Vec::new();
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(e))) => errors.push(format!("{e}")),
            Ok(Err(e)) => errors.push(format!("task panic: {e}")),
            Err(e) => errors.push(format!("join error: {e}")),
        }
    }

    if !errors.is_empty() {
        anyhow::bail!("failed to ensure peer-config dirs:\n  {}", errors.join("\n  "));
    }
    Ok(())
}

/// Phase 1: Check local and remote images in parallel, build distribution plans.
async fn plan_distribution(
    image_hosts: &HashMap<String, HashSet<String>>,
    ssh_connect_infos: &HashMap<String, SshConnectInfo>,
) -> Result<Vec<ImagePlan>> {
    // Check all local images in parallel
    let mut local_set = tokio::task::JoinSet::new();
    for image in image_hosts.keys() {
        let image = image.clone();
        local_set.spawn(tokio::task::spawn_blocking(move || -> Result<(String, String)> {
            let id = check_local_image(&image)?;
            Ok((image, id))
        }));
    }

    let mut local_ids: HashMap<String, String> = HashMap::new();
    while let Some(result) = local_set.join_next().await {
        let (image, id) = result???;
        local_ids.insert(image, id);
    }

    // Check all (image, host) pairs in parallel
    let mut remote_set = tokio::task::JoinSet::new();
    for (image, hosts) in image_hosts {
        let local_id = local_ids[image].clone();
        for host_addr in hosts {
            let info = ssh_connect_infos[host_addr].clone();
            let image = image.clone();
            let local_id = local_id.clone();
            remote_set.spawn(tokio::task::spawn_blocking(move || -> Result<(String, Option<String>)> {
                let missing = check_remote_image_blocking(&info, &image, &local_id)?;
                Ok((image, missing))
            }));
        }
    }

    // Collect missing hosts per image
    let mut missing_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut errors = Vec::new();
    while let Some(result) = remote_set.join_next().await {
        match result {
            Ok(Ok(Ok((image, Some(host))))) => {
                missing_map.entry(image).or_default().push(host);
            }
            Ok(Ok(Ok((_, None)))) => {} // host has image, skip
            Ok(Ok(Err(e))) => errors.push(format!("{e}")),
            Ok(Err(e)) => errors.push(format!("task panic: {e}")),
            Err(e) => errors.push(format!("join error: {e}")),
        }
    }

    if !errors.is_empty() {
        anyhow::bail!("failed during remote image checks:\n  {}", errors.join("\n  "));
    }

    // Build plans for images that have at least one missing host
    let plans: Vec<ImagePlan> = missing_map
        .into_iter()
        .map(|(image, missing_hosts)| {
            let archive_path = temp_archive_path(&image);
            ImagePlan {
                image,
                missing_hosts,
                archive_path,
            }
        })
        .collect();

    Ok(plans)
}

/// Phase 2: Export images and transfer/import to missing hosts in parallel.
async fn distribute_images(
    plans: Vec<ImagePlan>,
    ssh_connect_infos: &HashMap<String, SshConnectInfo>,
) -> Result<()> {
    let mut join_set = tokio::task::JoinSet::new();

    for plan in &plans {
        // Create a watch channel to signal export completion
        let (export_tx, export_rx) = tokio::sync::watch::channel(false);

        // Spawn export task
        let image = plan.image.clone();
        let archive_path = plan.archive_path.clone();
        join_set.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                export_image_blocking(&image, &archive_path)
            }).await?;
            if result.is_ok() {
                let _ = export_tx.send(true);
            }
            result
        });

        // Spawn transfer+import tasks for each missing host
        for host_addr in &plan.missing_hosts {
            let mut rx = export_rx.clone();
            let info = ssh_connect_infos[host_addr].clone();
            let local_archive = plan.archive_path.clone();
            let image_name = plan.image.clone();
            let remote_archive = PathBuf::from(format!(
                "/tmp/tm-image-{}.tar.gz",
                sanitize_image_name(&image_name)
            ));
            let host = host_addr.clone();

            join_set.spawn(async move {
                // Wait for export to complete
                rx.wait_for(|done| *done).await.map_err(|_| {
                    anyhow::anyhow!("export of '{}' was cancelled, skipping transfer to {}", image_name, host)
                })?;

                // Transfer
                let info_clone = info.clone();
                let la = local_archive.clone();
                let ra = remote_archive.clone();
                tokio::task::spawn_blocking(move || {
                    transfer_blocking(&info_clone, &la, &ra)
                }).await??;

                // Import and cleanup
                let ra = remote_archive;
                tokio::task::spawn_blocking(move || {
                    import_and_cleanup_blocking(&info, &ra)
                }).await??;

                tracing::info!("image '{}' distributed to {}", image_name, host);
                Ok(())
            });
        }
    }

    // Drain all tasks, collect errors
    let mut errors = Vec::new();
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => errors.push(format!("{e}")),
            Err(e) => errors.push(format!("join error: {e}")),
        }
    }

    // Clean up local archives
    for plan in &plans {
        if plan.archive_path.exists() {
            let _ = std::fs::remove_file(&plan.archive_path);
        }
    }

    if !errors.is_empty() {
        anyhow::bail!("image distribution errors:\n  {}", errors.join("\n  "));
    }

    Ok(())
}

/// Distribute all Docker images to remote hosts using a parallel async pipeline.
///
/// 1. Derives unique (image, set_of_hosts) pairs from assignments
/// 2. Checks local+remote images in parallel
/// 3. Ensures ~/peer-config dirs exist on all hosts
/// 4. Runs export→transfer→import pipeline with maximum parallelism
pub async fn distribute_all_images(
    assignments: &[PeerAssignment],
    ssh_connect_infos: &HashMap<String, SshConnectInfo>,
    console_mode: bool,
) -> Result<()> {
    // Derive unique (image, set_of_hosts) from assignments
    let mut image_hosts: HashMap<String, HashSet<String>> = HashMap::new();
    for a in assignments {
        image_hosts
            .entry(a.docker_image.clone())
            .or_default()
            .insert(a.host.address.clone());
    }

    if image_hosts.is_empty() {
        return Ok(());
    }

    console_print(console_mode, "checking Docker images...");

    // Phase 1: plan distribution (parallel checks)
    let plans = plan_distribution(&image_hosts, ssh_connect_infos).await?;

    // Ensure peer-config dirs exist on all hosts
    ensure_peer_config_dirs(ssh_connect_infos).await?;

    if plans.is_empty() {
        tracing::info!("all Docker images present on all remote hosts");
        console_print(console_mode, "all Docker images present on all remote hosts");
        return Ok(());
    }

    let total_missing: usize = plans.iter().map(|p| p.missing_hosts.len()).sum();
    tracing::info!(
        "distributing {} image(s) to {} host target(s)...",
        plans.len(),
        total_missing
    );
    console_print(
        console_mode,
        &format!(
            "distributing {} image(s) to {} host target(s)...",
            plans.len(),
            total_missing
        ),
    );

    // Phase 2: export → transfer → import pipeline
    distribute_images(plans, ssh_connect_infos).await?;

    tracing::info!("Docker image distribution complete");
    console_print(console_mode, "Docker image distribution complete");

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

    // Build env var args: extra_env first (global + peer-specific), system vars last (last wins)
    let mut env_args = String::new();
    for (k, v) in &assignment.extra_env {
        env_args.push_str(&format!("-e {k}={v} "));
    }
    let host_name = assignment.host.display_name();
    env_args.push_str(&format!(
        "-e REDIS_URL={redis_url} -e PEER_NAME={name} -e LISTEN_ADDR={listen} -e HOST_NAME={host_name}",
        listen = assignment.listen_addr
    ));

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

/// Remove specific Docker images on a remote host.
pub fn remove_images_on_host(
    mgr: &SshManager,
    host_address: &str,
    images: &[String],
    console_mode: bool,
) {
    for image in images {
        let cmd = format!("docker rmi -f '{image}'");
        match mgr.exec(&cmd) {
            Ok(ExitStatus::Success(_)) => {
                let msg = format!("  removed image '{image}' on {host_address}");
                tracing::info!("{msg}");
                console_print(console_mode, &msg);
            }
            Ok(ExitStatus::Failed(err)) => {
                tracing::warn!("failed to remove image '{image}' on {host_address}: {err}");
                console_print(
                    console_mode,
                    &format!("  warning: failed to remove '{image}' on {host_address}: {err}"),
                );
            }
            Err(e) => {
                tracing::warn!("failed to remove image '{image}' on {host_address}: {e}");
            }
        }
    }
}

/// Run `docker system prune -af` on a remote host.
pub fn prune_host(
    mgr: &SshManager,
    host_address: &str,
    console_mode: bool,
) {
    let cmd = "docker system prune -af";
    match mgr.exec(cmd) {
        Ok(ExitStatus::Success(out)) => {
            let msg = format!("  pruned Docker on {host_address}");
            tracing::info!("{msg}: {}", out.trim());
            console_print(console_mode, &msg);
        }
        Ok(ExitStatus::Failed(err)) => {
            tracing::warn!("docker system prune failed on {host_address}: {err}");
            console_print(
                console_mode,
                &format!("  warning: prune failed on {host_address}: {err}"),
            );
        }
        Err(e) => {
            tracing::warn!("docker system prune failed on {host_address}: {e}");
        }
    }
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
