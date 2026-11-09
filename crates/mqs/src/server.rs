//! Boot one MQS process: shared world + two listeners + runtime handshake.

use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::clock::Clock;
use crate::config::{detect_lan_ip, write_runtime, ProcessConfig, ProfileLock};
use crate::control::control_router;
use crate::db::Database;
use crate::error::{ApiError, MqsResult};
use crate::gateway::GatewayManager;
use crate::http::AppState;
use crate::sim::sim_router;
use crate::world::World;

pub struct ServerHandle {
    pub state: Arc<AppState>,
    pub world: Arc<World>,
    pub sim_addr: SocketAddr,
    pub control_addr: SocketAddr,
    pub handshake: Value,
    lock: Option<ProfileLock>,
    shutdown: Option<watch::Sender<bool>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl ServerHandle {
    /// Graceful shutdown: stop both listeners, close sockets, release the profile.
    pub async fn shutdown(mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(true);
        }
        for task in self.tasks.drain(..) {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
        }
        if let Some(gateway) = self.world.gateway.get() {
            gateway.close_all().await;
        }
        crate::config::clear_runtime(&self.state.config.home, &self.state.config.profile);
        self.lock.take();
    }
}

pub async fn start(mut config: ProcessConfig) -> Result<ServerHandle, ApiError> {
    let paths = config.paths();
    paths.ensure()?;
    let mut lock = ProfileLock::new(paths.clone());
    lock.acquire()?;

    let db = Arc::new(Database::open(&paths.db())?);
    let clock = Arc::new(Clock::new("real"));
    let world = World::new(db.clone(), clock, &config.profile, paths.clone());
    world.start()?;

    let gateway = Arc::new(GatewayManager::new(Arc::downgrade(&world)));
    let _ = world.gateway.set(gateway.clone());
    gateway.spawn_watchdog();

    if config.public_host.is_empty() && (config.host == "0.0.0.0" || config.host == "::") {
        config.public_host = detect_lan_ip();
    }

    let sim_listener = TcpListener::bind((config.host.as_str(), config.sim_port))
        .await
        .map_err(|err| {
            ApiError::new(
                10002,
                format!("cannot bind sim port {}: {err}", config.sim_port),
            )
        })?;
    let control_listener = TcpListener::bind((config.host.as_str(), config.control_port))
        .await
        .map_err(|err| {
            ApiError::new(
                10002,
                format!("cannot bind control port {}: {err}", config.control_port),
            )
        })?;
    let sim_addr = sim_listener
        .local_addr()
        .map_err(|err| ApiError::new(10002, format!("cannot read sim port: {err}")))?;
    let control_addr = control_listener
        .local_addr()
        .map_err(|err| ApiError::new(10002, format!("cannot read control port: {err}")))?;
    config.sim_port = sim_addr.port();
    config.control_port = control_addr.port();
    let nonce = crate::ids::short_token(32);
    config.gui_url = format!(
        "http://{}:{}/#t={}",
        config.bind_host(),
        config.control_port,
        nonce
    );
    *world.sim_base.write().unwrap() = config.base_url();

    let state = AppState::new_with_nonce(world.clone(), config.clone(), nonce);
    let sim = sim_router(state.clone());
    let control = control_router(state.clone());

    let mut handshake = config.handshake();
    handshake["started_at"] = Value::String(world.clock.iso(None));
    write_runtime(&config.home, &config.profile, &handshake)?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut sim_rx = shutdown_rx.clone();
    let sim_task = tokio::spawn(async move {
        let _ = axum::serve(sim_listener, sim)
            .with_graceful_shutdown(async move {
                while !*sim_rx.borrow() {
                    if sim_rx.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
    });
    let mut control_rx = shutdown_rx.clone();
    let control_task = tokio::spawn(async move {
        let _ = axum::serve(control_listener, control)
            .with_graceful_shutdown(async move {
                while !*control_rx.borrow() {
                    if control_rx.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
    });

    Ok(ServerHandle {
        state,
        world,
        sim_addr,
        control_addr,
        handshake,
        lock: Some(lock),
        shutdown: Some(shutdown_tx),
        tasks: vec![sim_task, control_task],
    })
}

/// Blocking entry point used by `mqs serve`.
pub async fn serve(config: ProcessConfig) -> MqsResult<()> {
    let handle = start(config).await?;
    let state = handle.state.clone();
    let handshake = handle.handshake.clone();
    eprintln!(
        "\n  QSandbox  ·  profile={}  ·  fake data only, not affiliated with Tencent\n\
         \x20 Simulate API : {}\n\
         \x20 Gateway      : {}\n\
         \x20 Control GUI  : {}\n\
         \x20 Control token: {}\n",
        state.config.profile,
        state.config.base_url(),
        state.config.ws_url(),
        state.config.gui_url,
        state.config.control_token,
    );
    println!("{}", handshake);
    use std::io::Write;
    let _ = std::io::stdout().flush();

    let mut signals = SignalSet::new();
    tokio::select! {
        _ = signals.wait() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    handle.shutdown().await;
    Ok(())
}

struct SignalSet {
    #[cfg(unix)]
    sigterm: Option<tokio::signal::unix::Signal>,
}

impl SignalSet {
    fn new() -> Self {
        Self {
            #[cfg(unix)]
            sigterm: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok(),
        }
    }

    async fn wait(&mut self) {
        #[cfg(unix)]
        {
            if let Some(signal) = &mut self.sigterm {
                let _ = signal.recv().await;
            }
        }
        #[cfg(not(unix))]
        {
            futures_util::future::pending::<()>().await;
        }
    }
}

/// Start in-process and return the handshake (used by tests and embedding).
pub async fn start_headless(config: ProcessConfig) -> MqsResult<ServerHandle> {
    start(config).await
}
