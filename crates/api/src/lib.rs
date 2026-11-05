//! Fotoko backend: the axum HTTP API over `fotoko-core`, the AWS adapters, the Lambda handlers
//! and the local/sim server. See `docs/contracts.md`.

pub mod api;
pub mod aws;
pub mod config;
pub mod error;
pub mod identity;
pub mod ids;
pub mod payos;

pub use api::{AppState, router};
pub use error::ApiError;
pub use identity::{Identity, IdentityMode};
