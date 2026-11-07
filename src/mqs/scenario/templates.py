"""Built-in scenario templates (YAML strings, also written to scenarios/)."""

from __future__ import annotations

SMOKE = """\
name: smoke
description: 群聊冒烟：用户发言 -> Bot 收到事件 -> Bot 被动回复（需要被测 Bot 已连接网关并实现 echo）
setup:
  settings:
    rate_mode: "off"
    require_online: true
  bots:
    - app_id: "10000"
      name: MQS Echo Bot
      scopes: [group_message, c2c_message, media, member]
  users:
    - alias: alice
      name: 测试用户甲
  groups:
    - alias: room
      name: 冒烟测试群
      owner: alice
      members: [alice]
      bots: ["10000"]
steps:
  - send:
      as: alice
      to: room
      text: ping
      at_bots: ["10000"]
  - expect_event:
      type: GROUP_AT_MESSAGE_CREATE
      conversation: "group:{{room}}"
      content_contains: ping
      timeout: 3s
  - expect_message:
      conversation: "group:{{room}}"
      direction: out
      content_contains: ping
      timeout: 15s
"""

C2C = """\
name: c2c
description: 单聊冒烟：用户私聊 Bot -> Bot 回复（需要被测 Bot 在线）
setup:
  settings:
    rate_mode: "off"
    require_online: true
  bots:
    - app_id: "10000"
      name: MQS Echo Bot
      scopes: [group_message, c2c_message, media]
  users:
    - alias: alice
      name: 私聊测试用户
steps:
  - send:
      as: alice
      to: "dm:10000:{{alice}}"
      text: hello-c2c
  - expect_event:
      type: C2C_MESSAGE_CREATE
      conversation: "dm:10000:{{alice}}"
      content_contains: hello-c2c
      timeout: 3s
  - expect_message:
      conversation: "dm:10000:{{alice}}"
      direction: out
      content_contains: hello-c2c
      timeout: 15s
"""

RATE_LIMIT = """\
name: rate_limit
description: 未认证个人 · 单聊主动消息 5/qps：第 6 条应 429（官方频率政策）
setup:
  settings:
    rate_mode: "unverified"
    require_online: false
  bots:
    - app_id: "10001"
      name: MQS Offline Bot
      scopes: [group_message, c2c_message, media]
  users:
    - alias: alice
      name: 限流测试用户
steps:
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json: {content: active-1}
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json: {content: active-2}
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json: {content: active-3}
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json: {content: active-4}
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json: {content: active-5}
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json: {content: active-6}
      expect_status: 429
      expect_code: 42900
"""

C2C_PASSIVE = """\
name: c2c_passive
description: 单聊被动回复窗口 60 分钟 / 每条 4 次（官方频率与时效规则）
setup:
  settings:
    rate_mode: "off"
    require_online: false
  bots:
    - app_id: "10001"
      name: MQS Offline Bot
      scopes: [group_message, c2c_message, media]
  users:
    - alias: alice
      name: 私聊窗口测试用户
steps:
  - send:
      as: alice
      to: "dm:10001:{{alice}}"
      text: c2c-window
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json:
        content: reply-1
        msg_id: "{{incoming_msg_id}}"
        msg_seq: 1
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json:
        content: reply-2
        msg_id: "{{incoming_msg_id}}"
        msg_seq: 2
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json:
        content: reply-3
        msg_id: "{{incoming_msg_id}}"
        msg_seq: 3
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json:
        content: reply-4
        msg_id: "{{incoming_msg_id}}"
        msg_seq: 4
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json:
        content: reply-5
        msg_id: "{{incoming_msg_id}}"
      expect_status: 400
      expect_code: 40057
  - advance: 61m
  - http:
      bot: "10001"
      method: POST
      target: user
      user: alice
      path: /v2/users/{openid}/messages
      json:
        content: too-late
        msg_id: "{{incoming_msg_id}}"
        msg_seq: 9
      expect_status: 400
      expect_code: 40057
"""

PASSIVE_WINDOW = """\
name: passive_window
description: 被动回复 5 分钟窗口：快进 6 分钟后回复应返回 40057
setup:
  settings:
    rate_mode: "off"
    require_online: false
    passive_window_group_ms: 300000
    passive_reply_limit_group: 5
  bots:
    - app_id: "10001"
      name: MQS Offline Bot
      scopes: [group_message, c2c_message, media]
  users:
    - alias: alice
      name: 窗口测试用户
  groups:
    - alias: room
      name: 窗口测试群
      owner: alice
      members: [alice]
      bots: ["10001"]
steps:
  - send:
      as: alice
      to: room
      text: window-ping
      at_bots: ["10001"]
  - advance: 6m
  - http:
      bot: "10001"
      method: POST
      target: group
      group: room
      path: /v2/groups/{openid}/messages
      json:
        content: too-late
        msg_id: "{{incoming_msg_id}}"
      expect_status: 400
      expect_code: 40057
"""

DUPLICATE_SEQ = """\
name: duplicate_seq
description: 相同 msg_id + msg_seq 重复发送应返回 40054
setup:
  settings:
    rate_mode: "off"
    require_online: false
  bots:
    - app_id: "10001"
      name: MQS Offline Bot
      scopes: [group_message, c2c_message, media]
  users:
    - alias: alice
      name: 去重测试用户
  groups:
    - alias: room
      name: 去重测试群
      owner: alice
      members: [alice]
      bots: ["10001"]
steps:
  - send:
      as: alice
      to: room
      text: dup-ping
      at_bots: ["10001"]
  - http:
      bot: "10001"
      method: POST
      target: group
      group: room
      path: /v2/groups/{openid}/messages
      json:
        content: first
        msg_id: "{{incoming_msg_id}}"
        msg_seq: 1
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: group
      group: room
      path: /v2/groups/{openid}/messages
      json:
        content: second
        msg_id: "{{incoming_msg_id}}"
        msg_seq: 1
      expect_status: 400
      expect_code: 40054
"""

QUOTE = """\
name: quote
description: 回复引用：第二条消息带 message_reference，事件 payload 中可见
setup:
  settings:
    rate_mode: "off"
  users:
    - alias: alice
      name: 引用测试用户
  groups:
    - alias: room
      name: 引用测试群
      owner: alice
      members: [alice]
      bots: ["10000"]
steps:
  - send:
      as: alice
      to: room
      text: 第一条
      at_bots: ["10000"]
  - send:
      as: alice
      to: room
      text: 引用第一条
      at_bots: ["10000"]
      reply_to: "{{last_message_id}}"
  - expect_event:
      type: GROUP_AT_MESSAGE_CREATE
      conversation: "group:{{room}}"
      content_contains: 引用第一条
      payload_contains: message_reference
      timeout: 3s
"""

PERMISSION = """\
name: permission
description: 权限不足：没有 group_message 能力的 Bot 发群消息应 403/40040
setup:
  settings:
    rate_mode: "off"
    require_online: false
  bots:
    - app_id: "10002"
      name: 只读 Bot
      scopes: [c2c_message]
  users:
    - alias: alice
      name: 权限测试用户
  groups:
    - alias: room
      name: 权限测试群
      owner: alice
      members: [alice]
      bots: ["10002"]
steps:
  - http:
      bot: "10002"
      method: POST
      target: group
      group: room
      path: /v2/groups/{openid}/messages
      json: {content: should-fail}
      expect_status: 403
      expect_code: 40040
"""

GROUP_MULTI = """\
name: group_multi
description: 群聊多用户：三个用户先后发言，Bot 侧收到三条 GROUP_AT_MESSAGE_CREATE
setup:
  settings:
    rate_mode: "off"
  users:
    - alias: alice
      name: 多用户甲
    - alias: bob
      name: 多用户乙
    - alias: carol
      name: 多用户丙
  groups:
    - alias: room
      name: 多用户测试群
      owner: alice
      members: [alice, bob, carol]
      bots: ["10000"]
steps:
  - send:
      as: alice
      to: room
      text: 我是甲
      at_bots: ["10000"]
  - expect_event:
      type: GROUP_AT_MESSAGE_CREATE
      conversation: "group:{{room}}"
      content_contains: 我是甲
      timeout: 3s
  - send:
      as: bob
      to: room
      text: 我是乙
      at_bots: ["10000"]
  - expect_event:
      type: GROUP_AT_MESSAGE_CREATE
      conversation: "group:{{room}}"
      content_contains: 我是乙
      timeout: 3s
  - send:
      as: carol
      to: room
      text: 我是丙
      at_bots: ["10000"]
  - expect_event:
      type: GROUP_AT_MESSAGE_CREATE
      conversation: "group:{{room}}"
      content_contains: 我是丙
      timeout: 3s
"""

MEDIA = """\
name: media
description: 媒体上传：multipart 上传 1x1 PNG -> file_info -> msg_type=7 发送
setup:
  settings:
    rate_mode: "off"
    require_online: false
  bots:
    - app_id: "10000"
      name: QSandbox 测试 Bot
      scopes: [group_message, c2c_message, media]
  users:
    - alias: alice
      name: 媒体测试用户
  groups:
    - alias: room
      name: 媒体测试群
      owner: alice
      members: [alice]
      bots: ["10000"]
steps:
  - send:
      as: alice
      to: room
      text: media please
      at_bots: ["10000"]
  - http:
      bot: "10000"
      method: POST
      target: group
      group: room
      path: /v2/groups/{openid}/files
      multipart:
        file:
          filename: pixel.png
          mime: image/png
          base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lK3kAAAAAElFTkSuQmCC"
        fields:
          file_type: "1"
      expect_status: 200
      capture:
        file_info: file_info
  - http:
      bot: "10000"
      method: POST
      target: group
      group: room
      path: /v2/groups/{openid}/messages
      json:
        content: ""
        msg_type: 7
        media:
          file_info: "{{file_info}}"
        msg_id: "{{incoming_msg_id}}"
      expect_status: 200
  - expect_message:
      conversation: "group:{{room}}"
      direction: out
      timeout: 3s
"""

BUILTINS = {
    "smoke": SMOKE,
    "quote": QUOTE,
    "permission": PERMISSION,
    "group_multi": GROUP_MULTI,
    "media": MEDIA,
    "c2c": C2C,
    "rate_limit": RATE_LIMIT,
    "c2c_passive": C2C_PASSIVE,
    "passive_window": PASSIVE_WINDOW,
    "duplicate_seq": DUPLICATE_SEQ,
}
