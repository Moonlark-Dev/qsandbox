//! Message semantics: delivery, openid scoping, passive window, dedupe, recall.

mod common;

use common::{next_json, Test};
use serde_json::{json, Value};

async fn last_inbound(test: &Test, conversation_id: &str) -> Value {
    let messages = test
        .world()
        .list_messages(conversation_id, 50, None)
        .unwrap();
    messages
        .into_iter()
        .rev()
        .find(|message| message["direction"] == "in")
        .expect("no inbound message")
}

#[tokio::test]
async fn user_message_reaches_bot_with_mention() {
    let test = Test::start().await;
    let (mut ws, _ready, _token) = test.connect_gateway("10000", 1 << 25).await;
    test.send_as_user("ping", Some(vec!["10000"])).await;
    let frame = next_json(&mut ws).await;
    assert_eq!(frame["t"], "GROUP_MESSAGE_CREATE");
    let payload = &frame["d"];
    assert_eq!(payload["content"], "ping");
    assert_eq!(payload["group_id"], test.group_id().await);
    assert_eq!(payload["author"]["member_openid"], payload["author"]["id"]);
    assert_eq!(payload["group_openid"].as_str().unwrap().len(), 32);
    let self_mention = payload["mentions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|mention| mention["bot"] == true)
        .expect("self mention");
    assert_eq!(self_mention["is_you"], true);
    ws.close(None).await.ok();
}

#[tokio::test]
async fn explicit_at_bots_are_recorded_for_the_chat_view() {
    let test = Test::start().await;
    let group = test.group_id().await;
    let result = test.send_as_user("hello", Some(vec!["10000"])).await;
    assert_eq!(result["message"]["content"], "hello");
    assert_eq!(result["message"]["at_bots"], json!(["10000"]));

    let conversation = format!("group:{group}");
    let inbound = last_inbound(&test, &conversation).await;
    assert_eq!(inbound["at_bots"], json!(["10000"]));
    let conversations = test.conversations().await;
    let group_conv = conversations
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == conversation)
        .unwrap();
    assert_eq!(group_conv["last_message"]["at_bots"], json!(["10000"]));

    let plain = test.send_as_user("plain group text", Some(vec![])).await;
    assert!(plain["message"].get("at_bots").is_none());
}

#[tokio::test]
async fn openids_are_scoped_per_bot() {
    let test = Test::start().await;
    test.ctl_ok(
        "POST",
        "/_mqs/bots",
        Some(json!({"app_id": "10001", "name": "Second Bot"})),
    )
    .await;
    let group = test.group_id().await;
    test.ctl_ok("POST", &format!("/_mqs/groups/{group}/bots/10001"), None)
        .await;
    test.send_as_user("both of you", Some(vec!["10000", "10001"]))
        .await;
    let events = test.world().list_events(0, 100, None).unwrap();
    let mut by_bot = std::collections::HashMap::new();
    for event in events {
        if event["type"] == "GROUP_MESSAGE_CREATE" {
            by_bot.insert(
                event["target_bot_id"].as_str().unwrap().to_string(),
                event["payload"].clone(),
            );
        }
    }
    assert_eq!(by_bot.len(), 2);
    assert_ne!(
        by_bot["10000"]["group_openid"],
        by_bot["10001"]["group_openid"]
    );
    assert_ne!(
        by_bot["10000"]["author"]["member_openid"],
        by_bot["10001"]["author"]["member_openid"]
    );
}

#[tokio::test]
async fn passive_reply_and_duplicate_seq() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let (mut ws, _ready, token) = test.connect_gateway("10000", 1 << 25).await;
    test.send_as_user("reply please", Some(vec!["10000"])).await;
    let group = test.group_id().await;
    let message = last_inbound(&test, &format!("group:{group}")).await;
    let openid = test.group_openid("10000", &group).await;
    let url = format!("{}/v2/groups/{openid}/messages", test.sim);
    let client = test.http();

    let first = client
        .post(&url)
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "first", "msg_id": message["msg_id"], "msg_seq": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status().as_u16(), 200, "{:?}", first.text().await);

    let duplicate = client
        .post(&url)
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "dup", "msg_id": message["msg_id"], "msg_seq": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status().as_u16(), 400);
    assert!(duplicate.headers().contains_key("x-tps-trace-id"));
    let body: Value = duplicate.json().await.unwrap();
    assert_eq!(body["code"], 40054);
    ws.close(None).await.ok();
}

#[tokio::test]
async fn passive_reply_limit_and_expiry() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let (_ws, _ready, token) = test.connect_gateway("10000", 1 << 25).await;
    test.send_as_user("five replies max", Some(vec!["10000"]))
        .await;
    let group = test.group_id().await;
    let message = last_inbound(&test, &format!("group:{group}")).await;
    let openid = test.group_openid("10000", &group).await;
    let url = format!("{}/v2/groups/{openid}/messages", test.sim);
    let client = test.http();
    for seq in 1..=5 {
        let response = client
            .post(&url)
            .header("authorization", format!("QQBot {token}"))
            .json(&json!({"content": format!("r{seq}"), "msg_id": message["msg_id"]}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200);
    }
    let over = client
        .post(&url)
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "sixth", "msg_id": message["msg_id"]}))
        .send()
        .await
        .unwrap();
    let body: Value = over.json().await.unwrap();
    assert_eq!(body["code"], 40057);

    // a fresh message expires after the 5-minute window
    test.send_as_user("expire me", Some(vec!["10000"])).await;
    let message = last_inbound(&test, &format!("group:{group}")).await;
    test.ctl_ok(
        "POST",
        "/_mqs/clock/advance",
        Some(json!({"duration": "6m"})),
    )
    .await;
    let expired = client
        .post(&url)
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "late", "msg_id": message["msg_id"]}))
        .send()
        .await
        .unwrap();
    let body: Value = expired.json().await.unwrap();
    assert_eq!(body["code"], 40057);
}

#[tokio::test]
async fn recall_only_own_messages() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let (_ws, _ready, token) = test.connect_gateway("10000", 1 << 25).await;
    test.send_as_user("recall me", Some(vec!["10000"])).await;
    let group = test.group_id().await;
    let message = last_inbound(&test, &format!("group:{group}")).await;
    let openid = test.group_openid("10000", &group).await;
    let client = test.http();

    let response = client
        .delete(format!(
            "{}/v2/groups/{openid}/messages/{}",
            test.sim,
            message["msg_id"].as_str().unwrap()
        ))
        .header("authorization", format!("QQBot {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);

    let sent: Value = client
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "bye", "msg_id": message["msg_id"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let response = client
        .delete(format!(
            "{}/v2/groups/{openid}/messages/{}",
            test.sim,
            sent["id"].as_str().unwrap()
        ))
        .header("authorization", format!("QQBot {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 204);
}

#[tokio::test]
async fn bot_offline_and_active_quota() {
    let test = Test::start().await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let token = test.token("10000").await;
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "offline"}))
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40056);

    test.settings(json!({"require_online": false, "rate_mode": "off"}))
        .await;
    test.world()
        .db
        .execute(
            "INSERT INTO active_quota(bot_id,conversation_id,day,used) VALUES(?,?,?,1000) \
             ON CONFLICT(bot_id,conversation_id,day) DO UPDATE SET used=1000",
            &mqs::p!["10000", format!("group:{group}"), test.world().clock.day()],
        )
        .unwrap();
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "over quota"}))
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40055);
}

#[tokio::test]
async fn only_mention_setting_switches_at_messages_to_group_at_event() {
    let test = Test::start().await;
    let group = test.group_id().await;
    test.ctl_ok(
        "PATCH",
        &format!("/_mqs/groups/{group}/bots/10000"),
        Some(json!({"recv_msg_setting": "only_mention"})),
    )
    .await;
    let (mut ws, _ready, _token) = test.connect_gateway("10000", 1 << 25).await;
    test.send_as_user("plain", Some(vec![])).await;
    let timeout =
        tokio::time::timeout(std::time::Duration::from_millis(600), next_json(&mut ws)).await;
    assert!(timeout.is_err(), "plain message must not be delivered");

    test.send_as_user("hey", Some(vec!["10000"])).await;
    let frame = next_json(&mut ws).await;
    assert_eq!(frame["t"], "GROUP_AT_MESSAGE_CREATE");
    assert!(frame["d"]["content"].as_str().unwrap().starts_with("<@!"));
    ws.close(None).await.ok();
}

#[tokio::test]
async fn c2c_passive_window_60min_and_4_replies() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let user = test.user_id().await;
    let bot_openid = test.user_openid("10000", &user).await;
    let conversation = format!("dm:10000:{user}");
    test.ctl_ok(
        "POST",
        "/_mqs/chat/send",
        Some(json!({"as_user": user, "to": conversation, "content": "hi bot"})),
    )
    .await;
    let message = last_inbound(&test, &conversation).await;
    let token = test.token("10000").await;
    let url = format!("{}/v2/users/{bot_openid}/messages", test.sim);
    for seq in 1..=4 {
        let response = test
            .http()
            .post(&url)
            .header("authorization", format!("QQBot {token}"))
            .json(&json!({"content": format!("r{seq}"), "msg_id": message["msg_id"]}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200, "reply {seq}");
    }
    let over = test
        .http()
        .post(&url)
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "fifth", "msg_id": message["msg_id"]}))
        .send()
        .await
        .unwrap();
    let body: Value = over.json().await.unwrap();
    assert_eq!(body["code"], 40057);
}

#[tokio::test]
async fn markdown_message_is_kept() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let token = test.token("10000").await;
    let markdown = json!({
        "custom_template_id": "1",
        "params": [{"key": "title", "values": ["hi"]}]
    });
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "", "markdown": markdown, "msg_type": 2}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let messages = test
        .world()
        .list_messages(&format!("group:{group}"), 10, None)
        .unwrap();
    let out = messages
        .iter()
        .rev()
        .find(|m| m["direction"] == "out")
        .unwrap();
    assert_eq!(out["markdown"], markdown);
}

#[tokio::test]
async fn keyboard_click_link_callback_and_command() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let user = test.user_id().await;
    let group = test.group_id().await;
    let conversation = format!("group:{group}");
    test.send_as_user("menu", Some(vec!["10000"])).await;
    let message = last_inbound(&test, &conversation).await;
    let openid = test.group_openid("10000", &group).await;
    let token = test.token("10000").await;
    // Official action.type: 0 = link, 1 = callback, 2 = command.
    let keyboard = json!({"content": {"rows": [
        {"buttons": [
            {"id": "cb", "action": {"type": 1, "data": "cb", "unsupport_tips": ""}},
            {"id": "link", "action": {"type": 0, "data": "https://example.com/docs", "unsupport_tips": ""}},
            {"id": "fill", "action": {"type": 2, "data": "/help", "unsupport_tips": ""}},
            {"id": "run", "action": {"type": 2, "data": "/status", "enter": true, "unsupport_tips": ""}},
            {"id": "quote", "action": {"type": 2, "data": "/quote", "enter": true, "reply": true, "unsupport_tips": ""}},
        ]}
    ]}});
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "menu", "msg_id": message["msg_id"], "keyboard": keyboard}))
        .send()
        .await
        .unwrap();
    let sent: Value = response.json().await.unwrap();
    let message_id = sent["id"].as_str().unwrap().to_string();

    // type 1 -> INTERACTION_CREATE
    let (status, callback) = test
        .ctl(
            "POST",
            &format!("/_mqs/messages/{message_id}/click"),
            Some(json!({"user_id": user, "index": 0})),
        )
        .await;
    assert_eq!(status, 200, "{callback}");
    assert_eq!(callback["kind"], "callback");
    // official chat_type: 0 = 频道, 1 = 群聊, 2 = 单聊
    assert_eq!(callback["interaction"]["payload"]["chat_type"], 1);
    let interaction = callback["interaction"]["id"].as_str().unwrap().to_string();
    let response = test
        .http()
        .put(format!("{}/v2/interactions/{interaction}", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"code": 0}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);

    // type 0 -> the client opens the URL itself
    let (status, link) = test
        .ctl(
            "POST",
            &format!("/_mqs/messages/{message_id}/click"),
            Some(json!({"user_id": user, "index": 1})),
        )
        .await;
    assert_eq!(status, 200, "{link}");
    assert_eq!(link["kind"], "link");
    assert_eq!(link["data"], "https://example.com/docs");

    // type 2 without enter (the official default) only fills the composer:
    // nothing is sent, and the client is told which bot to @.
    let (status, filled) = test
        .ctl(
            "POST",
            &format!("/_mqs/messages/{message_id}/click"),
            Some(json!({"user_id": user, "index": 2})),
        )
        .await;
    assert_eq!(status, 200, "{filled}");
    assert_eq!(filled["kind"], "command");
    assert_eq!(filled["enter"], false);
    assert_eq!(filled["data"], "/help");
    assert_eq!(filled["at_bots"], json!(["10000"]));
    assert!(filled["message"].is_null());
    let sent_help = test
        .world()
        .list_messages(&conversation, 50, None)
        .unwrap()
        .into_iter()
        .any(|m| m["direction"] == "in" && m["content"] == "/help");
    assert!(!sent_help, "enter=false must not send the command");

    // type 2 + enter -> send immediately, @ the bot that owns the button.
    let (status, ran) = test
        .ctl(
            "POST",
            &format!("/_mqs/messages/{message_id}/click"),
            Some(json!({"user_id": user, "index": 3})),
        )
        .await;
    assert_eq!(status, 200, "{ran}");
    assert_eq!(ran["kind"], "command");
    assert_eq!(ran["enter"], true);
    assert_eq!(ran["message"]["content"], "/status");
    assert_eq!(ran["message"]["raw"]["at_bots"], json!(["10000"]));

    // type 2 + enter + reply -> the sent command quotes the button message.
    let (status, quoted) = test
        .ctl(
            "POST",
            &format!("/_mqs/messages/{message_id}/click"),
            Some(json!({"user_id": user, "index": 4})),
        )
        .await;
    assert_eq!(status, 200, "{quoted}");
    assert_eq!(quoted["kind"], "command");
    let events = test
        .world()
        .list_events(0, 500, Some(&conversation))
        .unwrap();
    let event = events
        .iter()
        .find(|event| {
            event["payload"]["content"]
                .as_str()
                .map(|content| content.contains("/quote"))
                .unwrap_or(false)
        })
        .expect("the command event should be delivered");
    assert_eq!(
        event["payload"]["message_reference"]["message_id"],
        message_id
    );
}

#[tokio::test]
async fn markdown_text_tags_resolve_mentions_and_push_commands() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let user = test.user_id().await;
    let group = test.group_id().await;
    let conversation = format!("group:{group}");
    test.send_as_user("menu", Some(vec!["10000"])).await;
    let inbound = last_inbound(&test, &conversation).await;
    let openid = test.group_openid("10000", &group).await;
    let user_openid = test.user_openid("10000", &user).await;
    let token = test.token("10000").await;
    let users = test.ctl_ok("GET", "/_mqs/users", None).await;
    let nickname = users
        .as_array()
        .expect("users is a list")
        .iter()
        .find(|row| row["id"] == user)
        .and_then(|row| row["nickname"].as_str())
        .unwrap()
        .to_string();
    // text/show 按官方要求 urlencode；`%21` = `!`、`%20` = 空格。
    let markdown = format!(
        "欢迎 <qqbot-at-user id=\"{user_openid}\" />，出牌 <qqbot-cmd-input text=\"%21sweet-potato%20pass%202\" show=\"%E6%9E%97%E6%9F%9A\" reference=\"true\" />，开局 <qqbot-cmd-enter text=\"%21start\" />"
    );
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({
            "msg_type": 2,
            "markdown": {"content": markdown},
            "msg_id": inbound["msg_id"],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let sent: Value = response.json().await.unwrap();
    let message_id = sent["id"].as_str().unwrap().to_string();

    // `at_users` 把 per-bot openid 翻成群昵称，GUI 才能像真机一样显示 @某人。
    let message = test
        .world()
        .list_messages(&conversation, 50, None)
        .unwrap()
        .into_iter()
        .find(|item| item["msg_id"] == message_id)
        .unwrap();
    assert_eq!(message["at_users"][0]["id"], user_openid);
    assert_eq!(message["at_users"][0]["username"], nickname);

    // cmd-input：点击后只回填输入框，并把 @bot / 引用意图告诉 GUI。
    let (status, filled) = test
        .ctl(
            "POST",
            &format!("/_mqs/messages/{message_id}/cmd"),
            Some(json!({"user_id": user, "index": 0})),
        )
        .await;
    assert_eq!(status, 200, "{filled}");
    assert_eq!(filled["kind"], "command");
    assert_eq!(filled["enter"], false);
    assert_eq!(filled["reference"], true);
    assert_eq!(filled["data"], "!sweet-potato pass 2");
    assert_eq!(filled["show"], "林柚");
    assert_eq!(filled["at_bots"], json!(["10000"]));
    assert!(filled["message"].is_null());
    let pushed = test.world().list_messages(&conversation, 50, None).unwrap();
    assert!(
        !pushed
            .iter()
            .any(|item| { item["direction"] == "in" && item["content"] == "!sweet-potato pass 2" }),
        "cmd-input must wait for the user to press Enter"
    );

    // cmd-enter：点击即发送，群里自动 @ 这条消息的 Bot，事件也按 @ 投递。
    let (status, ran) = test
        .ctl(
            "POST",
            &format!("/_mqs/messages/{message_id}/cmd"),
            Some(json!({"user_id": user, "index": 1})),
        )
        .await;
    assert_eq!(status, 200, "{ran}");
    assert_eq!(ran["kind"], "command");
    assert_eq!(ran["enter"], true);
    assert_eq!(ran["message"]["content"], "!start");
    assert_eq!(ran["message"]["raw"]["at_bots"], json!(["10000"]));
    let events = test
        .world()
        .list_events(0, 500, Some(&conversation))
        .unwrap();
    let event = events
        .iter()
        .rev()
        .find(|event| {
            event["payload"]["content"]
                .as_str()
                .map(|content| content.ends_with("!start"))
                .unwrap_or(false)
        })
        .expect("the pushed command should be delivered to the bot");
    assert_eq!(event["payload"]["mentions"][0]["is_you"], true);

    // 越界索引是 10001，而不是 panic。
    let (status, error) = test
        .ctl(
            "POST",
            &format!("/_mqs/messages/{message_id}/cmd"),
            Some(json!({"user_id": user, "index": 9})),
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(error["code"], 10001);
}

#[tokio::test]
async fn recall_has_the_official_two_minute_limit() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let (_ws, _ready, token) = test.connect_gateway("10000", 1 << 25).await;
    test.send_as_user("recall later", Some(vec!["10000"])).await;
    let group = test.group_id().await;
    let message = last_inbound(&test, &format!("group:{group}")).await;
    let openid = test.group_openid("10000", &group).await;
    let sent: Value = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "too late", "msg_id": message["msg_id"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    test.ctl_ok(
        "POST",
        "/_mqs/clock/advance",
        Some(json!({"duration": "3m"})),
    )
    .await;
    let response = test
        .http()
        .delete(format!(
            "{}/v2/groups/{openid}/messages/{}",
            test.sim,
            sent["id"].as_str().unwrap()
        ))
        .header("authorization", format!("QQBot {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40034);
}

#[tokio::test]
async fn event_id_replies_are_passive_not_active() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let user = test.user_id().await;
    let group = test.group_id().await;
    let conversation = format!("group:{group}");
    test.send_as_user("menu", Some(vec!["10000"])).await;
    let message = last_inbound(&test, &conversation).await;
    let openid = test.group_openid("10000", &group).await;
    let token = test.token("10000").await;
    // a callback button message, then a click => INTERACTION_CREATE event
    let keyboard = json!({"content": {"rows": [{"buttons": [
        {"id": "1", "action": {"type": 1, "data": "cb", "unsupported_tips": ""}}
    ]}]}});
    let sent: Value = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "menu", "msg_id": message["msg_id"], "keyboard": keyboard}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let clicked = test
        .ctl_ok(
            "POST",
            &format!("/_mqs/messages/{}/click", sent["id"].as_str().unwrap()),
            Some(json!({"user_id": user, "index": 0})),
        )
        .await;
    let event_id = clicked["interaction"]["payload"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // replying to the event must be passive: no active quota consumed
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "event reply", "event_id": event_id}))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{body}");
    let used = test
        .world()
        .db
        .one(
            "SELECT COALESCE(SUM(used),0) AS c FROM active_quota WHERE bot_id='10000'",
            &[],
        )
        .unwrap()
        .map(|row| mqs::db::i(&row, "c"))
        .unwrap_or(0);
    assert_eq!(used, 0, "event replies must not consume the active quota");
}
