//! Status endpoint: the GUI/CLI/MCP entry point must always answer.

mod common;

use common::Test;
use serde_json::json;

#[tokio::test]
async fn status_reports_gateway_and_coverage_without_deadlock() {
    let test = Test::start().await;
    let data = test.ctl_ok("GET", "/_mqs/status", None).await;
    assert_eq!(data["profile"], "test");
    assert_eq!(data["gateway"]["connections"], 0);
    assert!(data["coverage"]["ascii"]
        .as_str()
        .unwrap()
        .contains("endpoints"));
    assert!(data["settings"]["rate_mode"].is_string());
    assert_eq!(
        data["sim_port"].as_u64(),
        Some(test.handle.sim_addr.port() as u64)
    );

    // with a live gateway connection the same endpoint must still answer
    let (ws, _ready, _token) = test.connect_gateway("10000", 1 << 25).await;
    let data = test.ctl_ok("GET", "/_mqs/status", None).await;
    assert_eq!(data["gateway"]["connections"], 1);
    assert_eq!(data["gateway"]["bots"], json!(["10000"]));
    drop(ws);
}

#[tokio::test]
async fn dashboard_settings_and_counts() {
    let test = Test::start().await;
    let status = test.ctl_ok("GET", "/_mqs/status", None).await;
    for key in ["users", "bots", "groups", "messages", "events", "frames"] {
        assert!(status["counts"][key].as_i64().unwrap() >= 0, "{key}");
    }
    let settings = test.ctl_ok("GET", "/_mqs/settings", None).await;
    assert_eq!(settings["rate_mode"], "certified");
    let patched = test
        .ctl_ok(
            "PATCH",
            "/_mqs/settings",
            Some(json!({"rate_mode": "off", "theme": "dark"})),
        )
        .await;
    assert_eq!(patched["rate_mode"], "off");
    assert_eq!(patched["theme"], "dark");
}
