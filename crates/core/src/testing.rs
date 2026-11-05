//! In-memory adapters for every port, for unit tests, the local server and the simulation e2e.
//! Behind the `testing` feature (always on for this crate's own tests).
//!
//! Every adapter is a cheap `Clone` handle onto shared state, so a test can keep one handle and
//! give another to the service.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use fotoko_protocol::{
    OrderId, OrderStatus, PayosWebhook, PayosWebhookData, PhotoId, PrintJob, PrinterId,
    PrinterStatus,
};

use crate::domain::{Order, Printer};
use crate::error::CoreError;
use crate::payos::sign_webhook_data;
use crate::ports::{
    Clock, IdGen, JobPublisher, ObjectStore, OrderRepo, PaymentGateway, PaymentLink,
    PaymentLinkRequest, PresignedPut, PrinterRepo, PutSpec,
};
use crate::service::{OrderService, Ports, ServiceConfig};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// In-memory [`OrderRepo`].
#[derive(Debug, Clone, Default)]
pub struct InMemoryOrderRepo {
    orders: Arc<Mutex<HashMap<OrderId, Order>>>,
}

impl InMemoryOrderRepo {
    /// Empty repo.
    pub fn new() -> Self {
        Self::default()
    }

    /// The stored order, for assertions.
    pub fn snapshot(&self, order_id: &OrderId) -> Option<Order> {
        lock(&self.orders).get(order_id).cloned()
    }

    /// Every stored order, sorted by id.
    pub fn all(&self) -> Vec<Order> {
        let mut orders: Vec<Order> = lock(&self.orders).values().cloned().collect();
        orders.sort_by(|a, b| a.order_id.cmp(&b.order_id));
        orders
    }
}

#[async_trait]
impl OrderRepo for InMemoryOrderRepo {
    async fn get(&self, order_id: &OrderId) -> Result<Option<Order>, CoreError> {
        Ok(self.snapshot(order_id))
    }

    async fn get_by_order_code(&self, order_code: i64) -> Result<Option<Order>, CoreError> {
        Ok(lock(&self.orders)
            .values()
            .find(|o| o.order_code == Some(order_code))
            .cloned())
    }

    async fn insert(&self, order: &Order) -> Result<(), CoreError> {
        let mut orders = lock(&self.orders);
        if orders.contains_key(&order.order_id) {
            return Err(CoreError::Conflict(format!(
                "order {} already exists",
                order.order_id
            )));
        }
        orders.insert(order.order_id.clone(), order.clone());
        Ok(())
    }

    async fn update(&self, order: &Order, expected: OrderStatus) -> Result<(), CoreError> {
        let mut orders = lock(&self.orders);
        let Some(stored) = orders.get_mut(&order.order_id) else {
            return Err(CoreError::NotFound(format!(
                "order {} not found",
                order.order_id
            )));
        };
        if stored.status != expected {
            return Err(CoreError::Conflict(format!(
                "order {} changed concurrently",
                order.order_id
            )));
        }
        *stored = order.clone();
        Ok(())
    }

    async fn list_due_for_expiry(&self, now: DateTime<Utc>) -> Result<Vec<Order>, CoreError> {
        let mut due: Vec<Order> = lock(&self.orders)
            .values()
            .filter(|o| o.is_due_for_expiry(now))
            .cloned()
            .collect();
        due.sort_by(|a, b| a.order_id.cmp(&b.order_id));
        Ok(due)
    }
}

/// In-memory [`PrinterRepo`].
#[derive(Debug, Clone, Default)]
pub struct InMemoryPrinterRepo {
    printers: Arc<Mutex<HashMap<PrinterId, Printer>>>,
}

impl InMemoryPrinterRepo {
    /// Empty repo.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a printer (seeding; not part of the port).
    pub fn insert(&self, printer: Printer) {
        lock(&self.printers).insert(printer.printer_id.clone(), printer);
    }

    /// The stored printer, for assertions.
    pub fn snapshot(&self, printer_id: &PrinterId) -> Option<Printer> {
        lock(&self.printers).get(printer_id).cloned()
    }
}

#[async_trait]
impl PrinterRepo for InMemoryPrinterRepo {
    async fn get(&self, printer_id: &PrinterId) -> Result<Option<Printer>, CoreError> {
        Ok(self.snapshot(printer_id))
    }

    async fn update_status(
        &self,
        printer_id: &PrinterId,
        status: PrinterStatus,
        last_seen_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let mut printers = lock(&self.printers);
        let Some(printer) = printers.get_mut(printer_id) else {
            return Err(CoreError::NotFound(format!(
                "printer {printer_id} not found"
            )));
        };
        printer.status = status;
        printer.last_seen_at = Some(last_seen_at);
        Ok(())
    }
}

/// In-memory [`ObjectStore`]. Presigned URLs are fake `https://store.test/...` strings; an
/// object "exists" once [`mark_uploaded`](Self::mark_uploaded) was called for its key.
#[derive(Debug, Clone, Default)]
pub struct InMemoryObjectStore {
    objects: Arc<Mutex<HashSet<String>>>,
}

impl InMemoryObjectStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pretend the client PUT the object.
    pub fn mark_uploaded(&self, key: &str) {
        lock(&self.objects).insert(key.to_owned());
    }

    /// Mark every photo of the order as uploaded.
    pub fn mark_order_uploaded(&self, order: &Order) {
        for photo in &order.photos {
            self.mark_uploaded(&photo.object_key(&order.order_id));
        }
    }
}

#[async_trait]
impl ObjectStore for InMemoryObjectStore {
    async fn presign_put(&self, spec: &PutSpec) -> Result<PresignedPut, CoreError> {
        let content_type = serde_json::to_value(spec.content_type)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        Ok(PresignedPut {
            url: format!(
                "https://store.test/{}?op=put&ttl={}&size={}",
                spec.key,
                spec.ttl.as_secs(),
                spec.size_bytes
            ),
            headers: BTreeMap::from([("content-type".to_owned(), content_type)]),
        })
    }

    async fn presign_get(&self, key: &str, ttl: std::time::Duration) -> Result<String, CoreError> {
        Ok(format!(
            "https://store.test/{key}?op=get&ttl={}",
            ttl.as_secs()
        ))
    }

    async fn exists(&self, key: &str) -> Result<bool, CoreError> {
        Ok(lock(&self.objects).contains(key))
    }
}

/// [`JobPublisher`] that records every job. [`fail_with`](Self::fail_with) makes it fail.
#[derive(Debug, Clone, Default)]
pub struct RecordingJobPublisher {
    jobs: Arc<Mutex<Vec<PrintJob>>>,
    failure: Arc<Mutex<Option<CoreError>>>,
}

impl RecordingJobPublisher {
    /// Empty publisher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Jobs published so far, oldest first.
    pub fn jobs(&self) -> Vec<PrintJob> {
        lock(&self.jobs).clone()
    }

    /// Make every following publish fail with `error`, or succeed again with `None`.
    pub fn fail_with(&self, error: Option<CoreError>) {
        *lock(&self.failure) = error;
    }
}

#[async_trait]
impl JobPublisher for RecordingJobPublisher {
    async fn publish_job(&self, job: &PrintJob) -> Result<(), CoreError> {
        if let Some(error) = lock(&self.failure).clone() {
            return Err(error);
        }
        lock(&self.jobs).push(job.clone());
        Ok(())
    }
}

/// [`PaymentGateway`] that records requests and answers with
/// `https://pay.test/{orderCode}` and QR `qr-{orderCode}`.
#[derive(Debug, Clone, Default)]
pub struct FakePaymentGateway {
    requests: Arc<Mutex<Vec<PaymentLinkRequest>>>,
    failure: Arc<Mutex<Option<CoreError>>>,
}

impl FakePaymentGateway {
    /// Gateway that succeeds.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests received so far, oldest first.
    pub fn requests(&self) -> Vec<PaymentLinkRequest> {
        lock(&self.requests).clone()
    }

    /// Make every following call fail with `error`, or succeed again with `None`.
    pub fn fail_with(&self, error: Option<CoreError>) {
        *lock(&self.failure) = error;
    }
}

#[async_trait]
impl PaymentGateway for FakePaymentGateway {
    async fn create_payment_link(
        &self,
        request: &PaymentLinkRequest,
    ) -> Result<PaymentLink, CoreError> {
        if let Some(error) = lock(&self.failure).clone() {
            return Err(error);
        }
        lock(&self.requests).push(request.clone());
        Ok(PaymentLink {
            checkout_url: format!("https://pay.test/{}", request.order_code),
            qr_code: format!("qr-{}", request.order_code),
        })
    }
}

/// [`Clock`] that always returns the same instant.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// [`Clock`] a test moves by hand.
#[derive(Debug, Clone)]
pub struct ManualClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl ManualClock {
    /// Clock starting at `start`.
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            now: Arc::new(Mutex::new(start)),
        }
    }

    /// Jump to `now`.
    pub fn set(&self, now: DateTime<Utc>) {
        *lock(&self.now) = now;
    }

    /// Move forward (or backward, for a negative duration).
    pub fn advance(&self, by: ChronoDuration) {
        let mut now = lock(&self.now);
        *now += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *lock(&self.now)
    }
}

/// [`IdGen`] with predictable output: orders `ord-1`, `ord-2`, ..., photos `pho-1`, ... and order
/// codes counting up from 1000.
#[derive(Debug, Clone)]
pub struct SequentialIdGen {
    orders: Arc<AtomicU64>,
    photos: Arc<AtomicU64>,
    codes: Arc<AtomicI64>,
}

impl Default for SequentialIdGen {
    fn default() -> Self {
        Self::starting_at_order_code(1000)
    }
}

impl SequentialIdGen {
    /// Generator whose first order code is 1000.
    pub fn new() -> Self {
        Self::default()
    }

    /// Make the next [`IdGen::order_code`] return `next` (shared by every clone).
    pub fn set_next_order_code(&self, next: i64) {
        self.codes.store(next, Ordering::SeqCst);
    }

    /// Generator whose first order code is `first`.
    pub fn starting_at_order_code(first: i64) -> Self {
        Self {
            orders: Arc::new(AtomicU64::new(1)),
            photos: Arc::new(AtomicU64::new(1)),
            codes: Arc::new(AtomicI64::new(first)),
        }
    }
}

impl IdGen for SequentialIdGen {
    fn order_id(&self) -> OrderId {
        OrderId::from(format!(
            "ord-{}",
            self.orders.fetch_add(1, Ordering::SeqCst)
        ))
    }

    fn photo_id(&self) -> PhotoId {
        PhotoId::from(format!(
            "pho-{}",
            self.photos.fetch_add(1, Ordering::SeqCst)
        ))
    }

    fn order_code(&self) -> i64 {
        self.codes.fetch_add(1, Ordering::SeqCst)
    }
}

/// A webhook as payOS would send it, signed with `checksum_key`. `code` is the payment result
/// (`"00"` = paid).
pub fn signed_webhook(
    checksum_key: &str,
    code: &str,
    order_code: i64,
    amount: u64,
) -> Result<PayosWebhook, CoreError> {
    let data = PayosWebhookData {
        order_code,
        amount,
        extra: BTreeMap::from([
            ("code".to_owned(), serde_json::json!(code)),
            ("currency".to_owned(), serde_json::json!("VND")),
            (
                "description".to_owned(),
                serde_json::json!(format!("Fotoko {order_code}")),
            ),
        ]),
    };
    Ok(PayosWebhook {
        code: code.to_owned(),
        desc: if code == "00" { "success" } else { "failed" }.to_owned(),
        success: code == "00",
        signature: sign_webhook_data(checksum_key, &data)?,
        data,
    })
}

/// Checksum key used by [`Harness`].
pub const TEST_CHECKSUM_KEY: &str = "test-checksum-key";
/// Public app URL used by [`Harness`].
pub const TEST_APP_URL: &str = "https://app.test";

/// A service wired to fresh in-memory adapters, with a handle on each.
#[derive(Debug, Clone)]
pub struct Harness {
    /// The service under test.
    pub service: OrderService,
    /// Order storage.
    pub orders: InMemoryOrderRepo,
    /// Printer storage.
    pub printers: InMemoryPrinterRepo,
    /// Photo storage.
    pub store: InMemoryObjectStore,
    /// Published jobs.
    pub publisher: RecordingJobPublisher,
    /// payOS stand-in.
    pub payments: FakePaymentGateway,
    /// Controllable time, starting at 2026-01-01T00:00:00Z.
    pub clock: ManualClock,
    /// Predictable ids.
    pub ids: SequentialIdGen,
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

impl Harness {
    /// Fresh harness with [`TEST_CHECKSUM_KEY`] and [`TEST_APP_URL`].
    pub fn new() -> Self {
        let orders = InMemoryOrderRepo::new();
        let printers = InMemoryPrinterRepo::new();
        let store = InMemoryObjectStore::new();
        let publisher = RecordingJobPublisher::new();
        let payments = FakePaymentGateway::new();
        let clock = ManualClock::new(DateTime::UNIX_EPOCH + ChronoDuration::seconds(1_767_225_600));
        let ids = SequentialIdGen::new();
        let service = OrderService::new(
            Ports {
                orders: Arc::new(orders.clone()),
                printers: Arc::new(printers.clone()),
                store: Arc::new(store.clone()),
                publisher: Arc::new(publisher.clone()),
                payments: Arc::new(payments.clone()),
                clock: Arc::new(clock.clone()),
                ids: Arc::new(ids.clone()),
            },
            ServiceConfig::new(TEST_APP_URL, TEST_CHECKSUM_KEY),
        );
        Self {
            service,
            orders,
            printers,
            store,
            publisher,
            payments,
            clock,
            ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Photo;
    use crate::payos::verify_webhook;
    use fotoko_protocol::ContentType;

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().expect("ts")
    }

    fn order(id: &str, code: Option<i64>, status: OrderStatus, expires: &str) -> Order {
        Order {
            order_id: id.into(),
            owner: "me".into(),
            printer_id: PrinterId::new("stand-001").expect("id"),
            status,
            amount_vnd: 1,
            order_code: code,
            checkout_url: None,
            qr_code: None,
            photos: vec![Photo {
                photo_id: "p1".into(),
                content_type: ContentType::Jpeg,
                size_bytes: 1,
                copies: 1,
                uploaded: false,
            }],
            created_at: ts("2026-01-01T00:00:00Z"),
            updated_at: ts("2026-01-01T00:00:00Z"),
            expires_at: ts(expires),
            error: None,
        }
    }

    #[tokio::test]
    async fn order_repo_insert_get_and_code_lookup() {
        let repo = InMemoryOrderRepo::new();
        let o = order(
            "o1",
            Some(5),
            OrderStatus::AwaitingPayment,
            "2026-01-01T00:15:00Z",
        );
        repo.insert(&o).await.expect("insert");
        assert_eq!(repo.get(&"o1".into()).await, Ok(Some(o.clone())));
        assert_eq!(repo.get(&"nope".into()).await, Ok(None));
        assert_eq!(repo.get_by_order_code(5).await, Ok(Some(o.clone())));
        assert_eq!(repo.get_by_order_code(6).await, Ok(None));
        assert_eq!(repo.all(), vec![o.clone()]);
        let err = repo.insert(&o).await.expect_err("duplicate");
        assert!(matches!(err, CoreError::Conflict(_)));
    }

    #[tokio::test]
    async fn order_repo_update_is_conditional() {
        let repo = InMemoryOrderRepo::new();
        let o = order(
            "o1",
            None,
            OrderStatus::AwaitingUpload,
            "2026-01-01T00:15:00Z",
        );
        repo.insert(&o).await.expect("insert");

        let mut next = o.clone();
        next.status = OrderStatus::AwaitingPayment;
        repo.update(&next, OrderStatus::AwaitingUpload)
            .await
            .expect("update");
        assert_eq!(repo.snapshot(&"o1".into()), Some(next.clone()));

        // Stored status is awaiting_payment now, so expecting awaiting_upload conflicts.
        let err = repo
            .update(&o, OrderStatus::AwaitingUpload)
            .await
            .expect_err("stale expectation");
        assert!(matches!(err, CoreError::Conflict(_)));
        assert_eq!(repo.snapshot(&"o1".into()), Some(next));

        let missing = order(
            "zz",
            None,
            OrderStatus::AwaitingUpload,
            "2026-01-01T00:15:00Z",
        );
        let err = repo
            .update(&missing, OrderStatus::AwaitingUpload)
            .await
            .expect_err("missing");
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn order_repo_lists_due_orders_sorted() {
        let repo = InMemoryOrderRepo::new();
        for o in [
            order(
                "b",
                None,
                OrderStatus::AwaitingUpload,
                "2026-01-01T00:10:00Z",
            ),
            order(
                "a",
                None,
                OrderStatus::AwaitingPayment,
                "2026-01-01T00:10:00Z",
            ),
            order(
                "c",
                None,
                OrderStatus::AwaitingUpload,
                "2026-01-01T00:30:00Z",
            ),
            order("d", None, OrderStatus::Paid, "2026-01-01T00:10:00Z"),
        ] {
            repo.insert(&o).await.expect("insert");
        }
        let due = repo
            .list_due_for_expiry(ts("2026-01-01T00:20:00Z"))
            .await
            .expect("list");
        let ids: Vec<_> = due.iter().map(|o| o.order_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[tokio::test]
    async fn printer_repo_get_and_update_status() {
        let repo = InMemoryPrinterRepo::new();
        let id = PrinterId::new("stand-001").expect("id");
        assert_eq!(repo.get(&id).await, Ok(None));
        let err = repo
            .update_status(&id, PrinterStatus::Busy, ts("2026-01-01T00:00:00Z"))
            .await
            .expect_err("unknown");
        assert!(matches!(err, CoreError::NotFound(_)));

        repo.insert(Printer {
            printer_id: id.clone(),
            name: "Stand 1".into(),
            price_per_print_vnd: 10_000,
            status: PrinterStatus::Online,
            last_seen_at: None,
        });
        repo.update_status(&id, PrinterStatus::Busy, ts("2026-01-01T00:00:05Z"))
            .await
            .expect("update");
        let printer = repo.get(&id).await.expect("get").expect("exists");
        assert_eq!(printer.status, PrinterStatus::Busy);
        assert_eq!(printer.last_seen_at, Some(ts("2026-01-01T00:00:05Z")));
    }

    #[tokio::test]
    async fn object_store_presigns_and_tracks_uploads() {
        let store = InMemoryObjectStore::new();
        let put = store
            .presign_put(&PutSpec {
                key: "orders/o1/p1.jpg".into(),
                content_type: ContentType::Jpeg,
                size_bytes: 42,
                ttl: std::time::Duration::from_secs(900),
            })
            .await
            .expect("presign");
        assert_eq!(
            put.url,
            "https://store.test/orders/o1/p1.jpg?op=put&ttl=900&size=42"
        );
        assert_eq!(
            put.headers,
            BTreeMap::from([("content-type".to_owned(), "image/jpeg".to_owned())])
        );
        assert_eq!(
            store
                .presign_get("orders/o1/p1.jpg", std::time::Duration::from_secs(900))
                .await,
            Ok("https://store.test/orders/o1/p1.jpg?op=get&ttl=900".to_owned())
        );
        assert_eq!(store.exists("orders/o1/p1.jpg").await, Ok(false));
        store.mark_uploaded("orders/o1/p1.jpg");
        assert_eq!(store.exists("orders/o1/p1.jpg").await, Ok(true));

        let o = order(
            "o2",
            None,
            OrderStatus::AwaitingUpload,
            "2026-01-01T00:15:00Z",
        );
        store.mark_order_uploaded(&o);
        assert_eq!(store.exists("orders/o2/p1.jpg").await, Ok(true));
    }

    fn job() -> PrintJob {
        PrintJob {
            job_id: "j1".into(),
            order_id: "o1".into(),
            printer_id: PrinterId::new("stand-001").expect("id"),
            media: fotoko_protocol::Media::Photo4x6,
            created_at: ts("2026-01-01T00:00:00Z"),
            photos: vec![],
        }
    }

    #[tokio::test]
    async fn publisher_records_and_can_fail() {
        let publisher = RecordingJobPublisher::new();
        publisher.publish_job(&job()).await.expect("publish");
        assert_eq!(publisher.jobs(), vec![job()]);

        publisher.fail_with(Some(CoreError::Internal("broker down".into())));
        let err = publisher.publish_job(&job()).await.expect_err("fails");
        assert_eq!(err, CoreError::Internal("broker down".into()));
        assert_eq!(publisher.jobs().len(), 1);

        publisher.fail_with(None);
        publisher.publish_job(&job()).await.expect("recovered");
        assert_eq!(publisher.jobs().len(), 2);
    }

    fn link_request() -> PaymentLinkRequest {
        PaymentLinkRequest {
            order_code: 1001,
            amount_vnd: 20_000,
            description: "Fotoko 1001".into(),
            return_url: "r".into(),
            cancel_url: "c".into(),
            expires_at: ts("2026-01-01T00:15:00Z"),
        }
    }

    #[tokio::test]
    async fn gateway_records_and_can_fail() {
        let gateway = FakePaymentGateway::new();
        let link = gateway
            .create_payment_link(&link_request())
            .await
            .expect("link");
        assert_eq!(link.checkout_url, "https://pay.test/1001");
        assert_eq!(link.qr_code, "qr-1001");
        assert_eq!(gateway.requests(), vec![link_request()]);

        gateway.fail_with(Some(CoreError::PaymentProvider("down".into())));
        let err = gateway
            .create_payment_link(&link_request())
            .await
            .expect_err("fails");
        assert_eq!(err, CoreError::PaymentProvider("down".into()));
        assert_eq!(gateway.requests().len(), 1);

        gateway.fail_with(None);
        assert!(gateway.create_payment_link(&link_request()).await.is_ok());
    }

    #[test]
    fn clocks() {
        let start = ts("2026-01-01T00:00:00Z");
        assert_eq!(FixedClock(start).now(), start);
        let clock = ManualClock::new(start);
        let shared = clock.clone();
        clock.advance(ChronoDuration::minutes(5));
        assert_eq!(shared.now(), ts("2026-01-01T00:05:00Z"));
        clock.set(start);
        assert_eq!(shared.now(), start);
    }

    #[test]
    fn id_gen_counts_up() {
        let ids = SequentialIdGen::new();
        assert_eq!(ids.order_id(), OrderId::from("ord-1"));
        assert_eq!(ids.order_id(), OrderId::from("ord-2"));
        assert_eq!(ids.photo_id(), PhotoId::from("pho-1"));
        assert_eq!(ids.order_code(), 1000);
        assert_eq!(ids.order_code(), 1001);
        let custom = SequentialIdGen::starting_at_order_code(5);
        assert_eq!(custom.order_code(), 5);
        custom.set_next_order_code(42);
        assert_eq!(custom.clone().order_code(), 42);
    }

    #[test]
    fn signed_webhook_verifies_and_reflects_result() {
        let paid = signed_webhook("k", "00", 7, 100).expect("webhook");
        assert!(verify_webhook("k", &paid));
        assert!(paid.success);
        assert_eq!(paid.desc, "success");
        assert_eq!(paid.data.order_code, 7);
        let failed = signed_webhook("k", "01", 7, 100).expect("webhook");
        assert!(verify_webhook("k", &failed));
        assert!(!failed.success);
        assert_eq!(failed.desc, "failed");
        assert!(!verify_webhook("other", &paid));
    }

    #[test]
    fn harness_wires_shared_state() {
        let h = Harness::new();
        assert_eq!(h.clock.now(), ts("2026-01-01T00:00:00Z"));
        assert!(h.orders.all().is_empty());
        let _ = Harness::default();
    }
}
