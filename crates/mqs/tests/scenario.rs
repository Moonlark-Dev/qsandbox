//! Scenario engine: builtin catalog, smoke with a real echo bot, JUnit, http step.

mod common;

use common::{spawn_echo_bot, Test};
use serde_json::{json, Value};

#[tokio::test]
async fn builtin_catalog() {
    let test = Test::start().await;
    let data = test.ctl_ok("GET", "/_mqs/scenarios", None).await;
    let items = data.as_array().unwrap();
    let builtins: Vec<&Value> = items
        .iter()
        .filter(|item| item["source"] == "builtin")
        .collect();
    assert_eq!(builtins.len(), 10);
    for names in ["smoke", "c2c", "rate_limit", "media", "quote"] {
        assert!(builtins.iter().any(|item| item["name"] == names), "{names}");
    }
}

#[tokio::test]
async fn smoke_scenario_with_echo_bot() {
    let test = Test::start().await;
    let bot = spawn_echo_bot(&test).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let script = mqs::scenario::builtin("smoke").unwrap();
    let (status, data) = test
        .ctl(
            "POST",
            "/_mqs/scenarios/run",
            Some(json!({"script": script})),
        )
        .await;
    assert_eq!(status, 200, "{data}");
    assert_eq!(data["ok"], true, "{data}");
    assert_eq!(data["steps"].as_array().unwrap().len(), 3);
    assert!(data["junit"].as_str().unwrap().contains("<testsuite"));
    bot.abort();
}

#[tokio::test]
async fn failure_reports_step_and_junit() {
    let test = Test::start().await;
    let script = r#"
name: failing
setup:
  settings:
    rate_mode: "off"
  users:
    - alias: alice
  groups:
    - alias: room
      owner: alice
      members: [alice]
steps:
  - send:
      as: alice
      to: room
      text: ping
  - expect_event:
      type: NEVER_GOING_TO_HAPPEN
      timeout: 300ms
"#;
    let data = test
        .ctl_ok(
            "POST",
            "/_mqs/scenarios/run",
            Some(json!({"script": script})),
        )
        .await;
    assert_eq!(data["ok"], false);
    assert!(data["error"].as_str().unwrap().contains("timeout"));
    let last = data["steps"].as_array().unwrap().last().unwrap();
    assert_eq!(last["ok"], false);
    assert!(data["junit"].as_str().unwrap().contains("<failure"));
    assert!(data["tap"].as_str().unwrap().contains("not ok"));
}

#[tokio::test]
async fn http_assertion_step() {
    let test = Test::start().await;
    let script = r#"
name: http-step
setup:
  settings:
    rate_mode: "off"
  bots:
    - app_id: "10000"
      name: Step Bot
steps:
  - http:
      bot: "10000"
      method: GET
      path: /users/@me
      expect_status: 200
      capture:
        bot_id: id
  - http:
      bot: "10000"
      method: POST
      path: /v2/groups/{openid}/messages
      target: group
      group: room
      json:
        content: "should fail offline"
      expect_code: 40056
"#;
    let data = test
        .ctl_ok(
            "POST",
            "/_mqs/scenarios/run",
            Some(json!({"script": script})),
        )
        .await;
    // the scenario has no group alias, so the second step fails with unknown openid
    assert_eq!(data["ok"], false);
    assert!(data["steps"].as_array().unwrap()[0]["ok"]
        .as_bool()
        .unwrap());
}

#[tokio::test]
async fn passive_window_template_without_bot() {
    let test = Test::start().await;
    let script = mqs::scenario::builtin("passive_window").unwrap();
    let data = test
        .ctl_ok(
            "POST",
            "/_mqs/scenarios/run",
            Some(json!({"script": script})),
        )
        .await;
    assert!(data["ok"].as_bool().unwrap(), "{data}");
}

#[tokio::test]
async fn group_recv_msg_setting() {
    let test = Test::start().await;
    let script = r#"
name: recv-setting
setup:
  settings:
    rate_mode: "off"
  users:
    - alias: alice
  groups:
    - alias: room
      owner: alice
      members: [alice]
      bots: ["10000"]
      recv_msg_setting: only_mention
steps:
  - send:
      as: alice
      to: room
      text: hey
      at_bots: ["10000"]
  - expect_event:
      type: GROUP_AT_MESSAGE_CREATE
      conversation: "group:{{room}}"
      content_contains: hey
"#;
    let data = test
        .ctl_ok(
            "POST",
            "/_mqs/scenarios/run",
            Some(json!({"script": script})),
        )
        .await;
    assert_eq!(data["ok"], true, "{data}");

    let bad = script.replace("only_mention", "nonsense");
    let (status, data) = test
        .ctl("POST", "/_mqs/scenarios/run", Some(json!({"script": bad})))
        .await;
    assert_eq!(status, 200);
    assert_eq!(data["ok"], false);
}
