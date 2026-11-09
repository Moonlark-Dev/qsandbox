//! Profile layout, process settings and the single-process file lock.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{ApiError, MqsResult};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_PROFILE: &str = "default";
pub const DEFAULT_SIM_PORT: u16 = 5700;
pub const DEFAULT_CONTROL_PORT: u16 = 5701;
pub const SCHEMA_VERSION: i64 = 1;

pub const RUNTIME_FILE: &str = "runtime.json";
pub const PROFILE_FILE: &str = "profile.json";
pub const DB_FILE: &str = "db.sqlite";
pub const MEDIA_DIR: &str = "media";
pub const LOCK_FILE: &str = "profile.lock";

pub fn default_home() -> PathBuf {
    std::env::var("MQS_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| resolve_home(Some(&v)))
        .unwrap_or_else(|| resolve_home(Some("~/.mqs")))
}

pub fn resolve_home(value: Option<&str>) -> PathBuf {
    match value {
        None | Some("") => default_home(),
        Some(text) => expand_tilde(text),
    }
}

fn expand_tilde(text: &str) -> PathBuf {
    if let Some(rest) = text.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    let path = PathBuf::from(text);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}

fn now_iso() -> String {
    chrono::Utc::now()
        .with_timezone(&crate::clock::tz())
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

#[derive(Debug, Clone)]
pub struct ProfilePaths {
    pub home: PathBuf,
    pub name: String,
}

impl ProfilePaths {
    pub fn new(home: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self {
            home: home.into(),
            name: name.into(),
        }
    }

    pub fn directory(&self) -> PathBuf {
        self.home.join("profiles").join(&self.name)
    }

    pub fn db(&self) -> PathBuf {
        self.directory().join(DB_FILE)
    }

    pub fn media(&self) -> PathBuf {
        self.directory().join(MEDIA_DIR)
    }

    pub fn runtime(&self) -> PathBuf {
        self.directory().join(RUNTIME_FILE)
    }

    pub fn meta(&self) -> PathBuf {
        self.directory().join(PROFILE_FILE)
    }

    pub fn lock(&self) -> PathBuf {
        self.directory().join(LOCK_FILE)
    }

    pub fn ensure(&self) -> MqsResult<()> {
        std::fs::create_dir_all(self.directory())?;
        std::fs::create_dir_all(self.media())?;
        self.update_index();
        if !self.meta().exists() {
            let meta = serde_json::json!({
                "name": self.name,
                "schema_version": SCHEMA_VERSION,
                "created_at": now_iso(),
                "notice": "MQS sandbox profile — fake data only, not affiliated with Tencent",
            });
            write_json_atomic(&self.meta(), &meta)?;
        }
        Ok(())
    }

    /// Maintain the global `profiles.json` index.
    fn update_index(&self) {
        let index_path = self.home.join("profiles.json");
        let mut data: Value = std::fs::read_to_string(&index_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| serde_json::json!({"profiles": {}}));
        if !data.get("profiles").map(|v| v.is_object()).unwrap_or(false) {
            data["profiles"] = serde_json::json!({});
        }
        let entry = &mut data["profiles"][&self.name];
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        entry["name"] = Value::String(self.name.clone());
        entry["path"] = Value::String(self.directory().to_string_lossy().into());
        if entry.get("created_at").is_none() {
            entry["created_at"] = Value::String(now_iso());
        }
        entry["last_opened_at"] = Value::String(now_iso());
        let _ = write_json_atomic(&index_path, &data);
    }
}

pub fn write_json_atomic(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Exclusive lock so a profile is only ever written by one process.
pub struct ProfileLock {
    paths: ProfilePaths,
    file: Option<File>,
}

impl ProfileLock {
    pub fn new(paths: ProfilePaths) -> Self {
        Self { paths, file: None }
    }

    pub fn acquire(&mut self) -> MqsResult<()> {
        self.paths.ensure()?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(self.paths.lock())?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc != 0 {
                return Err(ApiError::new(
                    10002,
                    format!(
                        "profile {:?} is locked by another mqs process",
                        self.paths.name
                    ),
                ));
            }
        }
        file.set_len(0)?;
        write!(file, "{}", std::process::id())?;
        file.flush()?;
        self.file = Some(file);
        Ok(())
    }

    pub fn release(&mut self) {
        if let Some(file) = self.file.take() {
            #[cfg(unix)]
            {
                use std::os::unix::io::AsRawFd;
                unsafe {
                    libc::flock(file.as_raw_fd(), libc::LOCK_UN);
                }
            }
            drop(file);
            let _ = std::fs::remove_file(self.paths.lock());
        }
    }
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Debug, Clone)]
pub struct ProcessConfig {
    pub profile: String,
    pub home: PathBuf,
    pub host: String,
    pub sim_port: u16,
    pub control_port: u16,
    pub control_token: String,
    pub log_level: String,
    pub control_origins: Vec<String>,
    pub expose_docs: bool,
    pub gui_url: String,
    pub public_host: String,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            profile: DEFAULT_PROFILE.to_string(),
            home: default_home(),
            host: "127.0.0.1".into(),
            sim_port: DEFAULT_SIM_PORT,
            control_port: DEFAULT_CONTROL_PORT,
            control_token: crate::ids::short_token(32),
            log_level: "info".into(),
            control_origins: Vec::new(),
            expose_docs: true,
            gui_url: String::new(),
            public_host: String::new(),
        }
    }
}

impl ProcessConfig {
    pub fn paths(&self) -> ProfilePaths {
        ProfilePaths::new(self.home.clone(), self.profile.clone())
    }

    pub fn dial_host(&self) -> String {
        if !self.public_host.is_empty() {
            return self.public_host.clone();
        }
        if self.host == "0.0.0.0" || self.host == "::" {
            return "127.0.0.1".into();
        }
        self.host.clone()
    }

    /// Host part used in the GUI URL (0.0.0.0 is not browsable).
    pub fn bind_host(&self) -> String {
        if self.host == "0.0.0.0" || self.host == "::" {
            "127.0.0.1".into()
        } else {
            self.host.clone()
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.dial_host(), self.sim_port)
    }

    pub fn ws_url(&self) -> String {
        format!("ws://{}:{}/websocket", self.dial_host(), self.sim_port)
    }

    pub fn handshake(&self) -> Value {
        serde_json::json!({
            "mqs": 1,
            "version": VERSION,
            "profile": self.profile,
            "host": self.host,
            "sim_port": self.sim_port,
            "control_port": self.control_port,
            "token": self.control_token,
            "base_url": self.base_url(),
            "ws_url": self.ws_url(),
            "pid": std::process::id(),
            "gui_url": self.gui_url,
        })
    }
}

pub fn read_runtime(home: &Path, profile: &str) -> Result<Value, ApiError> {
    let path = ProfilePaths::new(home, profile).runtime();
    let text = std::fs::read_to_string(&path).map_err(|_| {
        ApiError::new(
            10002,
            format!(
                "no running MQS server for profile {profile:?} ({} missing)",
                path.display()
            ),
        )
    })?;
    serde_json::from_str(&text).map_err(|e| ApiError::new(10002, format!("bad runtime.json: {e}")))
}

pub fn write_runtime(home: &Path, profile: &str, payload: &Value) -> MqsResult<()> {
    let paths = ProfilePaths::new(home, profile);
    paths.ensure()?;
    write_json_atomic(&paths.runtime(), payload)?;
    Ok(())
}

pub fn clear_runtime(home: &Path, profile: &str) {
    let _ = std::fs::remove_file(ProfilePaths::new(home, profile).runtime());
}

pub fn detect_lan_ip() -> String {
    for candidate in ["192.168.1.1:80", "10.0.0.1:80", "8.8.8.8:80"] {
        if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if socket.connect(candidate).is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    let ip = addr.ip().to_string();
                    if ip != "0.0.0.0" {
                        return ip;
                    }
                }
            }
        }
    }
    "127.0.0.1".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_layout() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ProfilePaths::new(dir.path(), "dev");
        paths.ensure().unwrap();
        assert!(paths.directory().exists());
        assert!(paths.media().exists());
        let meta: Value =
            serde_json::from_str(&std::fs::read_to_string(paths.meta()).unwrap()).unwrap();
        assert_eq!(meta["name"], "dev");
        assert!(meta["notice"].as_str().unwrap().contains("not affiliated"));
        let index: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("profiles.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(index["profiles"]["dev"]["name"], "dev");
    }

    #[test]
    fn lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ProfilePaths::new(dir.path(), "dev");
        let mut first = ProfileLock::new(paths.clone());
        first.acquire().unwrap();
        let mut second = ProfileLock::new(paths.clone());
        assert!(second.acquire().is_err());
        first.release();
        assert!(second.acquire().is_ok());
    }
}
