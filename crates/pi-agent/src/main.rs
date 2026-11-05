//! `fotoko-pi-agent --config /etc/fotoko/agent.toml`
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use fotoko_pi_agent::{Agent, Config};
use tracing_subscriber::EnvFilter;

/// Raspberry Pi print agent.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Path to the agent configuration.
    #[arg(long, default_value = "/etc/fotoko/agent.toml")]
    config: PathBuf,
}

/// Resolves on SIGTERM or ctrl-c.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load(&cli.config)?;
    Agent::from_config(config)?.run(shutdown_signal()).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fotoko-pi-agent: {error}");
            ExitCode::FAILURE
        }
    }
}
