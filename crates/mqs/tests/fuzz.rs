//! Robustness: malformed input must produce QQ errors, never a panic or a 500.

mod common;

use common::{next_json, Test};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn malformed_message_bodies() {
    let test = Test::start().await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let headers = test.bot_headers("10000").await;
    for body in [
        b"not json".as_slice(),
        br#"{"content": 123}"#,
        br#"{"msg_seq": "x"}"#,
        b"[]",
        b"null",
    ] {
        let response = test
            .http()
            .post(format!("{}/v2/groups/{openid}/messages", test.sim))
            .header("authorization", &headers)
            .header("content-type", "application/json")
            .body(body.to_vec())
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let payload: Value = response.json().await.unwrap_or(Value::Null);
        assert!(
            status == 400 || status == 403,
            "status {status} for {body:?}"
        );
        assert_ne!(status, 500, "body {body:?}");
        assert_eq!(payload["code"], 10001, "body {body:?}: {payload}");
    }
}

#[tokio::test]
async fn malformed_token_request() {
    let test = Test::start().await;
    for body in [b"{}".as_slice(), b"{\"appId\": \"10000\"}", b"garbage"] {
        let response = test
            .http()
            .post(format!("{}/app/getAppAccessToken", test.sim))
            .header("content-type", "application/json")
            .body(body.to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 400);
        let payload: Value = response.json().await.unwrap();
        assert_eq!(payload["code"], 10001);
    }
}

#[tokio::test]
async fn gateway_malformed_json_closes_4002() {
    let test = Test::start().await;
    let url = format!("ws://{}/websocket", test.sim.trim_start_matches("http://"));
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let _hello = next_json(&mut ws).await;
    ws.send(Message::Text("{not json".into())).await.unwrap();
    let closed = loop {
        match ws.next().await {
            Some(Ok(Message::Close(frame))) => break frame,
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => break None,
        }
    };
    assert_eq!(closed.map(|frame| u16::from(frame.code)), Some(4002));
}

#[tokio::test]
async fn invalid_shard_is_rejected() {
    let test = Test::start().await;
    let token = test.token("10000").await;
    let url = format!("ws://{}/websocket", test.sim.trim_start_matches("http://"));
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let _hello = next_json(&mut ws).await;
    ws.send(Message::Text(
        json!({"op": 2, "d": {"token": format!("QQBot {token}"), "intents": 1 << 25, "shard": [1, 1]}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let closed = loop {
        match ws.next().await {
            Some(Ok(Message::Close(frame))) => break frame,
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => break None,
        }
    };
    assert_eq!(closed.map(|frame| u16::from(frame.code)), Some(4010));
}

#[tokio::test]
async fn unknown_openid_is_a_qq_error() {
    let test = Test::start().await;
    let headers = test.bot_headers("10000").await;
    let response = test
        .http()
        .post(format!(
            "{}/v2/groups/{}/messages",
            test.sim,
            "A".repeat(32)
        ))
        .header("authorization", &headers)
        .json(&json!({"content": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 10001);
}
