//! AWS adapters for the `fotoko-core` ports and the wiring of the Lambda functions.

pub mod dynamo;
pub mod iot;
pub mod s3;
pub mod secrets;
pub mod unavailable;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use aws_config::{BehaviorVersion, SdkConfig};
use fotoko_core::{CoreError, OrderService, Ports, ServiceConfig, SystemClock};

use crate::api::ServiceRefresher;
use crate::config::{HttpConfig, TableConfig};
use crate::ids::RandomIdGen;
use crate::payos::{PayosClient, PayosCredentials};

/// Load region and credentials from the Lambda environment.
pub async fn sdk_config() -> SdkConfig {
    aws_config::defaults(BehaviorVersion::latest()).load().await
}

/// The SDK clients of the `fotoko-http` Lambda.
#[derive(Debug, Clone)]
struct HttpClients {
    dynamo: aws_sdk_dynamodb::Client,
    s3: aws_sdk_s3::Client,
    iot: aws_sdk_iotdataplane::Client,
}

fn http_clients(config: &HttpConfig, sdk: &SdkConfig) -> HttpClients {
    let iot_config = aws_sdk_iotdataplane::config::Builder::from(sdk)
        .endpoint_url(format!("https://{}", config.iot_data_endpoint))
        .build();
    HttpClients {
        dynamo: aws_sdk_dynamodb::Client::new(sdk),
        s3: aws_sdk_s3::Client::new(sdk),
        iot: aws_sdk_iotdataplane::Client::from_conf(iot_config),
    }
}

fn assemble_http(
    config: &HttpConfig,
    clients: HttpClients,
    credentials: PayosCredentials,
) -> Result<OrderService, CoreError> {
    let service_config = ServiceConfig::new(
        config.public_app_url.clone(),
        credentials.checksum_key.clone(),
    )?;
    let payments = PayosClient::new(&config.payos_base_url, credentials)?;
    Ok(OrderService::new(
        Ports {
            orders: Arc::new(dynamo::DynamoOrderRepo::new(
                clients.dynamo.clone(),
                &config.table_name,
            )),
            printers: Arc::new(dynamo::DynamoPrinterRepo::new(
                clients.dynamo,
                &config.table_name,
                config.default_price_vnd,
            )),
            store: Arc::new(s3::S3ObjectStore::new(clients.s3, &config.bucket_name)),
            publisher: Arc::new(iot::IotJobPublisher::new(clients.iot)),
            payments: Arc::new(payments),
            clock: Arc::new(SystemClock),
            ids: Arc::new(RandomIdGen),
        },
        service_config,
    ))
}

/// The service of the `fotoko-http` Lambda, given already loaded payOS credentials.
pub fn http_service(
    config: &HttpConfig,
    sdk: &SdkConfig,
    credentials: PayosCredentials,
) -> Result<OrderService, CoreError> {
    assemble_http(config, http_clients(config, sdk), credentials)
}

/// Cold start of `fotoko-http`: read the payOS secret while the SDK clients are built, then wire
/// the service. The returned refresher re-reads the secret after a verification failure.
pub async fn load_http_service(
    config: &HttpConfig,
) -> Result<(OrderService, Arc<SecretRefresher>), CoreError> {
    let sdk = sdk_config().await;
    let secrets = aws_sdk_secretsmanager::Client::new(&sdk);
    let (credentials, clients) = tokio::join!(
        secrets::load_payos_credentials(&secrets, &config.payos_secret_arn),
        async { http_clients(config, &sdk) }
    );
    let credentials = credentials?;
    let service = assemble_http(config, clients, credentials.clone())?;
    let refresher = SecretRefresher::new(
        config.clone(),
        sdk,
        secrets,
        credentials,
        REFRESH_MIN_INTERVAL,
    );
    Ok((service, Arc::new(refresher)))
}

/// A refresh is attempted at most this often, so bad webhook signatures (anyone can send them)
/// cannot turn into a stream of Secrets Manager calls.
pub const REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(60);

/// Re-reads the payOS secret and rebuilds the service when it changed (checksum key rotation).
pub struct SecretRefresher {
    config: HttpConfig,
    sdk: SdkConfig,
    secrets: aws_sdk_secretsmanager::Client,
    current: Mutex<PayosCredentials>,
    last_attempt: Mutex<Option<Instant>>,
    min_interval: Duration,
}

impl std::fmt::Debug for SecretRefresher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretRefresher").finish_non_exhaustive()
    }
}

impl SecretRefresher {
    /// Refresher that knows the credentials the running service was built with.
    pub fn new(
        config: HttpConfig,
        sdk: SdkConfig,
        secrets: aws_sdk_secretsmanager::Client,
        current: PayosCredentials,
        min_interval: Duration,
    ) -> Self {
        Self {
            config,
            sdk,
            secrets,
            current: Mutex::new(current),
            last_attempt: Mutex::new(None),
            min_interval,
        }
    }

    /// Claim the right to refresh now: `false` when the last attempt is too recent.
    fn may_attempt(&self) -> bool {
        let mut last = self
            .last_attempt
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if last.is_some_and(|t| now.duration_since(t) < self.min_interval) {
            return false;
        }
        *last = Some(now);
        true
    }
}

#[async_trait]
impl ServiceRefresher for SecretRefresher {
    async fn refresh(&self) -> Result<Option<OrderService>, CoreError> {
        if !self.may_attempt() {
            return Ok(None);
        }
        let fresh =
            secrets::load_payos_credentials(&self.secrets, &self.config.payos_secret_arn).await?;
        if *self.current.lock().unwrap_or_else(PoisonError::into_inner) == fresh {
            return Ok(None);
        }
        // Remember the new credentials only once a service was built from them, so a failed
        // build is retried at the next attempt instead of being mistaken for "unchanged".
        let service = http_service(&self.config, &self.sdk, fresh.clone())?;
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = fresh;
        Ok(Some(service))
    }
}

/// The service of the table-only Lambdas (`fotoko-iot-status`, `fotoko-expire`): table access only.
pub fn table_service(config: &TableConfig, sdk: &SdkConfig) -> Result<OrderService, CoreError> {
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

        let status = TableConfig {
            table_name: "t".into(),
            default_price_vnd: 10_000,
        };
        assert!(table_service(&status, &sdk()).is_ok());
    }

    mod refresher {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use aws_sdk_secretsmanager::config::{Credentials as SmCredentials, Region as SmRegion};
        use serde_json::json;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

        use super::*;

        /// Answers GetSecretValue with the next key in the list (the last one repeats).
        struct Rotating {
            keys: Vec<&'static str>,
            calls: Arc<AtomicUsize>,
        }

        impl Respond for Rotating {
            fn respond(&self, _request: &Request) -> ResponseTemplate {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                let key = self.keys[n.min(self.keys.len() - 1)];
                let secret =
                    json!({"clientId": "c", "apiKey": "k", "checksumKey": key}).to_string();
                ResponseTemplate::new(200).set_body_raw(
                    json!({"SecretString": secret}).to_string(),
                    "application/x-amz-json-1.1",
                )
            }
        }

        fn creds(key: &str) -> PayosCredentials {
            PayosCredentials {
                client_id: "c".into(),
                api_key: "k".into(),
                checksum_key: key.into(),
            }
        }

        async fn refresher(
            keys: Vec<&'static str>,
            interval: Duration,
        ) -> (SecretRefresher, Arc<AtomicUsize>, MockServer) {
            let server = MockServer::start().await;
            let calls = Arc::new(AtomicUsize::new(0));
            Mock::given(method("POST"))
                .respond_with(Rotating {
                    keys,
                    calls: Arc::clone(&calls),
                })
                .mount(&server)
                .await;
            let secrets = aws_sdk_secretsmanager::Client::from_conf(
                aws_sdk_secretsmanager::Config::builder()
                    .behavior_version(aws_sdk_secretsmanager::config::BehaviorVersion::latest())
                    .region(SmRegion::new("ap-southeast-1"))
                    .credentials_provider(SmCredentials::new("a", "b", None, None, "t"))
                    .endpoint_url(server.uri())
                    .build(),
            );
            let http = HttpConfig {
                table_name: "t".into(),
                bucket_name: "b".into(),
                iot_data_endpoint: "host.example".into(),
                payos_base_url: "https://payos.test".into(),
                payos_secret_arn: "arn".into(),
                public_app_url: "https://app.test".into(),
                default_price_vnd: 1,
            };
            (
                SecretRefresher::new(http, sdk(), secrets, creds("old"), interval),
                calls,
                server,
            )
        }

        #[tokio::test]
        async fn unchanged_secret_gives_nothing_and_a_new_key_gives_a_service() {
            let (r, calls, _server) = refresher(vec!["old", "new"], Duration::ZERO).await;
            assert!(r.refresh().await.expect("refresh").is_none());
            assert!(r.refresh().await.expect("refresh").is_some());
            // The new key is remembered: reading it again changes nothing.
            assert!(r.refresh().await.expect("refresh").is_none());
            assert_eq!(calls.load(Ordering::SeqCst), 3);
        }

        #[tokio::test]
        async fn a_failed_build_is_not_remembered_as_current() {
            // An empty checksum key cannot build a service.
            let (r, calls, _server) = refresher(vec![" ", "new"], Duration::ZERO).await;
            assert!(r.refresh().await.is_err());
            // The next attempt sees a change again and succeeds.
            assert!(r.refresh().await.expect("refresh").is_some());
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }

        #[tokio::test]
        async fn refreshes_are_rate_limited() {
            let (r, calls, _server) = refresher(vec!["new"], Duration::from_secs(3600)).await;
            assert!(r.refresh().await.expect("refresh").is_some());
            assert!(r.refresh().await.expect("refresh").is_none());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert!(format!("{r:?}").contains("SecretRefresher"));
        }

        #[tokio::test]
        async fn unusable_secret_is_an_error() {
            let (r, _calls, server) = refresher(vec!["old"], Duration::ZERO).await;
            server.reset().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(400).set_body_raw(
                    json!({"__type": "ResourceNotFoundException", "Message": "x"}).to_string(),
                    "application/x-amz-json-1.1",
                ))
                .mount(&server)
                .await;
            assert!(r.refresh().await.is_err());
        }
    }
}
