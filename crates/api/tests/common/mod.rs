//! Shared helpers of the integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use fotoko_api::local::{LocalApp, LocalAppBuilder};
use fotoko_api::payos::PayosCredentials;
use fotoko_core::payos::sign_webhook_data;
use fotoko_protocol::PayosWebhookData;
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers};

pub const CHECKSUM_KEY: &str = "it-checksum-key";

/// payOS stand-in: answers `POST /v2/payment-requests` with a correctly signed link.
pub struct PayosMock;

impl Respond for PayosMock {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("json body");
        let order_code = body["orderCode"].as_i64().expect("orderCode");
        let amount = body["amount"].as_u64().expect("amount");
        let data = PayosWebhookData {
            order_code,
            amount,
            extra: [
                ("description", json!(body["description"])),
                ("currency", json!("VND")),
                ("status", json!("PENDING")),
                (
                    "checkoutUrl",
                    json!(format!("https://pay.test/web/{order_code}")),
                ),
                ("qrCode", json!(format!("qr-{order_code}"))),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
        };
        let signature = sign_webhook_data(CHECKSUM_KEY, &data).expect("sign");
        ResponseTemplate::new(200).set_body_json(json!({
            "code": "00", "desc": "success", "signature": signature,
            "data": serde_json::to_value(&data).expect("data"),
        }))
    }
}

pub async fn payos_mock() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v2/payment-requests"))
        .and(matchers::header("x-client-id", "it-client"))
        .respond_with(PayosMock)
        .mount(&server)
        .await;
    server
}

pub fn credentials() -> PayosCredentials {
    PayosCredentials {
        client_id: "it-client".into(),
        api_key: "it-key".into(),
        checksum_key: CHECKSUM_KEY.into(),
    }
}

/// A local app served on a real port, with its payOS mock and its object directory.
pub struct Stack {
    pub app: LocalApp,
    pub base: String,
    pub payos: MockServer,
    pub http: reqwest::Client,
    pub _dir: tempfile::TempDir,
}

pub async fn stack(configure: impl FnOnce(LocalAppBuilder) -> LocalAppBuilder) -> Stack {
    let payos = payos_mock().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let builder = LocalApp::builder()
        .public_url(&base)
        .object_dir(dir.path())
        .url_secret("it-secret")
        .payos(payos.uri(), credentials())
        .public_app_url("https://app.test")
        .expire_every(None);
    let mut app = configure(builder).build().await.expect("build");
    app.serve(listener).expect("serve");
    Stack {
        app,
        base,
        payos,
        http: reqwest::Client::new(),
        _dir: dir,
    }
}

impl Stack {
    pub async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        who: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(who) = who {
            request = request.header("x-fotoko-identity", who);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("response");
        let status = response.status().as_u16();
        let text = response.text().await.expect("text");
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Create a two-photo order for `guest`, upload both photos through the returned URLs.
    pub async fn order_with_uploads(&self, guest: &str) -> Value {
        let (status, order) = self
            .call(
                reqwest::Method::POST,
                "/v1/orders",
                Some(guest),
                Some(json!({"printerId": "stand-001", "photos": [
                    {"contentType": "image/jpeg", "sizeBytes": 11, "copies": 2},
                    {"contentType": "image/png", "sizeBytes": 9, "copies": 1}
                ]})),
            )
            .await;
        assert_eq!(status, 201, "{order}");
        for (i, upload) in order["uploads"]
            .as_array()
            .expect("uploads")
            .iter()
            .enumerate()
        {
            let (bytes, content_type) = if i == 0 {
                (b"jpeg-bytes!".to_vec(), "image/jpeg")
            } else {
                (b"png-bytes".to_vec(), "image/png")
            };
            let mut put = self.http.put(upload["url"].as_str().expect("url"));
            for (name, value) in upload["headers"].as_object().expect("headers") {
                put = put.header(name, value.as_str().expect("header value"));
            }
            let response = put.body(bytes).send().await.expect("upload");
            assert_eq!(response.status().as_u16(), 200, "upload {content_type}");
        }
        order
    }
}

pub async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..100 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}
