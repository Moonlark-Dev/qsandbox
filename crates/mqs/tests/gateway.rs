//! Gateway protocol: hello/identify/ready/heartbeat/resume/intents/errors.

mod common;

use common::{next_json, Test};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn hello_identify_ready_and_heartbeat() {
    let test = Test::start().await;
    let (mut ws, ready, _token) = test.connect_gateway("10000", 1 << 25).await;
    assert_eq!(ready["version"], 1);
    assert_eq!(ready["shard"], json!([0, 1]));
    assert_eq!(ready["user"]["id"], "10000");
    ws.send(Message::Text(json!({"op": 1, "d": 0}).to_string().into()))
        .await
        .unwrap();
    let ack = next_json(&mut ws).await;
    assert_eq!(ack, json!({"op": 11, "d": null}));
    ws.close(None).await.ok();
}

#[tokio::test]
async fn intents_filtering() {
    let test = Test::start().await;
    let (mut ws, _ready, _token) = test.connect_gateway("10000", 1).await;
    let result = test.send_as_user("not for you", Some(vec!["10000"])).await;
    assert!(result["message"]["msg_id"].is_string());
    let timeout =
        tokio::time::timeout(std::time::Duration::from_millis(600), next_json(&mut ws)).await;
    assert!(timeout.is_err(), "unexpected frame for unsubscribed intent");
    ws.close(None).await.ok();
}

#[tokio::test]
async fn resume_replays_missed_events() {
    let test = Test::start().await;
    let (ws, ready, token) = test.connect_gateway("10000", 1 << 25).await;
    let session_id = ready["session_id"].as_str().unwrap().to_string();
    drop(ws);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    test.send_as_user("while offline", Some(vec!["10000"]))
        .await;

    let url = format!("ws://{}/websocket", test.sim.trim_start_matches("http://"));
    let (mut ws2, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let _hello = next_json(&mut ws2).await;
    ws2.send(Message::Text(
        json!({"op": 6, "d": {"token": format!("QQBot {token}"), "session_id": session_id, "seq": 0}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let resumed = next_json(&mut ws2).await;
    assert_eq!(resumed["t"], "RESUMED");
    let mut found = None;
    for _ in 0..10 {
        let frame = next_json(&mut ws2).await;
        if frame["t"] == "GROUP_MESSAGE_CREATE" {
            found = Some(frame);
            break;
        }
    }
    let event = found.expect("missed group message was replayed");
    assert!(event["d"]["content"]
        .as_str()
        .unwrap()
        .contains("while offline"));
    assert!(event["s"].as_i64().unwrap() >= 1);
    ws2.close(None).await.ok();
}

#[tokio::test]
async fn invalid_session_closes_socket() {
    let test = Test::start().await;
    let token = test.token("10000").await;
    let url = format!("ws://{}/websocket", test.sim.trim_start_matches("http://"));
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let _hello = next_json(&mut ws).await;
    ws.send(Message::Text(
        json!({"op": 6, "d": {"token": format!("QQBot {token}"), "session_id": "bogus", "seq": 0}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let invalid = next_json(&mut ws).await;
    assert_eq!(invalid["op"], 9);
    assert_eq!(invalid["d"], false);
    let closed = loop {
        match ws.next().await {
            Some(Ok(Message::Close(frame))) => break frame,
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => break None,
        }
    };
    assert_eq!(closed.map(|frame| u16::from(frame.code)), Some(4006));
}

#[tokio::test]
async fn bad_identify_token_closes_4004() {
    let test = Test::start().await;
    let url = format!("ws://{}/websocket", test.sim.trim_start_matches("http://"));
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let _hello = next_json(&mut ws).await;
    ws.send(Message::Text(
        json!({"op": 2, "d": {"token": "QQBot nope", "intents": 1 << 25}})
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
    assert_eq!(closed.map(|frame| u16::from(frame.code)), Some(4004));
}

#[tokio::test]
async fn gateway_url_and_frames_are_logged() {
    let test = Test::start().await;
    let headers = test.bot_headers("10000").await;
    let response = test
        .http()
        .get(format!("{}/gateway/bot", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["url"].as_str().unwrap().starts_with("ws://"));
    assert_eq!(body["shards"], 1);

    let (ws, _ready, _token) = test.connect_gateway("10000", 1 << 25).await;
    drop(ws);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let frames = test
        .world()
        .list_frames(0, 100, None, Some("ws"), None)
        .unwrap();
    let summaries: String = frames
        .iter()
        .filter_map(|frame| frame["summary"].as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(summaries.contains("READY"), "{summaries}");
    assert!(summaries.contains("gateway disconnected"), "{summaries}");
}
