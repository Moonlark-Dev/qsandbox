//! QQ active-message rate limits (official policy).

mod common;

use common::Test;
use serde_json::{json, Value};

async fn ensure_bot(test: &Test, app_id: &str) -> String {
    if test.world().bot_by_app_id(app_id).unwrap().is_none() {
        test.ctl_ok(
            "POST",
            "/_mqs/bots",
            Some(json!({
                "app_id": app_id,
                "name": format!("RL Bot {app_id}"),
                "scopes": ["group_message", "c2c_message", "media"],
            })),
        )
        .await;
    }
    test.bot_headers(app_id).await
}

async fn active_c2c(
    test: &Test,
    headers: &str,
    openid: &str,
    content: &str,
) -> (u16, Value, reqwest::header::HeaderMap) {
    let response = test
        .http()
        .post(format!("{}/v2/users/{openid}/messages", test.sim))
        .header("authorization", headers)
        .json(&json!({"content": content}))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let headers_map = response.headers().clone();
    let body = response.json().await.unwrap_or(Value::Null);
    (status, body, headers_map)
}

#[tokio::test]
async fn unverified_c2c_5_qps() {
    let test = Test::start().await;
    test.settings(json!({"rate_mode": "unverified", "require_online": false}))
        .await;
    let headers = ensure_bot(&test, "10001").await;
    let user = test.user_id().await;
    let openid = test.user_openid("10001", &user).await;
    for index in 0..5 {
        let (status, body, headers_map) =
            active_c2c(&test, &headers, &openid, &format!("c2c {index}")).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(headers_map.get("x-ratelimit-limit").unwrap(), "5");
    }
    let (status, body, headers_map) = active_c2c(&test, &headers, &openid, "sixth").await;
    assert_eq!(status, 429);
    assert_eq!(body["code"], 42900);
    let retry: f64 = headers_map
        .get("retry-after")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(retry > 0.0);
    assert_eq!(headers_map.get("x-ratelimit-limit").unwrap(), "5");
}

#[tokio::test]
async fn certified_c2c_10_qps() {
    let test = Test::start().await;
    test.settings(json!({"rate_mode": "certified", "require_online": false}))
        .await;
    let headers = ensure_bot(&test, "10001").await;
    let user = test.user_id().await;
    let openid = test.user_openid("10001", &user).await;
    for index in 0..10 {
        let (status, body, _) =
            active_c2c(&test, &headers, &openid, &format!("cert {index}")).await;
        assert_eq!(status, 200, "{body}");
    }
    let (status, _, headers_map) = active_c2c(&test, &headers, &openid, "eleventh").await;
    assert_eq!(status, 429);
    assert_eq!(headers_map.get("x-ratelimit-limit").unwrap(), "10");
}

#[tokio::test]
async fn unverified_group_bot_30_per_minute() {
    let test = Test::start().await;
    test.settings(json!({"rate_mode": "unverified", "require_online": false}))
        .await;
    let headers = ensure_bot(&test, "10001").await;
    let user = test.user_id().await;
    let mut groups = Vec::new();
    for _ in 0..2 {
        let group = test
            .ctl_ok(
                "POST",
                "/_mqs/groups",
                Some(json!({"owner_id": user, "member_ids": [], "bot_ids": ["10001"]})),
            )
            .await;
        groups.push(group["id"].as_str().unwrap().to_string());
    }
    let mut sent = 0;
    for group in &groups {
        let openid = test.group_openid("10001", group).await;
        for _ in 0..15 {
            let response = test
                .http()
                .post(format!("{}/v2/groups/{openid}/messages", test.sim))
                .header("authorization", &headers)
                .json(&json!({"content": format!("g{sent}")}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), 200);
            sent += 1;
        }
    }
    assert_eq!(sent, 30);
    let openid = test.group_openid("10001", &groups[0]).await;
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", &headers)
        .json(&json!({"content": "over bot limit"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 429);
    assert_eq!(response.headers().get("x-ratelimit-limit").unwrap(), "30");
}

#[tokio::test]
async fn relation_20_per_minute() {
    let test = Test::start().await;
    test.settings(json!({"rate_mode": "certified", "require_online": false}))
        .await;
    let headers = ensure_bot(&test, "10001").await;
    let user = test.user_id().await;
    let group = test
        .ctl_ok(
            "POST",
            "/_mqs/groups",
            Some(json!({"owner_id": user, "member_ids": [], "bot_ids": ["10001"]})),
        )
        .await;
    let openid = test
        .group_openid("10001", group["id"].as_str().unwrap())
        .await;
    for index in 0..20 {
        let response = test
            .http()
            .post(format!("{}/v2/groups/{openid}/messages", test.sim))
            .header("authorization", &headers)
            .json(&json!({"content": format!("rel {index}")}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200);
    }
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", &headers)
        .json(&json!({"content": "21st"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 429);
    assert_eq!(response.headers().get("x-ratelimit-limit").unwrap(), "20");
}

#[tokio::test]
async fn off_mode_is_unlimited() {
    let test = Test::start().await;
    test.settings(json!({"rate_mode": "off", "require_online": false}))
        .await;
    let headers = ensure_bot(&test, "10001").await;
    let user = test.user_id().await;
    let openid = test.user_openid("10001", &user).await;
    for index in 0..12 {
        let (status, body, _) = active_c2c(&test, &headers, &openid, &format!("off {index}")).await;
        assert_eq!(status, 200, "{body}");
    }
}

#[tokio::test]
async fn passive_replies_are_not_rate_limited() {
    let test = Test::start().await;
    test.settings(json!({"rate_mode": "unverified", "require_online": false}))
        .await;
    let headers = ensure_bot(&test, "10001").await;
    let user = test.user_id().await;
    let group = test
        .ctl_ok(
            "POST",
            "/_mqs/groups",
            Some(json!({"owner_id": user, "member_ids": [user], "bot_ids": ["10001"]})),
        )
        .await;
    let gid = group["id"].as_str().unwrap().to_string();
    let conversation = format!("group:{gid}");
    test.world()
        .send_as_user(&conversation, &user, "ping", 0, &[], None, None, None, &[])
        .await
        .unwrap();
    let messages = test.world().list_messages(&conversation, 10, None).unwrap();
    let message = messages
        .iter()
        .rev()
        .find(|m| m["direction"] == "in")
        .unwrap()["msg_id"]
        .as_str()
        .unwrap()
        .to_string();
    let openid = test.group_openid("10001", &gid).await;
    for seq in 1..=5 {
        let response = test
            .http()
            .post(format!("{}/v2/groups/{openid}/messages", test.sim))
            .header("authorization", &headers)
            .json(&json!({"content": format!("reply {seq}"), "msg_id": message, "msg_seq": seq}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200, "reply {seq}");
    }
}
