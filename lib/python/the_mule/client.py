import asyncio
import logging
import os
import queue
import threading

import redis as sync_redis
import redis.asyncio as aioredis

logger = logging.getLogger(__name__)

# Map Python log levels to protocol levels
_LEVEL_MAP = {
    logging.DEBUG: "debug",
    logging.INFO: "info",
    logging.WARNING: "warn",
    logging.ERROR: "error",
    logging.CRITICAL: "error",
}


class _RedisLogHandler(logging.Handler):
    """Logging handler that forwards log entries to Redis via a background thread."""

    def __init__(self, redis_url: str, log_key: str, peer_name: str):
        super().__init__()
        self._queue: queue.Queue[str | None] = queue.Queue(maxsize=4096)
        self._thread = threading.Thread(
            target=self._drain, args=(redis_url, log_key, peer_name), daemon=True
        )
        self._thread.start()

    def emit(self, record: logging.LogRecord) -> None:
        level = _LEVEL_MAP.get(record.levelno, "info")
        message = self.format(record)
        try:
            self._queue.put_nowait(f"{level}|{message}")
        except queue.Full:
            pass

    def _drain(self, redis_url: str, log_key: str, peer_name: str) -> None:
        client = sync_redis.Redis.from_url(redis_url)
        while True:
            entry = self._queue.get()
            if entry is None:
                break
            try:
                client.lpush(log_key, entry)
            except Exception:
                pass

    def close(self) -> None:
        self._queue.put(None)
        super().close()


class MuleClientBuilder:
    """Builder for constructing a MuleClient."""

    def __init__(self) -> None:
        self._redis_url: str | None = os.environ.get("REDIS_URL")
        self._peer_name: str | None = os.environ.get("PEER_NAME")
        log_level = os.environ.get("LOG_LEVEL", "INFO").upper()
        logging.basicConfig(level=getattr(logging, log_level, logging.INFO))

    def redis_url(self, url: str) -> "MuleClientBuilder":
        self._redis_url = url
        return self

    def peer_name(self, name: str) -> "MuleClientBuilder":
        self._peer_name = name
        return self

    async def build(self) -> "MuleClient":
        if not self._redis_url:
            raise ValueError("REDIS_URL not set")
        if not self._peer_name:
            raise ValueError("PEER_NAME not set")

        peer_name = self._peer_name
        redis_url = self._redis_url

        status_key = f"{peer_name}_status"
        command_key = f"{peer_name}_command"
        log_key = f"{peer_name}_log"

        # Install Redis log handler
        handler = _RedisLogHandler(redis_url, log_key, peer_name)
        logging.getLogger().addHandler(handler)

        # Create async Redis connection and eagerly verify connectivity
        conn = aioredis.Redis.from_url(
            redis_url, socket_connect_timeout=5
        )
        await conn.ping()

        return MuleClient(
            conn=conn,
            status_key=status_key,
            command_key=command_key,
            log_handler=handler,
        )


class MuleClient:
    """Client for communicating with The Mule orchestrator via Redis."""

    def __init__(
        self,
        conn: aioredis.Redis,
        status_key: str,
        command_key: str,
        log_handler: _RedisLogHandler,
    ) -> None:
        self._conn = conn
        self._status_key = status_key
        self._command_key = command_key
        self._log_handler = log_handler

    async def send_status(self, status: str) -> None:
        """Send a status update to the orchestrator."""
        await self._conn.set(self._status_key, status)

    def __aiter__(self) -> "MuleClient":
        return self

    async def __anext__(self) -> str:
        """Block until the next command is available, then return it as a raw string."""
        while True:
            result = await self._conn.blpop(self._command_key, timeout=1)
            if result is not None:
                _, value = result
                if isinstance(value, bytes):
                    return value.decode("utf-8")
                return str(value)
            await asyncio.sleep(0)

    async def close(self) -> None:
        """Close the Redis connection and log handler."""
        self._log_handler.close()
        await self._conn.aclose()
