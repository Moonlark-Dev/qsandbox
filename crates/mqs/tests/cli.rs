//! CLI: every command is a thin client of the control API.

mod common;

use common::{spawn_echo_bot, Test};
use std::process::Command;

fn mqs(test: &Test, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mqs"))
        .args(["--home", test.home().to_str().unwrap(), "--profile", "test"])
        .args(args)
        .env("MQS_HOME", test.home())
        .env("MQS_PROFILE", "test")
        .output()
        .expect("mqs binary runs")
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_json() {
    let test = Test::start().await;
    let output = mqs(&test, &["--json", "status"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(data["profile"], "test");
    assert_eq!(
        data["sim_port"].as_u64(),
        Some(test.handle.sim_addr.port() as u64)
    );
    assert!(data["coverage"]["ascii"]
        .as_str()
        .unwrap()
        .contains("endpoints"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_json() {
    let test = Test::start().await;
    let output = mqs(&test, &["--json", "token", "--bot", "10000"]);
    assert!(output.status.success());
    let data: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert!(data["access_token"].as_str().unwrap().starts_with("MQS_"));
    assert_eq!(data["app_id"], "10000");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_and_group_commands() {
    let test = Test::start().await;
    let output = mqs(&test, &["--json", "user", "create", "--name", "命令行用户"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let user: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(user["nickname"], "命令行用户");

    let output = mqs(&test, &["--json", "user", "list", "--available"]);
    let users: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert!(users.as_array().unwrap().len() >= 2);

    let output = mqs(
        &test,
        &[
            "--json",
            "group",
            "create",
            "--name",
            "命令行群",
            "--members",
            "2",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let group: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(group["name"], "命令行群");

    let output = mqs(&test, &["--json", "group", "list"]);
    let groups: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert!(groups.as_array().unwrap().len() >= 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_run_and_junit() {
    let test = Test::start().await;
    let bot = spawn_echo_bot(&test).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let junit = test.home().join("junit.xml");
    let output = mqs(
        &test,
        &[
            "--json",
            "script",
            "run",
            "smoke",
            "--junit",
            junit.to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(data["ok"], true, "{data}");
    let junit_text = std::fs::read_to_string(&junit).unwrap();
    assert!(junit_text.contains("<testsuite"));
    bot.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_runtime_exits_3() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mqs"))
        .args([
            "--home",
            dir.path().to_str().unwrap(),
            "--profile",
            "nope",
            "status",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no running MQS server"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor() {
    let test = Test::start().await;
    let output = mqs(&test, &["doctor"]);
    assert!(output.status.success(), "{}", stdout(&output));
    let text = stdout(&output);
    assert!(text.contains("[ok ] profile dir writable"));
    assert!(text.contains("[ok ] running server"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contract_export_is_offline() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("COVERAGE.md");
    let types = dir.path().join("api.ts");
    let output = Command::new(env!("CARGO_BIN_EXE_mqs"))
        .args([
            "contract",
            "export",
            "--out",
            out.to_str().unwrap(),
            "--types",
            types.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let markdown = std::fs::read_to_string(&out).unwrap();
    assert!(markdown.contains("接口覆盖率"));
    assert!(markdown.contains("/v2/groups/{group_openid}/messages"));
    let ts = std::fs::read_to_string(&types).unwrap();
    assert!(ts.contains("export interface MessageSendRequest"));
}
