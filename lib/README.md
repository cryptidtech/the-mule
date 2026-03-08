# The Mule Client Libraries

Client libraries for peer applications to communicate with The Mule orchestrator via Redis.

## Protocol

All communication between the orchestrator and peer containers happens through Redis. Commands and logs use Redis lists (LPUSH/BLPOP). Status updates use a Redis string (SET/GET).

### Redis Key Naming

For a peer named `alice`:

| Key | Direction | Description |
|-----|-----------|-------------|
| `alice_command` | orchestrator -> peer | Commands for the peer to execute |
| `alice_status` | peer -> orchestrator | Status updates from the peer |
| `alice_log` | peer -> orchestrator | Log entries from the peer |

### Command Format

Commands are pipe-delimited strings pushed to `{peer}_command`:

| Command | Format | Description |
|---------|--------|-------------|
| connect | `connect` | Join the DHT network |
| disconnect | `disconnect` | Leave the DHT network |
| shutdown | `shutdown` | Shut down the peer process |
| restart | `restart\|<delay_secs>` | Restart with a delay |
| push | `push\|<peer>\|<message>` | Push a message to another peer |
| pull | `pull` | Pull pending messages |
| rotate-key | `rotate-key` | Rotate the peer's signing key |
| track | `track\|<peer>` | Start tracking another peer's VLAD |
| peer | `peer\|<vlad>\|<multiaddr>` | Add a bootstrap peer |

### Status Format

Status strings set on `{peer}_status` via Redis SET:
- `started` or `started|<vlad>|<multiaddr>` — peer is ready
- `connecting`, `connected`, `disconnecting`, `disconnected` — network states
- `restarting` — about to restart
- `stopped` — peer has shut down

### Log Format

Log entries pushed to `{peer}_log` via Redis LPUSH as `"level|message"`:
- Levels: `debug`, `info`, `warn`, `error`
- Example: `info|connected to 3 peers`

### Environment Variables

All client libraries read configuration from environment variables set by the orchestrator:

| Variable | Description |
|----------|-------------|
| `REDIS_URL` | Redis connection URL (e.g., `redis://192.168.1.10:6399`) |
| `PEER_NAME` | This peer's name (e.g., `alice`) |
| `LISTEN_ADDR` | Multiaddr to listen on (e.g., `/ip4/0.0.0.0/udp/11984/quic-v1`) |
| `RUST_LOG` / `LOG_LEVEL` | Log level filter |

### Restart Protocol

To support restarts with a delay:

1. Peer receives `restart|<delay>` command
2. Peer sends `restarting` status
3. Peer writes delay to `/tmp/delay`
4. Peer exits with code 42
5. `entrypoint.sh` detects exit code 42, reads delay, sleeps, and re-runs the binary

## Libraries

- **[Rust](rust/README.md)** — `the_mule` crate with async Stream-based API
- **[Python](python/README.md)** — `the_mule` package with async iterator API
- **[Go](go/README.md)** — `the_mule` package with channel-based API
