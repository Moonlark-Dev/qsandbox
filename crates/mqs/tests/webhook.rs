//! Webhook: Ed25519 handshake, endpoint registration, delivery logging.

mod common;

use common::Test;
use serde_json::{json, Value};

#[tokio::test]
async fn handshake_signature() {
    let test = Test::start().await;
    let body: Value = test
        .http()
        .post(format!("{}/webhook/10000", test.sim))
        .json(&json!({"op": 13, "d": {"plain_token": "Arq0D5A61EgUu4OxUvOp", "event_ts": "1725442341"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["plain_token"], "Arq0D5A61EgUu4OxUvOp");
    let signature = body["signature"].as_str().unwrap();
    assert_eq!(signature.len(), 128);
    let expected =
        mqs::webhooks::sign_handshake("mqs-secret-10000", "1725442341", "Arq0D5A61EgUu4OxUvOp");
    assert_eq!(signature, expected);
}

#[tokio::test]
async fn register_webhook_endpoint() {
    let test = Test::start().await;
    let created = test
        .ctl_ok(
            "POST",
            "/_mqs/bots/10000/webhook",
            Some(json!({"url": "http://127.0.0.1:1/hook"})),
        )
        .await;
    assert!(created["id"].as_str().unwrap().starts_with("wh_"));
    let list = test.ctl_ok("GET", "/_mqs/bots/10000/webhook", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    test.ctl_ok(
        "DELETE",
        &format!(
            "/_mqs/bots/10000/webhook/{}",
            created["id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    let list = test.ctl_ok("GET", "/_mqs/bots/10000/webhook", None).await;
    assert_eq!(list.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn webhook_delivery_is_attempted_and_logged() {
    let test = Test::start().await;
    // point the webhook at the sandbox's own handshake endpoint so delivery succeeds
    test.ctl_ok(
        "POST",
        "/_mqs/bots/10000/webhook",
        Some(json!({"url": format!("{}/webhook/10000", test.sim)})),
    )
    .await;
    test.send_as_user("webhook me", Some(vec!["10000"])).await;
    let mut found = false;
    for _ in 0..40 {
        let frames = test
            .world()
            .list_frames(0, 200, None, Some("http"), None)
            .unwrap();
        if frames
            .iter()
            .any(|frame| frame["summary"].as_str().unwrap_or("").contains("webhook"))
        {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(found, "webhook delivery was not logged");
}
