//! `GET /v2/users/{openid}/info`（2026-11-16 新增的用户基本信息接口）。

mod common;

use common::Test;
use serde_json::{json, Value};

#[tokio::test]
async fn user_info_returns_profile_fields() {
    let test = Test::start().await;
    let user = test.user_id().await;
    let openid = test.user_openid("10000", &user).await;
    let profile = test
        .ctl_ok("GET", &format!("/_mqs/users/{user}"), None)
        .await;
    let headers = test.bot_headers("10000").await;

    let response = test
        .http()
        .get(format!("{}/v2/users/{openid}/info", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert!(
        !response.headers()["x-tps-trace-id"].is_empty(),
        "trace header is expected on every simulator response"
    );
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["username"], profile["nickname"]);
    assert_eq!(body["union_openid"].as_str().unwrap().len(), 32);
    assert_eq!(body["allow_proactive_msg"], true);
    let avatar = body["avatar"].as_str().unwrap();
    assert!(
        avatar.starts_with(&format!("{}/avatars/", test.sim)),
        "avatar should be an absolute URL the SDK can fetch: {avatar}"
    );
}

#[tokio::test]
async fn user_info_reports_the_bot_proactive_switch() {
    let test = Test::start().await;
    let user = test.user_id().await;
    let openid = test.user_openid("10000", &user).await;
    test.ctl_ok(
        "PATCH",
        "/_mqs/bots/10000",
        Some(json!({"allow_proactive_msg": false})),
    )
    .await;
    let headers = test.bot_headers("10000").await;

    let body: Value = test
        .http()
        .get(format!("{}/v2/users/{openid}/info", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["allow_proactive_msg"], false);
}

#[tokio::test]
async fn unknown_openid_returns_40184001() {
    let test = Test::start().await;
    let headers = test.bot_headers("10000").await;
    let missing = "A".repeat(32);
    let response = test
        .http()
        .get(format!("{}/v2/users/{missing}/info", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 403);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40184001);
    assert_eq!(body["err_code"], 40184001);
    assert!(body["trace_id"].as_str().unwrap().len() == 16);
}
