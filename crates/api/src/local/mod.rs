//! The local/sim server: the same HTTP API on in-memory repos, a local-fs object store with
//! presigned-style URLs and MQTT, for development, tests and the simulation e2e.

pub mod app;
pub mod mqtt;
pub mod objects;

pub use app::{LocalApp, LocalAppBuilder};
pub use objects::LocalObjectStore;
