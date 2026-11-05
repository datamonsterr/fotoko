//! Adapter tests against DynamoDB Local and MinIO (`docker-compose.test.yml`).
//!
//! Ignored by default; run with
//! `docker compose -f docker-compose.test.yml up -d && FOTOKO_IT=1 cargo test -p fotoko-api -- --include-ignored`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

use std::sync::Arc;
use std::time::Duration;

use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, AttributeValue, BillingMode, GlobalSecondaryIndex, KeySchemaElement,
    KeyType, Projection, ProjectionType, ScalarAttributeType,
};
use chrono::{DateTime, Utc};
use fotoko_api::aws::dynamo::{DynamoOrderRepo, DynamoPrinterRepo};
use fotoko_api::aws::s3::S3ObjectStore;
use fotoko_core::testing::{
    FakePaymentGateway, ManualClock, RecordingJobPublisher, SequentialIdGen,
};
use fotoko_core::{
    CoreError, ObjectStore, Order, OrderRepo, OrderService, Photo, Ports, PrinterRepo, PutSpec,
    ServiceConfig,
};
use fotoko_protocol::{ContentType, OrderStatus, PrinterId, PrinterStatus};

/// The tests are `#[ignore]`d; running them (`--include-ignored`) without `FOTOKO_IT=1` is a
/// mistake and fails loudly instead of passing without testing anything.
fn enabled() -> bool {
    assert!(
        std::env::var("FOTOKO_IT").is_ok_and(|v| v == "1"),
        "these tests need FOTOKO_IT=1 and `docker compose -f docker-compose.test.yml up -d`"
    );
    true
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

fn dynamo_url() -> String {
    std::env::var("FOTOKO_IT_DYNAMODB_URL").unwrap_or_else(|_| "http://127.0.0.1:18000".into())
}

fn s3_url() -> String {
    std::env::var("FOTOKO_IT_S3_URL").unwrap_or_else(|_| "http://127.0.0.1:19000".into())
}

fn dynamo_client() -> aws_sdk_dynamodb::Client {
    aws_sdk_dynamodb::Client::from_conf(
        aws_sdk_dynamodb::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("ap-southeast-1"))
            .credentials_provider(Credentials::new("local", "local", None, None, "it"))
            .endpoint_url(dynamo_url())
            .build(),
    )
}

fn key_element(name: &str, kind: KeyType) -> KeySchemaElement {
    KeySchemaElement::builder()
        .attribute_name(name)
        .key_type(kind)
        .build()
        .unwrap()
}

fn attr(name: &str) -> AttributeDefinition {
    AttributeDefinition::builder()
        .attribute_name(name)
        .attribute_type(ScalarAttributeType::S)
        .build()
        .unwrap()
}

fn index(name: &str, keys: Vec<KeySchemaElement>) -> GlobalSecondaryIndex {
    GlobalSecondaryIndex::builder()
        .index_name(name)
        .set_key_schema(Some(keys))
        .projection(
            Projection::builder()
                .projection_type(ProjectionType::All)
                .build(),
        )
        .build()
        .unwrap()
}

/// A fresh table with the layout the infra stack creates (pk/sk, gsi1, gsi2).
async fn table() -> (aws_sdk_dynamodb::Client, String) {
    let client = dynamo_client();
    let name = format!("fotoko-it-{}", uuid::Uuid::new_v4().simple());
    client
        .create_table()
        .table_name(&name)
        .billing_mode(BillingMode::PayPerRequest)
        .set_key_schema(Some(vec![
            key_element("pk", KeyType::Hash),
            key_element("sk", KeyType::Range),
        ]))
        .set_attribute_definitions(Some(vec![
            attr("pk"),
            attr("sk"),
            attr("gsi1pk"),
            attr("gsi2pk"),
            attr("gsi2sk"),
        ]))
        .global_secondary_indexes(index("gsi1", vec![key_element("gsi1pk", KeyType::Hash)]))
        .global_secondary_indexes(index(
            "gsi2",
            vec![
                key_element("gsi2pk", KeyType::Hash),
                key_element("gsi2sk", KeyType::Range),
            ],
        ))
        .send()
        .await
        .unwrap();
    (client, name)
}

fn order(id: &str, status: OrderStatus, code: Option<i64>, expires: &str) -> Order {
    Order {
        order_id: id.into(),
        owner: "guest".into(),
        printer_id: PrinterId::new("stand-001").unwrap(),
        status,
        amount_vnd: 20_000,
        order_code: code,
        checkout_url: code.map(|c| format!("https://pay.test/{c}")),
        qr_code: code.map(|c| format!("qr-{c}")),
        photos: vec![Photo {
            photo_id: "p1".into(),
            content_type: ContentType::Jpeg,
            size_bytes: 10,
            copies: 2,
            uploaded: false,
        }],
        created_at: at("2026-01-01T00:00:00Z"),
        updated_at: at("2026-01-01T00:00:00Z"),
        expires_at: at(expires),
        error: None,
    }
}

async fn repo() -> (DynamoOrderRepo, aws_sdk_dynamodb::Client, String) {
    let (client, name) = table().await;
    (DynamoOrderRepo::new(client.clone(), &name), client, name)
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn insert_get_and_duplicate_ids() {
    if !enabled() {
        return;
    }
    let (repo, _, _) = repo().await;
    let o = order(
        "o1",
        OrderStatus::AwaitingUpload,
        None,
        "2026-01-01T00:15:00Z",
    );
    repo.insert(&o).await.unwrap();
    assert_eq!(repo.get(&"o1".into()).await.unwrap(), Some(o.clone()));
    assert_eq!(repo.get(&"nope".into()).await.unwrap(), None);
    assert!(matches!(repo.insert(&o).await, Err(CoreError::Conflict(_))));
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn order_codes_are_unique_across_orders() {
    if !enabled() {
        return;
    }
    let (repo, _, _) = repo().await;
    let first = order(
        "o1",
        OrderStatus::AwaitingPayment,
        Some(42),
        "2026-01-01T00:15:00Z",
    );
    repo.insert(&first).await.unwrap();
    assert_eq!(
        repo.get_by_order_code(42).await.unwrap(),
        Some(first.clone())
    );
    assert_eq!(repo.get_by_order_code(43).await.unwrap(), None);

    // Insert with a taken code: Conflict, and the loser is not stored.
    let clash = order(
        "o2",
        OrderStatus::AwaitingPayment,
        Some(42),
        "2026-01-01T00:15:00Z",
    );
    assert!(matches!(
        repo.insert(&clash).await,
        Err(CoreError::Conflict(_))
    ));
    assert_eq!(repo.get(&"o2".into()).await.unwrap(), None);

    // Update that sets a taken code: Conflict, order unchanged.
    let mut other = order(
        "o3",
        OrderStatus::AwaitingUpload,
        None,
        "2026-01-01T00:15:00Z",
    );
    repo.insert(&other).await.unwrap();
    other.status = OrderStatus::AwaitingPayment;
    other.order_code = Some(42);
    let err = repo
        .update(&other, OrderStatus::AwaitingUpload)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Conflict(m) if m.contains("order code 42")));
    let stored = repo.get(&"o3".into()).await.unwrap().unwrap();
    assert_eq!(stored.status, OrderStatus::AwaitingUpload);
    assert_eq!(stored.order_code, None);
    assert_eq!(
        repo.get_by_order_code(42)
            .await
            .unwrap()
            .unwrap()
            .order_id
            .as_str(),
        "o1"
    );

    // The same order may keep updating while it holds its code.
    let mut paid = first.clone();
    paid.status = OrderStatus::Paid;
    repo.update(&paid, OrderStatus::AwaitingPayment)
        .await
        .unwrap();
    let mut printing = paid.clone();
    printing.status = OrderStatus::Printing;
    repo.update(&printing, OrderStatus::Paid).await.unwrap();
    assert_eq!(
        repo.get_by_order_code(42).await.unwrap().unwrap().status,
        OrderStatus::Printing
    );
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn updates_are_conditional_on_the_expected_status() {
    if !enabled() {
        return;
    }
    let (repo, _, _) = repo().await;
    let o = order(
        "o1",
        OrderStatus::AwaitingUpload,
        None,
        "2026-01-01T00:15:00Z",
    );
    repo.insert(&o).await.unwrap();

    let mut cancelled = o.clone();
    cancelled.status = OrderStatus::Cancelled;
    // Wrong expectation: Conflict, nothing written.
    assert!(matches!(
        repo.update(&cancelled, OrderStatus::AwaitingPayment).await,
        Err(CoreError::Conflict(_))
    ));
    assert_eq!(
        repo.get(&"o1".into()).await.unwrap().unwrap().status,
        OrderStatus::AwaitingUpload
    );
    // Right expectation: written.
    repo.update(&cancelled, OrderStatus::AwaitingUpload)
        .await
        .unwrap();
    assert_eq!(
        repo.get(&"o1".into()).await.unwrap().unwrap().status,
        OrderStatus::Cancelled
    );
    // The same stale write again loses.
    assert!(matches!(
        repo.update(&cancelled, OrderStatus::AwaitingUpload).await,
        Err(CoreError::Conflict(_))
    ));
    // Missing order, with and without a code.
    let ghost = order(
        "ghost",
        OrderStatus::Cancelled,
        None,
        "2026-01-01T00:15:00Z",
    );
    assert!(matches!(
        repo.update(&ghost, OrderStatus::AwaitingUpload).await,
        Err(CoreError::NotFound(_))
    ));
    let ghost = order(
        "ghost",
        OrderStatus::AwaitingPayment,
        Some(7),
        "2026-01-01T00:15:00Z",
    );
    assert!(matches!(
        repo.update(&ghost, OrderStatus::AwaitingUpload).await,
        Err(CoreError::NotFound(_))
    ));
    assert_eq!(
        repo.get_by_order_code(7).await.unwrap(),
        None,
        "no stray reservation"
    );
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn racing_writers_have_exactly_one_winner() {
    if !enabled() {
        return;
    }
    let (repo, _, _) = repo().await;
    let o = order(
        "o1",
        OrderStatus::AwaitingUpload,
        None,
        "2026-01-01T00:15:00Z",
    );
    repo.insert(&o).await.unwrap();
    let repo = Arc::new(repo);

    // Same order, same expected status.
    let mut handles = Vec::new();
    for i in 0..8_i64 {
        let repo = Arc::clone(&repo);
        let mut next = o.clone();
        next.status = OrderStatus::AwaitingPayment;
        next.order_code = Some(1000 + i);
        handles.push(tokio::spawn(async move {
            repo.update(&next, OrderStatus::AwaitingUpload).await
        }));
    }
    let mut wins = 0;
    for h in handles {
        match h.await.unwrap() {
            Ok(()) => wins += 1,
            Err(CoreError::Conflict(_)) => {}
            Err(other) => panic!("unexpected error {other:?}"),
        }
    }
    assert_eq!(wins, 1);

    // Different orders racing for one code.
    let mut handles = Vec::new();
    for i in 0..6 {
        let repo = Arc::clone(&repo);
        let id = format!("r{i}");
        let base = order(
            &id,
            OrderStatus::AwaitingUpload,
            None,
            "2026-01-01T00:15:00Z",
        );
        repo.insert(&base).await.unwrap();
        let mut next = base;
        next.status = OrderStatus::AwaitingPayment;
        next.order_code = Some(555);
        handles.push(tokio::spawn(async move {
            repo.update(&next, OrderStatus::AwaitingUpload).await
        }));
    }
    let mut wins = 0;
    for h in handles {
        if h.await.unwrap().is_ok() {
            wins += 1;
        }
    }
    assert_eq!(wins, 1);
    assert!(repo.get_by_order_code(555).await.unwrap().is_some());
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn due_orders_come_from_the_gsi_in_expiry_order() {
    if !enabled() {
        return;
    }
    let (repo, _, _) = repo().await;
    let now = at("2026-01-01T01:00:00Z");
    for o in [
        order(
            "late",
            OrderStatus::AwaitingUpload,
            None,
            "2026-01-01T00:30:00Z",
        ),
        order(
            "early",
            OrderStatus::AwaitingPayment,
            Some(1),
            "2026-01-01T00:10:00Z",
        ),
        order(
            "future",
            OrderStatus::AwaitingUpload,
            None,
            "2026-01-01T02:00:00Z",
        ),
        order("paid", OrderStatus::Paid, Some(2), "2026-01-01T00:05:00Z"),
        order(
            "exactly-now",
            OrderStatus::AwaitingUpload,
            None,
            "2026-01-01T01:00:00Z",
        ),
        order("done", OrderStatus::Expired, None, "2026-01-01T00:01:00Z"),
    ] {
        repo.insert(&o).await.unwrap();
    }
    let ids = |orders: Vec<Order>| {
        orders
            .into_iter()
            .map(|o| o.order_id.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(repo.list_due_for_expiry(now, 10).await.unwrap()),
        ["early", "late"]
    );
    assert_eq!(
        ids(repo.list_due_for_expiry(now, 1).await.unwrap()),
        ["early"]
    );
    assert!(
        repo.list_due_for_expiry(at("2026-01-01T00:00:00Z"), 10)
            .await
            .unwrap()
            .is_empty()
    );

    // Leaving the awaiting states removes an order from the index.
    let mut cancelled = repo.get(&"early".into()).await.unwrap().unwrap();
    cancelled.status = OrderStatus::Cancelled;
    repo.update(&cancelled, OrderStatus::AwaitingPayment)
        .await
        .unwrap();
    assert_eq!(
        ids(repo.list_due_for_expiry(now, 10).await.unwrap()),
        ["late"]
    );
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn service_expires_due_orders_on_dynamodb() {
    if !enabled() {
        return;
    }
    let (client, name) = table().await;
    let orders = Arc::new(DynamoOrderRepo::new(client.clone(), &name));
    let clock = ManualClock::new(at("2026-01-01T01:00:00Z"));
    let service = OrderService::new(
        Ports {
            orders: orders.clone(),
            printers: Arc::new(DynamoPrinterRepo::new(client, &name, 10_000)),
            store: Arc::new(fotoko_core::testing::InMemoryObjectStore::new()),
            publisher: Arc::new(RecordingJobPublisher::new()),
            payments: Arc::new(FakePaymentGateway::new()),
            clock: Arc::new(clock),
            ids: Arc::new(SequentialIdGen::new()),
        },
        ServiceConfig::new("https://app.test", "key").unwrap(),
    );
    for i in 0..5 {
        orders
            .insert(&order(
                &format!("o{i}"),
                OrderStatus::AwaitingUpload,
                None,
                "2026-01-01T00:10:00Z",
            ))
            .await
            .unwrap();
    }
    orders
        .insert(&order(
            "fresh",
            OrderStatus::AwaitingUpload,
            None,
            "2026-01-01T03:00:00Z",
        ))
        .await
        .unwrap();
    assert_eq!(service.expire_due().await.unwrap(), 5);
    assert_eq!(
        orders.get(&"o3".into()).await.unwrap().unwrap().status,
        OrderStatus::Expired
    );
    assert_eq!(
        orders.get(&"fresh".into()).await.unwrap().unwrap().status,
        OrderStatus::AwaitingUpload
    );
    assert_eq!(service.expire_due().await.unwrap(), 0);
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn printers_round_trip() {
    if !enabled() {
        return;
    }
    let (client, name) = table().await;
    let repo = DynamoPrinterRepo::new(client.clone(), &name, 10_000);
    let stand = PrinterId::new("stand-001").unwrap();
    assert_eq!(repo.get(&stand).await.unwrap(), None);

    client
        .put_item()
        .table_name(&name)
        .item("pk", AttributeValue::S("PRINTER#stand-001".into()))
        .item("sk", AttributeValue::S("META".into()))
        .item("name", AttributeValue::S("Stand 1".into()))
        .item("status", AttributeValue::S("offline".into()))
        .send()
        .await
        .unwrap();
    let printer = repo.get(&stand).await.unwrap().unwrap();
    assert_eq!(printer.name, "Stand 1");
    assert_eq!(printer.price_per_print_vnd, 10_000, "default price");
    assert_eq!(printer.status, PrinterStatus::Offline);
    assert_eq!(printer.last_seen_at, None);

    let seen = at("2026-01-01T00:00:05.250Z");
    repo.update_status(&stand, PrinterStatus::Online, seen)
        .await
        .unwrap();
    let printer = repo.get(&stand).await.unwrap().unwrap();
    assert_eq!(
        (printer.status, printer.last_seen_at),
        (PrinterStatus::Online, Some(seen))
    );
    assert_eq!(printer.name, "Stand 1", "update keeps other attributes");

    let ghost = PrinterId::new("ghost").unwrap();
    assert!(matches!(
        repo.update_status(&ghost, PrinterStatus::Online, seen)
            .await,
        Err(CoreError::NotFound(_))
    ));
    assert_eq!(
        repo.get(&ghost).await.unwrap(),
        None,
        "update must not create the item"
    );
}

fn s3_client() -> aws_sdk_s3::Client {
    aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("ap-southeast-1"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "fotoko",
                "fotoko-secret",
                None,
                None,
                "it",
            ))
            .endpoint_url(s3_url())
            .force_path_style(true)
            .build(),
    )
}

async fn bucket() -> S3ObjectStore {
    let client = s3_client();
    let name = format!("fotoko-it-{}", uuid::Uuid::new_v4().simple());
    client.create_bucket().bucket(&name).send().await.unwrap();
    S3ObjectStore::new(client, name)
}

fn spec(size: u64) -> PutSpec {
    PutSpec {
        key: "orders/o1/p1.jpg".into(),
        content_type: ContentType::Jpeg,
        size_bytes: size,
        ttl: Duration::from_secs(300),
    }
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn presigned_put_then_exists_then_get() {
    if !enabled() {
        return;
    }
    let store = bucket().await;
    let http = reqwest::Client::new();
    assert!(!store.exists("orders/o1/p1.jpg").await.unwrap());

    let put = store.presign_put(&spec(11)).await.unwrap();
    let mut request = http.put(&put.url).body("hello world");
    for (name, value) in &put.headers {
        request = request.header(name, value);
    }
    let response = request.send().await.unwrap();
    assert_eq!(
        response.status().as_u16(),
        200,
        "{}",
        response.text().await.unwrap()
    );
    assert!(store.exists("orders/o1/p1.jpg").await.unwrap());
    assert!(!store.exists("orders/o1/other.jpg").await.unwrap());

    let get = store
        .presign_get("orders/o1/p1.jpg", Duration::from_secs(300))
        .await
        .unwrap();
    let body = http.get(&get).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(body.as_ref(), b"hello world");
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn presigned_put_enforces_type_and_size() {
    if !enabled() {
        return;
    }
    let store = bucket().await;
    let http = reqwest::Client::new();
    let put = store.presign_put(&spec(11)).await.unwrap();

    let wrong_type = http
        .put(&put.url)
        .header("content-type", "image/png")
        .body("hello world")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_type.status().as_u16(), 403);
    let too_big = http
        .put(&put.url)
        .header("content-type", "image/jpeg")
        .body("hello world!!")
        .send()
        .await
        .unwrap();
    assert_eq!(too_big.status().as_u16(), 403);
    assert!(!store.exists("orders/o1/p1.jpg").await.unwrap());
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn storage_failures_are_internal_errors_without_leaks() {
    if !enabled() {
        return;
    }
    // A table that does not exist makes every call fail at the service.
    let client = dynamo_client();
    let orders = DynamoOrderRepo::new(client.clone(), "fotoko-it-missing-table");
    let printers = DynamoPrinterRepo::new(client, "fotoko-it-missing-table", 1);
    let plain = order(
        "o1",
        OrderStatus::AwaitingUpload,
        None,
        "2026-01-01T00:15:00Z",
    );
    let coded = order(
        "o2",
        OrderStatus::AwaitingPayment,
        Some(9),
        "2026-01-01T00:15:00Z",
    );
    let is_internal = |r: Result<(), CoreError>| matches!(r, Err(CoreError::Internal(m)) if m.starts_with("storage error"));
    assert!(is_internal(orders.get(&"o1".into()).await.map(|_| ())));
    assert!(is_internal(orders.get_by_order_code(9).await.map(|_| ())));
    assert!(is_internal(orders.insert(&plain).await));
    assert!(is_internal(orders.insert(&coded).await));
    assert!(is_internal(
        orders.update(&plain, OrderStatus::AwaitingUpload).await
    ));
    assert!(is_internal(
        orders.update(&coded, OrderStatus::AwaitingUpload).await
    ));
    assert!(is_internal(
        orders
            .list_due_for_expiry(at("2026-01-02T00:00:00Z"), 5)
            .await
            .map(|_| ())
    ));
    let stand = PrinterId::new("stand-001").unwrap();
    assert!(is_internal(printers.get(&stand).await.map(|_| ())));
    assert!(is_internal(
        printers
            .update_status(&stand, PrinterStatus::Online, at("2026-01-01T00:00:00Z"))
            .await
    ));
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn corrupt_items_are_internal_errors() {
    if !enabled() {
        return;
    }
    let (repo, client, name) = repo().await;
    client
        .put_item()
        .table_name(&name)
        .item("pk", AttributeValue::S("ORDER#bad".into()))
        .item("sk", AttributeValue::S("META".into()))
        .item("status", AttributeValue::S("awaiting_upload".into()))
        .item("gsi2pk", AttributeValue::S("DUE".into()))
        .item(
            "gsi2sk",
            AttributeValue::S("2026-01-01T00:00:00.000000000Z".into()),
        )
        .send()
        .await
        .unwrap();
    assert!(matches!(
        repo.get(&"bad".into()).await,
        Err(CoreError::Internal(_))
    ));
    // The due sweep skips the corrupt item and still returns the good one.
    repo.insert(&order(
        "good",
        OrderStatus::AwaitingUpload,
        None,
        "2026-01-01T00:30:00Z",
    ))
    .await
    .unwrap();
    let due = repo
        .list_due_for_expiry(at("2026-01-02T00:00:00Z"), 5)
        .await
        .unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].order_id.as_str(), "good");
    // A reservation without its order is "not found", not an error.
    client
        .put_item()
        .table_name(&name)
        .item("pk", AttributeValue::S("ORDERCODE#77".into()))
        .item("sk", AttributeValue::S("META".into()))
        .item("orderId", AttributeValue::S("gone".into()))
        .send()
        .await
        .unwrap();
    assert_eq!(repo.get_by_order_code(77).await.unwrap(), None);
}

#[tokio::test]
#[ignore = "needs docker compose; set FOTOKO_IT=1"]
async fn status_updates_after_checkout_do_not_rewrite_the_reservation() {
    if !enabled() {
        return;
    }
    let (repo, client, name) = repo().await;
    let o = order(
        "o1",
        OrderStatus::AwaitingUpload,
        None,
        "2026-01-01T00:15:00Z",
    );
    repo.insert(&o).await.unwrap();
    let mut paying = o.clone();
    paying.status = OrderStatus::AwaitingPayment;
    paying.order_code = Some(31);
    repo.update(&paying, OrderStatus::AwaitingUpload)
        .await
        .unwrap();

    // Mark the reservation; a plain status update must leave it alone.
    client
        .update_item()
        .table_name(&name)
        .key("pk", AttributeValue::S("ORDERCODE#31".into()))
        .key("sk", AttributeValue::S("META".into()))
        .update_expression("SET marker = :m")
        .expression_attribute_values(":m", AttributeValue::S("untouched".into()))
        .send()
        .await
        .unwrap();
    let mut paid = paying.clone();
    paid.status = OrderStatus::Paid;
    repo.update(&paid, OrderStatus::AwaitingPayment)
        .await
        .unwrap();
    let reservation = client
        .get_item()
        .table_name(&name)
        .key("pk", AttributeValue::S("ORDERCODE#31".into()))
        .key("sk", AttributeValue::S("META".into()))
        .send()
        .await
        .unwrap()
        .item
        .unwrap();
    assert_eq!(reservation["marker"].as_s().unwrap(), "untouched");
    assert_eq!(
        repo.get(&"o1".into()).await.unwrap().unwrap().status,
        OrderStatus::Paid
    );
}
