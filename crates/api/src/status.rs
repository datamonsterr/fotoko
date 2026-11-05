//! The `fotoko-iot-status` function: Pi job statuses and heartbeats forwarded by the IoT rule
//! `SELECT *, topic(3) AS printerId FROM 'fotoko/printers/+/+'`.
//!
//! The rule hands over the MQTT payload itself plus `printerId` (from the topic). Status and
//! heartbeat payloads are told apart by shape: a [`JobStatus`] has `jobId`, `orderId` and
//! `state`, a [`Heartbeat`] has `printerState`. The two never overlap, so the event is parsed as
//! a job status first and as a heartbeat second. Anything else is dropped with a warning:
//! retrying a payload that cannot be parsed would never help.

use fotoko_core::{CoreError, OrderService, StatusOutcome};
use fotoko_protocol::{ErrorCode, Heartbeat, JobStatus};
use serde::Deserialize;
use serde_json::Value;

/// A parsed IoT rule event.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum IotEvent {
    /// `fotoko/printers/{id}/status`.
    Status(JobStatus),
    /// `fotoko/printers/{id}/heartbeat`.
    Heartbeat(Heartbeat),
}

/// What handling one event did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOutcome {
    /// A job status changed an order.
    StatusApplied,
    /// A job status was stale, repeated or for an unknown order.
    StatusIgnored,
    /// A heartbeat updated the printer.
    HeartbeatApplied,
    /// The payload is neither a job status nor a heartbeat.
    Malformed,
    /// The service refused the event (unknown printer, foreign printer, ...); a retry cannot
    /// change that.
    Rejected(ErrorCode),
}

/// Parse a rule event; `None` when it is neither shape.
pub fn parse_event(value: Value) -> Option<IotEvent> {
    serde_json::from_value(value).ok()
}

/// Handle one event. `Err` only for failures worth a Lambda retry (`internal`,
/// `payment_provider`).
pub async fn handle_event(service: &OrderService, event: Value) -> Result<EventOutcome, CoreError> {
    let Some(parsed) = parse_event(event) else {
        tracing::warn!("dropping an IoT event that is neither a job status nor a heartbeat");
        return Ok(EventOutcome::Malformed);
    };
    let result = match &parsed {
        IotEvent::Status(status) => {
            service
                .handle_job_status(status)
                .await
                .map(|outcome| match outcome {
                    StatusOutcome::Applied => EventOutcome::StatusApplied,
                    StatusOutcome::Ignored => EventOutcome::StatusIgnored,
                })
        }
        IotEvent::Heartbeat(heartbeat) => service
            .handle_heartbeat(heartbeat)
            .await
            .map(|()| EventOutcome::HeartbeatApplied),
    };
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error @ (CoreError::Internal(_) | CoreError::PaymentProvider(_))) => Err(error),
        Err(error) => {
            tracing::warn!(code = ?error.code(), message = %error.message(), "IoT event rejected");
            Ok(EventOutcome::Rejected(error.code()))
        }
    }
}

#[cfg(test)]
mod tests {
    use fotoko_core::testing::Harness;
    use fotoko_core::{OrderRepo, Photo, Printer};
    use fotoko_protocol::{ContentType, JobState, OrderStatus, PrinterId, PrinterStatus};
    use serde_json::json;

    use super::*;

    fn status_event(state: &str, printer: &str) -> Value {
        json!({"jobId": "job-ord-1", "orderId": "ord-1", "printerId": printer, "state": state,
               "printedCopies": 0, "at": "2026-01-01T00:01:00Z"})
    }

    fn heartbeat_event(printer: &str, state: &str) -> Value {
        json!({"printerId": printer, "printerState": state, "agentVersion": "0.1.0",
               "at": "2026-01-01T00:01:00Z"})
    }

    async fn harness_with_paid_order() -> Harness {
        let h = Harness::new();
        h.printers.insert(Printer {
            printer_id: PrinterId::new("stand-001").expect("id"),
            name: "Stand".into(),
            price_per_print_vnd: 10_000,
            status: PrinterStatus::Online,
            last_seen_at: None,
        });
        let now = "2026-01-01T00:00:00Z".parse().expect("ts");
        h.orders
            .insert(&fotoko_core::Order {
                order_id: "ord-1".into(),
                owner: "me".into(),
                printer_id: PrinterId::new("stand-001").expect("id"),
                status: OrderStatus::Paid,
                amount_vnd: 10_000,
                order_code: Some(1),
                checkout_url: None,
                qr_code: None,
                photos: vec![Photo {
                    photo_id: "p1".into(),
                    content_type: ContentType::Jpeg,
                    size_bytes: 1,
                    copies: 1,
                    uploaded: true,
                }],
                created_at: now,
                updated_at: now,
                expires_at: now,
                error: None,
            })
            .await
            .expect("insert");
        h
    }

    #[test]
    fn parses_both_shapes() {
        match parse_event(status_event("printing", "stand-001")) {
            Some(IotEvent::Status(s)) => {
                assert_eq!(s.state, JobState::Printing);
                assert_eq!(s.order_id.as_str(), "ord-1");
            }
            other => panic!("expected a status, got {other:?}"),
        }
        assert!(matches!(
            parse_event(heartbeat_event("stand-001", "idle")),
            Some(IotEvent::Heartbeat(_))
        ));
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            json!(null),
            json!("text"),
            json!([]),
            json!({}),
            json!({"printerId": "stand-001"}),
            status_event("exploded", "stand-001"),
            heartbeat_event("stand-001", "sleeping"),
            heartbeat_event("a/b", "idle"),
        ] {
            assert_eq!(parse_event(bad.clone()), None, "{bad}");
        }
    }

    #[test]
    fn unknown_extra_fields_are_ignored() {
        let mut event = status_event("failed", "stand-001");
        event["error"] = json!({"code": "paper_out", "message": "no paper"});
        event["somethingNew"] = json!(1);
        assert!(matches!(parse_event(event), Some(IotEvent::Status(_))));
    }

    #[tokio::test]
    async fn status_updates_the_order() {
        let h = harness_with_paid_order().await;
        let outcome = handle_event(&h.service, status_event("printing", "stand-001")).await;
        assert_eq!(outcome, Ok(EventOutcome::StatusApplied));
        assert_eq!(
            h.orders.snapshot(&"ord-1".into()).expect("order").status,
            OrderStatus::Printing
        );
        let again = handle_event(&h.service, status_event("printing", "stand-001")).await;
        assert_eq!(again, Ok(EventOutcome::StatusIgnored));
    }

    #[tokio::test]
    async fn status_for_unknown_order_is_ignored() {
        let h = Harness::new();
        let outcome = handle_event(&h.service, status_event("completed", "stand-001")).await;
        assert_eq!(outcome, Ok(EventOutcome::StatusIgnored));
    }

    #[tokio::test]
    async fn status_from_another_printer_is_rejected() {
        let h = harness_with_paid_order().await;
        let outcome = handle_event(&h.service, status_event("completed", "stand-999")).await;
        assert_eq!(outcome, Ok(EventOutcome::Rejected(ErrorCode::Forbidden)));
        assert_eq!(
            h.orders.snapshot(&"ord-1".into()).expect("order").status,
            OrderStatus::Paid
        );
    }

    #[tokio::test]
    async fn heartbeat_updates_the_printer() {
        let h = harness_with_paid_order().await;
        let outcome = handle_event(&h.service, heartbeat_event("stand-001", "error")).await;
        assert_eq!(outcome, Ok(EventOutcome::HeartbeatApplied));
        let printer = h
            .printers
            .snapshot(&PrinterId::new("stand-001").expect("id"))
            .expect("printer");
        assert_eq!(printer.status, PrinterStatus::Error);
        assert!(printer.last_seen_at.is_some());
    }

    #[tokio::test]
    async fn heartbeat_for_unknown_printer_is_rejected() {
        let h = Harness::new();
        let outcome = handle_event(&h.service, heartbeat_event("ghost", "idle")).await;
        assert_eq!(outcome, Ok(EventOutcome::Rejected(ErrorCode::NotFound)));
    }

    #[tokio::test]
    async fn malformed_events_are_dropped_not_retried() {
        let h = Harness::new();
        assert_eq!(
            handle_event(&h.service, json!({"x": 1})).await,
            Ok(EventOutcome::Malformed)
        );
    }

    #[tokio::test]
    async fn storage_failures_are_retried() {
        let h = harness_with_paid_order().await;
        h.orders
            .fail_with("get", Some(CoreError::Internal("db down".into())));
        let err = handle_event(&h.service, status_event("printing", "stand-001")).await;
        assert_eq!(err, Err(CoreError::Internal("db down".into())));
        h.printers.fail_with(
            "update_status",
            Some(CoreError::PaymentProvider("odd".into())),
        );
        let err = handle_event(&h.service, heartbeat_event("stand-001", "idle")).await;
        assert!(matches!(err, Err(CoreError::PaymentProvider(_))));
    }
}
