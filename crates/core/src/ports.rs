//! Ports: the traits the use cases depend on. Adapters live in `fotoko-api` (AWS, local) and in
//! [`crate::testing`] (in-memory).
//!
//! The IO ports are `async` traits written with `#[async_trait]` so that they are object safe:
//! `fotoko-api` picks DynamoDB/S3/IoT or the in-memory/local adapters at runtime (`FOTOKO_MODE`)
//! and the service holds them as `Arc<dyn Port>`. Native `async fn` in traits would force seven
//! generic parameters on [`crate::OrderService`] and is not usable behind `dyn`.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use fotoko_protocol::{
    ContentType, OrderId, OrderStatus, PhotoId, PrintJob, PrinterId, PrinterStatus,
};

use crate::domain::{Order, Printer};
use crate::error::CoreError;

/// Persistence of orders.
#[async_trait]
pub trait OrderRepo: Send + Sync {
    /// The order, or `None`.
    async fn get(&self, order_id: &OrderId) -> Result<Option<Order>, CoreError>;

    /// The order that carries this payOS order code, or `None`.
    async fn get_by_order_code(&self, order_code: i64) -> Result<Option<Order>, CoreError>;

    /// Store a new order. `Conflict` when the id already exists.
    async fn insert(&self, order: &Order) -> Result<(), CoreError>;

    /// Replace the order if its stored status is still `expected` (conditional write).
    /// `Conflict` when the stored status differs, `NotFound` when the order does not exist.
    async fn update(&self, order: &Order, expected: OrderStatus) -> Result<(), CoreError>;

    /// Unpaid orders (`awaiting_upload`, `awaiting_payment`) with `expires_at < now`.
    async fn list_due_for_expiry(&self, now: DateTime<Utc>) -> Result<Vec<Order>, CoreError>;
}

/// Persistence of printers.
#[async_trait]
pub trait PrinterRepo: Send + Sync {
    /// The printer, or `None`.
    async fn get(&self, printer_id: &PrinterId) -> Result<Option<Printer>, CoreError>;

    /// Set status and `lastSeenAt`. `NotFound` when the printer does not exist.
    async fn update_status(
        &self,
        printer_id: &PrinterId,
        status: PrinterStatus,
        last_seen_at: DateTime<Utc>,
    ) -> Result<(), CoreError>;
}

/// What the client must PUT to upload one photo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutSpec {
    /// Object key `orders/{orderId}/{photoId}.{jpg|png}`.
    pub key: String,
    /// The upload must carry this content type.
    pub content_type: ContentType,
    /// Declared size; adapters may sign it into the URL.
    pub size_bytes: u64,
    /// How long the URL stays valid.
    pub ttl: Duration,
}

/// A presigned upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresignedPut {
    /// URL to PUT to.
    pub url: String,
    /// Headers the client must send with the PUT.
    pub headers: BTreeMap<String, String>,
}

/// Photo storage (S3).
#[async_trait]
pub trait ObjectStore: Send + Sync {
    /// Presigned PUT for one photo.
    async fn presign_put(&self, spec: &PutSpec) -> Result<PresignedPut, CoreError>;

    /// Presigned GET URL valid for `ttl`.
    async fn presign_get(&self, key: &str, ttl: Duration) -> Result<String, CoreError>;

    /// Whether the object exists.
    async fn exists(&self, key: &str) -> Result<bool, CoreError>;
}

/// Delivery of print jobs to Pis (MQTT `fotoko/printers/{printerId}/jobs`, QoS 1).
#[async_trait]
pub trait JobPublisher: Send + Sync {
    /// Publish a job on the printer's jobs topic (`fotoko_protocol::jobs_topic`).
    async fn publish_job(&self, job: &PrintJob) -> Result<(), CoreError>;
}

/// Input of `POST /v2/payment-requests`. The adapter adds credentials and the signature
/// ([`crate::payos::sign_payment_request`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentLinkRequest {
    /// Unique payOS order code.
    pub order_code: i64,
    /// Amount in VND.
    pub amount_vnd: u64,
    /// At most 25 characters.
    pub description: String,
    /// `{PUBLIC_APP_URL}/orders/{orderId}`.
    pub return_url: String,
    /// `{PUBLIC_APP_URL}/orders/{orderId}`.
    pub cancel_url: String,
    /// The link should stop working when the order expires (payOS `expiredAt`, not signed).
    pub expires_at: DateTime<Utc>,
}

/// A created payment link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentLink {
    /// payOS hosted checkout page.
    pub checkout_url: String,
    /// QR code payload.
    pub qr_code: String,
}

/// payOS. Failures are `CoreError::PaymentProvider`.
#[async_trait]
pub trait PaymentGateway: Send + Sync {
    /// Create a payment link.
    async fn create_payment_link(
        &self,
        request: &PaymentLinkRequest,
    ) -> Result<PaymentLink, CoreError>;
}

/// Source of time.
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> DateTime<Utc>;
}

/// [`Clock`] backed by the operating system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        let micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_micros()).ok())
            .unwrap_or(0);
        DateTime::from_timestamp_micros(micros).unwrap_or(DateTime::UNIX_EPOCH)
    }
}

/// Source of ids and payOS order codes.
pub trait IdGen: Send + Sync {
    /// A fresh order id.
    fn order_id(&self) -> OrderId;
    /// A fresh photo id.
    fn photo_id(&self) -> PhotoId;
    /// A fresh candidate payOS order code: positive and at most 2^53 - 1 (payOS limit).
    /// The service retries when the code is already taken.
    fn order_code(&self) -> i64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_is_recent() {
        let now = SystemClock.now();
        assert!(now > "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().expect("ts"));
    }
}
