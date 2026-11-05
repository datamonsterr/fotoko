//! payOS stand-in: signed payment-link responses and signed webhooks.

use fotoko_api::payos::PayosCredentials;
use fotoko_core::payos::sign_webhook_data;
use fotoko_protocol::PayosWebhookData;
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers};

/// Merchant client id the API is configured with.
pub const CLIENT_ID: &str = "e2e-client-id";
/// Merchant API key the API is configured with.
pub const API_KEY: &str = "e2e-api-key";
/// Checksum key shared by the API and the mock.
pub const CHECKSUM_KEY: &str = "e2e-checksum-key";

/// Credentials of the API under test.
pub fn credentials() -> PayosCredentials {
    PayosCredentials {
        client_id: CLIENT_ID.into(),
        api_key: API_KEY.into(),
        checksum_key: CHECKSUM_KEY.into(),
    }
}

/// The success response of `POST /v2/payment-requests` for `request`, signed with
/// `checksum_key`. `checkout_url` is where the customer pays.
pub fn payment_link_response(
    checksum_key: &str,
    request: &Value,
    checkout_url: &str,
) -> Result<Value, String> {
    let order_code = request["orderCode"].as_i64().ok_or("no orderCode")?;
    let amount = request["amount"].as_u64().ok_or("no amount")?;
    let data = PayosWebhookData {
        order_code,
        amount,
        extra: [
            ("description", json!(request["description"])),
            ("currency", json!("VND")),
            ("status", json!("PENDING")),
            ("checkoutUrl", json!(checkout_url)),
            ("qrCode", json!(format!("qr-{order_code}"))),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect(),
    };
    let signature = sign_webhook_data(checksum_key, &data).map_err(|e| e.to_string())?;
    Ok(json!({
        "code": "00", "desc": "success", "signature": signature,
        "data": serde_json::to_value(&data).map_err(|e| e.to_string())?,
    }))
}

/// Answers `POST /v2/payment-requests` with a signed link on `https://pay.test/web/{orderCode}`.
struct PaymentRequests;

impl Respond for PaymentRequests {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("json body");
        let url = format!("https://pay.test/web/{}", body["orderCode"]);
        match payment_link_response(CHECKSUM_KEY, &body, &url) {
            Ok(response) => ResponseTemplate::new(200).set_body_json(response),
            Err(why) => {
                ResponseTemplate::new(400).set_body_json(json!({"code": "20", "desc": why}))
            }
        }
    }
}

/// A wiremock server that plays payOS' payment-request endpoint.
pub async fn start_mock() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v2/payment-requests"))
        .and(matchers::header("x-client-id", CLIENT_ID))
        .and(matchers::header("x-api-key", API_KEY))
        .respond_with(PaymentRequests)
        .mount(&server)
        .await;
    server
}
