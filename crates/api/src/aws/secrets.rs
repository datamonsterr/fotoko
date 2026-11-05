//! Secrets Manager loader for the payOS credentials.

use aws_sdk_secretsmanager::Client;
use aws_smithy_types::error::display::DisplayErrorContext;
use fotoko_core::CoreError;

use crate::payos::PayosCredentials;

/// Read secret `secret_id` (name or ARN) and parse `{clientId,apiKey,checksumKey}`.
pub async fn load_payos_credentials(
    client: &Client,
    secret_id: &str,
) -> Result<PayosCredentials, CoreError> {
    let out = client
        .get_secret_value()
        .secret_id(secret_id)
        .send()
        .await
        .map_err(|e| {
            tracing::error!(error = %DisplayErrorContext(&e), "cannot read the payOS secret");
            CoreError::Internal("cannot read the payOS secret".into())
        })?;
    let text = out
        .secret_string()
        .ok_or_else(|| CoreError::Internal("the payOS secret has no SecretString".into()))?;
    PayosCredentials::from_secret_json(text)
}

#[cfg(test)]
mod tests {
    use aws_sdk_secretsmanager::config::{
        BehaviorVersion, Credentials, Region, retry::RetryConfig,
    };
    use serde_json::json;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn client(endpoint: &str) -> Client {
        Client::from_conf(
            aws_sdk_secretsmanager::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new("ap-southeast-1"))
                .credentials_provider(Credentials::new("AKID", "secret", None, None, "test"))
                .endpoint_url(endpoint)
                .retry_config(RetryConfig::disabled())
                .build(),
        )
    }

    async fn load(response: ResponseTemplate) -> Result<PayosCredentials, CoreError> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-amz-target", "secretsmanager.GetSecretValue"))
            .respond_with(response)
            .mount(&server)
            .await;
        load_payos_credentials(&client(&server.uri()), "arn:aws:secretsmanager:x").await
    }

    #[tokio::test]
    async fn loads_credentials() {
        let secret = r#"{"clientId":"c","apiKey":"k","checksumKey":"s"}"#;
        let creds = load(ResponseTemplate::new(200).set_body_raw(
            json!({"Name": "n", "SecretString": secret}).to_string(),
            "application/x-amz-json-1.1",
        ))
        .await
        .expect("credentials");
        assert_eq!(creds.client_id, "c");
        assert_eq!(creds.checksum_key, "s");
    }

    #[tokio::test]
    async fn rejects_missing_unparsable_and_denied_secrets() {
        let binary = load(ResponseTemplate::new(200).set_body_raw(
            json!({"Name": "n", "SecretBinary": "AAAA"}).to_string(),
            "application/x-amz-json-1.1",
        ))
        .await;
        assert_eq!(
            binary,
            Err(CoreError::Internal(
                "the payOS secret has no SecretString".into()
            ))
        );
        let junk = load(ResponseTemplate::new(200).set_body_raw(
            json!({"SecretString": "plain"}).to_string(),
            "application/x-amz-json-1.1",
        ))
        .await;
        assert!(matches!(junk, Err(CoreError::Internal(_))));
        let denied = load(ResponseTemplate::new(400).set_body_raw(
            json!({"__type": "AccessDeniedException", "Message": "no"}).to_string(),
            "application/x-amz-json-1.1",
        ))
        .await;
        assert_eq!(
            denied,
            Err(CoreError::Internal("cannot read the payOS secret".into()))
        );
    }
}
