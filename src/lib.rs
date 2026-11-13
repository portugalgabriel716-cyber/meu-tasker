//! Tasker — refill automático da Lidl Connect.
//!
//! Um executável só faz tudo: o serviço em segundo plano (motor + API local), a
//! janela e a linha de comando. Veja o README.
pub mod browser_login;
pub mod client;
pub mod config;
pub mod daemon;
pub mod engine;
#[cfg(feature = "janela")]
pub mod gui;
pub mod history;
pub mod lidl;
pub mod logs;
pub mod notify;
pub mod paths;
pub mod store;
pub mod stress;
pub mod system;
pub mod types;
pub mod util;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
