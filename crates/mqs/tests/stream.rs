//! 流式发送单聊消息：`POST /v2/users/{openid}/stream_messages` 的分片语义。

mod common;

use common::Test;
use serde_json::{json, Value};

const MAX_LEN: i64 = 4000;

async fn open_dm(test: &Test) -> (String, String, String) {
    // (user_openid, inbound msg_id, bot token)
    test.settings(json!({"require_online": false})).await;
    let user = test.user_id().await;
    let conversation = format!("dm:10000:{user}");
    test.ctl_ok(
        "POST",
        "/_mqs/chat/send",
        Some(json!({"as_user": user, "to": conversation, "content": "你好"})),
    )
    .await;
    let messages = test.world().list_messages(&conversation, 50, None).unwrap();
    let inbound = messages
        .iter()
        .rev()
        .find(|message| message["direction"] == "in")
        .expect("inbound dm");
    (
        test.user_openid("10000", &user).await,
        inbound["msg_id"].as_str().unwrap().to_string(),
        test.token("10000").await,
    )
}

async fn post_chunk(test: &Test, openid: &str, token: &str, body: Value) -> (u16, Value) {
    let response = test
        .http()
        .post(format!("{}/v2/users/{openid}/stream_messages", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap();
    (status, body)
}

fn outbound_messages(test: &Test, conversation: &str) -> Vec<Value> {
    test.world()
        .list_messages(conversation, 100, None)
        .unwrap()
        .into_iter()
        .filter(|message| message["direction"] == "out")
        .collect()
}

#[tokio::test]
async fn first_chunk_returns_stream_id_and_creates_markdown_message() {
    let test = Test::start().await;
    let (openid, msg_id, token) = open_dm(&test).await;
    let (status, body) = post_chunk(
        &test,
        &openid,
        &token,
        json!({
            "input_mode": "replace",
            "input_state": 1,
            "index": 0,
            "content_type": "markdown",
            "content_raw": "正在生成回答，请稍候",
            "msg_id": msg_id,
            "msg_seq": 1,
        }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let stream_id = body["id"].as_str().unwrap();
    assert_eq!(
        stream_id.len(),
        36,
        "stream_msg_id should be a UUID: {stream_id}"
    );
    assert_eq!(stream_id.matches('-').count(), 4);
    assert!(body["timestamp"].as_str().unwrap().contains('T'));
    assert!(body["ext_info"]["ref_idx"]
        .as_str()
        .unwrap()
        .starts_with("REFIDX_"));
    assert_eq!(body["remain_msg_len"], MAX_LEN - 10);

    let user = test.user_id().await;
    let conversation = format!("dm:10000:{user}");
    let messages = outbound_messages(&test, &conversation);
    assert_eq!(messages.len(), 1, "{messages:?}");
    let message = &messages[0];
    assert_eq!(message["msg_id"], stream_id);
    assert_eq!(message["msg_type"], 2);
    assert_eq!(message["content"], "正在生成回答，请稍候");
    assert_eq!(
        message["raw"]["markdown"]["content"],
        "正在生成回答，请稍候"
    );
    assert_eq!(message["raw"]["stream"]["stream_msg_id"], stream_id);
    assert_eq!(message["raw"]["stream"]["index"], 0);
    assert_eq!(message["raw"]["stream"]["input_state"], 1);
    assert_eq!(message["raw"]["stream"]["finished"], false);
    assert_eq!(message["raw"]["msg_seq"], 1);
}

#[tokio::test]
async fn append_and_replace_update_the_same_message_and_finish() {
    let test = Test::start().await;
    let (openid, msg_id, token) = open_dm(&test).await;
    let (_, first) = post_chunk(
        &test,
        &openid,
        &token,
        json!({"input_state": 1, "index": 0, "content_raw": "hello", "msg_id": msg_id, "msg_seq": 1}),
    )
    .await;
    let stream_id = first["id"].as_str().unwrap().to_string();
    assert_eq!(first["remain_msg_len"], MAX_LEN - 5);
    assert!(first["ext_info"]["ref_idx"].is_string());

    let (status, second) = post_chunk(
        &test,
        &openid,
        &token,
        json!({
            "stream_msg_id": stream_id,
            "input_state": 1,
            "index": 1,
            "content_raw": " world",
            "msg_seq": 1,
        }),
    )
    .await;
    assert_eq!(status, 200, "{second}");
    assert_eq!(second["id"], stream_id);
    assert_eq!(second["remain_msg_len"], MAX_LEN - 11);

    let (status, third) = post_chunk(
        &test,
        &openid,
        &token,
        json!({
            "stream_msg_id": stream_id,
            "input_mode": "replace",
            "input_state": 10,
            "index": 2,
            "content_type": "text",
            "content_raw": "hello world!",
            "msg_seq": 1,
        }),
    )
    .await;
    assert_eq!(status, 200, "{third}");
    assert_eq!(third["id"], stream_id);

    let user = test.user_id().await;
    let conversation = format!("dm:10000:{user}");
    let messages = outbound_messages(&test, &conversation);
    assert_eq!(
        messages.len(),
        1,
        "streaming must not create extra messages"
    );
    assert_eq!(messages[0]["content"], "hello world!");
    assert_eq!(messages[0]["raw"]["stream"]["index"], 2);
    assert_eq!(messages[0]["raw"]["stream"]["input_state"], 10);
    assert_eq!(messages[0]["raw"]["stream"]["finished"], true);
    assert_eq!(messages[0]["raw"]["stream"]["input_mode"], "replace");

    // 结束后不能再追加分片。
    let (status, body) = post_chunk(
        &test,
        &openid,
        &token,
        json!({"stream_msg_id": stream_id, "index": 3, "content_raw": "!"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], 10001);

    // 未知的 stream_msg_id 是 40034（message not found）。
    let (status, body) = post_chunk(
        &test,
        &openid,
        &token,
        json!({"stream_msg_id": "does-not-exist", "index": 1, "content_raw": "x"}),
    )
    .await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["code"], 40034);
}

#[tokio::test]
async fn replace_must_keep_the_delivered_prefix() {
    let test = Test::start().await;
    let (openid, msg_id, token) = open_dm(&test).await;
    let (_, first) = post_chunk(
        &test,
        &openid,
        &token,
        json!({"input_mode": "replace", "index": 0, "content_raw": "abc", "msg_id": msg_id, "msg_seq": 1}),
    )
    .await;
    let stream_id = first["id"].as_str().unwrap().to_string();

    let (status, body) = post_chunk(
        &test,
        &openid,
        &token,
        json!({
            "stream_msg_id": stream_id,
            "input_mode": "replace",
            "index": 1,
            "content_raw": "abX",
        }),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], 40007);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("must start with"));

    let user = test.user_id().await;
    let conversation = format!("dm:10000:{user}");
    assert_eq!(outbound_messages(&test, &conversation)[0]["content"], "abc");

    let (status, body) = post_chunk(
        &test,
        &openid,
        &token,
        json!({
            "stream_msg_id": stream_id,
            "input_mode": "replace",
            "index": 1,
            "content_raw": "abcd",
        }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        outbound_messages(&test, &conversation)[0]["content"],
        "abcd"
    );
}

#[tokio::test]
async fn replaying_a_chunk_is_idempotent() {
    let test = Test::start().await;
    let (openid, msg_id, token) = open_dm(&test).await;
    let (_, first) = post_chunk(
        &test,
        &openid,
        &token,
        json!({"index": 0, "content_raw": "a", "msg_id": msg_id, "msg_seq": 1}),
    )
    .await;
    let stream_id = first["id"].as_str().unwrap().to_string();
    let replay = json!({"stream_msg_id": stream_id, "index": 0, "content_raw": "a"});
    let (status, again) = post_chunk(&test, &openid, &token, replay.clone()).await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["id"], stream_id);
    assert_eq!(again["remain_msg_len"], MAX_LEN - 1);

    let user = test.user_id().await;
    let conversation = format!("dm:10000:{user}");
    assert_eq!(outbound_messages(&test, &conversation)[0]["content"], "a");

    // 同一个 index 但内容不同不是重放，直接拒绝。
    let (status, body) = post_chunk(
        &test,
        &openid,
        &token,
        json!({"stream_msg_id": stream_id, "index": 0, "content_raw": "b"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], 10001);

    let (status, body) = post_chunk(
        &test,
        &openid,
        &token,
        json!({"stream_msg_id": stream_id, "index": 1, "content_raw": "b"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(outbound_messages(&test, &conversation)[0]["content"], "ab");
}

#[tokio::test]
async fn stream_consumes_one_passive_reply_slot() {
    let test = Test::start().await;
    let (openid, msg_id, token) = open_dm(&test).await;
    let (_, first) = post_chunk(
        &test,
        &openid,
        &token,
        json!({"index": 0, "content_raw": "流式", "msg_id": msg_id, "msg_seq": 1}),
    )
    .await;
    let stream_id = first["id"].as_str().unwrap().to_string();
    // 续片复用同一个 msg_seq 不应再消耗被动回复次数。
    for index in 1..=2 {
        let (status, body) = post_chunk(
            &test,
            &openid,
            &token,
            json!({"stream_msg_id": stream_id, "index": index, "content_raw": format!("-{index}")}),
        )
        .await;
        assert_eq!(status, 200, "{body}");
    }

    // 单聊被动回复上限 4 次：流式已经用掉 1 次，普通发送还能发 3 次。
    let client = test.http();
    let url = format!("{}/v2/users/{openid}/messages", test.sim);
    for seq in 2..=4 {
        let response = client
            .post(&url)
            .header("authorization", format!("QQBot {token}"))
            .json(&json!({"content": format!("reply {seq}"), "msg_id": msg_id, "msg_seq": seq}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200, "reply {seq}");
    }
    let over = client
        .post(&url)
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"content": "fifth", "msg_id": msg_id}))
        .send()
        .await
        .unwrap();
    let body: Value = over.json().await.unwrap();
    assert_eq!(body["code"], 40057);
}

#[tokio::test]
async fn stream_validation_and_scope() {
    let test = Test::start().await;
    let (openid, msg_id, token) = open_dm(&test).await;
    for bad in [
        json!({"index": 0, "content_raw": "x", "input_state": 5, "msg_id": msg_id}),
        json!({"index": 0, "content_raw": "x", "input_mode": "merge", "msg_id": msg_id}),
        json!({"index": 0, "content_raw": "x", "content_type": "html", "msg_id": msg_id}),
        json!({"index": 2, "content_raw": "x", "msg_id": msg_id}),
        json!({"index": 0, "content_raw": 42, "msg_id": msg_id}),
    ] {
        let (status, body) = post_chunk(&test, &openid, &token, bad.clone()).await;
        assert_eq!(status, 400, "{bad} -> {body}");
        assert_eq!(body["code"], 10001, "{bad} -> {body}");
    }

    // 没有 c2c_message scope 的 Bot 不能发流式消息。
    test.ctl_ok("PATCH", "/_mqs/bots/10000", Some(json!({"scopes": []})))
        .await;
    let (status, body) = post_chunk(
        &test,
        &openid,
        &token,
        json!({"index": 0, "content_raw": "x", "msg_id": msg_id}),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["code"], 40040);
}

#[tokio::test]
async fn stream_length_limit_is_reported_through_remain_msg_len() {
    let test = Test::start().await;
    test.settings(json!({"stream_msg_max_len": 10})).await;
    let (openid, msg_id, token) = open_dm(&test).await;
    let (status, body) = post_chunk(
        &test,
        &openid,
        &token,
        json!({"index": 0, "content_raw": "0123456789", "msg_id": msg_id, "msg_seq": 1}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["remain_msg_len"], 0);
    let stream_id = body["id"].as_str().unwrap().to_string();
    let (status, body) = post_chunk(
        &test,
        &openid,
        &token,
        json!({"stream_msg_id": stream_id, "index": 1, "content_raw": "x"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], 10001);
}
