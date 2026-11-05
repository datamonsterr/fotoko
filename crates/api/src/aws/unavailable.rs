//! Ports a function does not have: the IoT status Lambda only reads and writes the table, so
//! the other ports fail loudly instead of silently doing nothing.

use std::time::Duration;

use async_trait::async_trait;
use fotoko_core::{
    CoreError, JobPublisher, ObjectStore, PaymentGateway, PaymentLink, PaymentLinkRequest,
    PresignedPut, PutSpec,
};
use fotoko_protocol::PrintJob;

fn unavailable(what: &str) -> CoreError {
    CoreError::Internal(format!("{what} is not available in this function"))
}

/// [`ObjectStore`] that always fails.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoObjectStore;

#[async_trait]
impl ObjectStore for NoObjectStore {
    async fn presign_put(&self, _spec: &PutSpec) -> Result<PresignedPut, CoreError> {
        Err(unavailable("the object store"))
    }
    async fn presign_get(&self, _key: &str, _ttl: Duration) -> Result<String, CoreError> {
        Err(unavailable("the object store"))
    }
    async fn exists(&self, _key: &str) -> Result<bool, CoreError> {
        Err(unavailable("the object store"))
    }
}

/// [`JobPublisher`] that always fails.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoJobPublisher;

#[async_trait]
impl JobPublisher for NoJobPublisher {
    async fn publish_job(&self, _job: &PrintJob) -> Result<(), CoreError> {
        Err(unavailable("job publishing"))
    }
}

/// [`PaymentGateway`] that always fails.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPaymentGateway;

#[async_trait]
impl PaymentGateway for NoPaymentGateway {
    async fn create_payment_link(
        &self,
        _request: &PaymentLinkRequest,
    ) -> Result<PaymentLink, CoreError> {
        Err(unavailable("payments"))
    }
}

#[cfg(test)]
mod tests {
    use fotoko_protocol::ContentType;

    use super::*;

    #[tokio::test]
    async fn every_call_fails_with_internal() {
        let spec = PutSpec {
            key: "k".into(),
            content_type: ContentType::Jpeg,
            size_bytes: 1,
            ttl: Duration::from_secs(1),
        };
        assert!(matches!(
            NoObjectStore.presign_put(&spec).await,
            Err(CoreError::Internal(_))
        ));
        assert!(matches!(
            NoObjectStore.presign_get("k", Duration::from_secs(1)).await,
            Err(CoreError::Internal(_))
        ));
        assert!(matches!(
            NoObjectStore.exists("k").await,
            Err(CoreError::Internal(_))
        ));
        let job: PrintJob = serde_json::from_value(serde_json::json!({
            "jobId": "j", "orderId": "o", "printerId": "p", "media": "4x6",
            "createdAt": "2026-01-01T00:00:00Z", "photos": []
        }))
        .expect("job");
        assert!(matches!(
            NoJobPublisher.publish_job(&job).await,
            Err(CoreError::Internal(_))
        ));
        let request = PaymentLinkRequest {
            order_code: 1,
            amount_vnd: 1,
            description: "d".into(),
            return_url: "u".into(),
            cancel_url: "u".into(),
            expires_at: "2026-01-01T00:00:00Z".parse().expect("ts"),
        };
        let err = NoPaymentGateway.create_payment_link(&request).await;
        assert_eq!(
            err,
            Err(CoreError::Internal(
                "payments is not available in this function".into()
            ))
        );
    }
}
