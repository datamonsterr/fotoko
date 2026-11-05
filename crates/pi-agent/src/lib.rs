//! The Fotoko Pi agent as a library: `Agent::new(config, printer)` then `run(shutdown)`.
//! The binary `fotoko-pi-agent` is a thin wrapper; the e2e crate runs the agent in-process
//! with a `SimBackend`.

pub mod agent;
pub mod config;
pub mod cups;
pub mod dedupe;
pub mod download;
pub mod handler;
pub mod mqtt;
pub mod printer;
pub mod sim_backend;
pub mod tls;

pub use agent::{Agent, AgentError, build_printer};
pub use config::Config;
pub use printer::{PrintError, Printer};
pub use sim_backend::SimBackend;
