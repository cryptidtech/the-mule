# Test Configuration YAML Schema

This document describes the full YAML schema for `tm` test configuration files.

## Top-level fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Name of the test run (used in log filenames and TUI header) |
| `timeout` | TimeoutConfig | no | Startup/shutdown timeout overrides |
| `redis` | RedisConfig | yes | Local Redis container configuration |
| `images` | list of string | no | Docker images to pre-pull locally before the test starts |
| `remove_images` | boolean | no | If `true`, remove images listed in `images` from all hosts after the test completes (default: `false`) |
| `peer_environment` | map or list | no | Environment variables applied to **all** peers. Peer-level `environment` values override these. Supports map (`KEY: VALUE`) or list (`- KEY=VALUE`) syntax. |
| `hosts` | list of HostConfig | yes | Remote hosts to run peer containers on |
| `peers` | list of PeerConfig | yes | Peer definitions (may be empty) |
| `commands` | list of TestCommand | yes | Timed command sequence |
| `log_level` | string | no | Log verbosity: "error", "warn", "info", "debug", or "trace" (default: "info") |

## RedisConfig

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `port` | integer | yes | Host port to bind Redis to |
| `image` | string | yes | Docker image for Redis (e.g. `redis:7-alpine`) |

## HostConfig

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `address` | string | yes | Hostname or IP of the remote host |
| `name` | string | no | Human-readable display name for the host (defaults to `address`) |
| `ssh_user` | string | yes | SSH username |
| `ssh_auth` | string | yes | `"agent"` to use SSH agent, or path to private key (e.g. `~/.ssh/id_ed25519`) |
| `base_port` | integer | yes | Starting UDP port for peer QUIC listeners on this host |
| `tags` | list of string | no | Tags for peer-to-host matching (see `runs_on` below) |

## PeerConfig

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique peer name (used as container name suffix and Redis key prefix) |
| `image` | string | yes | Docker image for this peer's container |
| `bootstrap` | list of string | no | Names of peers to bootstrap from after startup |
| `environment` | map of string->string | no | Extra environment variables passed to the container (overrides `peer_environment`) |
| `runs_on` | string or list of string | no | Host tag(s) required for this peer. Accepts a single string (`"gpu"`) or a list (`["gpu", "fast"]`). Empty means any host. |

## TestCommand

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `time` | integer | yes | Seconds after test start to send this command |
| `peer` | string | yes | Target peer name |
| `command` | string | yes | Command string (see known commands below) |

### Known commands

| Command | Description |
|---------|-------------|
| `connect` | Join the DHT network |
| `disconnect` | Leave the DHT network |
| `shutdown` | Shut down the peer process |
| `restart\|<delay>` | Restart with a delay in seconds |
| `push\|<peer>\|<message>` | Push a message to another peer |
| `pull` | Pull pending messages |
| `rotate-key` | Rotate the peer's signing key |
| `track\|<peer>` | Start tracking another peer's VLAD |
| `peer\|<vlad>\|<multiaddr>` | Add a bootstrap peer (sent automatically) |

## TimeoutConfig

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `startup` | integer | no | 60 | Seconds to wait for all peers to report "started" |
| `shutdown` | integer | no | 30 | Seconds to wait for all peers to report "stopped" |

If the `timeout` section is omitted entirely, both default values apply.
If only one field is specified, the other uses its default.

## Environment variable layering

Environment variables are applied in this order (later wins):

1. **`peer_environment`** (top-level) — global defaults for all peers
2. **`environment`** (per-peer) — peer-specific overrides
3. **System variables** — always set by `tm`, cannot be overridden:
   - `REDIS_URL` — Redis connection URL
   - `PEER_NAME` — peer's name
   - `LISTEN_ADDR` — multiaddr for the peer's QUIC listener
   - `HOST_NAME` — display name of the host running this peer

## Peer assignment algorithm

Peers are assigned to hosts as follows:

1. Sort peers alphabetically by name
2. Group peers by their sorted `runs_on` tags (empty = universal group)
3. For each group, find hosts whose `tags` contain **all** required tags (empty `runs_on` matches all hosts)
4. Error if no host matches a group's required tags
5. Round-robin peers within each group across their matching hosts
6. Port counters are shared across all groups (a host used by multiple groups accumulates ports correctly)
7. Final assignments are sorted by peer name for deterministic output

For example, with 5 peers (alice, bob, charlie, dave, eve), 2 hosts (host-0
with `base_port: 11984`, host-1 with `base_port: 11984`), no tags:

| Peer | Host | Port |
|------|------|------|
| alice | host-0 | 11984 |
| bob | host-1 | 11984 |
| charlie | host-0 | 11985 |
| dave | host-1 | 11985 |
| eve | host-0 | 11986 |

With tags — peers with `runs_on: [gpu]` are assigned only to hosts tagged `gpu`.

## CLI flags

| Flag | Description |
|------|-------------|
| `--tui` | Enable the ratatui TUI interface (default: console mode) |
| `--redis-url <URL>` | Use an external Redis instance instead of starting one |
| `--reset-hosts` | Connect to all hosts, remove Docker images listed in `images`, then exit |
| `--reset-hosts-all` | Connect to all hosts, run `docker system prune -af`, then exit |
| `--version` | Print version and exit |

`--reset-hosts` and `--reset-hosts-all` are standalone operations — they do not start Redis, distribute images, or run peers.

## Full example

```yaml
name: "smoke-test-5peer"

log_level: info

timeout:
  startup: 60
  shutdown: 30

redis:
  port: 6399
  image: "redis:7-alpine"

images:
  - "ghcr.io/cryptidtech/rust-mule:latest"
  - "ghcr.io/cryptidtech/python-mule:latest"
  - "ghcr.io/cryptidtech/go-mule:latest"

remove_images: false

peer_environment:
  LOG_LEVEL: info

hosts:
  - address: gpu-host-1
    name: "GPU Box 1"
    ssh_user: user
    ssh_auth: agent
    base_port: 11984
    tags: [gpu, fast]
  - address: cpu-host-1
    ssh_user: user
    ssh_auth: agent
    base_port: 11984
    tags: [cpu]

peers:
  - name: alice
    image: "ghcr.io/cryptidtech/rust-mule:latest"
    bootstrap: [bob, charlie]
    runs_on: gpu
    environment:
      RUST_LOG: debug
  - name: bob
    image: "ghcr.io/cryptidtech/python-mule:latest"
    bootstrap: [alice]
    runs_on: [cpu]
  - name: charlie
    image: "ghcr.io/cryptidtech/go-mule:latest"
    bootstrap: [alice, bob]
  - name: dave
    image: "ghcr.io/cryptidtech/rust-mule:latest"
    bootstrap: [alice, charlie]
  - name: eve
    image: "ghcr.io/cryptidtech/python-mule:latest"
    bootstrap: [bob, dave]

commands:
  - { time: 0,  peer: alice,   command: "connect" }
  - { time: 0,  peer: bob,     command: "connect" }
  - { time: 0,  peer: charlie, command: "connect" }
  - { time: 0,  peer: dave,    command: "connect" }
  - { time: 0,  peer: eve,     command: "connect" }
  - { time: 10, peer: alice,   command: "push|bob|hello-from-alice" }
  - { time: 15, peer: bob,     command: "pull" }
  - { time: 20, peer: charlie, command: "rotate-key" }
  - { time: 25, peer: eve,     command: "track|alice" }
  - { time: 30, peer: dave,    command: "disconnect" }
  - { time: 35, peer: dave,    command: "restart|5" }
  - { time: 55, peer: alice,   command: "shutdown" }
  - { time: 55, peer: bob,     command: "shutdown" }
  - { time: 55, peer: charlie, command: "shutdown" }
  - { time: 55, peer: dave,    command: "shutdown" }
  - { time: 55, peer: eve,     command: "shutdown" }
```
