//! IoT rule Lambda: applies Pi job statuses and heartbeats.

use fotoko_api::aws::{sdk_config, table_service};
use fotoko_api::config::{Mode, TableConfig, process_env};
use fotoko_api::status::handle_event;
use fotoko_api::telemetry;
use lambda_runtime::{Error, LambdaEvent, service_fn};
use serde_json::Value;

#[tokio::main]
async fn main() -> Result<(), Error> {
    telemetry::init_json();
    if Mode::from_lookup(&process_env)? != Mode::Aws {
        return Err("fotoko-iot-status only runs with FOTOKO_MODE=aws".into());
    }
    let config = TableConfig::from_lookup(&process_env)?;
    let service = table_service(&config, &sdk_config().await)?;
    lambda_runtime::run(service_fn(move |event: LambdaEvent<Value>| {
        let service = service.clone();
        async move {
            let outcome = handle_event(&service, event.payload).await?;
            tracing::info!(?outcome, "IoT event handled");
            Ok::<_, Error>(())
        }
    }))
    .await
}
