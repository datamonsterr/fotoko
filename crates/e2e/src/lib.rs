//! Simulation e2e for Fotoko: the whole flow (API, MQTT broker, payOS mock, Pi agent, printer
//! simulation) without AWS, payOS or a printer.
//!
//! * [`stack::Stack`] runs everything in one process for `cargo test -p fotoko-e2e`.
//! * The binaries `fotoko-sim-broker` and `fotoko-payos-mock` are the pieces the Playwright run
//!   (`scripts/e2e/sim-up.sh`) and `docker-compose.yml` start next to `fotoko-local` and
//!   `fotoko-pi-agent`.
//!
//! This is test support code: it panics on setup errors instead of returning them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod broker;
pub mod cloud;
pub mod fixtures;
pub mod payos;
pub mod stack;

pub use stack::{Stack, StackOptions};
