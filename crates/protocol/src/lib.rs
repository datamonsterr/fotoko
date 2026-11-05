//! Wire types shared by every Fotoko component: HTTP bodies, MQTT payloads, topic names and
//! limits. Pure data and pure functions, no IO. Source of truth: `docs/contracts.md`.

pub mod http;
pub mod limits;
pub mod mqtt;
pub mod topics;

pub use http::*;
pub use limits::*;
pub use mqtt::*;
pub use topics::*;
