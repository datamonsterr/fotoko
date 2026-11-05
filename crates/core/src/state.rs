//! Order status machine from `docs/contracts.md`.
//!
//! ```text
//! awaiting_upload --checkout--> awaiting_payment --payment--> paid --job started--> printing
//! printing --completed--> completed
//! awaiting_upload|awaiting_payment --cancel--> cancelled
//! awaiting_upload|awaiting_payment --expire--> expired
//! paid|printing --pi failed--> failed
//! ```

use fotoko_protocol::OrderStatus;

use crate::error::CoreError;

/// Something that happens to an order and may change its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderEvent {
    /// All photos uploaded, payment link created.
    Checkout,
    /// payOS confirmed the payment.
    PaymentConfirmed,
    /// The Pi received or started the print job.
    JobStarted,
    /// The Pi finished printing.
    JobCompleted,
    /// The Pi reported a failure.
    JobFailed,
    /// The customer cancelled.
    Cancel,
    /// The order outlived `expiresAt` unpaid.
    Expire,
}

impl OrderEvent {
    /// Every event, for exhaustive tests.
    pub const ALL: [Self; 7] = [
        Self::Checkout,
        Self::PaymentConfirmed,
        Self::JobStarted,
        Self::JobCompleted,
        Self::JobFailed,
        Self::Cancel,
        Self::Expire,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Checkout => "checkout",
            Self::PaymentConfirmed => "payment",
            Self::JobStarted => "job start",
            Self::JobCompleted => "job completion",
            Self::JobFailed => "job failure",
            Self::Cancel => "cancel",
            Self::Expire => "expiry",
        }
    }
}

/// Every status, for exhaustive tests.
pub const ALL_STATUSES: [OrderStatus; 8] = [
    OrderStatus::AwaitingUpload,
    OrderStatus::AwaitingPayment,
    OrderStatus::Paid,
    OrderStatus::Printing,
    OrderStatus::Completed,
    OrderStatus::Failed,
    OrderStatus::Cancelled,
    OrderStatus::Expired,
];

/// The status after `event`, or `Conflict` when the contract does not allow it.
pub fn transition(from: OrderStatus, event: OrderEvent) -> Result<OrderStatus, CoreError> {
    use OrderEvent as E;
    use OrderStatus as S;
    match (from, event) {
        (S::AwaitingUpload, E::Checkout) => Ok(S::AwaitingPayment),
        (S::AwaitingPayment, E::PaymentConfirmed) => Ok(S::Paid),
        (S::Paid, E::JobStarted) => Ok(S::Printing),
        (S::Printing, E::JobCompleted) => Ok(S::Completed),
        (S::Paid | S::Printing, E::JobFailed) => Ok(S::Failed),
        (S::AwaitingUpload | S::AwaitingPayment, E::Cancel) => Ok(S::Cancelled),
        (S::AwaitingUpload | S::AwaitingPayment, E::Expire) => Ok(S::Expired),
        _ => Err(CoreError::Conflict(format!(
            "order is {}: {} not allowed",
            status_name(from),
            event.name()
        ))),
    }
}

/// The wire name of a status (`awaiting_upload`, ...).
pub const fn status_name(status: OrderStatus) -> &'static str {
    match status {
        OrderStatus::AwaitingUpload => "awaiting_upload",
        OrderStatus::AwaitingPayment => "awaiting_payment",
        OrderStatus::Paid => "paid",
        OrderStatus::Printing => "printing",
        OrderStatus::Completed => "completed",
        OrderStatus::Failed => "failed",
        OrderStatus::Cancelled => "cancelled",
        OrderStatus::Expired => "expired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use OrderEvent as E;
    use OrderStatus as S;

    /// Every allowed (from, event, to). Everything else must be rejected.
    const ALLOWED: [(OrderStatus, OrderEvent, OrderStatus); 10] = [
        (S::AwaitingUpload, E::Checkout, S::AwaitingPayment),
        (S::AwaitingPayment, E::PaymentConfirmed, S::Paid),
        (S::Paid, E::JobStarted, S::Printing),
        (S::Printing, E::JobCompleted, S::Completed),
        (S::Paid, E::JobFailed, S::Failed),
        (S::Printing, E::JobFailed, S::Failed),
        (S::AwaitingUpload, E::Cancel, S::Cancelled),
        (S::AwaitingPayment, E::Cancel, S::Cancelled),
        (S::AwaitingUpload, E::Expire, S::Expired),
        (S::AwaitingPayment, E::Expire, S::Expired),
    ];

    #[test]
    fn allowed_transitions() {
        for (from, event, to) in ALLOWED {
            assert_eq!(transition(from, event), Ok(to), "{from:?} {event:?}");
        }
    }

    #[test]
    fn every_other_pair_is_rejected_with_conflict() {
        let mut rejected = 0;
        for from in ALL_STATUSES {
            for event in OrderEvent::ALL {
                if ALLOWED.iter().any(|(f, e, _)| *f == from && *e == event) {
                    continue;
                }
                rejected += 1;
                let err = transition(from, event).expect_err("must be rejected");
                assert!(matches!(err, CoreError::Conflict(_)), "{from:?} {event:?}");
            }
        }
        // 8 statuses x 7 events = 56 pairs, 10 allowed.
        assert_eq!(rejected, 46);
    }

    #[test]
    fn terminal_statuses_reject_everything() {
        for from in [S::Completed, S::Failed, S::Cancelled, S::Expired] {
            for event in OrderEvent::ALL {
                assert!(transition(from, event).is_err(), "{from:?} {event:?}");
            }
        }
    }

    #[test]
    fn conflict_message_names_status_and_event() {
        let err = transition(S::Paid, E::Cancel).expect_err("rejected");
        assert_eq!(err.message(), "order is paid: cancel not allowed");
    }

    #[test]
    fn event_and_status_names() {
        let names: Vec<_> = OrderEvent::ALL.iter().map(|e| e.name()).collect();
        assert_eq!(
            names,
            [
                "checkout",
                "payment",
                "job start",
                "job completion",
                "job failure",
                "cancel",
                "expiry"
            ]
        );
        for status in ALL_STATUSES {
            let wire = serde_json::to_value(status).expect("serialize");
            assert_eq!(wire, status_name(status));
        }
    }
}
