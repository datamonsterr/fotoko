//! payOS HTTP client: the [`PaymentGateway`] adapter used in AWS and local mode.
//!
//! `POST {base}/v2/payment-requests` with `x-client-id` / `x-api-key`; the request is signed with
//! the checksum key by `fotoko_core::payos::sign_payment_request`. The success response is
//! `{code:"00", desc, data:{orderCode, amount, checkoutUrl, qrCode, ...}, signature}` where
//! `signature` is the HMAC-SHA256 of the sorted `data` fields, the same scheme as webhooks, so
//! it is verified with `fotoko_core::payos::verify_webhook`.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use fotoko_core::payos::{sign_payment_request, verify_webhook};
use fotoko_core::{CoreError, PaymentGateway, PaymentLink, PaymentLinkRequest};
use fotoko_protocol::{PayosWebhook, PayosWebhookData};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// How much of an error body is logged.
const LOG_BODY_CHARS: usize = 300;

/// payOS merchant credentials (Secrets Manager JSON `{clientId,apiKey,checksumKey}`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayosCredentials {
    /// `x-client-id`.
    pub client_id: String,
    /// `x-api-key`.
    pub api_key: String,
    /// Signing key for requests, responses and webhooks.
    pub checksum_key: String,
}

impl fmt::Debug for PayosCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayosCredentials")
            .field("client_id", &self.client_id)
            .field("api_key", &"<redacted>")
            .field("checksum_key", &"<redacted>")
            .finish()
    }
}

impl PayosCredentials {
    /// Parse the Secrets Manager secret string.
    pub fn from_secret_json(secret: &str) -> Result<Self, CoreError> {
        let creds: Self = serde_json::from_str(secret).map_err(|_| {
            CoreError::Internal("payOS secret is not JSON {clientId,apiKey,checksumKey}".into())
        })?;
        if creds.client_id.trim().is_empty()
            || creds.api_key.trim().is_empty()
            || creds.checksum_key.trim().is_empty()
        {
            return Err(CoreError::Internal(
                "payOS secret has an empty clientId, apiKey or checksumKey".into(),
            ));
        }
        Ok(creds)
    }
}

/// [`PaymentGateway`] over the payOS REST API.
#[derive(Clone)]
pub struct PayosClient {
    http: reqwest::Client,
    base_url: String,
    credentials: PayosCredentials,
}

impl fmt::Debug for PayosClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayosClient")
            .field("base_url", &self.base_url)
            .field("credentials", &self.credentials)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Envelope {
    code: Option<String>,
    desc: Option<String>,
    data: Option<Value>,
    signature: Option<String>,
}

impl PayosClient {
    /// Client for `base_url` (no trailing slash needed).
    pub fn new(base_url: &str, credentials: PayosCredentials) -> Result<Self, CoreError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| CoreError::Internal(format!("cannot build the HTTP client: {e}")))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            credentials,
        })
    }

    /// The checksum key, for verifying webhooks.
    pub fn checksum_key(&self) -> &str {
        &self.credentials.checksum_key
    }

    fn body(&self, request: &PaymentLinkRequest) -> Result<Value, CoreError> {
        let signature = sign_payment_request(&self.credentials.checksum_key, request)?;
        Ok(json!({
            "orderCode": request.order_code,
            "amount": request.amount_vnd,
            "description": request.description,
            "cancelUrl": request.cancel_url,
            "returnUrl": request.return_url,
            "expiredAt": request.expires_at.timestamp(),
            "signature": signature,
        }))
    }

    fn verify(
        &self,
        request: &PaymentLinkRequest,
        envelope: Envelope,
    ) -> Result<PaymentLink, CoreError> {
        let fail = |why: &str| {
            tracing::warn!(order_code = request.order_code, why, "bad payOS response");
            CoreError::PaymentProvider("payOS returned an unusable response".into())
        };
        let Some(Value::Object(mut data)) = envelope.data else {
            return Err(fail("no data object"));
        };
        let signature = envelope.signature.ok_or_else(|| fail("no signature"))?;
        let order_code = data.remove("orderCode").and_then(|v| v.as_i64());
        let amount = data.remove("amount").and_then(|v| v.as_u64());
        let (Some(order_code), Some(amount)) = (order_code, amount) else {
            return Err(fail("data lacks orderCode or amount"));
        };
        let link = PaymentLink {
            checkout_url: string_field(&data, "checkoutUrl")
                .ok_or_else(|| fail("no checkoutUrl"))?,
            qr_code: string_field(&data, "qrCode").ok_or_else(|| fail("no qrCode"))?,
        };
        let signed = PayosWebhook {
            code: "00".into(),
            desc: String::new(),
            success: true,
            data: PayosWebhookData {
                order_code,
                amount,
                extra: data.into_iter().collect(),
            },
            signature,
        };
        if !verify_webhook(&self.credentials.checksum_key, &signed) {
            return Err(fail("signature mismatch"));
        }
        if order_code != request.order_code || amount != request.amount_vnd {
            return Err(fail("response is for another order"));
        }
        Ok(link)
    }
}

fn string_field(data: &Map<String, Value>, key: &str) -> Option<String> {
    data.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[async_trait]
impl PaymentGateway for PayosClient {
    async fn create_payment_link(
        &self,
        request: &PaymentLinkRequest,
    ) -> Result<PaymentLink, CoreError> {
        let body = self.body(request)?;
        let response = self
            .http
            .post(format!("{}/v2/payment-requests", self.base_url))
            .header("x-client-id", &self.credentials.client_id)
            .header("x-api-key", &self.credentials.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "payOS request failed");
                CoreError::PaymentProvider("payOS is unreachable".into())
            })?;
        let status = response.status();
        let text = response.text().await.map_err(|e| {
            tracing::warn!(error = %e, "payOS response unreadable");
            CoreError::PaymentProvider("payOS is unreachable".into())
        })?;
        let envelope: Option<Envelope> = serde_json::from_str(&text).ok();
        if !status.is_success() {
            let snippet: String = text.chars().take(LOG_BODY_CHARS).collect();
            tracing::warn!(http_status = status.as_u16(), body = %snippet, "payOS rejected the request");
            return Err(CoreError::PaymentProvider(format!(
                "payOS answered HTTP {}",
                status.as_u16()
            )));
        }
        let Some(envelope) = envelope else {
            tracing::warn!("payOS answered with a body that is not JSON");
            return Err(CoreError::PaymentProvider(
                "payOS returned an unusable response".into(),
            ));
        };
        if envelope.code.as_deref() != Some("00") {
            tracing::warn!(
                code = envelope.code.as_deref().unwrap_or(""),
                desc = envelope.desc.as_deref().unwrap_or(""),
                "payOS refused the payment link"
            );
            return Err(CoreError::PaymentProvider(format!(
                "payOS refused the payment link (code {})",
                envelope.code.as_deref().unwrap_or("none")
            )));
        }
        self.verify(request, envelope)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use fotoko_core::payos::sign_webhook_data;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const KEY: &str = "checksum-secret";

    fn creds() -> PayosCredentials {
        PayosCredentials {
            client_id: "cid".into(),
            api_key: "akey".into(),
            checksum_key: KEY.into(),
        }
    }

    fn request() -> PaymentLinkRequest {
        PaymentLinkRequest {
            order_code: 1234,
            amount_vnd: 30_000,
            description: "Fotoko 1234".into(),
            return_url: "https://app.test/orders/o1".into(),
            cancel_url: "https://app.test/orders/o1".into(),
            expires_at: "2026-01-01T00:15:00Z".parse::<DateTime<Utc>>().expect("ts"),
        }
    }

    fn signed_data(data: &Value) -> String {
        let mut map = data.as_object().expect("object").clone();
        let order_code = map
            .remove("orderCode")
            .and_then(|v| v.as_i64())
            .expect("code");
        let amount = map
            .remove("amount")
            .and_then(|v| v.as_u64())
            .expect("amount");
        sign_webhook_data(
            KEY,
            &PayosWebhookData {
                order_code,
                amount,
                extra: map.into_iter().collect(),
            },
        )
        .expect("sign")
    }

    fn good_data() -> Value {
        json!({
            "bin": "970422", "accountNumber": "1133", "accountName": "ACME",
            "amount": 30000, "description": "Fotoko 1234", "orderCode": 1234,
            "currency": "VND", "paymentLinkId": "abc", "status": "PENDING",
            "checkoutUrl": "https://pay.payos.vn/web/abc", "qrCode": "0002010102"
        })
    }

    fn envelope(data: &Value) -> Value {
        json!({"code": "00", "desc": "success", "data": data, "signature": signed_data(data)})
    }

    async fn client_for(server: &MockServer) -> PayosClient {
        PayosClient::new(&format!("{}/", server.uri()), creds()).expect("client")
    }

    async fn mount(server: &MockServer, response: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path("/v2/payment-requests"))
            .respond_with(response)
            .mount(server)
            .await;
    }

    async fn expect_provider_error(response: ResponseTemplate) -> String {
        let server = MockServer::start().await;
        mount(&server, response).await;
        match client_for(&server)
            .await
            .create_payment_link(&request())
            .await
        {
            Err(CoreError::PaymentProvider(message)) => message,
            other => panic!("expected a payment provider error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn creates_a_link_with_signed_request_and_credentials() {
        let server = MockServer::start().await;
        let expected_signature = sign_payment_request(KEY, &request()).expect("sig");
        Mock::given(method("POST"))
            .and(path("/v2/payment-requests"))
            .and(header("x-client-id", "cid"))
            .and(header("x-api-key", "akey"))
            .and(body_partial_json(json!({
                "orderCode": 1234, "amount": 30000, "description": "Fotoko 1234",
                "cancelUrl": "https://app.test/orders/o1",
                "returnUrl": "https://app.test/orders/o1",
                "expiredAt": 1_767_226_500,
                "signature": expected_signature,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(&good_data())))
            .expect(1)
            .mount(&server)
            .await;
        let link = client_for(&server)
            .await
            .create_payment_link(&request())
            .await
            .expect("link");
        assert_eq!(
            link,
            PaymentLink {
                checkout_url: "https://pay.payos.vn/web/abc".into(),
                qr_code: "0002010102".into()
            }
        );
    }

    #[tokio::test]
    async fn payos_refusal_is_a_provider_error() {
        let message = expect_provider_error(
            ResponseTemplate::new(200)
                .set_body_json(json!({"code": "231", "desc": "Đơn hàng đã tồn tại"})),
        )
        .await;
        assert_eq!(message, "payOS refused the payment link (code 231)");
        let message =
            expect_provider_error(ResponseTemplate::new(200).set_body_json(json!({"desc": "x"})))
                .await;
        assert_eq!(message, "payOS refused the payment link (code none)");
    }

    #[tokio::test]
    async fn http_errors_are_provider_errors() {
        for status in [401_u16, 429, 500] {
            let message = expect_provider_error(
                ResponseTemplate::new(status).set_body_json(json!({"code": "20", "desc": "bad"})),
            )
            .await;
            assert_eq!(message, format!("payOS answered HTTP {status}"));
        }
    }

    #[tokio::test]
    async fn unusable_responses_are_provider_errors() {
        let unusable = "payOS returned an unusable response";
        // Not JSON.
        assert_eq!(
            expect_provider_error(ResponseTemplate::new(200).set_body_string("<html>")).await,
            unusable
        );

        let data = good_data();
        let mut cases: Vec<Value> = Vec::new();
        // No data.
        cases.push(json!({"code": "00", "desc": "ok", "signature": "aa"}));
        // No signature.
        cases.push(json!({"code": "00", "desc": "ok", "data": data}));
        // Wrong signature.
        cases.push(json!({"code": "00", "desc": "ok", "data": data, "signature": "00"}));
        // Signed with a different key.
        let mut forged = envelope(&data);
        forged["signature"] = json!("0".repeat(64));
        cases.push(forged);
        // Tampered after signing.
        let mut tampered = envelope(&data);
        tampered["data"]["checkoutUrl"] = json!("https://evil.test/");
        cases.push(tampered);
        // Missing fields, each correctly signed.
        for key in ["checkoutUrl", "qrCode", "orderCode", "amount"] {
            let mut d = good_data();
            d.as_object_mut().expect("object").remove(key);
            cases.push(json!({"code": "00", "desc": "ok", "data": d.clone(),
                              "signature": "11"}));
        }
        for key in ["checkoutUrl", "qrCode"] {
            let mut d = good_data();
            d[key] = json!("");
            cases.push(envelope(&d));
        }
        // Signed, but for another order or amount.
        let mut other = good_data();
        other["orderCode"] = json!(999);
        cases.push(envelope(&other));
        let mut other = good_data();
        other["amount"] = json!(1);
        cases.push(envelope(&other));
        // data not an object.
        cases.push(json!({"code": "00", "desc": "ok", "data": "x", "signature": "aa"}));

        for body in cases {
            let message =
                expect_provider_error(ResponseTemplate::new(200).set_body_json(body.clone())).await;
            assert_eq!(message, unusable, "{body}");
        }
    }

    #[tokio::test]
    async fn network_failure_is_a_provider_error() {
        let client = PayosClient::new("http://127.0.0.1:1", creds()).expect("client");
        assert_eq!(
            client.create_payment_link(&request()).await,
            Err(CoreError::PaymentProvider("payOS is unreachable".into()))
        );
    }

    #[test]
    fn credentials_parse_and_redact() {
        let c = PayosCredentials::from_secret_json(
            r#"{"clientId":"c","apiKey":"k","checksumKey":"s","extra":1}"#,
        )
        .expect("parse");
        assert_eq!(
            (
                c.client_id.as_str(),
                c.api_key.as_str(),
                c.checksum_key.as_str()
            ),
            ("c", "k", "s")
        );
        let debug = format!(
            "{c:?} {:?}",
            PayosClient::new("http://x", c.clone()).expect("client")
        );
        assert!(!debug.contains("\"k\"") && !debug.contains("\"s\""));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn credentials_reject_bad_secrets() {
        for secret in [
            "nope",
            r#"{"clientId":"c"}"#,
            r#"{"clientId":"","apiKey":"k","checksumKey":"s"}"#,
            r#"{"clientId":"c","apiKey":" ","checksumKey":"s"}"#,
            r#"{"clientId":"c","apiKey":"k","checksumKey":""}"#,
        ] {
            assert!(
                matches!(
                    PayosCredentials::from_secret_json(secret),
                    Err(CoreError::Internal(_))
                ),
                "{secret}"
            );
        }
    }

    #[test]
    fn exposes_the_checksum_key() {
        let client = PayosClient::new("http://x", creds()).expect("client");
        assert_eq!(client.checksum_key(), KEY);
    }
}
