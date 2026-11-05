//! The `fotoko-expire` function, run by an EventBridge schedule: expires every unpaid order
//! past its `expiresAt`.
//!
//! [`OrderService::expire_due`] already loops in batches until the due index is empty, so one
//! invocation drains the backlog. If the Lambda times out half way, the next scheduled run picks
//! up where it stopped: every expiry is a conditional write and safe to repeat. A corrupt item in
//! the index is skipped by the repo (and logged) instead of blocking the sweep.

use fotoko_core::{CoreError, OrderService};
use serde::Serialize;

/// Result of one run, returned to Lambda (and visible in the logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExpireReport {
    /// Orders this run moved to `expired`.
    pub expired: usize,
}

/// Sweep once. The scheduled event carries nothing we need, so it is not parsed.
pub async fn handle(service: &OrderService) -> Result<ExpireReport, CoreError> {
    let expired = service.expire_due().await?;
    tracing::info!(expired, "expiry sweep done");
    Ok(ExpireReport { expired })
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use fotoko_core::testing::Harness;
    use fotoko_core::{Order, OrderRepo, Photo};
    use fotoko_protocol::{ContentType, OrderStatus, PrinterId};

    use super::*;

    async fn add(h: &Harness, id: &str, status: OrderStatus, expires_in: Duration) {
        let now = fotoko_core::Clock::now(&h.clock);
        h.orders
            .insert(&Order {
                order_id: id.into(),
                owner: "me".into(),
                printer_id: PrinterId::new("stand-001").expect("id"),
                status,
                amount_vnd: 1,
                order_code: None,
                checkout_url: None,
                qr_code: None,
                photos: vec![Photo {
                    photo_id: "p".into(),
                    content_type: ContentType::Jpeg,
                    size_bytes: 1,
                    copies: 1,
                    uploaded: false,
                }],
                created_at: now,
                updated_at: now,
                expires_at: now + expires_in,
                error: None,
            })
            .await
            .expect("insert");
    }

    #[tokio::test]
    async fn expires_only_overdue_unpaid_orders() {
        let h = Harness::new();
        add(
            &h,
            "overdue-upload",
            OrderStatus::AwaitingUpload,
            Duration::minutes(-5),
        )
        .await;
        add(
            &h,
            "overdue-payment",
            OrderStatus::AwaitingPayment,
            Duration::minutes(-1),
        )
        .await;
        add(
            &h,
            "fresh",
            OrderStatus::AwaitingUpload,
            Duration::minutes(10),
        )
        .await;
        add(&h, "paid", OrderStatus::Paid, Duration::minutes(-30)).await;

        assert_eq!(handle(&h.service).await, Ok(ExpireReport { expired: 2 }));
        let status = |id: &str| h.orders.snapshot(&id.into()).expect("order").status;
        assert_eq!(status("overdue-upload"), OrderStatus::Expired);
        assert_eq!(status("overdue-payment"), OrderStatus::Expired);
        assert_eq!(status("fresh"), OrderStatus::AwaitingUpload);
        assert_eq!(status("paid"), OrderStatus::Paid);
        // Nothing left: a second run is a no-op.
        assert_eq!(handle(&h.service).await, Ok(ExpireReport { expired: 0 }));
    }

    #[tokio::test]
    async fn storage_failure_fails_the_run() {
        let h = Harness::new();
        h.orders.fail_with(
            "list_due_for_expiry",
            Some(CoreError::Internal("db down".into())),
        );
        assert_eq!(
            handle(&h.service).await,
            Err(CoreError::Internal("db down".into()))
        );
    }

    #[test]
    fn report_serializes() {
        assert_eq!(
            serde_json::to_string(&ExpireReport { expired: 3 }).expect("json"),
            r#"{"expired":3}"#
        );
    }
}
