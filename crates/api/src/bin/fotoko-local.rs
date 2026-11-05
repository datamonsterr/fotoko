//! Local/sim server: the HTTP API on in-memory repos, a local-fs object store and MQTT.
//! Not deployed. Configured through environment variables (see `LocalConfig`).
#![allow(clippy::print_stdout, clippy::print_stderr)]

use fotoko_api::config::{LocalConfig, process_env};
use fotoko_api::local::LocalAppBuilder;
use fotoko_api::telemetry;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    telemetry::init_json();
    let config = LocalConfig::from_lookup(&process_env)?;
    let mut app = LocalAppBuilder::from_config(&config)?.build().await?;
    let listener = TcpListener::bind(config.addr).await?;
    let addr = app.serve(listener)?;
    println!("fotoko-local listening on http://{addr}");
    tracing::info!(%addr, public_url = %config.public_url, mqtt = ?config.mqtt_url, "fotoko-local started");
    tokio::signal::ctrl_c().await?;
    tracing::info!("fotoko-local stopping");
    Ok(())
}
