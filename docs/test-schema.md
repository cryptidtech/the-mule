# Test Configuration YAML Schema

This document describes the full YAML schema for `tm` test configuration files.

## Top-level fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `test_name` | string | yes | Name of the test run (used in log filenames and TUI header) |
| `redis` | RedisConfig | yes | Local Redis container configuration |
| `hosts` | list of HostConfig | yes | Remote hosts to run peer containers on |
| `docker_image` | string | yes | Docker image name for peer containers |
| `base_port` | integer | yes | Starting UDP port for peer QUIC listeners |
| `peers` | list of PeerConfig | yes | Peer definitions (may be empty) |
| `commands` | list of TestCommand | yes | Timed command sequence |
| `timeout` | TimeoutConfig | no | Startup/shutdown timeout overrides |

## RedisConfig

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `port` | integer | yes | Host port to bind Redis to |
| `image` | string | yes | Docker image for Redis (e.g. `redis:7-alpine`) |

## HostConfig

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `address` | string | yes | Hostname or IP of the remote host |
| `ssh_user` | string | yes | SSH username |
| `ssh_auth` | string | yes | `"agent"` to use SSH agent, or path to private key (e.g. `~/.ssh/id_ed25519`) |

## PeerConfig

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique peer name (used as container name suffix and Redis key prefix) |
| `bootstrap` | list of string | no | Names of peers to bootstrap from after startup |
| `env` | map of string→string | no | Extra environment variables passed to the container |

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

## Peer assignment algorithm

Peers are assigned to hosts as follows:

1. Sort peers alphabetically by name
2. Assign round-robin across the `hosts` list
3. Each host gets a separate port counter starting at `base_port`
4. Each peer on a host gets the next available port

For example, with 5 peers (alice, bob, charlie, dave, eve), 2 hosts, and
`base_port: 11984`:

| Peer | Host | Port |
|------|------|------|
| alice | host-0 | 11984 |
| bob | host-1 | 11984 |
| charlie | host-0 | 11985 |
| dave | host-1 | 11985 |
| eve | host-0 | 11986 |

## Full example

```yaml
test_name: "smoke-test-5peer"

redis:
  port: 6399
  image: "redis:7-alpine"

hosts:
  - address: peer0
    ssh_user: user
    ssh_auth: agent

docker_image: "vlademlia-test-peer:latest"
base_port: 11984

peers:
  - name: alice
    bootstrap: [bob, charlie]
    env:
      RUST_LOG: info
  - name: bob
    bootstrap: [alice]
  - name: charlie
    bootstrap: [alice, bob]
  - name: dave
    bootstrap: [alice, charlie]
  - name: eve
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

timeout:
  startup: 60
  shutdown: 30
```
