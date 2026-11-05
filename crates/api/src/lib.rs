//! Fotoko backend: the axum HTTP API over `fotoko-core`, the AWS adapters, the Lambda handlers
//! and the local/sim server. See `docs/contracts.md`.

pub mod api;
pub mod aws;
pub mod config;
pub mod error;
pub mod expire;
pub mod identity;
pub mod ids;
#[cfg(feature = "local")]
pub mod local;
pub mod payos;
pub mod status;
pub mod telemetry;

pub use api::{AppState, ServiceRefresher, router};
pub use error::ApiError;
pub use identity::{Identity, IdentityMode};
