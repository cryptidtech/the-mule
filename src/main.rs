use the_mule::config;
use the_mule::console;
use the_mule::docker_mgr;
use the_mule::orchestrator;
use the_mule::peer_monitor;
use the_mule::peer_monitor::PeerEvent;
use the_mule::redis_mgr;
use the_mule::ssh_mgr;
use the_mule::ui;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use indicatif::MultiProgress;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use the_mule::config::{assign_peers, TestConfig};
use the_mule::peer_monitor::PeerState;

#[derive(Parser)]
#[command(name = "tm")]
#[command(version)]
#[command(about = "Orchestrate distributed peer integration tests")]
struct Args {
    /// Path to test YAML config
    config: PathBuf,
    /// Enable the ratatui TUI interface (default: console mode)
    #[arg(long)]
    tui: bool,
    /// Use an external Redis instance instead of starting one (e.g. redis://host:6399)
    #[arg(long)]
    redis_url: Option<String>,
    /// Remove Docker images from all hosts and exit (images listed in config)
    #[arg(long)]
    reset_hosts: bool,
    /// Run `docker system prune -af` on all hosts and exit
    #[arg(long)]
    reset_hosts_all: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config: TestConfig = serde_yaml::from_reader(
        File::open(&args.config).context("failed to open config file")?,
    )
    .context("failed to parse config YAML")?;

    // Handle --reset-hosts / --reset-hosts-all (early exit, no Redis/log needed)
    if args.reset_hosts || args.reset_hosts_all {
        println!("connecting to hosts via SSH...");
        let mut ssh_managers: HashMap<String, ssh_mgr::SshManager> = HashMap::new();
        for host in &config.hosts {
            ssh_managers
                .entry(host.address.clone())
                .or_insert_with(|| {
                    ssh_mgr::SshManager::new(host).expect("failed to create SSH session")
                });
        }
        println!("connected to {} host(s)", ssh_managers.len());

        for host in &config.hosts {
            if let Some(mgr) = ssh_managers.get(&host.address) {
                if args.reset_hosts_all {
                    println!("pruning Docker on {}...", host.display_name());
                    docker_mgr::prune_host(mgr, &host.address, true);
                } else {
                    println!(
                        "removing {} image(s) on {}...",
                        config.images.len(),
                        host.display_name()
                    );
                    docker_mgr::remove_images_on_host(mgr, &host.address, &config.images, true);
                }
            }
        }
        println!("done");
        return Ok(());
    }

    // Create MultiProgress for console mode
    let multi: Option<Arc<MultiProgress>> = if !args.tui {
        Some(Arc::new(MultiProgress::new()))
    } else {
        None
    };

    // Create log file
    let now = chrono::Local::now();
    let log_filename = format!(
        "{}-{}.log",
        config.name,
        now.format("%Y-%m-%d-%H-%M-%S")
    );
    let log_file = File::create(&log_filename).context("failed to create log file")?;

    // Build log level filter from config (default: info)
    let filter_str = config
        .log_level
        .as_ref()
        .map(|l| l.as_filter_str())
        .unwrap_or("info");
    let env_filter = tracing_subscriber::EnvFilter::new(filter_str);

    // Configure tracing — file layer always present, console layer in console mode
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(log_file)
        .with_ansi(false);

    if let Some(ref m) = multi {
        let console_layer = tracing_subscriber::fmt::layer()
            .with_writer(console::IndicatifMakeWriter::new(m.clone()))
            .with_ansi(true);
        tracing_subscriber::registry()
            .with(file_layer)
            .with(console_layer)
            .with(env_filter)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(file_layer)
            .with(env_filter)
            .init();
    }

    tracing::info!(
        "Test started at {}, config: {}",
        now,
        args.config.display()
    );

    // Set up signal handler for graceful shutdown
    let shutdown_token = CancellationToken::new();
    let signal_shutdown = shutdown_token.clone();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => { tracing::info!("received SIGINT (Ctrl+C)"); }
            _ = sigterm.recv() => { tracing::info!("received SIGTERM"); }
        }
        signal_shutdown.cancel();
    });

    // Start Redis (or connect to external instance)
    let (redis_mgr, redis_client, redis_url) = if let Some(ref url) = args.redis_url {
        tracing::info!("using external Redis: {url}");
        let client = redis::Client::open(url.as_str())
            .context("failed to create Redis client from --redis-url")?;
        (None, client, url.clone())
    } else {
        let spinner = multi.as_ref().map(|m| console::new_spinner(m, "starting Redis..."));
        let mgr =
            redis_mgr::RedisManager::start(&config.redis).context("failed to start Redis")?;
        let client = mgr
            .client(config.redis.port)
            .context("failed to create Redis client")?;
        let url = format!("redis://{}:{}", local_ip(), config.redis.port);
        if let Some(ref s) = spinner {
            s.finish_with_message("Redis started");
        }
        // Wait a moment for Redis to be ready
        tokio::time::sleep(Duration::from_secs(2)).await;
        (Some(mgr), client, url)
    };
    tracing::info!("Redis URL for peers: {redis_url}");

    let mut redis_conn = redis_client
        .get_multiplexed_async_connection()
        .await
        .context("failed to connect to Redis")?;

    redis_mgr::enable_keyspace_notifications(&mut redis_conn).await;

    // Compute peer-to-host assignments (round-robin, unique ports)
    let assignments = assign_peers(&config).map_err(|e| anyhow::anyhow!(e))?;

    // Build SSH connect info (lightweight, no connections yet)
    let ssh_connect_infos: HashMap<String, ssh_mgr::SshConnectInfo> = config
        .hosts
        .iter()
        .map(|h| (h.address.clone(), ssh_mgr::SshConnectInfo { host: h.clone() }))
        .collect();

    // Create SSH managers for later peer start/stop (still synchronous)
    let ssh_spinner = multi.as_ref().map(|m| console::new_spinner(m, "connecting to hosts via SSH..."));
    let mut ssh_managers: HashMap<String, ssh_mgr::SshManager> = HashMap::new();
    for assignment in &assignments {
        ssh_managers
            .entry(assignment.host.address.clone())
            .or_insert_with(|| {
                ssh_mgr::SshManager::new(&assignment.host)
                    .expect("failed to create SSH session")
            });
    }
    if let Some(ref s) = ssh_spinner {
        s.finish_with_message(format!("connected to {} host(s)", ssh_managers.len()));
    }

    // Pre-pull images listed in the config
    if !config.images.is_empty() {
        docker_mgr::pull_images(&config.images, multi.as_ref())?;
    }

    // Distribute Docker images to hosts (async, maximally parallel)
    docker_mgr::distribute_all_images(&assignments, &ssh_connect_infos, multi.as_ref()).await?;

    // Phase 3: Clear stale Redis queues and start peer containers
    let clear_spinner = multi.as_ref().map(|m| console::new_spinner(m, "clearing stale Redis queues..."));
    for assignment in &assignments {
        let command_key = format!("{}_command", assignment.peer_name);
        let log_key = format!("{}_log", assignment.peer_name);
        redis::AsyncCommands::del::<_, ()>(&mut redis_conn, &command_key)
            .await
            .context(format!("failed to DEL {command_key}"))?;
        redis::AsyncCommands::del::<_, ()>(&mut redis_conn, &log_key)
            .await
            .context(format!("failed to DEL {log_key}"))?;
    }
    if let Some(ref s) = clear_spinner {
        s.finish_with_message("cleared stale Redis queues");
    }

    // Start peer containers with per-peer spinners
    let peer_spinners: Vec<_> = assignments
        .iter()
        .map(|a| {
            multi.as_ref().map(|m| {
                console::new_spinner(
                    m,
                    &format!("{}: starting {}", a.host.display_name(), a.peer_name),
                )
            })
        })
        .collect();

    for (i, assignment) in assignments.iter().enumerate() {
        let docker_cmd = docker_mgr::start_peer(assignment, &redis_url, &ssh_managers)
            .context(format!(
                "failed to start peer {} on {}",
                assignment.peer_name, assignment.host.address
            ))?;
        tracing::info!("docker run command: {docker_cmd}");
        if let Some(ref s) = peer_spinners[i] {
            s.finish_with_message(format!(
                "{}: started {}",
                assignment.host.display_name(),
                assignment.peer_name
            ));
        }
    }

    // Give containers a moment to initialize before spawning BLPOP monitors
    let init_spinner = multi.as_ref().map(|m| console::new_spinner(m, "waiting for containers to initialize..."));
    tokio::time::sleep(Duration::from_secs(2)).await;
    if let Some(ref s) = init_spinner {
        s.finish_with_message("containers initialized");
    }

    // Shared state for peer statuses and identity info
    let state = Arc::new(Mutex::new(PeerState::new()));
    let cancel = CancellationToken::new();

    // Create broadcast channel for console mode events
    let event_tx = if !args.tui {
        Some(tokio::sync::broadcast::channel::<PeerEvent>(256).0)
    } else {
        None
    };

    // Spawn per-peer monitor tasks
    let peer_names: Vec<String> = config.peers.iter().map(|p| p.name.clone()).collect();
    let mut monitor_handles = Vec::new();
    for name in &peer_names {
        let handle = tokio::spawn(peer_monitor::monitor_peer(
            name.clone(),
            redis_client.clone(),
            state.clone(),
            cancel.clone(),
            event_tx.clone(),
        ));
        monitor_handles.push(handle);
    }

    // Wait for all peers to report "started" (configurable timeout)
    let start_spinner = multi.as_ref().map(|m| console::new_spinner(m, "waiting for all peers to start..."));
    tracing::info!("waiting for all peers to start...");
    if let Err(e) = orchestrator::wait_for_peers_started(
        &state,
        &peer_names,
        Duration::from_secs(config.timeout.startup),
    )
    .await
    {
        tracing::error!("peer startup failed: {e}");
        if let Some(ref s) = start_spinner {
            s.abandon_with_message(format!("ERROR: peer startup failed: {e}"));
        }
        orchestrator::shutdown_all_peers(&peer_names, &mut redis_conn).await;
        cleanup(
            cancel,
            monitor_handles,
            &assignments,
            &ssh_managers,
            redis_mgr.as_ref(),
            &config,
        )
        .await;
        anyhow::bail!("peer startup failed: {e}");
    }
    tracing::info!("all peers started successfully");
    if let Some(ref s) = start_spinner {
        s.finish_with_message("all peers started");
    }

    // Send bootstrap peer commands
    orchestrator::send_bootstrap_commands(&config, &state, &mut redis_conn)
        .await
        .context("failed to send bootstrap commands")?;
    tracing::info!("bootstrap commands sent");

    // Build command batches
    let mut batches = ui::build_batches(&config.commands, &assignments);

    // Record test start time
    let test_start = Instant::now();
    tracing::info!("test timeline starting");

    // Run TUI or console mode
    let result = if args.tui {
        run_tui(
            &config,
            &assignments,
            &mut batches,
            &mut redis_conn,
            &state,
            test_start,
            &shutdown_token,
            &peer_names,
        )
        .await
    } else {
        run_console(
            &config,
            &mut batches,
            &mut redis_conn,
            &state,
            test_start,
            &shutdown_token,
            &peer_names,
            event_tx.as_ref().unwrap(),
        )
        .await
    };

    // Cleanup
    cleanup(
        cancel,
        monitor_handles,
        &assignments,
        &ssh_managers,
        redis_mgr.as_ref(),
        &config,
    )
    .await;

    result
}

/// Send any due command batches, returning the updated batch index.
async fn send_due_batches(
    batches: &mut [ui::CommandBatch],
    current_batch_idx: &mut usize,
    redis_conn: &mut redis::aio::MultiplexedConnection,
    test_start: Instant,
) {
    while *current_batch_idx < batches.len() {
        let target_time = Duration::from_secs(batches[*current_batch_idx].time);
        if test_start.elapsed() >= target_time {
            let batch = &batches[*current_batch_idx];
            for cmd in &batch.commands {
                let key = format!("{}_command", cmd.peer);
                if let Err(e) =
                    redis::AsyncCommands::lpush::<_, _, ()>(redis_conn, &key, &cmd.command).await
                {
                    tracing::warn!("failed to send command to {}: {e}", cmd.peer);
                } else {
                    let msg = format!(
                        "[{:.1}s] sent to {}: {}",
                        test_start.elapsed().as_secs_f64(),
                        cmd.peer,
                        cmd.command
                    );
                    tracing::info!("{msg}");
                }
            }
            batches[*current_batch_idx].sent = true;
            batches[*current_batch_idx].sent_at = Some(Instant::now());
            *current_batch_idx += 1;
        } else {
            break;
        }
    }
}

async fn run_tui(
    config: &TestConfig,
    assignments: &[config::PeerAssignment],
    batches: &mut [ui::CommandBatch],
    redis_conn: &mut redis::aio::MultiplexedConnection,
    state: &Arc<Mutex<PeerState>>,
    test_start: Instant,
    shutdown_token: &CancellationToken,
    peer_names: &[String],
) -> Result<()> {
    // Set up terminal for TUI
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend).context("failed to create terminal")?;

    let mut current_batch_idx: usize = 0;
    let mut shutdown_sent = false;
    let mut shutdown_started: Option<Instant> = None;

    let result = loop {
        // Send any due command batches
        send_due_batches(batches, &mut current_batch_idx, redis_conn, test_start).await;

        // Render TUI
        let statuses = {
            let state = state.lock().await;
            state.statuses.clone()
        };
        let elapsed = test_start.elapsed();

        terminal.draw(|frame| {
            ui::render(
                frame,
                &config.name,
                elapsed,
                &statuses,
                assignments,
                batches,
                current_batch_idx,
            );
        })?;

        // Check for user input (non-blocking, 100ms timeout)
        if let Some(ui::InputEvent::Quit) = ui::poll_input(Duration::from_millis(100))? {
            tracing::info!("user requested quit");
            orchestrator::shutdown_all_peers(peer_names, redis_conn).await;
            let _ = orchestrator::wait_for_peers_stopped(
                state,
                &peer_names.to_vec(),
                Duration::from_secs(config.timeout.shutdown),
            )
            .await;
            break Ok(());
        }

        // Check for signal-based shutdown
        if shutdown_token.is_cancelled() {
            tracing::info!("signal received, shutting down TUI");
            orchestrator::shutdown_all_peers(peer_names, redis_conn).await;
            let _ = orchestrator::wait_for_peers_stopped(
                state,
                &peer_names.to_vec(),
                Duration::from_secs(config.timeout.shutdown),
            )
            .await;
            break Ok(());
        }

        // Auto-shutdown: once all commands are sent, send shutdown to all peers
        if current_batch_idx >= batches.len() {
            if !shutdown_sent {
                tracing::info!("all commands sent, sending shutdown to all peers");
                orchestrator::shutdown_all_peers(peer_names, redis_conn).await;
                shutdown_sent = true;
                shutdown_started = Some(Instant::now());
            }

            let all_stopped = {
                let state = state.lock().await;
                peer_names.iter().all(|name| {
                    state
                        .statuses
                        .get(name)
                        .map(|s| s == "stopped")
                        .unwrap_or(false)
                })
            };
            if all_stopped {
                tracing::info!("all commands sent and all peers stopped — test complete");
                break Ok(());
            }

            if let Some(started) = shutdown_started {
                if started.elapsed() > Duration::from_secs(config.timeout.shutdown) {
                    tracing::warn!("shutdown timeout exceeded, exiting");
                    break Ok(());
                }
            }
        }
    };

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_console(
    config: &TestConfig,
    batches: &mut [ui::CommandBatch],
    redis_conn: &mut redis::aio::MultiplexedConnection,
    state: &Arc<Mutex<PeerState>>,
    test_start: Instant,
    shutdown_token: &CancellationToken,
    peer_names: &[String],
    event_tx: &tokio::sync::broadcast::Sender<PeerEvent>,
) -> Result<()> {
    let mut current_batch_idx: usize = 0;
    let mut event_rx = event_tx.subscribe();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut shutdown_sent = false;
    let mut shutdown_started: Option<Instant> = None;

    loop {
        tokio::select! {
            // Branch 1: peer events
            event = event_rx.recv() => {
                match event {
                    Ok(PeerEvent::StatusChange { peer, status }) => {
                        tracing::info!("{peer}: {status}");
                    }
                    Ok(PeerEvent::LogEntry { peer, level, message }) => {
                        if level == "error" || level == "warn" {
                            tracing::warn!("[{level}] {peer}: {message}");
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("console event receiver lagged by {n} messages");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            // Branch 2: tick — send due commands and check completion
            _ = tick.tick() => {
                send_due_batches(batches, &mut current_batch_idx, redis_conn, test_start).await;

                // Auto-shutdown: once all commands are sent, send shutdown to all peers
                if current_batch_idx >= batches.len() {
                    if !shutdown_sent {
                        tracing::info!("all commands sent, sending shutdown to all peers");
                        orchestrator::shutdown_all_peers(peer_names, redis_conn).await;
                        shutdown_sent = true;
                        shutdown_started = Some(Instant::now());
                    }

                    let all_stopped = {
                        let state = state.lock().await;
                        peer_names.iter().all(|name| {
                            state.statuses.get(name).map(|s| s == "stopped").unwrap_or(false)
                        })
                    };
                    if all_stopped {
                        tracing::info!("all commands sent and all peers stopped — test complete");
                        return Ok(());
                    }

                    if let Some(started) = shutdown_started {
                        if started.elapsed() > Duration::from_secs(config.timeout.shutdown) {
                            tracing::warn!("shutdown timeout exceeded ({}s), exiting", config.timeout.shutdown);
                            return Ok(());
                        }
                    }
                }
            }
            // Branch 3: shutdown signal
            _ = shutdown_token.cancelled() => {
                tracing::info!("signal received, initiating orderly shutdown");
                orchestrator::shutdown_all_peers(peer_names, redis_conn).await;
                tracing::info!("waiting up to {}s for peers to stop...", config.timeout.shutdown);
                match orchestrator::wait_for_peers_stopped(
                    state,
                    &peer_names.to_vec(),
                    Duration::from_secs(config.timeout.shutdown),
                ).await {
                    Ok(()) => tracing::info!("all peers stopped gracefully"),
                    Err(e) => tracing::warn!("shutdown timeout: {e} (force-stopping in cleanup)"),
                }
                return Ok(());
            }
        }
    }

    Ok(())
}

async fn cleanup(
    cancel: CancellationToken,
    monitor_handles: Vec<tokio::task::JoinHandle<()>>,
    assignments: &[config::PeerAssignment],
    ssh_managers: &HashMap<String, ssh_mgr::SshManager>,
    redis_mgr: Option<&redis_mgr::RedisManager>,
    config: &TestConfig,
) {
    // Cancel all monitor tasks
    cancel.cancel();
    for h in monitor_handles {
        let _ = h.await;
    }

    // Stop all peer containers via SSH
    for assignment in assignments {
        let _ = docker_mgr::stop_peer(
            &assignment.peer_name,
            &assignment.host.address,
            ssh_managers,
        );
    }

    // Remove images if configured
    if config.remove_images {
        for host in &config.hosts {
            if let Some(mgr) = ssh_managers.get(&host.address) {
                docker_mgr::remove_images_on_host(mgr, &host.address, &config.images, true);
            }
        }
    }

    // Stop Redis (only if we started it)
    if let Some(mgr) = redis_mgr {
        let _ = mgr.stop();
    }
}

/// Get the local machine's IP address.
/// Falls back to 127.0.0.1 if detection fails.
fn local_ip() -> String {
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ip_returns_dotted_string() {
        let ip = local_ip();
        assert!(!ip.is_empty());
        assert!(ip.contains('.'), "expected dotted IP, got: {ip}");
    }
}
