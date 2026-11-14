//! QSandbox — a faithful sandbox for the QQ Bot API v2.
//!
//! The crate is the whole product: the simulated QQ API, the gateway, the
//! control API that serves the Vue GUI, the CLI and the MCP server. The Tauri
//! desktop shell embeds it as a library.

pub mod assets;
pub mod cli;
pub mod clock;
pub mod community;
pub mod config;
pub mod contract;
pub mod control;
pub mod db;
pub mod error;
pub mod gateway;
pub mod http;
pub mod hub;
pub mod ids;
pub mod mcp;
pub mod messages;
pub mod models;
pub mod ratelimit;
pub mod scenario;
pub mod server;
pub mod sim;
pub mod text_tags;
pub mod webhooks;
pub mod world;

pub use config::VERSION;
