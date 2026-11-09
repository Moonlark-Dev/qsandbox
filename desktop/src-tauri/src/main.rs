//! QSandbox desktop shell.
//!
//! Tauri is only the window: the sandbox server runs in this same process, so a
//! desktop install is one binary and needs no sidecar (no Nuitka, no Python).
//! The window loads the control GUI served by the embedded server.

use std::sync::Mutex;

use mqs::config::ProcessConfig;
use mqs::server::ServerHandle;
use tauri::Manager;

struct DesktopState {
    server: Mutex<Option<ServerHandle>>,
}

fn main() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(DesktopState {
            server: Mutex::new(None),
        })
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let home = data_dir.join("mqs-home");
            let config = ProcessConfig {
                profile: std::env::var("MQS_PROFILE").unwrap_or_else(|_| "desktop".into()),
                home,
                host: "127.0.0.1".into(),
                sim_port: 0,
                control_port: 0,
                control_token: String::new(),
                log_level: "warn".into(),
                control_origins: Vec::new(),
                expose_docs: false,
                gui_url: String::new(),
                public_host: String::new(),
            };
            let handle = tauri::async_runtime::block_on(mqs::server::start(config))
                .map_err(|err| format!("QSandbox server failed to start: {}", err.message))?;
            let url = handle.state.config.gui_url.clone();
            let parsed = url
                .parse()
                .map_err(|err| format!("invalid GUI url {url}: {err}"))?;

            tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(parsed))
                .title("QSandbox · QQ Bot Sandbox")
                .inner_size(1320.0, 860.0)
                .min_inner_size(960.0, 640.0)
                .build()?;

            let state = app.state::<DesktopState>();
            *state.server.lock().unwrap() = Some(handle);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building QSandbox");

    app.run(|app_handle, event| {
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
            // Take the handle out first: holding the MutexGuard across the
            // blocking shutdown would borrow `state` for too long.
            let state = app_handle.state::<DesktopState>();
            let mut guard = state.server.lock().unwrap();
            let server = guard.take();
            drop(guard);
            if let Some(server) = server {
                tauri::async_runtime::block_on(server.shutdown());
            }
        }
    });
}
