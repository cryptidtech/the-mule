#!/usr/bin/env python3
import asyncio
import logging
import sys

from the_mule import MuleClientBuilder


async def main() -> None:
    client = await MuleClientBuilder().build()
    await client.send_status("started")

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
