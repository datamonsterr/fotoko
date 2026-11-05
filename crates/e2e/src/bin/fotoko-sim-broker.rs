//! Plain MQTT broker for the simulation (Playwright run, docker compose): no auth, no TLS.
//!
//! `FOTOKO_BROKER_ADDR` is the listen address (default `127.0.0.1:1883`).
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let addr = std::env::var("FOTOKO_BROKER_ADDR").unwrap_or_else(|_| "127.0.0.1:1883".to_owned());
    println!("fotoko-sim-broker listening on {addr}");
    match fotoko_e2e::broker::run(&addr) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fotoko-sim-broker: cannot run on {addr}: {error}");
            ExitCode::FAILURE
        }
    }
}
