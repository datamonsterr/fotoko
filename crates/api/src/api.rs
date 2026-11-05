//! The axum router implementing the HTTP API of `docs/contracts.md` over [`OrderService`].

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Instant;

use async_trait::async_trait;
use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRequest, Path, Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use fotoko_core::{CoreError, OrderService, WebhookOutcome};
use fotoko_protocol::{CreateOrderRequest, Health, OrderId, PayosWebhook, PrinterId, WebhookAck};
use serde::de::DeserializeOwned;
use tracing::Instrument;

use crate::error::ApiError;
use crate::identity::{Identity, IdentityMode};

/// Request bodies larger than this are rejected with 413. Order requests are a few hundred
/// bytes and a payOS webhook about 1 KiB.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Request id header, read from the caller when sane and echoed on every response.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// `CoreError::Validation` message the service uses for a bad webhook signature.
const INVALID_SIGNATURE: &str = "invalid webhook signature";

/// Re-reads the payOS secret after a verification failure, so a rotated checksum key does not
/// need a Lambda container recycle.
#[async_trait]
pub trait ServiceRefresher: Send + Sync {
    /// A service built on freshly loaded secrets, or `None` when nothing changed (or a refresh
    /// was done too recently).
    async fn refresh(&self) -> Result<Option<OrderService>, CoreError>;
}

/// Shared state of the router.
#[derive(Clone)]
pub struct AppState {
    service: Arc<RwLock<OrderService>>,
    identity_mode: IdentityMode,
    refresher: Option<Arc<dyn ServiceRefresher>>,
}

impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState")
            .field("identity_mode", &self.identity_mode)
            .field("refreshable", &self.refresher.is_some())
            .finish_non_exhaustive()
    }
}

impl AppState {
    /// State over `service`; identities come from `identity_mode`.
    pub fn new(service: OrderService, identity_mode: IdentityMode) -> Self {
        Self {
            service: Arc::new(RwLock::new(service)),
            identity_mode,
            refresher: None,
        }
    }

    /// Let the router refresh the service when payOS verification fails once.
    #[must_use]
    pub fn with_refresher(mut self, refresher: Arc<dyn ServiceRefresher>) -> Self {
        self.refresher = Some(refresher);
        self
    }

    /// Where caller identities come from.
    pub fn identity_mode(&self) -> IdentityMode {
        self.identity_mode
    }

    /// The current service (cheap clone).
    pub fn service(&self) -> OrderService {
        self.service
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Ask the refresher for a new service and swap it in. `true` when the service changed.
    async fn refresh(&self) -> bool {
        let Some(refresher) = &self.refresher else {
            return false;
        };
        match refresher.refresh().await {
            Ok(Some(fresh)) => {
                *self.service.write().unwrap_or_else(PoisonError::into_inner) = fresh;
                tracing::info!("payOS secret changed, service rebuilt");
                true
            }
            Ok(None) => false,
            Err(error) => {
                tracing::error!(%error, "refreshing the payOS secret failed");
                false
            }
        }
    }
}

/// The HTTP API with request ids and access logs.
pub fn router(state: AppState) -> Router {
    with_request_context(api_routes(state))
}

/// Request id header and access log span around every route of `router`.
pub fn with_request_context(router: Router) -> Router {
    router.layer(middleware::from_fn(request_context))
}

/// The routes of the HTTP API without the request-context layer, so that the local server can
/// merge its own routes before layering.
pub fn api_routes(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/printers/{printer_id}", get(get_printer))
        .route("/v1/orders", post(create_order))
        .route("/v1/orders/{order_id}", get(get_order))
        .route("/v1/orders/{order_id}/checkout", post(checkout))
        .route("/v1/orders/{order_id}/cancel", post(cancel))
        .route("/v1/payments/payos/webhook", post(payos_webhook))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

/// A JSON body that reports problems in the contract error format.
struct JsonBody<T>(T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for JsonBody<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state).await.map_err(|rejection| {
            if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                ApiError::with_status(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    CoreError::Validation(format!(
                        "request body is larger than {MAX_BODY_BYTES} bytes"
                    )),
                )
            } else {
                ApiError::from(CoreError::Validation("unreadable request body".into()))
            }
        })?;
        parse_json(&bytes).map(Self)
    }
}

fn parse_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(bytes)
        .map_err(|e| CoreError::Validation(format!("invalid JSON body: {e}")).into())
}

fn printer_id(raw: &str) -> Result<PrinterId, ApiError> {
    PrinterId::new(raw).map_err(|e| CoreError::Validation(e.to_string()).into())
}

async fn healthz() -> Json<Health> {
    Json(Health { ok: true })
}

async fn not_found() -> ApiError {
    CoreError::NotFound("route not found".into()).into()
}

async fn method_not_allowed() -> ApiError {
    ApiError::with_status(
        StatusCode::METHOD_NOT_ALLOWED,
        CoreError::Validation("method not allowed".into()),
    )
}

async fn get_printer(
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> Result<Json<fotoko_protocol::PrinterInfo>, ApiError> {
    let id = printer_id(&raw)?;
    Ok(Json(state.service().get_printer(&id).await?))
}

async fn create_order(
    State(state): State<AppState>,
    Identity(owner): Identity,
    JsonBody(request): JsonBody<CreateOrderRequest>,
) -> Result<(StatusCode, Json<fotoko_protocol::CreateOrderResponse>), ApiError> {
    let response = state.service().create_order(&owner, &request).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

async fn get_order(
    State(state): State<AppState>,
    Identity(owner): Identity,
    Path(order_id): Path<String>,
) -> Result<Json<fotoko_protocol::Order>, ApiError> {
    Ok(Json(
        state
            .service()
            .get_order(&owner, &OrderId::from(order_id))
            .await?,
    ))
}

async fn checkout(
    State(state): State<AppState>,
    Identity(owner): Identity,
    Path(order_id): Path<String>,
) -> Result<Json<fotoko_protocol::CheckoutResponse>, ApiError> {
    let result = state
        .service()
        .checkout(&owner, &OrderId::from(order_id))
        .await;
    if matches!(result, Err(CoreError::PaymentProvider(_))) {
        // A rotated payOS key shows up here first; the next request then uses the new one.
        state.refresh().await;
    }
    Ok(Json(result?))
}

async fn cancel(
    State(state): State<AppState>,
    Identity(owner): Identity,
    Path(order_id): Path<String>,
) -> Result<Json<fotoko_protocol::Order>, ApiError> {
    Ok(Json(
        state
            .service()
            .cancel(&owner, &OrderId::from(order_id))
            .await?,
    ))
}

/// payOS webhook. Acknowledged (200) for every outcome that retrying cannot improve: paid,
/// duplicate, not paid, a late payment (logged for a manual refund) and an unknown order code
/// (payOS sends a sample webhook when the URL is confirmed). A bad signature is 400.
async fn payos_webhook(
    State(state): State<AppState>,
    JsonBody(webhook): JsonBody<PayosWebhook>,
) -> Result<Json<WebhookAck>, ApiError> {
    let order_code = webhook.data.order_code;
    let mut result = state.service().handle_payos_webhook(&webhook).await;
    if matches!(&result, Err(CoreError::Validation(m)) if m == INVALID_SIGNATURE)
        && state.refresh().await
    {
        // The key was rotated since this container started: verify once more with the new one.
        result = state.service().handle_payos_webhook(&webhook).await;
    }
    match result {
        Ok(WebhookOutcome::LatePayment) => {
            tracing::warn!(
                order_code,
                amount_vnd = webhook.data.amount,
                "late payment for a cancelled or expired order: manual refund needed"
            );
        }
        Ok(outcome) => tracing::info!(order_code, ?outcome, "payOS webhook handled"),
        Err(CoreError::NotFound(_)) => {
            tracing::info!(
                order_code,
                "payOS webhook for an unknown order code, acknowledged"
            );
        }
        Err(error) => return Err(error.into()),
    }
    Ok(Json(WebhookAck { success: true }))
}

fn sane_request_id(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 64
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// Request id, access log span and timing.
async fn request_context(request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|v| sane_request_id(v))
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    let span = tracing::info_span!(
        "request",
        request_id = %request_id,
        method = %request.method(),
        path = %request.uri().path(),
    );
    let started = Instant::now();
    let mut response = next.run(request).instrument(span.clone()).await;
    span.in_scope(|| {
        tracing::info!(
            status = response.status().as_u16(),
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "request done"
        );
    });
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Method, Request};
    use fotoko_core::Printer;
    use fotoko_core::testing::{Harness, TEST_CHECKSUM_KEY, signed_webhook};
    use fotoko_protocol::{OrderStatus, PrinterStatus};
    use http_body_util::BodyExt;
    use lambda_http::request::RequestContext;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    struct Reply {
        status: u16,
        headers: axum::http::HeaderMap,
        body: Value,
    }

    fn printer(id: &str, status: PrinterStatus) -> Printer {
        Printer {
            printer_id: PrinterId::new(id).expect("id"),
            name: "Stand".into(),
            price_per_print_vnd: 10_000,
            status,
            last_seen_at: None,
        }
    }

    fn harness() -> Harness {
        let h = Harness::new();
        h.printers
            .insert(printer("stand-001", PrinterStatus::Online));
        h.printers
            .insert(printer("stand-off", PrinterStatus::Offline));
        h
    }

    fn app(h: &Harness, mode: IdentityMode) -> Router {
        router(AppState::new(h.service.clone(), mode))
    }

    async fn call(app: &Router, request: Request<Body>) -> Reply {
        let response = app.clone().oneshot(request).await.expect("response");
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json body")
        };
        Reply {
            status,
            headers,
            body,
        }
    }

    fn req(method: Method, uri: &str, who: Option<&str>, body: Option<Value>) -> Request<Body> {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(who) = who {
            b = b.header("x-fotoko-identity", who);
        }
        match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(v.to_string()))
                .expect("request"),
            None => b.body(Body::empty()).expect("request"),
        }
    }

    fn order_body(printer: &str) -> Value {
        json!({"printerId": printer, "photos": [
            {"contentType": "image/jpeg", "sizeBytes": 1000, "copies": 2},
            {"contentType": "image/png", "sizeBytes": 2000, "copies": 1}
        ]})
    }

    async fn create(app: &Router, who: &str) -> Value {
        let r = call(
            app,
            req(
                Method::POST,
                "/v1/orders",
                Some(who),
                Some(order_body("stand-001")),
            ),
        )
        .await;
        assert_eq!(r.status, 201, "{}", r.body);
        r.body
    }

    /// Creates, uploads and checks out an order owned by `me`; returns (orderId, orderCode).
    async fn checked_out(h: &Harness, app: &Router) -> (String, i64) {
        let created = create(app, "me").await;
        let id = created["orderId"].as_str().expect("id").to_owned();
        let order = h.orders.snapshot(&id.as_str().into()).expect("stored");
        h.store.mark_order_uploaded(&order);
        let r = call(
            app,
            req(
                Method::POST,
                &format!("/v1/orders/{id}/checkout"),
                Some("me"),
                None,
            ),
        )
        .await;
        assert_eq!(r.status, 200, "{}", r.body);
        (id, r.body["orderCode"].as_i64().expect("code"))
    }

    async fn webhook(app: &Router, code: &str, order_code: i64, amount: u64) -> Reply {
        let hook = signed_webhook(TEST_CHECKSUM_KEY, code, order_code, amount).expect("sign");
        call(
            app,
            req(
                Method::POST,
                "/v1/payments/payos/webhook",
                None,
                Some(serde_json::to_value(hook).expect("json")),
            ),
        )
        .await
    }

    fn error_of(r: &Reply) -> (&str, &str) {
        (
            r.body["error"]["code"].as_str().expect("code"),
            r.body["error"]["message"].as_str().expect("message"),
        )
    }

    #[tokio::test]
    async fn healthz_is_public() {
        let h = harness();
        for mode in [IdentityMode::Local, IdentityMode::Aws] {
            let r = call(&app(&h, mode), req(Method::GET, "/healthz", None, None)).await;
            assert_eq!((r.status, r.body), (200, json!({"ok": true})));
        }
    }

    #[tokio::test]
    async fn printer_route() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let r = call(&a, req(Method::GET, "/v1/printers/stand-001", None, None)).await;
        assert_eq!(r.status, 200);
        assert_eq!(
            r.body,
            json!({"printerId": "stand-001", "name": "Stand", "status": "online",
                   "pricePerPrintVnd": 10000, "media": ["4x6"]})
        );
        let r = call(&a, req(Method::GET, "/v1/printers/nope", None, None)).await;
        assert_eq!((r.status, error_of(&r).0), (404, "not_found"));
        let r = call(&a, req(Method::GET, "/v1/printers/a%2Bb", None, None)).await;
        assert_eq!((r.status, error_of(&r).0), (400, "validation"));
    }

    #[tokio::test]
    async fn create_order_returns_201_with_uploads() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let body = create(&a, "me").await;
        assert_eq!(body["status"], "awaiting_upload");
        assert_eq!(body["amountVnd"], 30_000);
        assert_eq!(body["uploads"].as_array().expect("uploads").len(), 2);
        assert_eq!(body["uploads"][0]["method"], "PUT");
        assert_eq!(h.orders.all()[0].owner, "me");
    }

    #[tokio::test]
    async fn order_routes_need_an_identity() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        for (method, uri, body) in [
            (Method::POST, "/v1/orders", Some(order_body("stand-001"))),
            (Method::GET, "/v1/orders/x", None),
            (Method::POST, "/v1/orders/x/checkout", None),
            (Method::POST, "/v1/orders/x/cancel", None),
        ] {
            let r = call(&a, req(method, uri, None, body)).await;
            assert_eq!((r.status, error_of(&r).0), (403, "forbidden"), "{uri}");
        }
    }

    #[tokio::test]
    async fn create_order_error_codes() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let post = |body: Value| req(Method::POST, "/v1/orders", Some("me"), Some(body));

        let r = call(&a, post(json!({"printerId": "stand-001", "photos": []}))).await;
        assert_eq!((r.status, error_of(&r).0), (400, "validation"));
        let r = call(&a, post(order_body("stand-off"))).await;
        assert_eq!((r.status, error_of(&r).0), (400, "validation"));
        let r = call(&a, post(order_body("ghost"))).await;
        assert_eq!((r.status, error_of(&r).0), (404, "not_found"));
        let r = call(
            &a,
            post(json!({"printerId": "stand-001", "photos": [
            {"contentType": "image/gif", "sizeBytes": 1, "copies": 1}]})),
        )
        .await;
        assert_eq!((r.status, error_of(&r).0), (400, "validation"));
        assert!(error_of(&r).1.starts_with("invalid JSON body"));
        let r = call(
            &a,
            Request::builder()
                .method(Method::POST)
                .uri("/v1/orders")
                .header("x-fotoko-identity", "me")
                .body(Body::from("not json"))
                .expect("request"),
        )
        .await;
        assert_eq!((r.status, error_of(&r).0), (400, "validation"));
    }

    /// A request body whose stream breaks half way.
    struct BrokenBody;

    impl http_body::Body for BrokenBody {
        type Data = Bytes;
        type Error = std::io::Error;

        fn poll_frame(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
            std::task::Poll::Ready(Some(Err(std::io::Error::other("connection reset"))))
        }
    }

    #[tokio::test]
    async fn unreadable_body_is_400_validation() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/v1/orders")
            .header("x-fotoko-identity", "me")
            .body(Body::new(BrokenBody))
            .expect("request");
        let r = call(&a, request).await;
        assert_eq!(
            (r.status, error_of(&r)),
            (400, ("validation", "unreadable request body"))
        );
    }

    #[tokio::test]
    async fn oversized_body_is_413_validation() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let big = "x".repeat(MAX_BODY_BYTES + 1);
        let r = call(
            &a,
            req(
                Method::POST,
                "/v1/orders",
                Some("me"),
                Some(json!({"printerId": big, "photos": []})),
            ),
        )
        .await;
        assert_eq!((r.status, error_of(&r).0), (413, "validation"));
        let r = call(
            &a,
            req(
                Method::POST,
                "/v1/payments/payos/webhook",
                None,
                Some(json!({"pad": "x".repeat(MAX_BODY_BYTES)})),
            ),
        )
        .await;
        assert_eq!(r.status, 413);
    }

    #[tokio::test]
    async fn get_order_is_owner_only() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let id = create(&a, "me").await["orderId"]
            .as_str()
            .expect("id")
            .to_owned();
        let uri = format!("/v1/orders/{id}");
        let r = call(&a, req(Method::GET, &uri, Some("me"), None)).await;
        assert_eq!(r.status, 200);
        assert_eq!(r.body["orderId"], id.as_str());
        assert_eq!(r.body["photos"][0]["uploaded"], false);
        let r = call(&a, req(Method::GET, &uri, Some("other"), None)).await;
        assert_eq!((r.status, error_of(&r).0), (403, "forbidden"));
        let r = call(&a, req(Method::GET, "/v1/orders/none", Some("me"), None)).await;
        assert_eq!((r.status, error_of(&r).0), (404, "not_found"));
    }

    #[tokio::test]
    async fn checkout_requires_uploads_then_succeeds() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let id = create(&a, "me").await["orderId"]
            .as_str()
            .expect("id")
            .to_owned();
        let uri = format!("/v1/orders/{id}/checkout");
        let r = call(&a, req(Method::POST, &uri, Some("me"), None)).await;
        assert_eq!((r.status, error_of(&r).0), (409, "conflict"));

        h.store
            .mark_order_uploaded(&h.orders.snapshot(&id.as_str().into()).expect("order"));
        let r = call(&a, req(Method::POST, &uri, Some("me"), None)).await;
        assert_eq!(r.status, 200);
        assert_eq!(r.body["status"], "awaiting_payment");
        assert_eq!(r.body["checkoutUrl"], "https://pay.test/1000");

        let r = call(&a, req(Method::POST, &uri, Some("other"), None)).await;
        assert_eq!((r.status, error_of(&r).0), (403, "forbidden"));
    }

    #[tokio::test]
    async fn checkout_payment_failure_is_502() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let id = create(&a, "me").await["orderId"]
            .as_str()
            .expect("id")
            .to_owned();
        h.store
            .mark_order_uploaded(&h.orders.snapshot(&id.as_str().into()).expect("order"));
        h.payments
            .fail_with(Some(CoreError::PaymentProvider("payOS said no".into())));
        let r = call(
            &a,
            req(
                Method::POST,
                &format!("/v1/orders/{id}/checkout"),
                Some("me"),
                None,
            ),
        )
        .await;
        assert_eq!(
            (r.status, error_of(&r)),
            (502, ("payment_provider", "payOS said no"))
        );
    }

    #[tokio::test]
    async fn cancel_before_paid_only() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let id = create(&a, "me").await["orderId"]
            .as_str()
            .expect("id")
            .to_owned();
        let uri = format!("/v1/orders/{id}/cancel");
        let r = call(&a, req(Method::POST, &uri, Some("other"), None)).await;
        assert_eq!(r.status, 403);
        let r = call(&a, req(Method::POST, &uri, Some("me"), None)).await;
        assert_eq!(
            (r.status, r.body["status"].as_str()),
            (200, Some("cancelled"))
        );
        let r = call(&a, req(Method::POST, &uri, Some("me"), None)).await;
        assert_eq!((r.status, error_of(&r).0), (409, "conflict"));
    }

    #[tokio::test]
    async fn internal_errors_do_not_leak() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        h.orders.fail_with(
            "get",
            Some(CoreError::Internal("table fotoko-prod is on fire".into())),
        );
        let r = call(&a, req(Method::GET, "/v1/orders/x", Some("me"), None)).await;
        assert_eq!(r.status, 500);
        assert_eq!(error_of(&r), ("internal", "internal server error"));
        assert!(!r.body.to_string().contains("fire"));
    }

    #[tokio::test]
    async fn webhook_paid_then_duplicate() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let (id, code) = checked_out(&h, &a).await;
        let r = webhook(&a, "00", code, 30_000).await;
        assert_eq!((r.status, r.body), (200, json!({"success": true})));
        assert_eq!(
            h.orders
                .snapshot(&id.as_str().into())
                .expect("order")
                .status,
            OrderStatus::Paid
        );
        assert_eq!(h.publisher.jobs().len(), 1);
        let r = webhook(&a, "00", code, 30_000).await;
        assert_eq!((r.status, r.body), (200, json!({"success": true})));
    }

    #[tokio::test]
    async fn webhook_not_paid_is_acknowledged_and_changes_nothing() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let (id, code) = checked_out(&h, &a).await;
        let r = webhook(&a, "01", code, 30_000).await;
        assert_eq!((r.status, r.body), (200, json!({"success": true})));
        assert_eq!(
            h.orders
                .snapshot(&id.as_str().into())
                .expect("order")
                .status,
            OrderStatus::AwaitingPayment
        );
        assert!(h.publisher.jobs().is_empty());
    }

    #[tokio::test]
    async fn webhook_late_payment_is_acknowledged() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let (id, code) = checked_out(&h, &a).await;
        let r = call(
            &a,
            req(
                Method::POST,
                &format!("/v1/orders/{id}/cancel"),
                Some("me"),
                None,
            ),
        )
        .await;
        assert_eq!(r.status, 200);
        let r = webhook(&a, "00", code, 30_000).await;
        assert_eq!((r.status, r.body), (200, json!({"success": true})));
        assert_eq!(
            h.orders
                .snapshot(&id.as_str().into())
                .expect("order")
                .status,
            OrderStatus::Cancelled
        );
        assert!(h.publisher.jobs().is_empty());
    }

    #[tokio::test]
    async fn webhook_unknown_order_code_is_acknowledged() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let r = webhook(&a, "00", 123, 3000).await;
        assert_eq!((r.status, r.body), (200, json!({"success": true})));
    }

    #[tokio::test]
    async fn webhook_rejections() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let (_, code) = checked_out(&h, &a).await;

        let mut forged =
            serde_json::to_value(signed_webhook("other-key", "00", code, 30_000).expect("sign"))
                .expect("json");
        let r = call(
            &a,
            req(
                Method::POST,
                "/v1/payments/payos/webhook",
                None,
                Some(forged.clone()),
            ),
        )
        .await;
        assert_eq!((r.status, error_of(&r).0), (400, "validation"));
        forged["signature"] = json!("zz");
        let r = call(
            &a,
            req(
                Method::POST,
                "/v1/payments/payos/webhook",
                None,
                Some(forged),
            ),
        )
        .await;
        assert_eq!(r.status, 400);

        let r = call(
            &a,
            req(
                Method::POST,
                "/v1/payments/payos/webhook",
                None,
                Some(json!({"hello": 1})),
            ),
        )
        .await;
        assert_eq!((r.status, error_of(&r).0), (400, "validation"));

        let r = webhook(&a, "00", code, 1).await;
        assert_eq!((r.status, error_of(&r).0), (400, "validation"));
    }

    #[tokio::test]
    async fn webhook_internal_failure_is_not_acknowledged() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let (_, code) = checked_out(&h, &a).await;
        h.publisher
            .fail_with(Some(CoreError::Internal("mqtt down".into())));
        let r = webhook(&a, "00", code, 30_000).await;
        assert_eq!((r.status, error_of(&r).0), (500, "internal"));
    }

    #[tokio::test]
    async fn unknown_route_and_wrong_method() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let r = call(&a, req(Method::GET, "/nope", None, None)).await;
        assert_eq!((r.status, error_of(&r).0), (404, "not_found"));
        let r = call(&a, req(Method::DELETE, "/v1/orders", Some("me"), None)).await;
        assert_eq!((r.status, error_of(&r).0), (405, "validation"));
    }

    #[tokio::test]
    async fn request_id_is_echoed_or_generated() {
        let h = harness();
        let a = app(&h, IdentityMode::Local);
        let mut request = req(Method::GET, "/healthz", None, None);
        request
            .headers_mut()
            .insert(REQUEST_ID_HEADER, HeaderValue::from_static("abc-123"));
        let r = call(&a, request).await;
        assert_eq!(r.headers[REQUEST_ID_HEADER], "abc-123");

        let mut request = req(Method::GET, "/healthz", None, None);
        request.headers_mut().insert(
            REQUEST_ID_HEADER,
            HeaderValue::from_static("bad id with spaces"),
        );
        let r = call(&a, request).await;
        let generated = r.headers[REQUEST_ID_HEADER]
            .to_str()
            .expect("str")
            .to_owned();
        assert_ne!(generated, "bad id with spaces");
        assert_eq!(generated.len(), 36);

        let r = call(&a, req(Method::GET, "/healthz", None, None)).await;
        assert_eq!(r.headers[REQUEST_ID_HEADER].len(), 36);
    }

    #[test]
    fn request_id_sanity() {
        assert!(sane_request_id("a-B_1.2:3"));
        assert!(!sane_request_id(""));
        assert!(!sane_request_id(&"a".repeat(65)));
        assert!(!sane_request_id("a b"));
    }

    #[tokio::test]
    async fn aws_mode_trusts_only_the_request_context() {
        let h = harness();
        let a = app(&h, IdentityMode::Aws);
        // The header is ignored in aws mode.
        let r = call(
            &a,
            req(
                Method::POST,
                "/v1/orders",
                Some("attacker"),
                Some(order_body("stand-001")),
            ),
        )
        .await;
        assert_eq!((r.status, error_of(&r).0), (403, "forbidden"));

        let ctx = serde_json::from_value(json!({
            "http": {"method": "POST", "path": "/v1/orders"},
            "authorizer": {"iam": {"cognitoIdentity":
                {"amr": [], "identityId": "ap-southeast-1:guest-9"}}}
        }))
        .expect("context");
        let mut request = req(
            Method::POST,
            "/v1/orders",
            Some("attacker"),
            Some(order_body("stand-001")),
        );
        request
            .extensions_mut()
            .insert(RequestContext::ApiGatewayV2(ctx));
        let r = call(&a, request).await;
        assert_eq!(r.status, 201);
        assert_eq!(h.orders.all()[0].owner, "ap-southeast-1:guest-9");

        // Public routes need no identity.
        let r = call(&a, req(Method::GET, "/v1/printers/stand-001", None, None)).await;
        assert_eq!(r.status, 200);
    }

    struct FakeRefresher {
        next: std::sync::Mutex<Result<Option<OrderService>, CoreError>>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeRefresher {
        fn new(next: Result<Option<OrderService>, CoreError>) -> Arc<Self> {
            Arc::new(Self {
                next: std::sync::Mutex::new(next),
                calls: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ServiceRefresher for FakeRefresher {
        async fn refresh(&self) -> Result<Option<OrderService>, CoreError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.next.lock().expect("lock").clone()
        }
    }

    /// The harness's ports with another payOS checksum key (a rotated secret).
    fn rotated_service(h: &Harness, key: &str) -> OrderService {
        OrderService::new(
            fotoko_core::Ports {
                orders: Arc::new(h.orders.clone()),
                printers: Arc::new(h.printers.clone()),
                store: Arc::new(h.store.clone()),
                publisher: Arc::new(h.publisher.clone()),
                payments: Arc::new(h.payments.clone()),
                clock: Arc::new(h.clock.clone()),
                ids: Arc::new(h.ids.clone()),
            },
            fotoko_core::ServiceConfig::new(fotoko_core::testing::TEST_APP_URL, key)
                .expect("config"),
        )
    }

    fn app_with(h: &Harness, refresher: Arc<FakeRefresher>) -> Router {
        router(AppState::new(h.service.clone(), IdentityMode::Local).with_refresher(refresher))
    }

    async fn webhook_with_key(app: &Router, key: &str, order_code: i64, amount: u64) -> Reply {
        let hook = signed_webhook(key, "00", order_code, amount).expect("sign");
        call(
            app,
            req(
                Method::POST,
                "/v1/payments/payos/webhook",
                None,
                Some(serde_json::to_value(hook).expect("json")),
            ),
        )
        .await
    }

    #[tokio::test]
    async fn rotated_key_is_picked_up_once_on_a_signature_failure() {
        let h = harness();
        let (id, code) = checked_out(&h, &app(&h, IdentityMode::Local)).await;
        let refresher = FakeRefresher::new(Ok(Some(rotated_service(&h, "rotated-key"))));
        let a = app_with(&h, refresher.clone());

        let r = webhook_with_key(&a, "rotated-key", code, 30_000).await;
        assert_eq!((r.status, r.body), (200, json!({"success": true})));
        assert_eq!(refresher.calls(), 1);
        assert_eq!(
            h.orders
                .snapshot(&id.as_str().into())
                .expect("order")
                .status,
            OrderStatus::Paid
        );
        // The rotated service is now the current one: no refresh needed for the next webhook.
        let r = webhook_with_key(&a, "rotated-key", code, 30_000).await;
        assert_eq!(r.status, 200);
        assert_eq!(refresher.calls(), 1);
    }

    #[tokio::test]
    async fn bad_signatures_stay_400_when_nothing_changed_or_refresh_fails() {
        let h = harness();
        let (_, code) = checked_out(&h, &app(&h, IdentityMode::Local)).await;
        for next in [Ok(None), Err(CoreError::Internal("secrets down".into()))] {
            let refresher = FakeRefresher::new(next);
            let a = app_with(&h, refresher.clone());
            let r = webhook_with_key(&a, "wrong-key", code, 30_000).await;
            assert_eq!(
                (r.status, error_of(&r)),
                (400, ("validation", INVALID_SIGNATURE))
            );
            assert_eq!(refresher.calls(), 1);
        }
        // Without a refresher the first verdict stands.
        let r = webhook_with_key(&app(&h, IdentityMode::Local), "wrong-key", code, 30_000).await;
        assert_eq!(r.status, 400);
    }

    #[tokio::test]
    async fn other_webhook_errors_do_not_refresh() {
        let h = harness();
        let (_, code) = checked_out(&h, &app(&h, IdentityMode::Local)).await;
        let refresher = FakeRefresher::new(Ok(None));
        let a = app_with(&h, refresher.clone());
        let r = webhook_with_key(&a, TEST_CHECKSUM_KEY, code, 1).await; // amount mismatch
        assert_eq!(r.status, 400);
        let r = webhook_with_key(&a, TEST_CHECKSUM_KEY, code, 30_000).await;
        assert_eq!(r.status, 200);
        assert_eq!(refresher.calls(), 0);
    }

    #[tokio::test]
    async fn checkout_provider_failure_triggers_a_refresh_but_no_retry() {
        let h = harness();
        let refresher = FakeRefresher::new(Ok(None));
        let a = app_with(&h, refresher.clone());
        let id = create(&a, "me").await["orderId"]
            .as_str()
            .expect("id")
            .to_owned();
        h.store
            .mark_order_uploaded(&h.orders.snapshot(&id.as_str().into()).expect("order"));
        h.payments
            .fail_with(Some(CoreError::PaymentProvider("bad key".into())));
        let r = call(
            &a,
            req(
                Method::POST,
                &format!("/v1/orders/{id}/checkout"),
                Some("me"),
                None,
            ),
        )
        .await;
        assert_eq!(r.status, 502);
        assert_eq!(refresher.calls(), 1);
        assert_eq!(h.payments.requests().len(), 0);
    }

    #[test]
    fn state_debug_does_not_dump_the_service() {
        let h = harness();
        let state = AppState::new(h.service.clone(), IdentityMode::Aws);
        let text = format!("{state:?}");
        assert!(text.contains("refreshable: false"));
        assert_eq!(state.identity_mode(), IdentityMode::Aws);
    }
}
