//! payOS stand-in for the Playwright run and docker compose.
//!
//! * `POST /v2/payment-requests` creates a payment link like payOS (signed response whose
//!   `checkoutUrl` is this server's hosted page).
//! * `GET /web/{orderCode}` is the hosted checkout page with a "Pay" button.
//! * `POST /web/{orderCode}/pay` sends the signed paid webhook to the API, then redirects the
//!   customer to the order's `returnUrl`.
//!
//! Environment: `PAYOS_CHECKSUM_KEY`, `PAYOS_CLIENT_ID` and `PAYOS_API_KEY` (the API's
//! credentials, all required), `FOTOKO_API_URL` (webhook target, default
//! `http://127.0.0.1:8787`), `PAYOS_MOCK_ADDR` (listen address, default `127.0.0.1:0`, the
//! chosen address is printed) and `PAYOS_MOCK_PUBLIC_URL` (base of `checkoutUrl`, default the
//! listen address).
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, extract::Request};
use fotoko_core::testing::signed_webhook;
use fotoko_e2e::payos::payment_link_response;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone)]
struct Link {
    amount: u64,
    description: String,
    return_url: String,
    cancel_url: String,
}

struct Mock {
    client_id: String,
    api_key: String,
    checksum_key: String,
    api_url: String,
    public_url: String,
    http: reqwest::Client,
    links: Mutex<HashMap<i64, Link>>,
}

type Shared = Arc<Mock>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaymentRequest {
    order_code: i64,
    amount: u64,
    description: String,
    return_url: String,
    cancel_url: String,
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("environment variable {name} is required"))
}

fn error_body(desc: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"code": "20", "desc": desc})),
    )
        .into_response()
}

async fn create_payment_request(
    State(mock): State<Shared>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if header("x-client-id") != Some(mock.client_id.as_str())
        || header("x-api-key") != Some(mock.api_key.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"code": "401", "desc": "bad credentials"})),
        )
            .into_response();
    }
    let Ok(raw) = serde_json::from_slice::<Value>(&body) else {
        return error_body("body is not JSON");
    };
    let Ok(request) = serde_json::from_value::<PaymentRequest>(raw.clone()) else {
        return error_body("missing fields");
    };
    let checkout_url = format!("{}/web/{}", mock.public_url, request.order_code);
    let response = match payment_link_response(&mock.checksum_key, &raw, &checkout_url) {
        Ok(response) => response,
        Err(why) => return error_body(&why),
    };
    if let Ok(mut links) = mock.links.lock() {
        links.insert(
            request.order_code,
            Link {
                amount: request.amount,
                description: request.description,
                return_url: request.return_url,
                cancel_url: request.cancel_url,
            },
        );
    }
    Json(response).into_response()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn link(mock: &Mock, order_code: i64) -> Option<Link> {
    mock.links.lock().ok()?.get(&order_code).cloned()
}

async fn checkout_page(State(mock): State<Shared>, Path(order_code): Path<i64>) -> Response {
    let Some(link) = link(&mock, order_code) else {
        return (StatusCode::NOT_FOUND, "unknown payment link").into_response();
    };
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>payOS mock</title></head>
<body style="font-family: sans-serif; max-width: 24rem; margin: 2rem auto">
<h1 id="payos-title">payOS (mock)</h1>
<p id="payos-description">{description}</p>
<p id="payos-amount">{amount} VND</p>
<form method="post" action="/web/{order_code}/pay"><button id="payos-pay" type="submit">Pay</button></form>
<p><a id="payos-cancel" href="{cancel}">Cancel</a></p>
</body>
</html>
"#,
        description = escape(&link.description),
        amount = link.amount,
        cancel = escape(&link.cancel_url),
    ))
    .into_response()
}

async fn pay(
    State(mock): State<Shared>,
    Path(order_code): Path<i64>,
    _request: Request,
) -> Response {
    let Some(link) = link(&mock, order_code) else {
        return (StatusCode::NOT_FOUND, "unknown payment link").into_response();
    };
    let webhook = match signed_webhook(&mock.checksum_key, "00", order_code, link.amount) {
        Ok(webhook) => webhook,
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    };
    let sent = mock
        .http
        .post(format!("{}/v1/payments/payos/webhook", mock.api_url))
        .json(&webhook)
        .send()
        .await;
    match sent {
        Ok(response) if response.status().is_success() => {
            let separator = if link.return_url.contains('?') {
                '&'
            } else {
                '?'
            };
            Redirect::to(&format!(
                "{}{separator}code=00&status=PAID&orderCode={order_code}",
                link.return_url
            ))
            .into_response()
        }
        Ok(response) => {
            let status = response.status();
            eprintln!("fotoko-payos-mock: the API answered {status} to the webhook");
            (
                StatusCode::BAD_GATEWAY,
                format!("webhook refused: {status}"),
            )
                .into_response()
        }
        Err(error) => {
            eprintln!("fotoko-payos-mock: cannot deliver the webhook: {error}");
            (StatusCode::BAD_GATEWAY, "webhook not delivered").into_response()
        }
    }
}

async fn healthz() -> Json<Value> {
    Json(json!({"ok": true}))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = std::env::var("PAYOS_MOCK_ADDR").unwrap_or_else(|_| "127.0.0.1:0".to_owned());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let local = listener.local_addr()?;
    let public_url = std::env::var("PAYOS_MOCK_PUBLIC_URL")
        .unwrap_or_else(|_| format!("http://{local}"))
        .trim_end_matches('/')
        .to_owned();
    let mock = Arc::new(Mock {
        client_id: env("PAYOS_CLIENT_ID")?,
        api_key: env("PAYOS_API_KEY")?,
        checksum_key: env("PAYOS_CHECKSUM_KEY")?,
        api_url: std::env::var("FOTOKO_API_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8787".to_owned())
            .trim_end_matches('/')
            .to_owned(),
        public_url,
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?,
        links: Mutex::new(HashMap::new()),
    });
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/v2/payment-requests", post(create_payment_request))
        .route("/web/{order_code}", get(checkout_page))
        .route("/web/{order_code}/pay", post(pay))
        .with_state(mock);
    println!("fotoko-payos-mock listening on http://{local}");
    axum::serve(listener, app).await?;
    Ok(())
}
