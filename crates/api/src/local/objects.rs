//! Local stand-in for S3: files under a directory, served with presigned-style URLs.
//!
//! `presign_*` return `{base}/_objects/{key}?op=put|get&exp=..&ct=..&size=..&sig=..` where `sig`
//! is the HMAC-SHA256 of `op, key, exp, ct, size`. The PUT and GET routes below verify the
//! signature and the expiry, like S3 does: the declared content type and size are enforced on
//! upload.

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::put;
use fotoko_core::{Clock, CoreError, ObjectStore, PresignedPut, PutSpec};
use fotoko_protocol::{ContentType, MAX_PHOTO_BYTES};
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use sha2::Sha256;

use crate::error::ApiError;

/// URL prefix of the object routes.
pub const OBJECT_PREFIX: &str = "/_objects";

type HmacSha256 = Hmac<Sha256>;

/// [`ObjectStore`] on the local file system.
#[derive(Clone)]
pub struct LocalObjectStore {
    dir: PathBuf,
    base_url: String,
    secret: Vec<u8>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for LocalObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalObjectStore")
            .field("dir", &self.dir)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

/// An object key is `orders/{orderId}/{photoId}.{ext}`: three plain path segments.
fn safe_key(key: &str) -> bool {
    let segments: Vec<&str> = key.split('/').collect();
    segments.len() == 3
        && segments[0] == "orders"
        && segments.iter().all(|seg| {
            !seg.is_empty()
                && !seg.starts_with('.')
                && seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
}

fn mime_of(key: &str) -> &'static str {
    if key.ends_with(".png") {
        "image/png"
    } else if key.ends_with(".jpg") {
        "image/jpeg"
    } else {
        "application/octet-stream"
    }
}

impl LocalObjectStore {
    /// Store under `dir`; URLs point at `base_url` and are signed with `secret`.
    pub fn new(
        dir: impl Into<PathBuf>,
        base_url: impl Into<String>,
        secret: impl Into<Vec<u8>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            dir: dir.into(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            secret: secret.into(),
            clock,
        }
    }

    fn path_of(&self, key: &str) -> Result<PathBuf, CoreError> {
        if safe_key(key) {
            Ok(self.dir.join(key))
        } else {
            Err(CoreError::Validation(format!("invalid object key {key:?}")))
        }
    }

    fn mac(
        &self,
        op: &str,
        key: &str,
        exp: i64,
        ct: &str,
        size: u64,
    ) -> Result<HmacSha256, CoreError> {
        let mut mac = HmacSha256::new_from_slice(&self.secret)
            .map_err(|_| CoreError::Internal("invalid URL signing secret".into()))?;
        mac.update(format!("{op}\n{key}\n{exp}\n{ct}\n{size}").as_bytes());
        Ok(mac)
    }

    fn url(
        &self,
        op: &str,
        key: &str,
        ttl: Duration,
        ct: &str,
        size: u64,
    ) -> Result<String, CoreError> {
        let ttl = chrono::Duration::from_std(ttl)
            .map_err(|_| CoreError::Internal("presign lifetime out of range".into()))?;
        let exp = (self.clock.now() + ttl).timestamp();
        let sig = hex::encode(self.mac(op, key, exp, ct, size)?.finalize().into_bytes());
        Ok(format!(
            "{}{OBJECT_PREFIX}/{key}?op={op}&exp={exp}&ct={ct}&size={size}&sig={sig}",
            self.base_url
        ))
    }

    /// Check a request against its signed query.
    fn authorize(&self, op: &str, key: &str, q: &SignedQuery) -> Result<(), ApiError> {
        let denied =
            || ApiError::from(CoreError::Forbidden("invalid or expired object URL".into()));
        if q.op != op || q.exp < self.clock.now().timestamp() || !safe_key(key) {
            return Err(denied());
        }
        let ct = q.ct.as_deref().unwrap_or("");
        let size = q.size.unwrap_or(0);
        let given = hex::decode(&q.sig).map_err(|_| denied())?;
        let mac = self.mac(op, key, q.exp, ct, size)?;
        mac.verify_slice(&given).map_err(|_| denied())
    }

    /// Router with the PUT and GET routes of the presigned URLs.
    pub fn routes(self: &Arc<Self>) -> Router {
        let body_limit = usize::try_from(MAX_PHOTO_BYTES).unwrap_or(usize::MAX);
        Router::new()
            .route(
                &format!("{OBJECT_PREFIX}/{{*key}}"),
                put(put_object).get(get_object),
            )
            .layer(DefaultBodyLimit::max(body_limit))
            .with_state(Arc::clone(self))
    }
}

#[async_trait]
impl ObjectStore for LocalObjectStore {
    async fn presign_put(&self, spec: &PutSpec) -> Result<PresignedPut, CoreError> {
        self.path_of(&spec.key)?;
        let ct = content_type_text(spec.content_type);
        Ok(PresignedPut {
            url: self.url("put", &spec.key, spec.ttl, ct, spec.size_bytes)?,
            headers: [("content-type".to_owned(), ct.to_owned())].into(),
        })
    }

    async fn presign_get(&self, key: &str, ttl: Duration) -> Result<String, CoreError> {
        self.path_of(key)?;
        self.url("get", key, ttl, "", 0)
    }

    async fn exists(&self, key: &str) -> Result<bool, CoreError> {
        let path = self.path_of(key)?;
        tokio::fs::try_exists(&path)
            .await
            .map_err(|e| CoreError::Internal(format!("cannot stat {}: {e}", path.display())))
    }
}

fn content_type_text(content_type: ContentType) -> &'static str {
    match content_type {
        ContentType::Jpeg => "image/jpeg",
        ContentType::Png => "image/png",
    }
}

#[derive(Debug, Deserialize)]
struct SignedQuery {
    op: String,
    exp: i64,
    ct: Option<String>,
    size: Option<u64>,
    sig: String,
}

async fn put_object(
    State(store): State<Arc<LocalObjectStore>>,
    Path(key): Path<String>,
    Query(query): Query<SignedQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    store.authorize("put", &key, &query)?;
    let sent = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    if sent != query.ct.as_deref() {
        return Err(
            CoreError::Forbidden("content-type does not match the signed URL".into()).into(),
        );
    }
    if u64::try_from(body.len()).ok() != query.size {
        return Err(CoreError::Forbidden("body size does not match the signed URL".into()).into());
    }
    write_atomically(&store.path_of(&key)?, &body).await?;
    Ok(StatusCode::OK)
}

async fn write_atomically(path: &FsPath, data: &[u8]) -> Result<(), CoreError> {
    let io = |what: &str, e: std::io::Error| CoreError::Internal(format!("{what}: {e}"));
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| io("cannot create the object directory", e))?;
    }
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    let written = match tokio::fs::write(&tmp, data).await {
        Ok(()) => tokio::fs::rename(&tmp, path)
            .await
            .map_err(|e| io("cannot store the object", e)),
        Err(e) => Err(io("cannot write the object", e)),
    };
    if written.is_err() {
        // Best effort: do not leave half-written temp files behind.
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    written
}

async fn get_object(
    State(store): State<Arc<LocalObjectStore>>,
    Path(key): Path<String>,
    Query(query): Query<SignedQuery>,
) -> Result<Response, ApiError> {
    store.authorize("get", &key, &query)?;
    match tokio::fs::read(store.path_of(&key)?).await {
        Ok(bytes) => Ok(([(header::CONTENT_TYPE, mime_of(&key))], bytes).into_response()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(CoreError::NotFound("object not found".into()).into())
        }
        Err(e) => Err(CoreError::Internal(format!("cannot read the object: {e}")).into()),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Method, Request};
    use fotoko_core::testing::ManualClock;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    const BASE: &str = "http://local.test";

    struct Fixture {
        store: Arc<LocalObjectStore>,
        clock: ManualClock,
        _dir: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let clock = ManualClock::new("2026-01-01T00:00:00Z".parse().expect("ts"));
        let store = Arc::new(LocalObjectStore::new(
            dir.path(),
            format!("{BASE}/"),
            "secret",
            Arc::new(clock.clone()),
        ));
        Fixture {
            store,
            clock,
            _dir: dir,
        }
    }

    fn spec(size: u64) -> PutSpec {
        PutSpec {
            key: "orders/o1/p1.jpg".into(),
            content_type: ContentType::Jpeg,
            size_bytes: size,
            ttl: Duration::from_secs(900),
        }
    }

    fn path_and_query(url: &str) -> String {
        url.strip_prefix(BASE).expect("base url").to_owned()
    }

    async fn send(
        f: &Fixture,
        method: Method,
        uri: &str,
        ct: Option<&str>,
        body: &[u8],
    ) -> (u16, Vec<u8>) {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(ct) = ct {
            b = b.header("content-type", ct);
        }
        let response = f
            .store
            .routes()
            .oneshot(b.body(Body::from(body.to_vec())).expect("request"))
            .await
            .expect("response");
        let status = response.status().as_u16();
        (
            status,
            response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes()
                .to_vec(),
        )
    }

    #[tokio::test]
    async fn put_then_exists_then_get() {
        let f = fixture();
        assert_eq!(f.store.exists("orders/o1/p1.jpg").await, Ok(false));
        let put = f.store.presign_put(&spec(5)).await.expect("presign");
        assert_eq!(put.headers["content-type"], "image/jpeg");
        assert!(
            put.url
                .starts_with("http://local.test/_objects/orders/o1/p1.jpg?op=put&exp=1767226500")
        );

        let (status, _) = send(
            &f,
            Method::PUT,
            &path_and_query(&put.url),
            Some("image/jpeg"),
            b"hello",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(f.store.exists("orders/o1/p1.jpg").await, Ok(true));

        let get = f
            .store
            .presign_get("orders/o1/p1.jpg", Duration::from_secs(60))
            .await
            .expect("presign");
        let (status, body) = send(&f, Method::GET, &path_and_query(&get), None, b"").await;
        assert_eq!((status, body.as_slice()), (200, b"hello".as_slice()));
    }

    #[tokio::test]
    async fn png_is_served_with_its_mime_type() {
        let f = fixture();
        let mut spec = spec(1);
        spec.key = "orders/o1/p2.png".into();
        spec.content_type = ContentType::Png;
        let put = f.store.presign_put(&spec).await.expect("presign");
        assert_eq!(put.headers["content-type"], "image/png");
        let (status, _) = send(
            &f,
            Method::PUT,
            &path_and_query(&put.url),
            Some("image/png"),
            b"x",
        )
        .await;
        assert_eq!(status, 200);
        let get = f
            .store
            .presign_get("orders/o1/p2.png", Duration::from_secs(60))
            .await
            .expect("get");
        let response = f
            .store
            .routes()
            .oneshot(
                Request::builder()
                    .uri(path_and_query(&get))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.headers()["content-type"], "image/png");
    }

    #[tokio::test]
    async fn upload_must_match_the_signed_type_and_size() {
        let f = fixture();
        let uri = path_and_query(&f.store.presign_put(&spec(5)).await.expect("presign").url);
        assert_eq!(
            send(&f, Method::PUT, &uri, Some("image/png"), b"hello")
                .await
                .0,
            403
        );
        assert_eq!(send(&f, Method::PUT, &uri, None, b"hello").await.0, 403);
        assert_eq!(
            send(&f, Method::PUT, &uri, Some("image/jpeg"), b"hell")
                .await
                .0,
            403
        );
        assert_eq!(f.store.exists("orders/o1/p1.jpg").await, Ok(false));
    }

    #[tokio::test]
    async fn expired_tampered_and_wrong_op_urls_are_refused() {
        let f = fixture();
        let uri = path_and_query(&f.store.presign_put(&spec(5)).await.expect("presign").url);

        // Tampered size, key and signature.
        let bigger = uri.replace("size=5", "size=6");
        assert_eq!(
            send(&f, Method::PUT, &bigger, Some("image/jpeg"), b"hello!")
                .await
                .0,
            403
        );
        let other_key = uri.replace("p1.jpg", "p9.jpg");
        assert_eq!(
            send(&f, Method::PUT, &other_key, Some("image/jpeg"), b"hello")
                .await
                .0,
            403
        );
        let bad_sig = format!("{}0", &uri[..uri.len() - 1]);
        assert_eq!(
            send(&f, Method::PUT, &bad_sig, Some("image/jpeg"), b"hello")
                .await
                .0,
            403
        );
        let not_hex = format!("{}zz", &uri[..uri.len() - 2]);
        assert_eq!(
            send(&f, Method::PUT, &not_hex, Some("image/jpeg"), b"hello")
                .await
                .0,
            403
        );

        // A PUT URL cannot be used to GET.
        assert_eq!(send(&f, Method::GET, &uri, None, b"").await.0, 403);

        // Expired.
        f.clock.advance(chrono::Duration::seconds(901));
        assert_eq!(
            send(&f, Method::PUT, &uri, Some("image/jpeg"), b"hello")
                .await
                .0,
            403
        );
    }

    #[tokio::test]
    async fn get_of_a_missing_object_is_404() {
        let f = fixture();
        let get = f
            .store
            .presign_get("orders/o1/none.jpg", Duration::from_secs(60))
            .await
            .expect("get");
        assert_eq!(
            send(&f, Method::GET, &path_and_query(&get), None, b"")
                .await
                .0,
            404
        );
    }

    #[tokio::test]
    async fn keys_cannot_escape_the_directory() {
        let f = fixture();
        for key in [
            "../x",
            "orders/../x",
            "orders/o1/../../x",
            "orders//p.jpg",
            "orders/o1",
            "other/o1/p.jpg",
            "orders/o1/.hidden",
            "orders/o 1/p.jpg",
            "orders/o1/p.jpg/extra",
            "",
        ] {
            let mut s = spec(1);
            s.key = key.into();
            assert!(
                matches!(f.store.presign_put(&s).await, Err(CoreError::Validation(_))),
                "{key}"
            );
            assert!(matches!(
                f.store.presign_get(key, Duration::from_secs(1)).await,
                Err(CoreError::Validation(_))
            ));
            assert!(matches!(
                f.store.exists(key).await,
                Err(CoreError::Validation(_))
            ));
        }
        // A correctly signed URL for a bad key is still refused by the route.
        let (status, _) = send(
            &f,
            Method::GET,
            "/_objects/orders/o1/.x?op=get&exp=9999999999&sig=00",
            None,
            b"",
        )
        .await;
        assert_eq!(status, 403);
    }

    #[tokio::test]
    async fn huge_ttl_is_an_internal_error() {
        let f = fixture();
        let result = f.store.presign_get("orders/o1/p1.jpg", Duration::MAX).await;
        assert!(matches!(result, Err(CoreError::Internal(_))), "{result:?}");
    }

    #[tokio::test]
    async fn exists_surfaces_io_errors() {
        let f = fixture();
        // A file where a directory is expected makes `stat` fail with ENOTDIR.
        let blocker = f._dir.path().join("orders");
        std::fs::write(&blocker, b"file").expect("write");
        let result = f.store.exists("orders/o1/p1.jpg").await;
        assert!(matches!(result, Err(CoreError::Internal(_))), "{result:?}");
        let uri = path_and_query(&f.store.presign_put(&spec(1)).await.expect("presign").url);
        assert_eq!(
            send(&f, Method::PUT, &uri, Some("image/jpeg"), b"x")
                .await
                .0,
            500
        );
    }

    #[tokio::test]
    async fn failed_stores_leave_no_temp_files() {
        let f = fixture();
        // A directory where the object should go makes the final rename fail.
        let target = f._dir.path().join("orders/o1/p1.jpg");
        std::fs::create_dir_all(&target).expect("mkdir");
        let uri = path_and_query(&f.store.presign_put(&spec(1)).await.expect("presign").url);
        assert_eq!(
            send(&f, Method::PUT, &uri, Some("image/jpeg"), b"x")
                .await
                .0,
            500
        );
        let names: Vec<String> = std::fs::read_dir(f._dir.path().join("orders/o1"))
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["p1.jpg"], "no *.tmp-* leftovers");
    }

    #[test]
    fn debug_hides_the_secret() {
        let f = fixture();
        assert!(!format!("{:?}", f.store).contains("secret"));
    }
}
