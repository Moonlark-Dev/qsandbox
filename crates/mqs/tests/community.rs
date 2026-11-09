//! Group management: info, bot state, mute, blacklist, join requests.

mod common;

use common::Test;
use serde_json::{json, Value};

async fn whitelist(test: &Test) {
    test.ctl_ok(
        "PATCH",
        "/_mqs/bots/10000",
        Some(json!({"whitelisted": true})),
    )
    .await;
}

#[tokio::test]
async fn group_info_and_bot_state() {
    let test = Test::start().await;
    whitelist(&test).await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let headers = test.bot_headers("10000").await;
    let info: Value = test
        .http()
        .get(format!("{}/v2/groups/{openid}/info", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["group_openid"], openid);
    assert!(info["group_member_num"].as_i64().unwrap() >= 4);

    let state: Value = test
        .http()
        .get(format!("{}/v2/groups/{openid}/bot_state", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["allow_proactive_msg"], true);
    assert_eq!(state["recv_msg_setting"], "all");
    assert_eq!(state["member_role"], "member");
}

#[tokio::test]
async fn group_mute_add_and_del() {
    let test = Test::start().await;
    whitelist(&test).await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let headers = test.bot_headers("10000").await;
    let members: Value = test
        .ctl_ok("GET", &format!("/_mqs/groups/{group}/members"), None)
        .await;
    let target = members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "member")
        .unwrap();
    let user_id = target["user_id"].as_str().unwrap().to_string();
    let member_openid = test.user_openid("10000", &user_id).await;

    let expire = test
        .world()
        .clock
        .iso(Some(test.world().clock.now_ms() + 600_000));
    let response = test
        .http()
        .post(format!(
            "{}/v2/groups/{openid}/restrict_chat_setting",
            test.sim
        ))
        .header("authorization", &headers)
        .json(&json!({"members": [{"op": "add", "member_openid": member_openid, "mute_expire_at": expire}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        200,
        "{:?}",
        response.text().await
    );

    let state: Value = test
        .http()
        .get(format!(
            "{}/v2/groups/{openid}/restrict_chat_setting",
            test.sim
        ))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["members"][0]["member_openid"], member_openid);

    let response = test
        .http()
        .post(format!(
            "{}/v2/groups/{openid}/restrict_chat_setting",
            test.sim
        ))
        .header("authorization", &headers)
        .json(&json!({"members": [{"op": "del", "member_openid": member_openid}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let state: Value = test
        .http()
        .get(format!(
            "{}/v2/groups/{openid}/restrict_chat_setting",
            test.sim
        ))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["members"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn group_blacklist_add_remove() {
    let test = Test::start().await;
    whitelist(&test).await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let headers = test.bot_headers("10000").await;
    let members: Value = test
        .ctl_ok("GET", &format!("/_mqs/groups/{group}/members"), None)
        .await;
    let target = members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "member")
        .unwrap();
    let user_id = target["user_id"].as_str().unwrap().to_string();
    let member_openid = test.user_openid("10000", &user_id).await;

    // still in the group -> add fails and is reported in fail_openids
    let body: Value = test
        .http()
        .post(format!("{}/v2/groups/{openid}/member_blacklist", test.sim))
        .header("authorization", &headers)
        .json(&json!({"op": "add", "member_openids": [member_openid]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["fail_openids"][0], member_openid);

    test.ctl_ok(
        "DELETE",
        &format!("/_mqs/groups/{group}/members/{user_id}"),
        None,
    )
    .await;
    let body: Value = test
        .http()
        .post(format!("{}/v2/groups/{openid}/member_blacklist", test.sim))
        .header("authorization", &headers)
        .json(&json!({"op": "add", "member_openids": [member_openid]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["fail_openids"].as_array().unwrap().len(), 0);

    let page: Value = test
        .http()
        .get(format!("{}/v2/groups/{openid}/member_blacklist", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["users"][0]["member_openid"], member_openid);

    let body: Value = test
        .http()
        .post(format!("{}/v2/groups/{openid}/member_blacklist", test.sim))
        .header("authorization", &headers)
        .json(&json!({"op": "del", "member_openids": [member_openid]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["fail_openids"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn join_request_approve_and_decline() {
    let test = Test::start().await;
    whitelist(&test).await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let headers = test.bot_headers("10000").await;
    let outsider: Value = test
        .ctl_ok(
            "POST",
            "/_mqs/users",
            Some(json!({"nickname": "门外汉", "actor_type": "human"})),
        )
        .await;
    let outsider_id = outsider["id"].as_str().unwrap().to_string();

    test.ctl_ok(
        "POST",
        &format!("/_mqs/groups/{group}/join-requests"),
        Some(json!({"user_id": outsider_id, "verify_message": "让我进去"})),
    )
    .await;
    let list: Value = test
        .http()
        .get(format!("{}/v2/groups/{openid}/join_request_list", test.sim))
        .header("authorization", &headers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["list"].as_array().unwrap().len(), 1);
    let request_id = list["list"][0]["join_request_id"]
        .as_str()
        .unwrap()
        .to_string();
    let outsider_openid = test.user_openid("10000", &outsider_id).await;

    let response = test
        .http()
        .post(format!(
            "{}/v2/groups/{openid}/approval_join_request/{outsider_openid}",
            test.sim
        ))
        .header("authorization", &headers)
        .json(&json!({"op": "approve", "join_request_id": request_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let members: Value = test
        .ctl_ok("GET", &format!("/_mqs/groups/{group}/members"), None)
        .await;
    assert!(members
        .as_array()
        .unwrap()
        .iter()
        .any(|member| member["user_id"] == outsider_id));

    // decline with blacklist
    let second: Value = test
        .ctl_ok(
            "POST",
            "/_mqs/users",
            Some(json!({"nickname": "再门外汉", "actor_type": "human"})),
        )
        .await;
    let second_id = second["id"].as_str().unwrap().to_string();
    test.ctl_ok(
        "POST",
        &format!("/_mqs/groups/{group}/join-requests"),
        Some(json!({"user_id": second_id})),
    )
    .await;
    let second_openid = test.user_openid("10000", &second_id).await;
    let response = test
        .http()
        .post(format!(
            "{}/v2/groups/{openid}/approval_join_request/{second_openid}",
            test.sim
        ))
        .header("authorization", &headers)
        .json(&json!({"op": "decline", "reject_reason": "no", "add_to_member_blacklist": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let blacklist: Value = test
        .ctl_ok("GET", &format!("/_mqs/groups/{group}/blacklist"), None)
        .await;
    assert!(blacklist
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["user_id"] == second_id));
}

#[tokio::test]
async fn proactive_switch_blocks_active_messages() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    test.ctl_ok(
        "PATCH",
        "/_mqs/bots/10000",
        Some(json!({"allow_proactive_msg": false})),
    )
    .await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let headers = test.bot_headers("10000").await;
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", &headers)
        .json(&json!({"content": "proactive"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 403);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40040);
}

#[tokio::test]
async fn group_add_remove_robot_events() {
    let test = Test::start().await;
    let (mut ws, _ready, _token) = test.connect_gateway("10000", 1 << 25).await;
    test.ctl_ok("POST", "/_mqs/bots", Some(json!({"app_id": "10002"})))
        .await;
    let group = test.group_id().await;
    test.ctl_ok("POST", &format!("/_mqs/groups/{group}/bots/10002"), None)
        .await;
    // the gateway connection is for bot 10000; the event targets 10002, so read the DB
    let events = test.world().list_events(0, 50, None).unwrap();
    let types: Vec<String> = events
        .iter()
        .filter_map(|event| event["type"].as_str().map(str::to_string))
        .collect();
    assert!(types.contains(&"GROUP_ADD_ROBOT".to_string()), "{types:?}");
    ws.close(None).await.ok();
}
