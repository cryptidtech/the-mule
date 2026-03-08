# Quickstart Guide

## Prerequisites

The `tm` test driver SSHes into hosts to run Docker containers. For a single-machine test, you need:

1. **Docker** installed and running
2. **SSH server** running locally (the driver SSHes to `localhost`)
3. **SSH key** for passwordless auth to localhost
4. **Rust toolchain** (to build `tm`)

## Step 1: Set up localhost SSH access

```bash
# Install sshd if not already present
sudo apt-get install -y openssh-server
sudo systemctl start sshd

# Generate an SSH key if you don't have one
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519 -N ""

# Authorize yourself to SSH to localhost
cat ~/.ssh/id_ed25519.pub >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys

# Test it works (should connect without a password prompt)
ssh -o StrictHostKeyChecking=no $(whoami)@localhost echo "SSH works"
```

## Step 2: Build the `tm` CLI

```bash
cd the-mule
cargo build --release
```

The binary is at `target/release/tm`. Optionally copy it to your PATH:

```bash
cp target/release/tm ~/.local/bin/
```

## Step 3: Build or pull a peer Docker image

You need at least one peer Docker image. You can use one of the reference peer
images from the `lib/` directory, or build your own (see the
[write-a-rust-test-app](docs/write-a-rust-test-app.md),
[write-a-go-test-app](docs/write-a-go-test-app.md), or
[write-a-python-test-app](docs/write-a-python-test-app.md) guides).

To build the Rust reference peer:

```bash
cd the-mule/lib/rust
docker build -t rust-mule:latest .
```

## Step 4: Run a smoke test

```bash
cd the-mule

# The driver needs RUST_LOG for its own file-based logging
RUST_LOG=info ./target/release/tm examples/smoke-test-1-rust-peer.yaml --verbose
```

### Using an external Redis instance

If you already have a Redis server running, skip the built-in container:

```bash
RUST_LOG=info tm examples/smoke-test-1-rust-peer.yaml --redis-url redis://localhost:6379
```

## What happens

The driver will:

1. Start a local Redis container on port `6399`
2. Enable Redis keyspace notifications (`CONFIG SET notify-keyspace-events K$`)
3. SSH to `localhost` and launch peer containers on the configured ports
4. Wait up to 60s for all peers to report `started` via Redis
5. Send `peer|<VLAD>|<multiaddr>` bootstrap commands to each peer (if configured)
6. Execute the timed command timeline (connect, push, pull, rotate-key, etc.)
7. Send `shutdown` to all peers once all commands have been sent
8. Wait up to 30s for all peers to report `stopped`
9. Clean up all containers

A log file is written to the current directory: `<test_name>-YYYY-MM-DD-HH-MM-SS.log`

### Console mode vs TUI mode

By default `tm` runs in **console mode**, printing status changes and commands
to stdout. Add `--tui` for a ratatui terminal UI with live peer status and
command timeline panels:

```bash
tm examples/smoke-test-5peer.yaml --tui
```

Press `q` or Ctrl+C to abort the test early (peers will be sent shutdown first).

## Step 5: Verify results

After the test:

```bash
# Check the log file for the full trace
cat smoke-test-*.log

# Verify all test containers are cleaned up
docker ps -a | grep tm-

# If any remain, clean them manually
docker rm -f $(docker ps -a --filter "name=tm-" -q)
```

## What success looks like

- **Console output**: Each peer reports `started`, commands are sent at the
  correct times, and the test ends with "all peers stopped -- test complete".
- **Log file**: Contains entries like:
  ```
  enabled Redis keyspace notifications (K$)
  all peers started successfully
  sent bootstrap: alice -> bob (...)
  [0.0s] sent to alice: connect
  [10.0s] sent to alice: push|bob|hello-from-alice
  [15.0s] sent to bob: pull
  all commands sent and all peers stopped -- test complete
  ```
- **No errors** in the log about failed SSH connections, missing containers,
  or Redis timeouts.

## Writing a test config

Test configs are YAML files. See [docs/test-schema.md](docs/test-schema.md)
for the full schema reference. Here is a minimal example:

```yaml
test_name: "my-test"

redis:
  port: 6399
  image: "redis:7-alpine"

hosts:
  - address: localhost
    ssh_user: user
    ssh_auth: agent
    base_port: 11984

peers:
  - name: alice
    image: "my-peer:latest"
    bootstrap: [bob]
  - name: bob
    image: "my-peer:latest"
    bootstrap: [alice]

commands:
  - { time: 0,  peer: alice, command: "connect" }
  - { time: 0,  peer: bob,   command: "connect" }
  - { time: 10, peer: alice, command: "push|bob|hello" }
  - { time: 15, peer: bob,   command: "pull" }

timeout:
  startup: 60
  shutdown: 30
```

### Multi-host setup

To run across multiple machines, add more entries to `hosts:` and ensure SSH
access is configured for each:

```yaml
hosts:
  - address: 192.168.1.10
    ssh_user: ubuntu
    ssh_auth: ~/.ssh/id_ed25519
    base_port: 11984
  - address: 192.168.1.11
    ssh_user: ubuntu
    ssh_auth: agent
    base_port: 11984
```

Peers are assigned round-robin across hosts. Docker images are automatically
exported, transferred via SCP, and loaded on remote hosts if not already present.

## Troubleshooting

- **"failed to connect to localhost:22"** — sshd is not running or `ssh_user` /
  `ssh_auth` is wrong. Test with: `ssh <user>@localhost echo ok`
- **"timeout waiting for peers to start"** — the Docker image was not built, or
  peers cannot reach Redis. Check container logs: `docker logs tm-<peer_name>`
- **Redis connection refused** — port 6399 may be in use. Change `redis.port`
  in the YAML, or use `--redis-url` to point to an existing instance.
- **Port conflicts** — if the configured ports are in use, change `base_port`
  in the YAML.
- **"failed to enable keyspace notifications"** — this is a warning, not fatal.
  It means the external Redis instance disallows `CONFIG SET`. The orchestrator
  will still work if notifications are already enabled on the server, but if
  they are not, peer status changes will not be detected. Ensure your Redis
  has `notify-keyspace-events` set to include at least `K$`.
