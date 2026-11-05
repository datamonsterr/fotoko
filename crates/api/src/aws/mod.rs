//! AWS adapters for the `fotoko-core` ports and the wiring of the Lambda functions.

pub mod dynamo;
pub mod iot;
pub mod s3;
pub mod secrets;
pub mod unavailable;

use std::sync::Arc;

use aws_config::{BehaviorVersion, SdkConfig};
use fotoko_core::{CoreError, OrderService, Ports, ServiceConfig, SystemClock};

use crate::config::{HttpConfig, IotStatusConfig};
use crate::ids::RandomIdGen;
use crate::payos::{PayosClient, PayosCredentials};

/// Load region and credentials from the Lambda environment.
pub async fn sdk_config() -> SdkConfig {
    aws_config::defaults(BehaviorVersion::latest()).load().await
}

/// The service of the `fotoko-http` Lambda, given already loaded payOS credentials.
pub fn http_service(
    config: &HttpConfig,
    sdk: &SdkConfig,
    credentials: PayosCredentials,
) -> Result<OrderService, CoreError> {
    let dynamo = aws_sdk_dynamodb::Client::new(sdk);
    let s3 = aws_sdk_s3::Client::new(sdk);
    let iot_config = aws_sdk_iotdataplane::config::Builder::from(sdk)
        .endpoint_url(format!("https://{}", config.iot_data_endpoint))
        .build();
    let iot = aws_sdk_iotdataplane::Client::from_conf(iot_config);
    let service_config = ServiceConfig::new(
        config.public_app_url.clone(),
        credentials.checksum_key.clone(),
    )?;
    let payments = PayosClient::new(&config.payos_base_url, credentials)?;
    Ok(OrderService::new(
        Ports {
            orders: Arc::new(dynamo::DynamoOrderRepo::new(
                dynamo.clone(),
                &config.table_name,
            )),
            printers: Arc::new(dynamo::DynamoPrinterRepo::new(
                dynamo,
                &config.table_name,
                config.default_price_vnd,
            )),
            store: Arc::new(s3::S3ObjectStore::new(s3, &config.bucket_name)),
            publisher: Arc::new(iot::IotJobPublisher::new(iot)),
            payments: Arc::new(payments),
            clock: Arc::new(SystemClock),
            ids: Arc::new(RandomIdGen),
        },
        service_config,
    ))
}

/// Cold start of `fotoko-http`: read the payOS secret, then wire the service.
pub async fn load_http_service(config: &HttpConfig) -> Result<OrderService, CoreError> {
    let sdk = sdk_config().await;
    let secrets = aws_sdk_secretsmanager::Client::new(&sdk);
    let credentials = secrets::load_payos_credentials(&secrets, &config.payos_secret_arn).await?;
    http_service(config, &sdk, credentials)
}

/// The service of the `fotoko-iot-status` Lambda: table access only.
pub fn status_service(
    config: &IotStatusConfig,
    sdk: &SdkConfig,
) -> Result<OrderService, CoreError> {
    let dynamo = aws_sdk_dynamodb::Client::new(sdk);
    Ok(OrderService::new(
        Ports {
            orders: Arc::new(dynamo::DynamoOrderRepo::new(
                dynamo.clone(),
                &config.table_name,
            )),
            printers: Arc::new(dynamo::DynamoPrinterRepo::new(
                dynamo,
                &config.table_name,
                config.default_price_vnd,
            )),
            store: Arc::new(unavailable::NoObjectStore),
            publisher: Arc::new(unavailable::NoJobPublisher),
            payments: Arc::new(unavailable::NoPaymentGateway),
            clock: Arc::new(SystemClock),
            ids: Arc::new(RandomIdGen),
        },
        // Status handling never verifies webhooks; the key only has to be non-empty.
        ServiceConfig::new("http://unused.invalid", "unused")?,
    ))
}

#[cfg(test)]
mod tests {
    use aws_config::Region;
    use aws_sdk_dynamodb::config::Credentials;

    use super::*;

    fn sdk() -> SdkConfig {
        SdkConfig::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("ap-southeast-1"))
            .credentials_provider(aws_credential_types_provider())
            .build()
    }

    fn aws_credential_types_provider() -> aws_sdk_dynamodb::config::SharedCredentialsProvider {
        aws_sdk_dynamodb::config::SharedCredentialsProvider::new(Credentials::new(
            "AKID", "secret", None, None, "test",
        ))
    }

    #[tokio::test]
    async fn services_wire_up_offline() {
        let http = HttpConfig {
            table_name: "t".into(),
            bucket_name: "b".into(),
            iot_data_endpoint: "host.example".into(),
            payos_base_url: "https://payos.test".into(),
            payos_secret_arn: "arn".into(),
            public_app_url: "https://app.test".into(),
            default_price_vnd: 10_000,
        };
        let creds = PayosCredentials {
            client_id: "c".into(),
            api_key: "k".into(),
            checksum_key: "s".into(),
        };
        assert!(http_service(&http, &sdk(), creds).is_ok());

        let empty_key = PayosCredentials {
            client_id: "c".into(),
            api_key: "k".into(),
            checksum_key: " ".into(),
        };
        assert!(http_service(&http, &sdk(), empty_key).is_err());

        let status = IotStatusConfig {
            table_name: "t".into(),
            default_price_vnd: 10_000,
        };
        assert!(status_service(&status, &sdk()).is_ok());
    }
}
