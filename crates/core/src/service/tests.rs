use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use fotoko_protocol::{
    ContentType, CreateOrderRequest, CreateOrderResponse, Heartbeat, JobState, JobStatus,
    MAX_PHOTO_BYTES, Media, OrderId, OrderStatus, PhotoSpec, PiError, PiErrorCode, PrinterId,
    PrinterState, PrinterStatus,
};

use super::*;
use crate::domain::Printer;
use crate::testing::{Harness, TEST_CHECKSUM_KEY, signed_webhook};

const OWNER: &str = "guest-1";

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().expect("ts")
}

fn pid() -> PrinterId {
    PrinterId::new("stand-001").expect("id")
}

fn harness() -> Harness {
    let h = Harness::new();
    seed_printer(&h, PrinterStatus::Online, 10_000);
    h
}

fn seed_printer(h: &Harness, status: PrinterStatus, price: u64) {
    h.printers.insert(Printer {
        printer_id: pid(),
        name: "Stand 1".into(),
        price_per_print_vnd: price,
        status,
        last_seen_at: None,
    });
}

fn spec(content_type: ContentType, size_bytes: u64, copies: u32) -> PhotoSpec {
    PhotoSpec {
        content_type,
        size_bytes,
        copies,
    }
}

fn request(photos: Vec<PhotoSpec>) -> CreateOrderRequest {
    CreateOrderRequest {
        printer_id: pid(),
        photos,
    }
}

fn two_photos() -> CreateOrderRequest {
    request(vec![
        spec(ContentType::Jpeg, 1000, 2),
        spec(ContentType::Png, 2000, 3),
    ])
}

async fn create(h: &Harness) -> CreateOrderResponse {
    h.service
        .create_order(OWNER, &two_photos())
        .await
        .expect("create")
}

async fn uploaded(h: &Harness) -> OrderId {
    let created = create(h).await;
    let order = h.orders.snapshot(&created.order_id).expect("stored");
    h.store.mark_order_uploaded(&order);
    created.order_id
}

async fn awaiting_payment(h: &Harness) -> (OrderId, i64) {
    let id = uploaded(h).await;
    let checkout = h.service.checkout(OWNER, &id).await.expect("checkout");
    (id, checkout.order_code)
}

async fn paid(h: &Harness) -> OrderId {
    let (id, code) = awaiting_payment(h).await;
    pay(h, code, 50_000).await.expect("paid");
    id
}

async fn pay(h: &Harness, code: i64, amount: u64) -> Result<WebhookOutcome, CoreError> {
    let webhook = signed_webhook(TEST_CHECKSUM_KEY, "00", code, amount).expect("webhook");
    h.service.handle_payos_webhook(&webhook).await
}

fn status_of(h: &Harness, id: &OrderId) -> OrderStatus {
    h.orders.snapshot(id).expect("stored").status
}

fn job_status(id: &OrderId, state: JobState, error: Option<PiError>) -> JobStatus {
    JobStatus {
        job_id: format!("job-{id}").into(),
        order_id: id.clone(),
        printer_id: pid(),
        state,
        printed_copies: 0,
        error,
        at: ts("2026-01-01T00:01:00Z"),
    }
}

// ---- get_printer ----

#[tokio::test]
async fn get_printer_returns_info() {
    let h = harness();
    let info = h.service.get_printer(&pid()).await.expect("printer");
    assert_eq!(info.name, "Stand 1");
    assert_eq!(info.status, PrinterStatus::Online);
    assert_eq!(info.price_per_print_vnd, 10_000);
    assert_eq!(info.media, vec![Media::Photo4x6]);
}

#[tokio::test]
async fn get_printer_unknown_is_not_found() {
    let h = Harness::new();
    let err = h.service.get_printer(&pid()).await.expect_err("unknown");
    assert_eq!(
        err,
        CoreError::NotFound("printer stand-001 not found".into())
    );
}

// ---- create_order ----

#[tokio::test]
async fn create_order_prices_stores_and_presigns() {
    let h = harness();
    let created = create(&h).await;
    assert_eq!(created.order_id.as_str(), "ord-1");
    assert_eq!(created.status, OrderStatus::AwaitingUpload);
    assert_eq!(created.amount_vnd, 50_000);
    assert_eq!(created.expires_at, ts("2026-01-01T00:15:00Z"));
    assert_eq!(created.uploads.len(), 2);

    let first = &created.uploads[0];
    assert_eq!(first.photo_id.as_str(), "pho-1");
    assert_eq!(first.method, UploadMethod::Put);
    assert_eq!(first.expires_at, ts("2026-01-01T00:15:00Z"));
    assert_eq!(
        first.url,
        "https://store.test/orders/ord-1/pho-1.jpg?op=put&ttl=900&size=1000"
    );
    assert_eq!(
        first.headers.get("content-type").map(String::as_str),
        Some("image/jpeg")
    );
    assert_eq!(
        created.uploads[1].url,
        "https://store.test/orders/ord-1/pho-2.png?op=put&ttl=900&size=2000"
    );

    let stored = h.orders.snapshot(&created.order_id).expect("stored");
    assert_eq!(stored.owner, OWNER);
    assert_eq!(stored.status, OrderStatus::AwaitingUpload);
    assert_eq!(stored.order_code, None);
    assert_eq!(stored.created_at, ts("2026-01-01T00:00:00Z"));
    assert_eq!(stored.photos.len(), 2);
    assert!(stored.photos.iter().all(|p| !p.uploaded));
}

#[tokio::test]
async fn create_order_rejects_unknown_printer() {
    let h = Harness::new();
    let err = h
        .service
        .create_order(OWNER, &two_photos())
        .await
        .expect_err("unknown");
    assert!(matches!(err, CoreError::NotFound(_)));
    assert!(h.orders.all().is_empty());
}

#[tokio::test]
async fn create_order_requires_online_printer() {
    for status in [
        PrinterStatus::Offline,
        PrinterStatus::Busy,
        PrinterStatus::Error,
    ] {
        let h = Harness::new();
        seed_printer(&h, status, 10_000);
        let err = h
            .service
            .create_order(OWNER, &two_photos())
            .await
            .expect_err("not online");
        assert!(matches!(err, CoreError::Validation(_)), "{status:?}");
        assert!(h.orders.all().is_empty());
    }
}

#[tokio::test]
async fn create_order_enforces_limits() {
    let h = harness();
    let bad = [
        request(vec![]),
        request(vec![spec(ContentType::Jpeg, 100, 1); 11]),
        request(vec![spec(ContentType::Jpeg, 100, 0)]),
        request(vec![spec(ContentType::Jpeg, 100, 6)]),
        request(vec![spec(ContentType::Jpeg, 0, 1)]),
        request(vec![spec(ContentType::Png, MAX_PHOTO_BYTES + 1, 1)]),
    ];
    for req in bad {
        let err = h
            .service
            .create_order(OWNER, &req)
            .await
            .expect_err("invalid");
        assert!(matches!(err, CoreError::Validation(_)), "{req:?}");
    }
    assert!(h.orders.all().is_empty());

    // Boundaries are accepted.
    let ok = request(vec![spec(ContentType::Png, MAX_PHOTO_BYTES, 5); 10]);
    let created = h.service.create_order(OWNER, &ok).await.expect("boundary");
    assert_eq!(created.amount_vnd, 10 * 5 * 10_000);
}

#[tokio::test]
async fn create_order_price_overflow_is_validation() {
    let h = Harness::new();
    seed_printer(&h, PrinterStatus::Online, u64::MAX);
    let err = h
        .service
        .create_order(OWNER, &two_photos())
        .await
        .expect_err("overflow");
    assert!(matches!(err, CoreError::Validation(_)));
}

#[tokio::test]
async fn create_order_ttl_out_of_range_is_internal() {
    let h = harness();
    let mut service = h.service.clone();
    service.config.order_ttl = std::time::Duration::MAX;
    let err = service
        .create_order(OWNER, &two_photos())
        .await
        .expect_err("ttl");
    assert!(matches!(err, CoreError::Internal(_)));
}

// ---- checkout ----

#[tokio::test]
async fn checkout_creates_link_and_moves_to_awaiting_payment() {
    let h = harness();
    let id = uploaded(&h).await;
    h.clock.advance(ChronoDuration::minutes(1));
    let res = h.service.checkout(OWNER, &id).await.expect("checkout");
    assert_eq!(res.status, OrderStatus::AwaitingPayment);
    assert_eq!(res.order_code, 1000);
    assert_eq!(res.checkout_url, "https://pay.test/1000");
    assert_eq!(res.qr_code, "qr-1000");

    let requests = h.payments.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].order_code, 1000);
    assert_eq!(requests[0].amount_vnd, 50_000);
    assert_eq!(requests[0].description, "Fotoko 1000");
    assert_eq!(requests[0].return_url, "https://app.test/orders/ord-1");
    assert_eq!(requests[0].cancel_url, "https://app.test/orders/ord-1");
    assert_eq!(requests[0].expires_at, ts("2026-01-01T00:15:00Z"));

    let stored = h.orders.snapshot(&id).expect("stored");
    assert_eq!(stored.status, OrderStatus::AwaitingPayment);
    assert_eq!(stored.order_code, Some(1000));
    assert_eq!(stored.updated_at, ts("2026-01-01T00:01:00Z"));
    assert!(stored.photos.iter().all(|p| p.uploaded));
}

#[tokio::test]
async fn checkout_is_idempotent_while_awaiting_payment() {
    let h = harness();
    let id = uploaded(&h).await;
    let first = h.service.checkout(OWNER, &id).await.expect("first");
    let second = h.service.checkout(OWNER, &id).await.expect("second");
    assert_eq!(first, second);
    assert_eq!(h.payments.requests().len(), 1);
}

#[tokio::test]
async fn checkout_conflicts_when_a_photo_is_missing() {
    let h = harness();
    let created = create(&h).await;
    let order = h.orders.snapshot(&created.order_id).expect("stored");
    // Only the first photo arrives.
    h.store
        .mark_uploaded(&order.photos[0].object_key(&order.order_id));
    let err = h
        .service
        .checkout(OWNER, &created.order_id)
        .await
        .expect_err("missing");
    assert_eq!(
        err,
        CoreError::Conflict("photo pho-2 is not uploaded".into())
    );
    assert_eq!(
        status_of(&h, &created.order_id),
        OrderStatus::AwaitingUpload
    );
    assert!(h.payments.requests().is_empty());
}

#[tokio::test]
async fn checkout_checks_owner_and_existence() {
    let h = harness();
    let id = uploaded(&h).await;
    let err = h
        .service
        .checkout("intruder", &id)
        .await
        .expect_err("owner");
    assert!(matches!(err, CoreError::Forbidden(_)));
    let err = h
        .service
        .checkout(OWNER, &"nope".into())
        .await
        .expect_err("missing");
    assert!(matches!(err, CoreError::NotFound(_)));
}

#[tokio::test]
async fn checkout_conflicts_in_other_states() {
    let h = harness();
    let id = uploaded(&h).await;
    h.service.cancel(OWNER, &id).await.expect("cancel");
    let err = h.service.checkout(OWNER, &id).await.expect_err("cancelled");
    assert!(matches!(err, CoreError::Conflict(_)));

    let paid_id = paid(&h).await;
    let err = h.service.checkout(OWNER, &paid_id).await.expect_err("paid");
    assert!(matches!(err, CoreError::Conflict(_)));
}

#[tokio::test]
async fn checkout_after_expiry_expires_the_order_and_conflicts() {
    let h = harness();
    let id = uploaded(&h).await;
    h.clock.advance(ChronoDuration::minutes(16));
    let err = h.service.checkout(OWNER, &id).await.expect_err("expired");
    assert!(matches!(err, CoreError::Conflict(_)));
    assert_eq!(status_of(&h, &id), OrderStatus::Expired);
    assert!(h.payments.requests().is_empty());
}

#[tokio::test]
async fn checkout_awaiting_payment_without_link_conflicts() {
    let h = harness();
    let id = uploaded(&h).await;
    let mut order = h.orders.snapshot(&id).expect("stored");
    let prior = order.status;
    order.status = OrderStatus::AwaitingPayment;
    h.orders.update(&order, prior).await.expect("seed");
    let err = h.service.checkout(OWNER, &id).await.expect_err("no link");
    assert!(matches!(err, CoreError::Conflict(_)));
}

#[tokio::test]
async fn checkout_gateway_failure_leaves_order_untouched() {
    let h = harness();
    let id = uploaded(&h).await;
    h.payments
        .fail_with(Some(CoreError::PaymentProvider("payOS down".into())));
    let err = h.service.checkout(OWNER, &id).await.expect_err("gateway");
    assert_eq!(err, CoreError::PaymentProvider("payOS down".into()));
    let stored = h.orders.snapshot(&id).expect("stored");
    assert_eq!(stored.status, OrderStatus::AwaitingUpload);
    assert_eq!(stored.order_code, None);

    h.payments.fail_with(None);
    assert!(h.service.checkout(OWNER, &id).await.is_ok());
}

#[tokio::test]
async fn checkout_redraws_a_taken_order_code() {
    let h = harness();
    let (_, first_code) = awaiting_payment(&h).await;
    assert_eq!(first_code, 1000);
    // Rewind the generator so the next draw collides with the taken code.
    h.ids.set_next_order_code(1000);
    let second = h
        .service
        .create_order(OWNER, &two_photos())
        .await
        .expect("create");
    h.store
        .mark_order_uploaded(&h.orders.snapshot(&second.order_id).expect("stored"));
    let res = h
        .service
        .checkout(OWNER, &second.order_id)
        .await
        .expect("checkout");
    assert_eq!(res.order_code, 1001);
}

struct StuckIds;

impl IdGen for StuckIds {
    fn order_id(&self) -> OrderId {
        "ord-x".into()
    }
    fn photo_id(&self) -> fotoko_protocol::PhotoId {
        "pho-x".into()
    }
    fn order_code(&self) -> i64 {
        1000
    }
}

#[tokio::test]
async fn checkout_gives_up_when_every_code_is_taken() {
    let h = harness();
    awaiting_payment(&h).await; // takes code 1000
    let mut ports = h.service.ports.clone();
    ports.ids = Arc::new(StuckIds);
    let service = OrderService::new(ports, h.service.config.clone());
    let second = service
        .create_order(OWNER, &two_photos())
        .await
        .expect("create");
    h.store
        .mark_order_uploaded(&h.orders.snapshot(&second.order_id).expect("stored"));
    let err = service
        .checkout(OWNER, &second.order_id)
        .await
        .expect_err("exhausted");
    assert!(matches!(err, CoreError::Internal(_)));
}

#[tokio::test]
async fn checkout_description_is_capped_and_url_slash_trimmed() {
    let h = harness();
    let mut service = h.service.clone();
    service.config.public_app_url = "https://app.test/".into();
    let ids = crate::testing::SequentialIdGen::starting_at_order_code(9_007_199_254_740_991);
    service.ports.ids = Arc::new(ids);
    let created = service
        .create_order(OWNER, &two_photos())
        .await
        .expect("create");
    h.store
        .mark_order_uploaded(&h.orders.snapshot(&created.order_id).expect("stored"));
    service
        .checkout(OWNER, &created.order_id)
        .await
        .expect("checkout");
    let request = &h.payments.requests()[0];
    assert_eq!(request.description, "Fotoko 9007199254740991");
    assert!(request.description.chars().count() <= 25);
    assert_eq!(request.return_url, "https://app.test/orders/ord-1");
}

#[tokio::test]
async fn description_is_truncated_to_25_chars() {
    let h = harness();
    let mut service = h.service.clone();
    // 20-digit codes never come from IdGen in practice; this proves the cap itself.
    struct Huge;
    impl IdGen for Huge {
        fn order_id(&self) -> OrderId {
            "ord-h".into()
        }
        fn photo_id(&self) -> fotoko_protocol::PhotoId {
            "pho-h".into()
        }
        fn order_code(&self) -> i64 {
            i64::MAX
        }
    }
    service.ports.ids = Arc::new(Huge);
    let created = service
        .create_order(OWNER, &two_photos())
        .await
        .expect("create");
    h.store
        .mark_order_uploaded(&h.orders.snapshot(&created.order_id).expect("stored"));
    service
        .checkout(OWNER, &created.order_id)
        .await
        .expect("checkout");
    assert_eq!(h.payments.requests()[0].description.chars().count(), 25);
}

// ---- get_order ----

#[tokio::test]
async fn get_order_reflects_upload_progress() {
    let h = harness();
    let created = create(&h).await;
    let order = h.orders.snapshot(&created.order_id).expect("stored");
    h.store
        .mark_uploaded(&order.photos[1].object_key(&order.order_id));

    let dto = h
        .service
        .get_order(OWNER, &created.order_id)
        .await
        .expect("get");
    assert_eq!(dto.status, OrderStatus::AwaitingUpload);
    assert_eq!(
        dto.photos.iter().map(|p| p.uploaded).collect::<Vec<_>>(),
        [false, true]
    );
    assert_eq!(dto.order_code, None);
    // The refresh is a view; nothing is persisted.
    assert!(
        h.orders
            .snapshot(&created.order_id)
            .expect("stored")
            .photos
            .iter()
            .all(|p| !p.uploaded)
    );
}

#[tokio::test]
async fn get_order_after_checkout_has_code_and_uploaded_photos() {
    let h = harness();
    let (id, code) = awaiting_payment(&h).await;
    let dto = h.service.get_order(OWNER, &id).await.expect("get");
    assert_eq!(dto.status, OrderStatus::AwaitingPayment);
    assert_eq!(dto.order_code, Some(code));
    assert!(dto.photos.iter().all(|p| p.uploaded));
}

#[tokio::test]
async fn get_order_checks_owner_and_existence() {
    let h = harness();
    let id = uploaded(&h).await;
    let err = h
        .service
        .get_order("intruder", &id)
        .await
        .expect_err("owner");
    assert!(matches!(err, CoreError::Forbidden(_)));
    let err = h
        .service
        .get_order(OWNER, &"nope".into())
        .await
        .expect_err("missing");
    assert!(matches!(err, CoreError::NotFound(_)));
}

#[tokio::test]
async fn get_order_expires_overdue_unpaid_orders() {
    let h = harness();
    let id = uploaded(&h).await;
    h.clock.advance(ChronoDuration::minutes(15));
    let dto = h.service.get_order(OWNER, &id).await.expect("at the limit");
    assert_eq!(dto.status, OrderStatus::AwaitingUpload);

    h.clock.advance(ChronoDuration::seconds(1));
    let dto = h.service.get_order(OWNER, &id).await.expect("late");
    assert_eq!(dto.status, OrderStatus::Expired);
    assert_eq!(status_of(&h, &id), OrderStatus::Expired);
}

#[tokio::test]
async fn paid_orders_never_expire() {
    let h = harness();
    let id = paid(&h).await;
    h.clock.advance(ChronoDuration::hours(5));
    let dto = h.service.get_order(OWNER, &id).await.expect("get");
    assert_eq!(dto.status, OrderStatus::Paid);
}

/// Repo wrapper that can lose a race or fail on update.
#[derive(Debug, Clone)]
struct FaultyRepo {
    inner: crate::testing::InMemoryOrderRepo,
    /// Before the next update, silently move the stored order to this status.
    flip_to: Arc<Mutex<Option<OrderStatus>>>,
    /// Fail every update with this error.
    update_error: Arc<Mutex<Option<CoreError>>>,
}

impl FaultyRepo {
    fn new(inner: crate::testing::InMemoryOrderRepo) -> Self {
        Self {
            inner,
            flip_to: Arc::default(),
            update_error: Arc::default(),
        }
    }
}

#[async_trait]
impl OrderRepo for FaultyRepo {
    async fn get(&self, id: &OrderId) -> Result<Option<Order>, CoreError> {
        self.inner.get(id).await
    }
    async fn get_by_order_code(&self, code: i64) -> Result<Option<Order>, CoreError> {
        self.inner.get_by_order_code(code).await
    }
    async fn insert(&self, order: &Order) -> Result<(), CoreError> {
        self.inner.insert(order).await
    }
    async fn update(&self, order: &Order, expected: OrderStatus) -> Result<(), CoreError> {
        let error = self.update_error.lock().expect("lock").clone();
        if let Some(error) = error {
            return Err(error);
        }
        let flip = self.flip_to.lock().expect("lock").take();
        if let Some(status) = flip {
            let mut current = self.inner.snapshot(&order.order_id).expect("stored");
            let before = current.status;
            current.status = status;
            self.inner.update(&current, before).await?;
        }
        self.inner.update(order, expected).await
    }
    async fn list_due_for_expiry(&self, now: DateTime<Utc>) -> Result<Vec<Order>, CoreError> {
        self.inner.list_due_for_expiry(now).await
    }
}

fn service_over(h: &Harness, repo: &FaultyRepo) -> OrderService {
    let mut service = h.service.clone();
    service.ports.orders = Arc::new(repo.clone());
    service
}

#[tokio::test]
async fn get_order_prefers_a_concurrent_change_over_expiry() {
    let h = harness();
    let id = uploaded(&h).await;
    let repo = FaultyRepo::new(h.orders.clone());
    *repo.flip_to.lock().expect("lock") = Some(OrderStatus::Cancelled);
    h.clock.advance(ChronoDuration::minutes(20));
    let dto = service_over(&h, &repo)
        .get_order(OWNER, &id)
        .await
        .expect("get");
    assert_eq!(dto.status, OrderStatus::Cancelled);
}

#[tokio::test]
async fn get_order_propagates_storage_errors_during_expiry() {
    let h = harness();
    let id = uploaded(&h).await;
    let repo = FaultyRepo::new(h.orders.clone());
    *repo.update_error.lock().expect("lock") = Some(CoreError::Internal("db".into()));
    h.clock.advance(ChronoDuration::minutes(20));
    let err = service_over(&h, &repo)
        .get_order(OWNER, &id)
        .await
        .expect_err("db");
    assert_eq!(err, CoreError::Internal("db".into()));
}

// ---- cancel ----

#[tokio::test]
async fn cancel_before_payment() {
    let h = harness();
    let id = uploaded(&h).await;
    let dto = h.service.cancel(OWNER, &id).await.expect("cancel upload");
    assert_eq!(dto.status, OrderStatus::Cancelled);
    assert_eq!(status_of(&h, &id), OrderStatus::Cancelled);

    let (id, _) = awaiting_payment(&h).await;
    let dto = h.service.cancel(OWNER, &id).await.expect("cancel payment");
    assert_eq!(dto.status, OrderStatus::Cancelled);
}

#[tokio::test]
async fn cancel_rejects_paid_and_repeat() {
    let h = harness();
    let id = paid(&h).await;
    let err = h.service.cancel(OWNER, &id).await.expect_err("paid");
    assert!(matches!(err, CoreError::Conflict(_)));
    assert_eq!(status_of(&h, &id), OrderStatus::Paid);

    let other = uploaded(&h).await;
    h.service.cancel(OWNER, &other).await.expect("first");
    let err = h.service.cancel(OWNER, &other).await.expect_err("again");
    assert!(matches!(err, CoreError::Conflict(_)));
}

#[tokio::test]
async fn cancel_checks_owner_and_existence() {
    let h = harness();
    let id = uploaded(&h).await;
    let err = h.service.cancel("intruder", &id).await.expect_err("owner");
    assert!(matches!(err, CoreError::Forbidden(_)));
    assert_eq!(status_of(&h, &id), OrderStatus::AwaitingUpload);
    let err = h
        .service
        .cancel(OWNER, &"nope".into())
        .await
        .expect_err("missing");
    assert!(matches!(err, CoreError::NotFound(_)));
}

// ---- payOS webhook ----

#[tokio::test]
async fn webhook_marks_paid_and_publishes_the_job() {
    let h = harness();
    let (id, code) = awaiting_payment(&h).await;
    h.clock.advance(ChronoDuration::minutes(2));
    let outcome = pay(&h, code, 50_000).await.expect("webhook");
    assert_eq!(outcome, WebhookOutcome::Paid);
    assert_eq!(status_of(&h, &id), OrderStatus::Paid);
    assert_eq!(
        h.orders.snapshot(&id).expect("stored").updated_at,
        ts("2026-01-01T00:02:00Z")
    );

    let jobs = h.publisher.jobs();
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.job_id.as_str(), "job-ord-1");
    assert_eq!(job.order_id, id);
    assert_eq!(job.printer_id, pid());
    assert_eq!(job.media, Media::Photo4x6);
    assert_eq!(job.created_at, ts("2026-01-01T00:02:00Z"));
    assert_eq!(job.photos.len(), 2);
    assert_eq!(job.photos[0].photo_id.as_str(), "pho-1");
    assert_eq!(
        job.photos[0].url,
        "https://store.test/orders/ord-1/pho-1.jpg?op=get&ttl=900"
    );
    assert_eq!(job.photos[0].content_type, ContentType::Jpeg);
    assert_eq!(job.photos[0].copies, 2);
    assert_eq!(
        job.photos[1].url,
        "https://store.test/orders/ord-1/pho-2.png?op=get&ttl=900"
    );
    assert_eq!(job.photos[1].copies, 3);
}

#[tokio::test]
async fn webhook_rejects_bad_signature() {
    let h = harness();
    let (id, code) = awaiting_payment(&h).await;
    let mut webhook = signed_webhook(TEST_CHECKSUM_KEY, "00", code, 50_000).expect("webhook");
    webhook.signature = "00".repeat(32);
    let err = h
        .service
        .handle_payos_webhook(&webhook)
        .await
        .expect_err("bad sig");
    assert_eq!(
        err,
        CoreError::Validation("invalid webhook signature".into())
    );

    let forged = signed_webhook("wrong-key", "00", code, 50_000).expect("webhook");
    let err = h
        .service
        .handle_payos_webhook(&forged)
        .await
        .expect_err("wrong key");
    assert!(matches!(err, CoreError::Validation(_)));
    assert_eq!(status_of(&h, &id), OrderStatus::AwaitingPayment);
    assert!(h.publisher.jobs().is_empty());
}

#[tokio::test]
async fn webhook_for_unknown_order_code_is_not_found() {
    let h = harness();
    let err = pay(&h, 424_242, 1).await.expect_err("unknown");
    assert!(matches!(err, CoreError::NotFound(_)));
}

#[tokio::test]
async fn webhook_with_failure_code_changes_nothing() {
    let h = harness();
    let (id, code) = awaiting_payment(&h).await;
    let webhook = signed_webhook(TEST_CHECKSUM_KEY, "01", code, 50_000).expect("webhook");
    let outcome = h
        .service
        .handle_payos_webhook(&webhook)
        .await
        .expect("webhook");
    assert_eq!(outcome, WebhookOutcome::NotPaid);
    assert_eq!(status_of(&h, &id), OrderStatus::AwaitingPayment);
    assert!(h.publisher.jobs().is_empty());
}

#[tokio::test]
async fn webhook_amount_mismatch_is_validation_and_changes_nothing() {
    let h = harness();
    let (id, code) = awaiting_payment(&h).await;
    for amount in [49_999, 50_001, 0] {
        let err = pay(&h, code, amount).await.expect_err("mismatch");
        assert!(matches!(err, CoreError::Validation(_)), "{amount}");
    }
    assert_eq!(status_of(&h, &id), OrderStatus::AwaitingPayment);
    assert!(h.publisher.jobs().is_empty());
}

#[tokio::test]
async fn duplicate_webhook_while_paid_succeeds_and_republishes_the_same_job() {
    let h = harness();
    let (id, code) = awaiting_payment(&h).await;
    pay(&h, code, 50_000).await.expect("first");
    let outcome = pay(&h, code, 50_000).await.expect("duplicate");
    assert_eq!(outcome, WebhookOutcome::Duplicate);
    assert_eq!(status_of(&h, &id), OrderStatus::Paid);
    let jobs = h.publisher.jobs();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].job_id, jobs[1].job_id);
}

#[tokio::test]
async fn duplicate_webhook_after_printing_is_a_pure_no_op() {
    for state in [JobState::Printing, JobState::Completed, JobState::Failed] {
        let h = harness();
        let (id, code) = awaiting_payment(&h).await;
        pay(&h, code, 50_000).await.expect("first");
        h.service
            .handle_job_status(&job_status(&id, JobState::Received, None))
            .await
            .expect("received");
        if state != JobState::Printing {
            h.service
                .handle_job_status(&job_status(&id, state, None))
                .await
                .expect("final");
        }
        let before = h.orders.snapshot(&id).expect("stored");
        let outcome = pay(&h, code, 50_000).await.expect("duplicate");
        assert_eq!(outcome, WebhookOutcome::Duplicate, "{state:?}");
        assert_eq!(h.orders.snapshot(&id), Some(before));
        assert_eq!(h.publisher.jobs().len(), 1, "{state:?}");
    }
}

#[tokio::test]
async fn webhook_for_cancelled_order_conflicts() {
    let h = harness();
    let (id, code) = awaiting_payment(&h).await;
    h.service.cancel(OWNER, &id).await.expect("cancel");
    let err = pay(&h, code, 50_000).await.expect_err("cancelled");
    assert!(matches!(err, CoreError::Conflict(_)));
    assert_eq!(status_of(&h, &id), OrderStatus::Cancelled);
    assert!(h.publisher.jobs().is_empty());
}

#[tokio::test]
async fn webhook_for_expired_order_conflicts() {
    let h = harness();
    let (id, code) = awaiting_payment(&h).await;
    h.clock.advance(ChronoDuration::minutes(16));
    assert_eq!(h.service.expire_due().await, Ok(1));
    let err = pay(&h, code, 50_000).await.expect_err("expired");
    assert!(matches!(err, CoreError::Conflict(_)));
    assert_eq!(status_of(&h, &id), OrderStatus::Expired);
}

#[tokio::test]
async fn webhook_publish_failure_is_healed_by_the_retry() {
    let h = harness();
    let (id, code) = awaiting_payment(&h).await;
    h.publisher
        .fail_with(Some(CoreError::Internal("broker down".into())));
    let err = pay(&h, code, 50_000).await.expect_err("publish fails");
    assert_eq!(err, CoreError::Internal("broker down".into()));
    assert_eq!(status_of(&h, &id), OrderStatus::Paid);
    assert!(h.publisher.jobs().is_empty());

    h.publisher.fail_with(None);
    assert_eq!(pay(&h, code, 50_000).await, Ok(WebhookOutcome::Duplicate));
    assert_eq!(h.publisher.jobs().len(), 1);
}

#[tokio::test]
async fn webhook_race_surfaces_a_conflict() {
    let h = harness();
    let (_, code) = awaiting_payment(&h).await;
    let repo = FaultyRepo::new(h.orders.clone());
    *repo.flip_to.lock().expect("lock") = Some(OrderStatus::Cancelled);
    let webhook = signed_webhook(TEST_CHECKSUM_KEY, "00", code, 50_000).expect("webhook");
    let err = service_over(&h, &repo)
        .handle_payos_webhook(&webhook)
        .await
        .expect_err("race");
    assert!(matches!(err, CoreError::Conflict(_)));
    assert!(h.publisher.jobs().is_empty());
}

// ---- job status ----

#[tokio::test]
async fn received_and_printing_move_paid_to_printing() {
    for state in [JobState::Received, JobState::Printing] {
        let h = harness();
        let id = paid(&h).await;
        let outcome = h
            .service
            .handle_job_status(&job_status(&id, state, None))
            .await;
        assert_eq!(outcome, Ok(StatusOutcome::Applied), "{state:?}");
        assert_eq!(status_of(&h, &id), OrderStatus::Printing);
    }
}

#[tokio::test]
async fn repeated_printing_reports_are_ignored() {
    let h = harness();
    let id = paid(&h).await;
    h.service
        .handle_job_status(&job_status(&id, JobState::Received, None))
        .await
        .expect("first");
    for state in [JobState::Received, JobState::Printing] {
        let outcome = h
            .service
            .handle_job_status(&job_status(&id, state, None))
            .await;
        assert_eq!(outcome, Ok(StatusOutcome::Ignored), "{state:?}");
    }
    assert_eq!(status_of(&h, &id), OrderStatus::Printing);
}

#[tokio::test]
async fn completed_finishes_a_printing_order() {
    let h = harness();
    let id = paid(&h).await;
    h.service
        .handle_job_status(&job_status(&id, JobState::Printing, None))
        .await
        .expect("start");
    let outcome = h
        .service
        .handle_job_status(&job_status(&id, JobState::Completed, None))
        .await;
    assert_eq!(outcome, Ok(StatusOutcome::Applied));
    assert_eq!(status_of(&h, &id), OrderStatus::Completed);
}

#[tokio::test]
async fn completed_without_a_received_report_still_completes() {
    let h = harness();
    let id = paid(&h).await;
    let outcome = h
        .service
        .handle_job_status(&job_status(&id, JobState::Completed, None))
        .await;
    assert_eq!(outcome, Ok(StatusOutcome::Applied));
    assert_eq!(status_of(&h, &id), OrderStatus::Completed);
}

#[tokio::test]
async fn failed_records_the_pi_error() {
    let cases = [
        (PiErrorCode::PaperOut, "paper_out"),
        (PiErrorCode::RibbonOut, "ribbon_out"),
        (PiErrorCode::PrinterOffline, "printer_offline"),
        (PiErrorCode::DownloadFailed, "download_failed"),
        (PiErrorCode::InvalidImage, "invalid_image"),
        (PiErrorCode::DriverError, "driver_error"),
    ];
    for (code, wire) in cases {
        for start in [false, true] {
            let h = harness();
            let id = paid(&h).await;
            if start {
                h.service
                    .handle_job_status(&job_status(&id, JobState::Printing, None))
                    .await
                    .expect("start");
            }
            let error = PiError {
                code,
                message: "out of paper".into(),
            };
            let outcome = h
                .service
                .handle_job_status(&job_status(&id, JobState::Failed, Some(error)))
                .await;
            assert_eq!(outcome, Ok(StatusOutcome::Applied));
            let stored = h.orders.snapshot(&id).expect("stored");
            assert_eq!(stored.status, OrderStatus::Failed);
            let error = stored.error.expect("error recorded");
            assert_eq!(
                (error.code.as_str(), error.message.as_str()),
                (wire, "out of paper")
            );
        }
    }
}

#[tokio::test]
async fn failed_without_error_gets_a_default() {
    let h = harness();
    let id = paid(&h).await;
    h.service
        .handle_job_status(&job_status(&id, JobState::Failed, None))
        .await
        .expect("failed");
    let error = h
        .orders
        .snapshot(&id)
        .expect("stored")
        .error
        .expect("error");
    assert_eq!(error.code, "print_failed");
}

#[tokio::test]
async fn stale_and_unknown_reports_are_ignored() {
    let h = harness();
    // Unknown order.
    let outcome = h
        .service
        .handle_job_status(&job_status(&"ghost".into(), JobState::Printing, None))
        .await;
    assert_eq!(outcome, Ok(StatusOutcome::Ignored));

    // Order that is not paid yet.
    let (waiting, _) = awaiting_payment(&h).await;
    for state in [
        JobState::Received,
        JobState::Printing,
        JobState::Completed,
        JobState::Failed,
    ] {
        let outcome = h
            .service
            .handle_job_status(&job_status(&waiting, state, None))
            .await;
        assert_eq!(outcome, Ok(StatusOutcome::Ignored), "{state:?}");
    }
    assert_eq!(status_of(&h, &waiting), OrderStatus::AwaitingPayment);

    // Terminal orders stay put.
    let done = paid(&h).await;
    h.service
        .handle_job_status(&job_status(&done, JobState::Completed, None))
        .await
        .expect("done");
    for state in [
        JobState::Received,
        JobState::Printing,
        JobState::Completed,
        JobState::Failed,
    ] {
        let outcome = h
            .service
            .handle_job_status(&job_status(&done, state, None))
            .await;
        assert_eq!(outcome, Ok(StatusOutcome::Ignored), "{state:?}");
    }
    assert_eq!(status_of(&h, &done), OrderStatus::Completed);
}

#[tokio::test]
async fn status_from_another_printer_is_forbidden() {
    let h = harness();
    let id = paid(&h).await;
    let mut report = job_status(&id, JobState::Printing, None);
    report.printer_id = PrinterId::new("stand-999").expect("id");
    let err = h
        .service
        .handle_job_status(&report)
        .await
        .expect_err("mismatch");
    assert!(matches!(err, CoreError::Forbidden(_)));
    assert_eq!(status_of(&h, &id), OrderStatus::Paid);
}

// ---- heartbeat ----

#[tokio::test]
async fn heartbeat_maps_state_and_stamps_last_seen() {
    let cases = [
        (PrinterState::Idle, PrinterStatus::Online),
        (PrinterState::Printing, PrinterStatus::Busy),
        (PrinterState::Error, PrinterStatus::Error),
        (PrinterState::Offline, PrinterStatus::Offline),
    ];
    for (state, expected) in cases {
        let h = harness();
        h.clock.advance(ChronoDuration::seconds(30));
        let beat = Heartbeat {
            printer_id: pid(),
            printer_state: state,
            agent_version: "0.1.0".into(),
            at: ts("2026-01-01T00:00:00Z"),
        };
        h.service.handle_heartbeat(&beat).await.expect("heartbeat");
        let printer = h.printers.snapshot(&pid()).expect("printer");
        assert_eq!(printer.status, expected);
        assert_eq!(printer.last_seen_at, Some(ts("2026-01-01T00:00:30Z")));
    }
}

#[tokio::test]
async fn heartbeat_from_unknown_printer_is_not_found() {
    let h = Harness::new();
    let beat = Heartbeat {
        printer_id: pid(),
        printer_state: PrinterState::Idle,
        agent_version: "0.1.0".into(),
        at: ts("2026-01-01T00:00:00Z"),
    };
    let err = h
        .service
        .handle_heartbeat(&beat)
        .await
        .expect_err("unknown");
    assert!(matches!(err, CoreError::NotFound(_)));
}

// ---- expire_due ----

#[tokio::test]
async fn expire_due_expires_only_overdue_unpaid_orders() {
    let h = harness();
    let first = uploaded(&h).await;
    let (second, _) = awaiting_payment(&h).await;
    let paid_id = paid(&h).await;
    h.clock.advance(ChronoDuration::minutes(10));
    let fresh = uploaded(&h).await;

    assert_eq!(h.service.expire_due().await, Ok(0));

    h.clock.advance(ChronoDuration::minutes(6));
    assert_eq!(h.service.expire_due().await, Ok(2));
    assert_eq!(status_of(&h, &first), OrderStatus::Expired);
    assert_eq!(status_of(&h, &second), OrderStatus::Expired);
    assert_eq!(status_of(&h, &paid_id), OrderStatus::Paid);
    assert_eq!(status_of(&h, &fresh), OrderStatus::AwaitingUpload);

    assert_eq!(h.service.expire_due().await, Ok(0));
}

#[tokio::test]
async fn expire_due_skips_orders_that_changed_concurrently() {
    let h = harness();
    uploaded(&h).await;
    h.clock.advance(ChronoDuration::minutes(16));
    let repo = FaultyRepo::new(h.orders.clone());
    *repo.flip_to.lock().expect("lock") = Some(OrderStatus::Cancelled);
    assert_eq!(service_over(&h, &repo).expire_due().await, Ok(0));
}

#[tokio::test]
async fn expire_due_propagates_storage_errors() {
    let h = harness();
    uploaded(&h).await;
    h.clock.advance(ChronoDuration::minutes(16));
    let repo = FaultyRepo::new(h.orders.clone());
    *repo.update_error.lock().expect("lock") = Some(CoreError::Internal("db".into()));
    let err = service_over(&h, &repo).expire_due().await.expect_err("db");
    assert_eq!(err, CoreError::Internal("db".into()));
}

// ---- plumbing ----

#[test]
fn config_defaults_and_debug_redaction() {
    let config = ServiceConfig::new("https://app.test", "super-secret");
    assert_eq!(config.order_ttl.as_secs(), 900);
    assert_eq!(config.download_ttl.as_secs(), 900);
    let shown = format!("{config:?}");
    assert!(!shown.contains("super-secret"));
    assert!(shown.contains("<redacted>"));
    assert_eq!(format!("{:?}", harness().service.ports), "Ports { .. }");
}
