"""A real botpy echo bot pointed at MQS.

    pip install qq-botpy
    MQS_BASE_URL=http://127.0.0.1:5700 python examples/botpy_echo/bot.py

It replies to group @ messages and C2C messages with ``echo: <text>``.
"""

import os
import re
import sys

import mqs.compat.botpy  # noqa: F401  — must run before Client talks to QQ

import botpy
from botpy.message import C2CMessage, GroupMessage


class EchoClient(botpy.Client):
    async def on_ready(self) -> None:
        print(f"[echo] ready as {self.robot.name}", flush=True)

    async def on_group_at_message_create(self, message: GroupMessage) -> None:
        text = re.sub(r"<@![^>]+>", "", message.content or "").strip()
        print(f"[echo] group message: {text}", flush=True)
        await message.reply(content=f"echo: {text}")

    async def on_c2c_message_create(self, message: C2CMessage) -> None:
        text = re.sub(r"<@![^>]+>", "", message.content or "").strip()
        print(f"[echo] c2c message: {text}", flush=True)
        await message.reply(content=f"echo: {text}")


def main() -> None:
    app_id = os.environ.get("MQS_APPID", "10000")
    secret = os.environ.get("MQS_SECRET", "mqs-secret-10000")
    print(f"[echo] connecting to {os.environ.get('MQS_BASE_URL', 'http://127.0.0.1:5700')} as {app_id}", flush=True)
    client = EchoClient(intents=botpy.Intents(public_messages=True))
    client.run(appid=app_id, secret=secret)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
