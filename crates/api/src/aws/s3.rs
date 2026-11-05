//! S3 adapter: presigned PUT/GET URLs and existence checks.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_smithy_types::error::display::DisplayErrorContext;
use fotoko_core::{CoreError, ObjectStore, PresignedPut, PutSpec};
use fotoko_protocol::ContentType;

/// Headers that the client sets itself and must not be told to send.
const SKIPPED_HEADERS: [&str; 2] = ["host", "content-length"];

/// [`ObjectStore`] on one S3 bucket.
#[derive(Debug, Clone)]
pub struct S3ObjectStore {
    client: Client,
    bucket: String,
}

impl S3ObjectStore {
    /// Store on `bucket`.
    pub fn new(client: Client, bucket: impl Into<String>) -> Self {
        Self {
            client,
            bucket: bucket.into(),
        }
    }
}

fn presign_config(ttl: Duration) -> Result<PresigningConfig, CoreError> {
    PresigningConfig::expires_in(ttl)
        .map_err(|e| CoreError::Internal(format!("invalid presign lifetime: {e}")))
}

fn internal<E: std::error::Error + 'static>(op: &str, error: &E) -> CoreError {
    tracing::error!(op, error = %DisplayErrorContext(error), "S3 call failed");
    CoreError::Internal(format!("storage error during {op}"))
}

#[async_trait]
impl ObjectStore for S3ObjectStore {
    /// The content type and length are part of the signature: S3 refuses an upload whose
    /// `content-type` differs, or whose size is not the declared one, so the size limit
    /// of the order is enforced by S3 itself.
    async fn presign_put(&self, spec: &PutSpec) -> Result<PresignedPut, CoreError> {
        let content_type = match spec.content_type {
            ContentType::Jpeg => "image/jpeg",
            ContentType::Png => "image/png",
        };
        let length = i64::try_from(spec.size_bytes)
            .map_err(|_| CoreError::Validation("sizeBytes is too large".into()))?;
        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&spec.key)
            .content_type(content_type)
            .content_length(length)
            .presigned(presign_config(spec.ttl)?)
            .await
            .map_err(|e| internal("presign_put", &e))?;
        let headers: BTreeMap<String, String> = request
            .headers()
            .filter(|(name, _)| !SKIPPED_HEADERS.contains(&name.to_ascii_lowercase().as_str()))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
            .collect();
        Ok(PresignedPut {
            url: request.uri().to_owned(),
            headers,
        })
    }

    async fn presign_get(&self, key: &str, ttl: Duration) -> Result<String, CoreError> {
        let request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(presign_config(ttl)?)
            .await
            .map_err(|e| internal("presign_get", &e))?;
        Ok(request.uri().to_owned())
    }

    /// `HeadObject`. The role has `s3:ListBucket`, so a missing key is a 404; a 403 is treated
    /// as "not uploaded" too, which is what S3 answers without that permission.
    async fn exists(&self, key: &str) -> Result<bool, CoreError> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(error) => {
                let status = error.raw_response().map(|r| r.status().as_u16());
                if error.as_service_error().is_some_and(|e| e.is_not_found()) || status == Some(404)
                {
                    Ok(false)
                } else if status == Some(403) {
                    tracing::warn!(
                        key,
                        "HeadObject was denied, treating the photo as not uploaded"
                    );
                    Ok(false)
                } else {
                    Err(internal("head_object", &error))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region, retry::RetryConfig};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn store(endpoint: &str) -> S3ObjectStore {
        let config = aws_sdk_s3::Config::builder()
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
            .force_path_style(true)
            .retry_config(RetryConfig::disabled())
            .build();
        S3ObjectStore::new(Client::from_conf(config), "bucket")
    }

    fn spec(size: u64) -> PutSpec {
        PutSpec {
            key: "orders/o1/p1.jpg".into(),
            content_type: ContentType::Jpeg,
            size_bytes: size,
            ttl: Duration::from_secs(900),
        }
    }

    #[tokio::test]
    async fn presigned_put_signs_type_and_length() {
        let put = store("http://s3.test")
            .presign_put(&spec(1234))
            .await
            .expect("presign");
        assert!(
            put.url
                .starts_with("http://s3.test/bucket/orders/o1/p1.jpg?"),
            "{}",
            put.url
        );
        assert!(put.url.contains("X-Amz-Expires=900"));
        let signed = put
            .url
            .split('&')
            .find(|p| p.starts_with("X-Amz-SignedHeaders="))
            .expect("signed headers")
            .to_ascii_lowercase();
        assert!(signed.contains("content-type"), "{signed}");
        assert!(signed.contains("content-length"), "{signed}");
        assert_eq!(
            put.headers,
            BTreeMap::from([("content-type".to_owned(), "image/jpeg".to_owned())])
        );
    }

    #[tokio::test]
    async fn presigned_put_png_and_bad_sizes() {
        let mut png = spec(10);
        png.content_type = ContentType::Png;
        let put = store("http://s3.test")
            .presign_put(&png)
            .await
            .expect("presign");
        assert_eq!(put.headers["content-type"], "image/png");

        let err = store("http://s3.test").presign_put(&spec(u64::MAX)).await;
        assert!(matches!(err, Err(CoreError::Validation(_))));

        let mut long_ttl = spec(10);
        long_ttl.ttl = Duration::from_secs(8 * 24 * 3600);
        let err = store("http://s3.test").presign_put(&long_ttl).await;
        assert!(matches!(err, Err(CoreError::Internal(_))), "{err:?}");
    }

    #[tokio::test]
    async fn presigned_get() {
        let url = store("http://s3.test")
            .presign_get("orders/o1/p1.jpg", Duration::from_secs(900))
            .await
            .expect("presign");
        assert!(url.starts_with("http://s3.test/bucket/orders/o1/p1.jpg?"));
        assert!(url.contains("X-Amz-Expires=900"));
        let err = store("http://s3.test")
            .presign_get("k", Duration::from_secs(8 * 24 * 3600))
            .await;
        assert!(matches!(err, Err(CoreError::Internal(_))));
    }

    async fn exists_with(status: u16) -> Result<bool, CoreError> {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/bucket/orders/o1/p1.jpg"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        store(&server.uri()).exists("orders/o1/p1.jpg").await
    }

    #[tokio::test]
    async fn exists_maps_status_codes() {
        assert_eq!(exists_with(200).await, Ok(true));
        assert_eq!(exists_with(404).await, Ok(false));
        assert_eq!(exists_with(403).await, Ok(false));
        assert!(matches!(
            exists_with(500).await,
            Err(CoreError::Internal(_))
        ));
        assert!(matches!(
            exists_with(400).await,
            Err(CoreError::Internal(_))
        ));
    }

    #[tokio::test]
    async fn exists_unreachable_is_internal() {
        let result = store("http://127.0.0.1:1").exists("k").await;
        assert!(matches!(result, Err(CoreError::Internal(_))));
    }
}
