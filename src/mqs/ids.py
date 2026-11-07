"""Deterministic identifiers and fake-but-plausible data generation."""

from __future__ import annotations

import hashlib
import random
import secrets
import string

ALPHABET = string.ascii_uppercase + string.digits

_ADJECTIVES = (
    "星 月 风 云 雨 雪 霜 雾 山 海 林 森 花 叶 光 夕 晨 夏 秋 冬 竹 兰 梅 松 鹤 鹿 鲸 猫 兔 熊 "
    "青 白 玄 黛 浅 深 远 近 微 小 大 长 短 轻 暖 冷 晴 阴 朝 暮"
).split()
_NOUNS = (
    "野 见 禾 满 川 岛 原 音 玲 汐 柠 糖 团 子 酱 喵 眠 梦 歌 语 书 画 舟 桥 灯 窗 巷 井 泉 溪 "
    "柚 栗 桃 杏 莓 橘 葵 葵 芽 苗 舟 遥 归 行 止 问 答 呼 吸"
).split()
_CITIES = ["上海", "北京", "广州", "深圳", "杭州", "成都", "武汉", "南京", "西安", "重庆", "苏州", "长沙"]
_BEHAVIORS = ["silent", "normal", "chatty", "spammer"]
_LOREM = [
    "在的，有什么可以帮你？",
    "收到，我看看。",
    "这个功能我测过了，没问题。",
    "稍等，我先查下日志。",
    "哈哈，确实。",
    "好耶。",
    "我先潜水了，有事 @ 我。",
    "今天群里好热闹。",
    "这个接口的限流有点紧。",
    "刚上线，来打个招呼。",
]


def base36(number: int, width: int = 0) -> str:
    chars = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    if number == 0:
        out = "0"
    else:
        out = ""
        while number:
            number, rem = divmod(number, 36)
            out = chars[rem] + out
    return out.rjust(width, "0")


def short_token(n: int = 26) -> str:
    return "".join(secrets.choice(ALPHABET) for _ in range(n))


def new_msg_id(prefix: str = "MQS_MSG") -> str:
    return f"{prefix}_{short_token(24)}"


def new_event_id() -> str:
    return f"MQS_EVT_{short_token(20)}"


def new_session_id() -> str:
    return short_token(32)


def openid_for(bot_app_id: str, kind: str, internal_id: str) -> str:
    """QQ openids are scoped per bot; derive them deterministically."""
    digest = hashlib.sha256(f"{bot_app_id}:{kind}:{internal_id}".encode()).hexdigest()
    return digest[:32].upper()


def union_openid_for(internal_id: str) -> str:
    return hashlib.sha256(f"union:{internal_id}".encode()).hexdigest()[:32].upper()


def media_hash(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def avatar_svg(seed: str, label: str | None = None, kind: str = "user") -> str:
    """Deterministic local avatar: background color encodes the entity kind."""
    h = int(hashlib.sha256(seed.encode()).hexdigest()[:8], 16)
    palettes = {
        "bot": (262, 12),     # violet → pink
        "user": (194, 18),    # blue → cyan
        "npc": (150, 14),     # green
        "group": (28, 16),    # amber
        "guild": (28, 16),
    }
    base, spread = palettes.get(kind, palettes["user"])
    hue = (base + (h % spread) - spread // 2) % 360
    hue2 = (hue + 28) % 360
    bg1 = f"hsl({hue},72%,62%)"
    bg2 = f"hsl({hue2},68%,46%)"
    text = (label or seed).strip()
    initial = text[0].upper() if text else "?"
    if not initial.isalnum():
        initial = text[:2] if text else "?"
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96" viewBox="0 0 96 96">'
        f'<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">'
        f'<stop offset="0" stop-color="{bg1}"/><stop offset="1" stop-color="{bg2}"/></linearGradient></defs>'
        f'<rect width="96" height="96" rx="24" fill="url(#g)"/>'
        f'<text x="48" y="62" font-size="42" font-family="PingFang SC, Microsoft YaHei, sans-serif" ' 
        f'font-weight="700" text-anchor="middle" fill="#fff">{initial}</text></svg>'
    )


class FakeData:
    """Seeded generator: same seed + same call order => same world."""

    def __init__(self, seed: int = 42) -> None:
        self.seed = seed
        self.rng = random.Random(seed)
        self._used_names: set[str] = set()

    def reseed(self, seed: int) -> None:
        self.seed = seed
        self.rng = random.Random(seed)
        self._used_names.clear()

    def nickname(self) -> str:
        for _ in range(200):
            name = self.rng.choice(_ADJECTIVES) + self.rng.choice(_NOUNS)
            if name not in self._used_names:
                self._used_names.add(name)
                return name
        # fall back to a suffix when the pool is exhausted
        name = f"{self.rng.choice(_ADJECTIVES)}{self.rng.choice(_NOUNS)}{self.rng.randint(0, 999)}"
        self._used_names.add(name)
        return name

    def city(self) -> str:
        return self.rng.choice(_CITIES)

    def gender(self) -> int:
        return self.rng.choice([0, 1, 2])

    def age(self) -> int:
        return self.rng.randint(14, 40)

    def behavior(self) -> str:
        return self.rng.choices(_BEHAVIORS, weights=[3, 5, 3, 1])[0]

    def sentence(self) -> str:
        return self.rng.choice(_LOREM)

    def scopes(self) -> list[str]:
        pool = [
            ["group_message"],
            ["group_message", "c2c_message"],
            ["group_message", "c2c_message", "media"],
            ["group_message", "media"],
            ["group_message", "member"],
            ["group_message", "c2c_message", "media", "member"],
        ]
        return list(self.rng.choice(pool))

    def group_name(self) -> str:
        prefixes = ["摸鱼", "测试", "深夜", "开发", "临时", "内测", "文档", "摸虾"]
        suffixes = ["小组", "交流群", "指挥部", "后援会", "水群", "互助会"]
        return f"{self.rng.choice(prefixes)}{self.rng.choice(suffixes)}{self.rng.randint(1, 99)}"

    def bot_name(self) -> str:
        return f"{self.rng.choice(['月', '星', '云', '风', '雪'])}见BOT{self.rng.randint(1, 99)}"
