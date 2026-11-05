//! The local server end to end over real HTTP: create an order, upload through the returned
//! URLs, check out against a payOS mock, pay with a signed webhook, read the job back.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{CHECKSUM_KEY, stack};
use fotoko_core::testing::signed_webhook;
use reqwest::Method;
use serde_json::{Value, json};

#[tokio::test]
async fn order_to_paid_job() {
    let s = stack(|b| b).await;

    // Printer is seeded online.
    let (status, printer) = s
        .call(Method::GET, "/v1/printers/stand-001", None, None)
        .await;
    assert_eq!((status, printer["status"].as_str()), (200, Some("online")));

    let created = s.order_with_uploads("guest-1").await;
    let order_id = created["orderId"].as_str().unwrap().to_owned();
    assert_eq!(created["amountVnd"], 30_000);

    // The order sees both uploads.
    let (_, order) = s
        .call(
            Method::GET,
            &format!("/v1/orders/{order_id}"),
            Some("guest-1"),
            None,
        )
        .await;
    assert_eq!(order["status"], "awaiting_upload");
    assert!(
        order["photos"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["uploaded"] == true)
    );
    // Another guest cannot read it.
    let (status, _) = s
        .call(
            Method::GET,
            &format!("/v1/orders/{order_id}"),
            Some("guest-2"),
            None,
        )
        .await;
    assert_eq!(status, 403);

    // Checkout talks to the payOS mock.
    let (status, checkout) = s
        .call(
            Method::POST,
            &format!("/v1/orders/{order_id}/checkout"),
            Some("guest-1"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{checkout}");
    let order_code = checkout["orderCode"].as_i64().unwrap();
    assert_eq!(
        checkout["checkoutUrl"],
        format!("https://pay.test/web/{order_code}")
    );
    let requests = s.payos.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(sent["amount"], 30_000);
    assert_eq!(
        sent["returnUrl"],
        format!("https://app.test/orders/{order_id}")
    );

    // payOS pays: signed webhook.
    let hook = signed_webhook(CHECKSUM_KEY, "00", order_code, 30_000).unwrap();
    let (status, ack) = s
        .call(
            Method::POST,
            "/v1/payments/payos/webhook",
            None,
            Some(serde_json::to_value(hook).unwrap()),
        )
        .await;
    assert_eq!((status, ack), (200, json!({"success": true})));
    let (_, order) = s
        .call(
            Method::GET,
            &format!("/v1/orders/{order_id}"),
            Some("guest-1"),
            None,
        )
        .await;
    assert_eq!(order["status"], "paid");

    // The job carries working download URLs.
    let jobs = s.app.recorded_jobs.jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].order_id.as_str(), order_id);
    assert_eq!(jobs[0].photos.len(), 2);
    let first = s.http.get(&jobs[0].photos[0].url).send().await.unwrap();
    assert_eq!(first.status().as_u16(), 200);
    assert_eq!(first.bytes().await.unwrap().as_ref(), b"jpeg-bytes!");
    let second = s.http.get(&jobs[0].photos[1].url).send().await.unwrap();
    assert_eq!(second.bytes().await.unwrap().as_ref(), b"png-bytes");
}

#[tokio::test]
async fn upload_rules_and_checkout_without_uploads() {
    let s = stack(|b| b).await;
    let (_, created) = s
        .call(
            Method::POST,
            "/v1/orders",
            Some("guest-1"),
            Some(json!({"printerId": "stand-001", "photos": [
                {"contentType": "image/jpeg", "sizeBytes": 4, "copies": 1}]})),
        )
        .await;
    let order_id = created["orderId"].as_str().unwrap();
    let url = created["uploads"][0]["url"].as_str().unwrap();

    // Not uploaded yet.
    let (status, body) = s
        .call(
            Method::POST,
            &format!("/v1/orders/{order_id}/checkout"),
            Some("guest-1"),
            None,
        )
        .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (409, Some("conflict"))
    );
    assert!(s.payos.received_requests().await.unwrap().is_empty());

    // Wrong content type and wrong size are refused like S3 would.
    let wrong_type = s
        .http
        .put(url)
        .header("content-type", "image/png")
        .body("abcd")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_type.status().as_u16(), 403);
    let wrong_size = s
        .http
        .put(url)
        .header("content-type", "image/jpeg")
        .body("abcde")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_size.status().as_u16(), 403);

    // The right upload works, then checkout succeeds.
    let ok = s
        .http
        .put(url)
        .header("content-type", "image/jpeg")
        .body("abcd")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status().as_u16(), 200);
    let (status, _) = s
        .call(
            Method::POST,
            &format!("/v1/orders/{order_id}/checkout"),
            Some("guest-1"),
            None,
        )
        .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn missing_identity_is_forbidden_and_errors_are_json() {
    let s = stack(|b| b).await;
    let (status, body) = s
        .call(Method::POST, "/v1/orders", None, Some(json!({})))
        .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (403, Some("forbidden"))
    );
    let (status, body) = s.call(Method::GET, "/healthz", None, None).await;
    assert_eq!((status, body), (200, json!({"ok": true})));
}

#[tokio::test]
async fn overdue_orders_are_expired_by_the_sweep() {
    let s = stack(|b| b.expire_every(Some(std::time::Duration::from_millis(50)))).await;
    let created = s.order_with_uploads("guest-1").await;
    let order_id = fotoko_protocol::OrderId::from(created["orderId"].as_str().unwrap());
    // Backdate the order so the sweep finds it overdue.
    let mut order = s.app.orders.snapshot(&order_id).unwrap();
    let status = order.status;
    order.expires_at = order.created_at - chrono::Duration::minutes(1);
    use fotoko_core::OrderRepo;
    s.app.orders.update(&order, status).await.unwrap();
    common::eventually("the sweep to expire the order", || async {
        s.app.orders.snapshot(&order_id).unwrap().status == fotoko_protocol::OrderStatus::Expired
    })
    .await;
}

#[tokio::test]
async fn from_config_seeds_printers_and_accepts_a_custom_publisher() {
    use std::sync::Arc;

    use fotoko_api::config::LocalConfig;
    use fotoko_api::local::LocalAppBuilder;
    use fotoko_core::SystemClock;
    use fotoko_core::testing::RecordingJobPublisher;
    use fotoko_protocol::{PrinterId, PrinterStatus};

    let dir = tempfile::tempdir().unwrap();
    let dir_text = dir.path().display().to_string();
    let lookup = |name: &str| {
        Some(
            match name {
                "PAYOS_CLIENT_ID" => "c",
                "PAYOS_API_KEY" => "k",
                "PAYOS_CHECKSUM_KEY" => "s",
                "FOTOKO_LOCAL_PRINTERS" => "a-1, b-2",
                "FOTOKO_LOCAL_DIR" => dir_text.as_str(),
                "DEFAULT_PRICE_VND" => "12000",
                _ => return None,
            }
            .to_owned(),
        )
    };
    let config = LocalConfig::from_lookup(&lookup).unwrap();
    let publisher = RecordingJobPublisher::new();
    let app = LocalAppBuilder::from_config(&config)
        .unwrap()
        .publisher(Arc::new(publisher.clone()))
        .clock(Arc::new(SystemClock))
        .build()
        .await
        .unwrap();
    for id in ["a-1", "b-2"] {
        let printer = app.printers.snapshot(&PrinterId::new(id).unwrap()).unwrap();
        assert_eq!(
            (printer.status, printer.price_per_print_vnd),
            (PrinterStatus::Online, 12_000)
        );
    }
    assert!(
        app.printers
            .snapshot(&PrinterId::new("stand-001").unwrap())
            .is_none()
    );

    // An id the protocol rejects fails the build before anything runs.
    let bad = |name: &str| {
        if name == "FOTOKO_LOCAL_PRINTERS" {
            Some("a/b".to_owned())
        } else {
            lookup(name)
        }
    };
    let config = LocalConfig::from_lookup(&bad).unwrap();
    assert!(LocalAppBuilder::from_config(&config).is_err());
}

#[tokio::test]
async fn default_app_seeds_stand_001_and_rejects_a_bad_mqtt_url() {
    use fotoko_api::local::LocalApp;
    let app = LocalApp::builder()
        .expire_every(None)
        .build()
        .await
        .unwrap();
    let printer = app
        .printers
        .snapshot(&fotoko_protocol::PrinterId::new("stand-001").unwrap())
        .unwrap();
    assert_eq!(printer.status, fotoko_protocol::PrinterStatus::Online);
    let failed = LocalApp::builder()
        .mqtt_url(Some("http://nope".into()))
        .build()
        .await;
    assert!(failed.is_err());
}
