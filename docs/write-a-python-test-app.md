# Write a Python Test App

This guide walks through building a peer application in Python that runs under
The Mule test orchestrator.

## Prerequisites

- Python 3.10+
- Docker (for building the container image)
- A working `tm` setup (see [QUICKSTART.md](../QUICKSTART.md))

## 1. Install the client library

The client library lives in `lib/python/` of The Mule repository:

```bash
pip install -e path/to/the-mule/lib/python/
```

Or copy the `the_mule/` package into your project.

## 2. Write the peer application

The client library handles Redis connection, log forwarding via a background
thread, and async command iteration. Your code needs to:

1. Build the client
2. Send a `"started"` status (with optional VLAD and multiaddr)
3. Iterate over incoming commands
4. Send a `"stopped"` status before exiting

```python
#!/usr/bin/env python3
import asyncio
import logging
import sys

from the_mule import MuleClientBuilder

async def main() -> None:
    client = await MuleClientBuilder().build()

    # Tell the orchestrator we are ready.
    # Use "started|<vlad>|<multiaddr>" if your app has identity/network info.
    await client.send_status("started")

    # Process commands from the orchestrator.
    # MuleClient is an async iterator that yields raw command strings.
    async for command in client:
        logging.info(f"received: {command}")

        if command == "connect":
            logging.info("connecting to network...")
            # ... your connect logic ...
            await client.send_status("connected")

        elif command == "disconnect":
            logging.info("disconnecting...")
            # ... your disconnect logic ...
            await client.send_status("disconnected")

        elif command.startswith("push|"):
            parts = command.split("|", 2)
            peer, message = parts[1], parts[2]
            logging.info(f"push to {peer}: {message}")
            # ... your push logic ...

        elif command == "pull":
            logging.info("pulling messages...")
            # ... your pull logic ...

        elif command == "rotate-key":
            logging.info("rotating key...")
            # ... your key rotation logic ...

        elif command.startswith("track|"):
            peer = command.split("|", 1)[1]
            logging.info(f"tracking {peer}")
            # ... your tracking logic ...

        elif command.startswith("peer|"):
            parts = command.split("|", 2)
            vlad, multiaddr = parts[1], parts[2]
            logging.info(f"adding bootstrap peer: {vlad} at {multiaddr}")
            # ... add peer to your routing table ...

        elif command.startswith("restart|"):
            delay = command.split("|", 1)[1]
            await client.send_status("restarting")
            with open("/tmp/delay", "w") as f:
                f.write(delay)
            sys.exit(42)

        elif command == "shutdown":
            await client.send_status("stopped")
            break

    await client.close()

asyncio.run(main())
```

### How commands are received

The `MuleClient` async iterator calls `BLPOP {peer}_command 0` on each
iteration — this blocks on the Redis server until a command is available, then
returns it as a string. Because it uses `redis.asyncio`, the `await` yields
control to the asyncio event loop so your other coroutines (status updates, log
handling, etc.) continue to run. There is no polling.

### How logs are forwarded

The client installs a `logging.Handler` that captures log records and forwards
them to Redis via `LPUSH {peer}_log "level|message"` in a background thread.
Use `logging.info()`, `logging.error()`, etc. as normal and they appear in the
orchestrator's log output and TUI automatically.

## 3. Create a Dockerfile

```dockerfile
FROM python:3.12-slim

WORKDIR /app
COPY the_mule/ ./the_mule/
COPY my_peer.py .
RUN pip install redis

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
```

### entrypoint.sh

The entrypoint script supports the restart protocol (exit code 42):

```bash
#!/bin/bash
set -e
while true; do
    python3 /app/my_peer.py "$@" && break
    EXIT_CODE=$?
    if [ "$EXIT_CODE" -eq 42 ] && [ -f /tmp/delay ]; then
        DELAY=$(cat /tmp/delay)
        rm -f /tmp/delay
        echo "restarting in ${DELAY}s..."
        sleep "$DELAY"
    else
        exit $EXIT_CODE
    fi
done
```

## 4. Build the image

```bash
docker build -t my-python-peer:latest .
```

## 5. Write a test config

```yaml
# my-test.yaml
name: "my-python-peer-test"

timeout:
  startup: 60
  shutdown: 30

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
    image: "my-python-peer:latest"
    bootstrap: [bob]
    environment:
      - LOG_LEVEL=INFO
  - name: bob
    image: "my-python-peer:latest"
    bootstrap: [alice]
    environment:
      - LOG_LEVEL=INFO

commands:
  - { time: 0,  peer: alice, command: "connect" }
  - { time: 0,  peer: bob,   command: "connect" }
  - { time: 10, peer: alice, command: "push|bob|hello" }
  - { time: 15, peer: bob,   command: "pull" }
```

## 6. Run the test

```bash
RUST_LOG=info tm my-test.yaml --verbose
```

## Environment variables

The orchestrator sets these environment variables on your container:

| Variable | Description |
|----------|-------------|
| `REDIS_URL` | Redis connection URL (e.g., `redis://192.168.1.10:6399`) |
| `PEER_NAME` | This peer's name (e.g., `alice`) |
| `LISTEN_ADDR` | Multiaddr to listen on (e.g., `/ip4/0.0.0.0/udp/11984/quic-v1`) |
| `LOG_LEVEL` | Python log level (from peer config `environment`) |

## API reference

| Item | Description |
|------|-------------|
| `MuleClientBuilder()` | Create builder, reads `REDIS_URL`, `PEER_NAME`, `LOG_LEVEL` from env |
| `.redis_url(url)` / `.peer_name(name)` | Override env var values |
| `await .build()` | Connect to Redis, install log handler |
| `await client.send_status(status)` | SET the status key (triggers orchestrator notification) |
| `async for command in client:` | Yields raw command strings via `BLPOP` |
| `await client.close()` | Close Redis connection and log handler |
