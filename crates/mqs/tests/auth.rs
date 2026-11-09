//! Auth fidelity: token issuance, expiry, error codes, trace headers.

mod common;

use common::Test;
use serde_json::json;

#[tokio::test]
async fn token_issue_and_use() {
    let test = Test::start().await;
    let response = test
        .http()
        .post(format!("{}/app/getAppAccessToken", test.sim))
        .json(&json!({"appId": "10000", "clientSecret": "mqs-secret-10000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    let token = body["access_token"].as_str().unwrap();
    assert!(token.starts_with("MQS_"));
    assert_eq!(body["expires_in"], "7200");

    let me = test
        .http()
        .get(format!("{}/users/@me", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(me.status().as_u16(), 200);
    assert!(me.headers().contains_key("x-tps-trace-id"));
    let me: serde_json::Value = me.json().await.unwrap();
    assert_eq!(me["id"], "10000");
    assert_eq!(me["bot"], true);
}

#[tokio::test]
async fn token_is_reused_while_valid() {
    let test = Test::start().await;
    let request = |app: &'static str, secret: &'static str| {
        let http = test.http();
        let sim = test.sim.clone();
        async move {
            let response = http
                .post(format!("{sim}/app/getAppAccessToken"))
                .json(&json!({"appId": app, "clientSecret": secret}))
                .send()
                .await
                .unwrap();
            response.json::<serde_json::Value>().await.unwrap()
        }
    };
    let first = request("10000", "mqs-secret-10000").await;
    let second = request("10000", "mqs-secret-10000").await;
    assert_eq!(first["access_token"], second["access_token"]);
    let expires_in: i64 = second["expires_in"].as_str().unwrap().parse().unwrap();
    assert!((7000..=7200).contains(&expires_in), "{expires_in}");
}

#[tokio::test]
async fn wrong_secret_and_unknown_app() {
    let test = Test::start().await;
    let response = test
        .http()
        .post(format!("{}/app/getAppAccessToken", test.sim))
        .json(&json!({"appId": "10000", "clientSecret": "wrong"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], 11244);

    let response = test
        .http()
        .post(format!("{}/app/getAppAccessToken", test.sim))
        .json(&json!({"appId": "99999", "clientSecret": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], 11243);
    // the official message APIs document `err_code`; both names are emitted
    assert_eq!(body["err_code"], 11243);
}

#[tokio::test]
async fn missing_or_bad_token() {
    let test = Test::start().await;
    let response = test
        .http()
        .get(format!("{}/users/@me", test.sim))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], 11241);

    let response = test
        .http()
        .get(format!("{}/users/@me", test.sim))
        .header("authorization", "QQBot not-a-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], 11241);
}

#[tokio::test]
async fn token_expiry_uses_virtual_clock() {
    let test = Test::start().await;
    let headers = test.bot_headers("10000").await;
    test.settings(json!({"clock_mode": "virtual"})).await;
    let response = test
        .http()
        .get(format!("{}/users/@me", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);

    test.ctl_ok(
        "POST",
        "/_mqs/clock/advance",
        Some(json!({"duration": "2h"})),
    )
    .await;
    let response = test
        .http()
        .get(format!("{}/users/@me", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], 11242);
}

#[tokio::test]
async fn validation_errors_are_qq_shaped() {
    let test = Test::start().await;
    let (status, body) = test
        .ctl(
            "POST",
            "/_mqs/chat/send",
            Some(json!({"content": "no user"})),
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["code"], 10001);
    assert!(body.get("detail").is_none());
    assert!(body["message"]
        .as_str()
        .unwrap()
        .starts_with("invalid parameter"));

    let (status, body) = test
        .ctl(
            "POST",
            "/_mqs/chat/send",
            Some(json!({"as_user": "u_x", "to": "group:nope", "content": "x"})),
        )
        .await;
    assert!(status == 400 || status == 403);
    assert!(body.get("detail").is_none());
}
