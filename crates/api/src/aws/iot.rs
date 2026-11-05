//! AWS IoT Core publisher for print jobs.

use async_trait::async_trait;
use aws_sdk_iotdataplane::Client;
use aws_smithy_types::Blob;
use aws_smithy_types::error::display::DisplayErrorContext;
use fotoko_core::{CoreError, JobPublisher};
use fotoko_protocol::{PrintJob, jobs_topic};

/// [`JobPublisher`] on the IoT data plane (QoS 1).
#[derive(Debug, Clone)]
pub struct IotJobPublisher {
    client: Client,
}

impl IotJobPublisher {
    /// Publisher over `client`, which must point at the account's IoT data endpoint.
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl JobPublisher for IotJobPublisher {
    async fn publish_job(&self, job: &PrintJob) -> Result<(), CoreError> {
        let payload = serde_json::to_vec(job)
            .map_err(|e| CoreError::Internal(format!("cannot serialize the print job: {e}")))?;
        self.client
            .publish()
            .topic(jobs_topic(&job.printer_id))
            .qos(1)
            .payload(Blob::new(payload))
            .send()
            .await
            .map_err(|e| {
                tracing::error!(error = %DisplayErrorContext(&e), "IoT publish failed");
                CoreError::Internal("could not publish the print job".into())
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use aws_sdk_iotdataplane::config::{BehaviorVersion, Credentials, Region, retry::RetryConfig};
    use fotoko_protocol::{ContentType, JobPhoto, Media};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn publisher(endpoint: &str) -> IotJobPublisher {
        let config = aws_sdk_iotdataplane::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("ap-southeast-1"))
            .credentials_provider(Credentials::new(
                "AKIDEXAMPLE",
                "secret",
                None,
                None,
                "test",
            ))
            .endpoint_url(endpoint)
            .retry_config(RetryConfig::disabled())
            .build();
        IotJobPublisher::new(Client::from_conf(config))
    }

    fn job() -> PrintJob {
        PrintJob {
            job_id: "job-o1".into(),
            order_id: "o1".into(),
            printer_id: fotoko_protocol::PrinterId::new("stand-001").expect("id"),
            media: Media::Photo4x6,
            created_at: "2026-01-01T00:00:00Z".parse().expect("ts"),
            photos: vec![JobPhoto {
                photo_id: "p1".into(),
                url: "https://s3.test/x".into(),
                content_type: ContentType::Jpeg,
                copies: 2,
            }],
        }
    }

    #[tokio::test]
    async fn publishes_json_on_the_jobs_topic_with_qos_1() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/topics/fotoko%2Fprinters%2Fstand-001%2Fjobs"))
            .and(query_param("qos", "1"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        publisher(&server.uri())
            .publish_job(&job())
            .await
            .expect("published");
        let requests = server.received_requests().await.expect("recorded");
        let sent: PrintJob = serde_json::from_slice(&requests[0].body).expect("job json");
        assert_eq!(sent, job());
    }

    #[tokio::test]
    async fn failures_are_internal_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(serde_json::json!({"message": "no"})),
            )
            .mount(&server)
            .await;
        let err = publisher(&server.uri()).publish_job(&job()).await;
        assert_eq!(
            err,
            Err(CoreError::Internal(
                "could not publish the print job".into()
            ))
        );
    }
}
