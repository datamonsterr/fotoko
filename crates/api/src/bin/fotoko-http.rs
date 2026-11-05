//! API Gateway HTTP API Lambda: the axum router over DynamoDB, S3, IoT and payOS.

use fotoko_api::aws::load_http_service;
use fotoko_api::config::{HttpConfig, Mode, process_env};
use fotoko_api::{AppState, IdentityMode, router, telemetry};

#[tokio::main]
async fn main() -> Result<(), lambda_http::Error> {
    telemetry::init_json();
    // Fail fast at cold start: a bad deployment never serves a request.
    if Mode::from_lookup(&process_env)? != Mode::Aws {
        return Err("fotoko-http only runs with FOTOKO_MODE=aws; use fotoko-local".into());
    }
    let config = HttpConfig::from_lookup(&process_env)?;
    let (service, refresher) = load_http_service(&config).await?;
    let app = router(AppState::new(service, IdentityMode::Aws).with_refresher(refresher));
    lambda_http::run(app).await
}
