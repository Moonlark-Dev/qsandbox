//! Invite-only (11253) APIs, sandbox exposure switch and pagination.

mod common;

use common::Test;
use serde_json::{json, Value};

#[tokio::test]
async fn public_bot_gets_11253() {
    let test = Test::start().await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let headers = test.bot_headers("10000").await;
    let response = test
        .http()
        .get(format!("{}/v2/groups/{openid}/members", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 403);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 11253);
    assert!(body["hint"].as_str().unwrap().contains("白名单"));
}

#[tokio::test]
async fn whitelisted_bot_can_list_members() {
    let test = Test::start().await;
    test.ctl_ok(
        "PATCH",
        "/_mqs/bots/10000",
        Some(json!({"whitelisted": true})),
    )
    .await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let headers = test.bot_headers("10000").await;
    let body: Value = test
        .http()
        .get(format!("{}/v2/groups/{openid}/members", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let members = body["members"].as_array().unwrap();
    // bootstrap world: 4 virtual users + the default bot
    assert_eq!(members.len(), 5, "{members:?}");
    assert!(members
        .iter()
        .any(|member| member["member_role"] == "owner"));
    // bots are members too, and carry bot: true
    assert!(members.iter().any(|member| member["bot"] == true));

    let member_openid = members
        .iter()
        .find(|member| member["bot"] == false)
        .unwrap()["member_openid"]
        .as_str()
        .unwrap()
        .to_string();
    let detail: Value = test
        .http()
        .get(format!(
            "{}/v2/groups/{openid}/members/{member_openid}",
            test.sim
        ))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["member_openid"], member_openid);
}

#[tokio::test]
async fn sandbox_can_disable_tencent_whitelist() {
    let test = Test::start().await;
    test.settings(json!({"feature.whitelist": false})).await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let headers = test.bot_headers("10000").await;
    let response = test
        .http()
        .get(format!("{}/v2/groups/{openid}/members", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
}

#[tokio::test]
async fn expose_public_hides_invite_endpoints() {
    let test = Test::start().await;
    test.ctl_ok(
        "PATCH",
        "/_mqs/bots/10000",
        Some(json!({"whitelisted": true})),
    )
    .await;
    test.settings(json!({"feature.expose": "public"})).await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let headers = test.bot_headers("10000").await;
    let response = test
        .http()
        .get(format!("{}/v2/groups/{openid}/members", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
async fn member_list_pagination_cursor() {
    let test = Test::start().await;
    test.ctl_ok(
        "PATCH",
        "/_mqs/bots/10000",
        Some(json!({"whitelisted": true})),
    )
    .await;
    let group = test.group_id().await;
    for index in 0..35 {
        let user: Value = test
            .ctl_ok(
                "POST",
                "/_mqs/users",
                Some(json!({"nickname": format!("批量{index}"), "actor_type": "human"})),
            )
            .await;
        test.ctl_ok(
            "POST",
            &format!("/_mqs/groups/{group}/members"),
            Some(json!({"user_id": user["id"]})),
        )
        .await;
    }
    let openid = test.group_openid("10000", &group).await;
    let headers = test.bot_headers("10000").await;
    let first: Value = test
        .http()
        .get(format!("{}/v2/groups/{openid}/members", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["members"].as_array().unwrap().len(), 30);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    assert!(!cursor.is_empty());
    let second: Value = test
        .http()
        .get(format!(
            "{}/v2/groups/{openid}/members?cursor={cursor}",
            test.sim
        ))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!second["members"].as_array().unwrap().is_empty());
}
