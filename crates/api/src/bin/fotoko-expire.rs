//! EventBridge schedule Lambda: expires overdue unpaid orders.

use fotoko_api::aws::{sdk_config, table_service};
use fotoko_api::config::{Mode, TableConfig, process_env};
use fotoko_api::expire::handle;
use fotoko_api::telemetry;
use lambda_runtime::{Error, LambdaEvent, service_fn};
use serde_json::Value;

#[tokio::main]
async fn main() -> Result<(), Error> {
    telemetry::init_json();
    if Mode::from_lookup(&process_env)? != Mode::Aws {
        return Err("fotoko-expire only runs with FOTOKO_MODE=aws".into());
    }
    let config = TableConfig::from_lookup(&process_env)?;
    let service = table_service(&config, &sdk_config().await)?;
    lambda_runtime::run(service_fn(move |_event: LambdaEvent<Value>| {
        let service = service.clone();
        async move { Ok::<_, Error>(handle(&service).await?) }
    }))
    .await
}
