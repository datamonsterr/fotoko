//! The whole flow in one process: customer HTTP calls, payOS webhooks, the MQTT broker, the Pi
//! agent and the printer simulation. Every test has its own stack on ephemeral ports and
//! temporary directories; waits are polls with a deadline, never fixed sleeps.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use chrono::Utc;
use fotoko_e2e::fixtures::{landscape_jpeg, portrait_png};
use fotoko_e2e::stack::{PRINTER, Stack, StackOptions, WAIT, eventually};
use fotoko_protocol::{
    DEFAULT_PRICE_VND, JobState, Media, OrderStatus, PrintJob, PrinterState, PrinterStatus,
};
use reqwest::Method;
use serde_json::json;

async fn stack() -> Stack {
    Stack::start(StackOptions::default()).await
}

/// 3 prints: one landscape JPEG and a portrait PNG twice.
fn two_photos() -> [fotoko_e2e::fixtures::Photo; 2] {
    [landscape_jpeg(1), portrait_png(2)]
}

#[tokio::test]
async fn happy_path_prints_every_copy_and_completes() {
    let s = stack().await;
    let photos = two_photos();

    let created = s.create_uploaded_order("guest-happy", &photos).await;
    let order_id = created.order_id.to_string();
    assert_eq!(created.status, OrderStatus::AwaitingUpload);
    assert_eq!(created.amount_vnd, 3 * DEFAULT_PRICE_VND);
    let order = s.order("guest-happy", &order_id).await;
    assert!(order.photos.iter().all(|p| p.uploaded), "{order:?}");

    let checkout = s.checkout("guest-happy", &order_id).await;
    assert_eq!(checkout.status, OrderStatus::AwaitingPayment);
    assert_eq!(
        checkout.checkout_url,
        format!("https://pay.test/web/{}", checkout.order_code)
    );
    s.pay(checkout.order_code, created.amount_vnd).await;

    let done = s
        .wait_order_status("guest-happy", &order_id, OrderStatus::Completed)
        .await;
    assert!(done.error.is_none(), "{done:?}");

    // Exactly three pages: photo 0 once, photo 1 twice, fitted to a 6x4 / 4x6 inch sheet.
    let job_id = format!("job-{order_id}");
    let (first, second) = (&created.uploads[0].photo_id, &created.uploads[1].photo_id);
    let pages: Vec<_> = s
        .wait_pages(3)
        .await
        .into_iter()
        .map(|p| (p.name, p.width, p.height))
        .collect();
    assert_eq!(
        pages,
        vec![
            (format!("{job_id}-0-{first}-1.png"), 1800, 1200),
            (format!("{job_id}-1-{second}-1.png"), 1200, 1800),
            (format!("{job_id}-1-{second}-2.png"), 1200, 1800),
        ]
    );

    // One job on the wire, for the right printer and photos; the Pi reported it through.
    let jobs = s.cloud.jobs();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0].job_id.as_str(), job_id);
    assert_eq!(jobs[0].printer_id.as_str(), PRINTER);
    assert_eq!(jobs[0].media, Media::Photo4x6);
    assert_eq!(
        jobs[0].photos.iter().map(|p| p.copies).collect::<Vec<_>>(),
        [1, 2]
    );
    let completed = eventually("the completed status on the wire", WAIT, || async {
        s.cloud
            .statuses()
            .into_iter()
            .find(|st| st.job_id.as_str() == job_id && st.state == JobState::Completed)
    })
    .await;
    assert_eq!(completed.printed_copies, 3);
    assert!(completed.error.is_none());
    s.wait_printer(PrinterStatus::Online).await;
}

#[tokio::test]
async fn paper_out_fails_the_order_with_the_pi_error() {
    let s = Stack::start(StackOptions {
        paper: 1,
        ..StackOptions::default()
    })
    .await;
    let paid = s.place_paid_order("guest-paper", &two_photos()).await;

    let failed = s
        .wait_order_status("guest-paper", &paid.order_id, OrderStatus::Failed)
        .await;
    assert_eq!(failed.error.expect("error").code, "paper_out");
    // The one sheet there was got printed before the paper ran out.
    assert_eq!(s.sim_pages().len(), 1);
    // The empty tray shows in the heartbeat: the printer is no longer bookable.
    s.wait_printer(PrinterStatus::Error).await;
    let (status, body) = s
        .try_create_order("guest-paper-2", &[landscape_jpeg(1)])
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test]
async fn bad_webhook_signature_is_refused_and_changes_nothing() {
    let s = stack().await;
    let created = s
        .create_uploaded_order("guest-sig", &[landscape_jpeg(1)])
        .await;
    let order_id = created.order_id.to_string();
    let checkout = s.checkout("guest-sig", &order_id).await;

    let forged = s.forged_webhook(checkout.order_code, created.amount_vnd);
    let (status, body) = s.send_webhook(&forged).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "validation");

    assert_eq!(
        s.order("guest-sig", &order_id).await.status,
        OrderStatus::AwaitingPayment
    );
    s.cloud.barrier().await;
    assert!(s.cloud.jobs().is_empty());
    assert!(s.sim_pages().is_empty());
}

#[tokio::test]
async fn duplicate_webhooks_print_one_set() {
    let s = stack().await;
    let paid = s.place_paid_order("guest-dup", &two_photos()).await;
    // Two more deliveries right behind the first (payOS retries), whether the order is still
    // `paid` (job republished, the Pi dedupes) or already `printing`.
    s.pay(paid.order_code, paid.amount_vnd).await;
    s.pay(paid.order_code, paid.amount_vnd).await;
    s.wait_order_status("guest-dup", &paid.order_id, OrderStatus::Completed)
        .await;
    // And one after completion.
    s.pay(paid.order_code, paid.amount_vnd).await;

    s.cloud.barrier().await;
    let job_id = format!("job-{}", paid.order_id);
    assert!(
        s.cloud.jobs().iter().all(|j| j.job_id.as_str() == job_id),
        "{:?}",
        s.cloud.jobs()
    );
    assert_eq!(s.wait_pages(3).await.len(), 3);
    assert_eq!(
        s.order("guest-dup", &paid.order_id).await.status,
        OrderStatus::Completed
    );
}

#[tokio::test]
async fn amount_mismatch_is_refused_and_changes_nothing() {
    let s = stack().await;
    let created = s
        .create_uploaded_order("guest-amount", &[landscape_jpeg(2)])
        .await;
    let order_id = created.order_id.to_string();
    let checkout = s.checkout("guest-amount", &order_id).await;

    let (status, body) = s
        .webhook("00", checkout.order_code, created.amount_vnd - 1)
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "validation");
    assert_eq!(
        s.order("guest-amount", &order_id).await.status,
        OrderStatus::AwaitingPayment
    );
    s.cloud.barrier().await;
    assert!(s.cloud.jobs().is_empty());
}

#[tokio::test]
async fn a_failed_payment_code_changes_nothing_and_the_order_stays_payable() {
    let s = stack().await;
    let created = s
        .create_uploaded_order("guest-notpaid", &[landscape_jpeg(1)])
        .await;
    let order_id = created.order_id.to_string();
    let checkout = s.checkout("guest-notpaid", &order_id).await;

    let (status, body) = s
        .webhook("01", checkout.order_code, created.amount_vnd)
        .await;
    assert_eq!((status, body), (200, json!({"success": true})));
    assert_eq!(
        s.order("guest-notpaid", &order_id).await.status,
        OrderStatus::AwaitingPayment
    );
    s.cloud.barrier().await;
    assert!(s.cloud.jobs().is_empty());

    s.pay(checkout.order_code, created.amount_vnd).await;
    s.wait_order_status("guest-notpaid", &order_id, OrderStatus::Completed)
        .await;
    assert_eq!(s.wait_pages(1).await.len(), 1);
}

#[tokio::test]
async fn an_expired_order_ignores_a_late_payment() {
    let s = stack().await;
    let created = s
        .create_uploaded_order("guest-late", &[landscape_jpeg(1)])
        .await;
    let order_id = created.order_id.to_string();
    let checkout = s.checkout("guest-late", &order_id).await;

    // The order lives 15 minutes.
    s.advance_clock(Duration::from_secs(16 * 60));
    assert_eq!(
        s.order("guest-late", &order_id).await.status,
        OrderStatus::Expired
    );

    // payOS still delivers the payment: acknowledged (no retry storm), nothing printed.
    let (status, body) = s
        .webhook("00", checkout.order_code, created.amount_vnd)
        .await;
    assert_eq!((status, body), (200, json!({"success": true})));
    s.cloud.barrier().await;
    assert!(s.cloud.jobs().is_empty());
    assert!(s.sim_pages().is_empty());
    assert_eq!(
        s.order("guest-late", &order_id).await.status,
        OrderStatus::Expired
    );
}

#[tokio::test]
async fn cancel_works_before_payment_only() {
    let s = stack().await;

    // Cancel after checkout; a payment that still arrives is not printed.
    let created = s
        .create_uploaded_order("guest-cancel", &[landscape_jpeg(1)])
        .await;
    let order_id = created.order_id.to_string();
    let checkout = s.checkout("guest-cancel", &order_id).await;
    let (status, body) = s
        .call(
            Method::POST,
            &format!("/v1/orders/{order_id}/cancel"),
            Some("guest-cancel"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "cancelled");
    let (status, _) = s
        .webhook("00", checkout.order_code, created.amount_vnd)
        .await;
    assert_eq!(status, 200);
    s.cloud.barrier().await;
    assert!(s.cloud.jobs().is_empty());
    assert_eq!(
        s.order("guest-cancel", &order_id).await.status,
        OrderStatus::Cancelled
    );

    // Somebody else's order cannot be cancelled either.
    let (status, body) = s
        .call(
            Method::POST,
            &format!("/v1/orders/{order_id}/cancel"),
            Some("someone-else"),
            None,
        )
        .await;
    assert_eq!(status, 403, "{body}");

    // A paid order cannot be cancelled and prints regardless.
    let paid = s.place_paid_order("guest-cancel", &[portrait_png(1)]).await;
    let (status, body) = s
        .call(
            Method::POST,
            &format!("/v1/orders/{}/cancel", paid.order_id),
            Some("guest-cancel"),
            None,
        )
        .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "conflict");
    s.wait_order_status("guest-cancel", &paid.order_id, OrderStatus::Completed)
        .await;
    assert_eq!(s.wait_pages(1).await.len(), 1);
}

#[tokio::test]
async fn a_stopped_agent_takes_the_printer_offline() {
    let mut s = stack().await;
    s.stop_agent().await;
    s.wait_printer(PrinterStatus::Offline).await;

    let (status, body) = s
        .call(Method::GET, "/v1/printers/stand-001", None, None)
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "offline");
    let (status, body) = s.try_create_order("guest-off", &[landscape_jpeg(1)]).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "validation");

    // Back online after a restart.
    s.start_agent().await;
    s.create_order("guest-off", &[landscape_jpeg(1)]).await;
}

#[tokio::test]
async fn a_crashed_agent_goes_offline_through_its_last_will() {
    let mut s = stack().await;
    let beats_before = s.cloud.heartbeats().len();
    s.crash_agent().await;
    s.wait_printer(PrinterStatus::Offline).await;
    // The announcement came from the broker (the agent said nothing), on the heartbeat topic.
    assert!(
        s.cloud.heartbeats()[beats_before..]
            .iter()
            .any(|beat| beat.printer_state == PrinterState::Offline)
    );
    let (status, _) = s
        .try_create_order("guest-crash", &[landscape_jpeg(1)])
        .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_job_for_another_printer_is_ignored() {
    let s = stack().await;
    // A job addressed to stand-999 lands on stand-001's topic (a misconfiguration or an
    // attack). It is dropped: nothing printed, no status.
    let foreign: PrintJob = serde_json::from_value(json!({
        "jobId": "job-foreign", "orderId": "ord-foreign", "printerId": "stand-999",
        "media": "4x6", "createdAt": Utc::now(),
        "photos": [{"photoId": "pho-foreign", "url": format!("{}/nothing.jpg", s.base),
                    "contentType": "image/jpeg", "copies": 1}]
    }))
    .expect("job");
    s.cloud.publish_job_on(PRINTER, &foreign).await;

    // The agent handles jobs in order, so once a real job is through the foreign one was seen.
    let paid = s
        .place_paid_order("guest-foreign", &[landscape_jpeg(1)])
        .await;
    s.wait_order_status("guest-foreign", &paid.order_id, OrderStatus::Completed)
        .await;
    let pages = s.wait_pages(1).await;
    assert!(
        pages[0]
            .name
            .starts_with(&format!("job-{}-", paid.order_id))
    );
    assert!(
        s.cloud
            .statuses()
            .iter()
            .all(|st| st.order_id.as_str() != "ord-foreign" && st.job_id.as_str() != "job-foreign"),
        "{:?}",
        s.cloud.statuses()
    );
}

#[tokio::test]
async fn a_payment_while_the_agent_is_down_prints_once_the_agent_is_back() {
    let mut s = Stack::start(StackOptions {
        agent: false,
        ..StackOptions::default()
    })
    .await;
    // Nobody listens: the job is published into the void (or queued for the persistent session).
    let paid = s.place_paid_order("guest-down", &two_photos()).await;
    assert_eq!(
        s.order("guest-down", &paid.order_id).await.status,
        OrderStatus::Paid
    );
    assert!(s.sim_pages().is_empty());

    s.start_agent().await;
    // payOS retries the webhook: while the order is `paid` this republishes the same job id.
    s.pay(paid.order_code, paid.amount_vnd).await;
    s.wait_order_status("guest-down", &paid.order_id, OrderStatus::Completed)
        .await;
    // Whether the broker queued the first job or only the republish arrived, one set printed.
    assert_eq!(s.wait_pages(3).await.len(), 3);
    let job_id = format!("job-{}", paid.order_id);
    assert!(s.cloud.jobs().iter().all(|j| j.job_id.as_str() == job_id));
}

#[tokio::test]
async fn a_restart_mid_job_fails_the_order_and_a_webhook_retry_does_not_reprint() {
    let mut s = Stack::start(StackOptions {
        page_latency_ms: 300,
        ..StackOptions::default()
    })
    .await;
    let paid = s.place_paid_order("guest-restart", &two_photos()).await;
    s.wait_order_status("guest-restart", &paid.order_id, OrderStatus::Printing)
        .await;

    // Power loss while the first page is being written, then the Pi comes back.
    s.crash_agent().await;
    s.start_agent().await;

    // The new agent finds the unfinished job in its ledger and reports it failed.
    let failed = s
        .wait_order_status("guest-restart", &paid.order_id, OrderStatus::Failed)
        .await;
    assert_eq!(failed.error.expect("error").code, "driver_error");

    // `failed` is final: the webhook retry is acknowledged but the order is not printed again
    // (a customer whose print failed gets a refund and a new order, see docs/contracts.md).
    let jobs_before = s.cloud.jobs().len();
    s.pay(paid.order_code, paid.amount_vnd).await;
    s.cloud.barrier().await;
    assert_eq!(s.cloud.jobs().len(), jobs_before);
    assert_eq!(
        s.order("guest-restart", &paid.order_id).await.status,
        OrderStatus::Failed
    );
}
