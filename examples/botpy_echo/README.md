# botpy echo bot

这是 v0.1 验收用的“被测 Bot”：一个用**真实 botpy SDK** 写的 echo bot，只把连接目标换成 MQS。

## 运行

```bash
# 1. 起沙箱（另开终端）
mqs serve --profile dev

# 2. 安装 botpy 并运行
pip install qq-botpy
MQS_BASE_URL=http://127.0.0.1:5700 python examples/botpy_echo/bot.py
# 日志出现 [echo] ready as MQS 测试 Bot 即成功
```

## 为什么需要 `mqs.compat.botpy`

botpy 把 REST 域名（`botpy.http.Route.DOMAIN`）和 access token 地址
（`https://bots.qq.com/app/getAppAccessToken`）都写死在代码里，没有 `base_url` 配置项。
`mqs.compat.botpy` 只做两件事：

1. `Route.SCHEME/DOMAIN` 指向 MQS；
2. 把 `Token.update_access_token` 换成请求 MQS 的 `POST /app/getAppAccessToken`。

网关地址不需要补——MQS 的 `GET /gateway/bot` 会返回基于请求 Host 的 `ws://...`。

## 验证

```bash
mqs script run smoke     # 需要本 echo bot 在线
```

已知差异与支持范围见 [docs/COMPAT.md](../../docs/COMPAT.md)。
