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
description: strict 档令牌桶：连续回复第 3 次应 429，且带 retry_after
setup:
  settings:
    rate_mode: "strict"
    require_online: false
  bots:
    - app_id: "10001"
      name: MQS Offline Bot
      scopes: [group_message, c2c_message, media]
  users:
    - alias: alice
      name: 限流测试用户
  groups:
    - alias: room
      name: 限流测试群
      owner: alice
      members: [alice]
      bots: ["10001"]
steps:
  - send:
      as: alice
      to: room
      text: rate-ping
      at_bots: ["10001"]
  - http:
      bot: "10001"
      method: POST
      target: group
      group: room
      path: /v2/groups/{openid}/messages
      json:
        content: reply-1
        msg_id: "{{incoming_msg_id}}"
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: group
      group: room
      path: /v2/groups/{openid}/messages
      json:
        content: reply-2
        msg_id: "{{incoming_msg_id}}"
      expect_status: 200
  - http:
      bot: "10001"
      method: POST
      target: group
      group: room
      path: /v2/groups/{openid}/messages
      json:
        content: reply-3
        msg_id: "{{incoming_msg_id}}"
      expect_status: 429
      expect_code: 42900
"""

PASSIVE_WINDOW = """\
name: passive_window
description: 被动回复 5 分钟窗口：快进 6 分钟后回复应返回 40057
setup:
  settings:
    rate_mode: "off"
    require_online: false
    passive_window_ms: 300000
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

BUILTINS = {
    "smoke": SMOKE,
    "c2c": C2C,
    "rate_limit": RATE_LIMIT,
    "passive_window": PASSIVE_WINDOW,
    "duplicate_seq": DUPLICATE_SEQ,
}
