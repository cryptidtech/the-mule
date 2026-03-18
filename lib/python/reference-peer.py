#!/usr/bin/env python3
import asyncio
import logging
import os
import socket
import sys

from the_mule import MuleClientBuilder


def local_ip() -> str:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("8.8.8.8", 80))
        return s.getsockname()[0]
    except Exception:
        return "0.0.0.0"
    finally:
        s.close()


def extract_port(listen_addr: str) -> str:
    parts = listen_addr.split("/")
    for i, p in enumerate(parts):
        if p == "udp" and i + 1 < len(parts):
            return parts[i + 1]
    return "0"


async def main() -> None:
    client = await MuleClientBuilder().build()
    ip = local_ip()
    port = extract_port(os.environ.get("LISTEN_ADDR", ""))
    await client.send_status(f"started|/ip4/{ip}/udp/{port}/quic-v1")

    async for command in client:
        logging.info(f"received command: {command}")
        if command == "shutdown":
            await client.send_status("stopped")
            break
        elif command.startswith("restart|"):
            delay = command.split("|", 1)[1]
            await client.send_status("restarting")
            with open("/tmp/delay", "w") as f:
                f.write(delay)
            sys.exit(42)


if __name__ == "__main__":
    asyncio.run(main())
