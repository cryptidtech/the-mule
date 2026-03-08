# The Mule — Python Client Library

Python client library for peer applications running under The Mule test orchestrator.

## Installation

```bash
pip install -e lib/python/
```

## Usage

```python
import asyncio
import logging
from the_mule import MuleClientBuilder

async def main():
    client = await MuleClientBuilder().build()
    await client.send_status("started")

    async for command in client:
        logging.info(f"received: {command}")
        if command == "shutdown":
            await client.send_status("stopped")
            break

    await client.close()

asyncio.run(main())
```

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `REDIS_URL` | yes | Redis connection URL |
| `PEER_NAME` | yes | This peer's name |
| `LOG_LEVEL` | no | Python log level (e.g., `INFO`) |

## How it works

- **Commands**: the async iterator calls `BLPOP {peer}_command 0` (truly
  blocking, no polling). Because `redis.asyncio` is used, the `await` yields
  to the asyncio event loop so other coroutines run freely.
- **Logs**: a `logging.Handler` captures log records and forwards them to Redis
  via `LPUSH {peer}_log "level|message"` in a background thread.
- **Status**: `send_status()` calls `SET {peer}_status <value>`, which triggers
  a keyspace notification on the orchestrator side.

## API

- `MuleClientBuilder()` — reads env vars
- `.redis_url(url)` / `.peer_name(name)` — override
- `.build()` — connect, install log handler
- `MuleClient.send_status(status)` — push status to orchestrator
- `async for command in client:` — yields raw command strings
- `MuleClient.close()` — clean up
