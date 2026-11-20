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
    assert_eq!(
        data["local_base_url"],
        format!("http://127.0.0.1:{}", test.handle.sim_addr.port())
    );
    assert_eq!(
        data["local_ws_url"],
        format!("ws://127.0.0.1:{}/websocket", test.handle.sim_addr.port())
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
async fn json_mode_reports_errors_as_json_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mqs"))
        .args([
            "--home",
            dir.path().to_str().unwrap(),
            "--profile",
            "nope",
            "--json",
            "status",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let data: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("stdout must be one JSON document");
    assert_eq!(data["ok"], false);
    assert_eq!(data["exit_code"], 3);
    assert!(data["message"]
        .as_str()
        .unwrap()
        .contains("no running MQS server"));
    assert!(data["hint"].as_str().unwrap().contains("mqs serve"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_help_documents_session_syntax_and_errors_hint_at_it() {
    let test = Test::start().await;
    let output = mqs(&test, &["send", "--help"]);
    let help = stdout(&output);
    assert!(help.contains("group:<group_id>"), "{help}");
    assert!(help.contains("dm:<app_id>:<user_id>"), "{help}");

    let user = test.user_id().await;
    let output = mqs(
        &test,
        &[
            "--json",
            "send",
            "--as-user",
            &user,
            "--to",
            "user:u_000001",
            "--text",
            "hi",
        ],
    );
    assert_eq!(output.status.code(), Some(5));
    let error: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(error["ok"], false);
    assert_eq!(error["exit_code"], 5);
    assert_eq!(error["code"], 10001);
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("cannot resolve conversation"), "{message}");
    assert!(message.contains("group:<group_id>"), "{message}");
    assert!(message.contains("dm:<app_id>:<user_id>"), "{message}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bot_list_redacts_secret_unless_asked() {
    let test = Test::start().await;
    let output = mqs(&test, &["--json", "bot", "list"]);
    let bots: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(bots[0]["secret"], "***");

    let output = mqs(&test, &["--json", "--show-secret", "bot", "list"]);
    let bots: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(bots[0]["secret"], "mqs-secret-10000");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_wait_reply_and_messages_commands() {
    let test = Test::start().await;
    let bot = spawn_echo_bot(&test).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let user = test.user_id().await;
    let conversation = format!("group:{}", test.group_id().await);

    let output = mqs(
        &test,
        &[
            "--json",
            "send",
            "--as-user",
            &user,
            "--to",
            &conversation,
            "--text",
            "ping",
            "--at-bot",
            "10000",
            "--wait-reply",
            "5s",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(data["sent"]["delivered_to"], serde_json::json!(["10000"]));
    assert_eq!(data["reply"]["sender_type"], "bot");
    assert_eq!(data["reply"]["content"], "echo: ping");
    let msg_id = data["reply"]["msg_id"].as_str().unwrap().to_string();

    let output = mqs(
        &test,
        &[
            "--json",
            "messages",
            "list",
            "--conversation",
            &conversation,
        ],
    );
    let list: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let items = list["items"].as_array().unwrap();
    assert!(items.len() >= 2, "{list}");
    assert_eq!(items.last().unwrap()["content"], "echo: ping");

    let output = mqs(&test, &["--json", "messages", "show", &msg_id]);
    let message: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(message["content"], "echo: ping");
    assert_eq!(message["msg_id"], msg_id);
    bot.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_wait_reply_times_out_with_json_error() {
    let test = Test::start().await;
    let user = test.user_id().await;
    let conversation = format!("group:{}", test.group_id().await);
    let output = mqs(
        &test,
        &[
            "--json",
            "send",
            "--as-user",
            &user,
            "--to",
            &conversation,
            "--text",
            "nobody is listening",
            "--wait-reply",
            "300ms",
        ],
    );
    assert_eq!(output.status.code(), Some(5));
    let error: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(error["ok"], false);
    assert!(error["message"].as_str().unwrap().contains("no bot reply"));
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
