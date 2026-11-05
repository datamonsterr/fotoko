//! Image download: https only (http when allowed), size cap while streaming, magic bytes and
//! header checks, temporary files that clean up after themselves.

use std::error::Error as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fotoko_printer_sim::probe_file;
use fotoko_protocol::{ContentType, PiErrorCode};
use reqwest::{Client, Url, redirect};
use thiserror::Error;
use tokio::io::AsyncWriteExt;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const JPEG_SIGNATURE: [u8; 3] = [0xFF, 0xD8, 0xFF];
/// Bytes needed to decide a signature.
const SIGNATURE_LEN: usize = PNG_SIGNATURE.len();

/// Why a photo could not be fetched or accepted.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// The URL does not parse or has no host.
    #[error("invalid image url")]
    BadUrl,
    /// The scheme is not allowed.
    #[error("image url must use https")]
    InsecureScheme,
    /// The HTTP client could not be built.
    #[error("cannot build http client: {0}")]
    Client(String),
    /// Connection, TLS or timeout failure.
    #[error("download failed: {0}")]
    Network(String),
    /// Non-2xx answer.
    #[error("download failed: server answered {0}")]
    Status(u16),
    /// Local filesystem failure.
    #[error("cannot store image: {0}")]
    Io(String),
    /// More bytes than allowed.
    #[error("image exceeds the {0} byte limit")]
    TooLarge(u64),
    /// Leading bytes do not match the declared content type.
    #[error("image content does not match {0}")]
    BadMagic(&'static str),
    /// Header check failed (unreadable, wrong format or absurd size).
    #[error("invalid image: {0}")]
    Invalid(String),
}

impl DownloadError {
    /// Pi error code for this failure.
    pub fn code(&self) -> PiErrorCode {
        match self {
            Self::TooLarge(_) | Self::BadMagic(_) | Self::Invalid(_) => PiErrorCode::InvalidImage,
            Self::BadUrl
            | Self::InsecureScheme
            | Self::Client(_)
            | Self::Network(_)
            | Self::Status(_)
            | Self::Io(_) => PiErrorCode::DownloadFailed,
        }
    }
}

/// A downloaded file that is deleted when dropped.
#[derive(Debug)]
pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// Where the file lives.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Parse an image URL and enforce the scheme rule.
pub fn check_url(url: &str, allow_http: bool) -> Result<Url, DownloadError> {
    let parsed = Url::parse(url).map_err(|_| DownloadError::BadUrl)?;
    if parsed.host_str().is_none() {
        return Err(DownloadError::BadUrl);
    }
    match parsed.scheme() {
        "https" => Ok(parsed),
        "http" if allow_http => Ok(parsed),
        _ => Err(DownloadError::InsecureScheme),
    }
}

/// Whether leading bytes match the declared type.
pub fn magic_matches(head: &[u8], content_type: ContentType) -> bool {
    match content_type {
        ContentType::Jpeg => head.starts_with(&JPEG_SIGNATURE),
        ContentType::Png => head.starts_with(&PNG_SIGNATURE),
    }
}

/// File extension for a content type.
pub fn extension(content_type: ContentType) -> &'static str {
    match content_type {
        ContentType::Jpeg => "jpg",
        ContentType::Png => "png",
    }
}

fn mime(content_type: ContentType) -> &'static str {
    match content_type {
        ContentType::Jpeg => "image/jpeg",
        ContentType::Png => "image/png",
    }
}

/// Make an id safe as a file name component: ASCII alphanumerics, `-` and `_` stay, everything
/// else becomes `_`, and the result is never empty.
pub fn sanitize_component(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "_".to_owned()
    } else {
        cleaned
    }
}

fn describe(error: reqwest::Error) -> String {
    // Never include the URL: it carries a presigned signature.
    let error = error.without_url();
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// HTTP client for presigned image URLs.
#[derive(Debug, Clone)]
pub struct Downloader {
    client: Client,
    allow_http: bool,
    max_bytes: u64,
}

impl Downloader {
    /// `timeout` bounds each whole download. Redirects are not followed.
    pub fn new(allow_http: bool, max_bytes: u64, timeout: Duration) -> Result<Self, DownloadError> {
        crate::tls::install_crypto_provider();
        let client = Client::builder()
            .redirect(redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10).min(timeout))
            .timeout(timeout)
            .user_agent(concat!("fotoko-pi-agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| DownloadError::Client(describe(e)))?;
        Ok(Self {
            client,
            allow_http,
            max_bytes,
        })
    }

    /// Download `url` into `dir` as `{stem}.{jpg|png}` and validate it against `content_type`.
    /// On any error nothing is left on disk.
    pub async fn fetch(
        &self,
        url: &str,
        content_type: ContentType,
        dir: &Path,
        stem: &str,
    ) -> Result<TempFile, DownloadError> {
        let url = check_url(url, self.allow_http)?;
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| DownloadError::Network(describe(e)))?;
        if !response.status().is_success() {
            return Err(DownloadError::Status(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|len| len > self.max_bytes)
        {
            return Err(DownloadError::TooLarge(self.max_bytes));
        }
        let path = dir.join(format!(
            "{}.{}",
            sanitize_component(stem),
            extension(content_type)
        ));
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|e| DownloadError::Io(e.to_string()))?;
        let temp = TempFile { path };
        let label = mime(content_type);
        let mut total = 0u64;
        let mut head = Vec::with_capacity(SIGNATURE_LEN);
        let mut checked = false;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| DownloadError::Network(describe(e)))?
        {
            total += chunk.len() as u64;
            if total > self.max_bytes {
                return Err(DownloadError::TooLarge(self.max_bytes));
            }
            if !checked {
                let missing = SIGNATURE_LEN - head.len();
                head.extend_from_slice(&chunk[..chunk.len().min(missing)]);
                if head.len() >= SIGNATURE_LEN {
                    if !magic_matches(&head, content_type) {
                        return Err(DownloadError::BadMagic(label));
                    }
                    checked = true;
                }
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| DownloadError::Io(e.to_string()))?;
        }
        if !checked && !magic_matches(&head, content_type) {
            return Err(DownloadError::BadMagic(label));
        }
        file.flush()
            .await
            .map_err(|e| DownloadError::Io(e.to_string()))?;
        drop(file);
        let info = probe_file(temp.path()).map_err(|e| DownloadError::Invalid(e.to_string()))?;
        if info.mime != label {
            return Err(DownloadError::BadMagic(label));
        }
        Ok(temp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{StatusCode, header};
    use axum::routing::get;
    use image::{ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;
    use std::net::SocketAddr;

    fn encode(format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        RgbImage::from_pixel(16, 12, Rgb([9, 9, 9]))
            .write_to(&mut out, format)
            .unwrap();
        out.into_inner()
    }

    async fn serve(app: Router) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    fn app() -> Router {
        let png = encode(ImageFormat::Png);
        let jpg = encode(ImageFormat::Jpeg);
        Router::new()
            .route(
                "/png",
                get({
                    let b = png.clone();
                    move || async move { b }
                }),
            )
            .route("/jpg", get(move || async move { jpg }))
            .route("/missing", get(|| async { StatusCode::NOT_FOUND }))
            .route("/big", get(|| async { vec![0x89u8; 4096] }))
            .route(
                "/chunked",
                get(|| async {
                    let chunks = (0..10).map(|i| {
                        let mut chunk = vec![0u8; 1024];
                        if i == 0 {
                            chunk[..8].copy_from_slice(&PNG_SIGNATURE);
                        }
                        Ok::<_, std::io::Error>(chunk)
                    });
                    Body::from_stream(futures_util::stream::iter(chunks))
                }),
            )
            .route(
                "/gif",
                get(|| async { b"GIF89a-not-a-jpeg-or-png".to_vec() }),
            )
            .route("/short", get(|| async { vec![0xFFu8, 0xD8] }))
            .route("/empty", get(|| async { Vec::<u8>::new() }))
            .route(
                "/truncated",
                get({
                    let b = png;
                    move || async move { b[..b.len() / 2].to_vec() }
                }),
            )
            .route(
                "/redirect",
                get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/png")]) }),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    "late"
                }),
            )
            .route(
                "/cut",
                get(|| async {
                    let chunks = vec![
                        Ok::<_, std::io::Error>(vec![
                            0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A,
                        ]),
                        Err(std::io::Error::other("connection cut")),
                    ];
                    Body::from_stream(futures_util::stream::iter(chunks))
                }),
            )
    }

    fn downloader(max: u64) -> Downloader {
        Downloader::new(true, max, Duration::from_secs(5)).unwrap()
    }

    async fn fetch(
        path: &str,
        ct: ContentType,
        max: u64,
    ) -> (Result<TempFile, DownloadError>, tempfile::TempDir) {
        let addr = serve(app()).await;
        let dir = tempfile::tempdir().unwrap();
        let result = downloader(max)
            .fetch(&format!("http://{addr}{path}"), ct, dir.path(), "job1-ph1")
            .await;
        (result, dir)
    }

    fn dir_is_empty(dir: &tempfile::TempDir) -> bool {
        std::fs::read_dir(dir.path()).unwrap().next().is_none()
    }

    #[test]
    fn url_rules() {
        assert!(
            check_url(
                "https://bucket.s3.amazonaws.com/k?X-Amz-Signature=abc",
                false
            )
            .is_ok()
        );
        assert!(check_url("http://127.0.0.1:1/x", true).is_ok());
        assert!(matches!(
            check_url("http://127.0.0.1:1/x", false),
            Err(DownloadError::InsecureScheme)
        ));
        assert!(matches!(
            check_url("ftp://h/x", true),
            Err(DownloadError::InsecureScheme)
        ));
        assert!(matches!(
            check_url("file:///etc/passwd", true),
            Err(DownloadError::BadUrl)
        ));
        assert!(matches!(
            check_url("data:text/plain,hi", true),
            Err(DownloadError::BadUrl)
        ));
        assert!(matches!(
            check_url("not a url", true),
            Err(DownloadError::BadUrl)
        ));
        assert!(matches!(
            check_url("https://", true),
            Err(DownloadError::BadUrl)
        ));
    }

    #[test]
    fn magic_bytes() {
        let png = encode(ImageFormat::Png);
        let jpg = encode(ImageFormat::Jpeg);
        assert!(magic_matches(&png, ContentType::Png));
        assert!(!magic_matches(&png, ContentType::Jpeg));
        assert!(magic_matches(&jpg, ContentType::Jpeg));
        assert!(!magic_matches(&jpg, ContentType::Png));
        assert!(!magic_matches(&[], ContentType::Jpeg));
        assert!(!magic_matches(&[0xFF, 0xD8], ContentType::Jpeg));
        assert!(!magic_matches(&PNG_SIGNATURE[..7], ContentType::Png));
    }

    #[test]
    fn extensions_and_sanitizing() {
        assert_eq!(extension(ContentType::Jpeg), "jpg");
        assert_eq!(extension(ContentType::Png), "png");
        assert_eq!(sanitize_component("job-1_ph"), "job-1_ph");
        assert_eq!(sanitize_component("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize_component(""), "_");
        assert_eq!(sanitize_component("ünï"), "_n_");
    }

    #[test]
    fn error_codes() {
        use PiErrorCode::{DownloadFailed, InvalidImage};
        let cases = [
            (DownloadError::BadUrl, DownloadFailed),
            (DownloadError::InsecureScheme, DownloadFailed),
            (DownloadError::Client("x".into()), DownloadFailed),
            (DownloadError::Network("x".into()), DownloadFailed),
            (DownloadError::Status(404), DownloadFailed),
            (DownloadError::Io("x".into()), DownloadFailed),
            (DownloadError::TooLarge(1), InvalidImage),
            (DownloadError::BadMagic("image/png"), InvalidImage),
            (DownloadError::Invalid("x".into()), InvalidImage),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code, "{error}");
        }
    }

    #[tokio::test]
    async fn downloads_png_and_jpeg_and_cleans_up_on_drop() {
        let (png, dir) = fetch("/png", ContentType::Png, 1 << 20).await;
        let png = png.unwrap();
        assert_eq!(png.path(), dir.path().join("job1-ph1.png"));
        assert_eq!(std::fs::read(png.path()).unwrap(), encode(ImageFormat::Png));
        drop(png);
        assert!(dir_is_empty(&dir));
        let (jpg, dir) = fetch("/jpg", ContentType::Jpeg, 1 << 20).await;
        assert_eq!(jpg.unwrap().path(), dir.path().join("job1-ph1.jpg"));
    }

    #[tokio::test]
    async fn rejects_declared_type_mismatch() {
        let (r, dir) = fetch("/png", ContentType::Jpeg, 1 << 20).await;
        assert!(matches!(r, Err(DownloadError::BadMagic("image/jpeg"))));
        assert!(dir_is_empty(&dir));
    }

    #[tokio::test]
    async fn rejects_non_2xx_and_redirects() {
        for (path, status) in [("/missing", 404), ("/redirect", 302)] {
            let (r, dir) = fetch(path, ContentType::Png, 1 << 20).await;
            assert!(
                matches!(r, Err(DownloadError::Status(s)) if s == status),
                "{path}"
            );
            assert!(dir_is_empty(&dir));
        }
    }

    #[tokio::test]
    async fn enforces_size_cap_by_content_length_and_while_streaming() {
        for path in ["/big", "/chunked"] {
            let (r, dir) = fetch(path, ContentType::Png, 2048).await;
            assert!(matches!(r, Err(DownloadError::TooLarge(2048))), "{path}");
            assert!(dir_is_empty(&dir), "{path}");
        }
    }

    #[tokio::test]
    async fn rejects_bad_magic_short_and_empty_bodies() {
        for path in ["/gif", "/short", "/empty"] {
            let (r, dir) = fetch(path, ContentType::Jpeg, 1 << 20).await;
            assert!(
                matches!(r, Err(DownloadError::BadMagic(_))),
                "{path}: {r:?}"
            );
            assert!(dir_is_empty(&dir), "{path}");
        }
    }

    #[tokio::test]
    async fn truncated_body_with_intact_header_passes_the_header_check() {
        let (r, dir) = fetch("/truncated", ContentType::Png, 1 << 20).await;
        // Only decoding would fail, and that is the printer's job.
        assert!(r.is_ok());
        drop(r);
        assert!(dir_is_empty(&dir));
    }

    #[tokio::test]
    async fn rejects_corrupt_header() {
        // Valid signature, garbage after it.
        let app = Router::new().route(
            "/bad",
            get(|| async { [&PNG_SIGNATURE[..], b"garbage garbage garbage"].concat() }),
        );
        let addr = serve(app).await;
        let dir = tempfile::tempdir().unwrap();
        let r = downloader(1 << 20)
            .fetch(
                &format!("http://{addr}/bad"),
                ContentType::Png,
                dir.path(),
                "j",
            )
            .await;
        assert!(matches!(r, Err(DownloadError::Invalid(_))), "{r:?}");
        assert!(dir_is_empty(&dir));
    }

    #[tokio::test]
    async fn network_failures() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing listens on this port.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let r = downloader(1 << 20)
            .fetch(
                &format!("http://127.0.0.1:{port}/x"),
                ContentType::Png,
                dir.path(),
                "j",
            )
            .await;
        let Err(DownloadError::Network(message)) = r else {
            panic!("{r:?}")
        };
        assert!(!message.contains("/x"), "url leaked: {message}");
        // Body cut mid-stream.
        let (r, dir) = fetch("/cut", ContentType::Png, 1 << 20).await;
        assert!(matches!(r, Err(DownloadError::Network(_))), "{r:?}");
        assert!(dir_is_empty(&dir));
    }

    #[tokio::test]
    async fn times_out_slow_servers() {
        let addr = serve(app()).await;
        let dir = tempfile::tempdir().unwrap();
        let d = Downloader::new(true, 1 << 20, Duration::from_millis(200)).unwrap();
        let r = d
            .fetch(
                &format!("http://{addr}/slow"),
                ContentType::Png,
                dir.path(),
                "j",
            )
            .await;
        assert!(matches!(r, Err(DownloadError::Network(_))), "{r:?}");
    }

    #[tokio::test]
    async fn refuses_http_unless_allowed_and_bad_urls() {
        let dir = tempfile::tempdir().unwrap();
        let strict = Downloader::new(false, 1 << 20, Duration::from_secs(1)).unwrap();
        let r = strict
            .fetch("http://127.0.0.1:9/x", ContentType::Png, dir.path(), "j")
            .await;
        assert!(matches!(r, Err(DownloadError::InsecureScheme)));
    }

    #[tokio::test]
    async fn unwritable_directory_is_io_error() {
        let addr = serve(app()).await;
        let dir = tempfile::tempdir().unwrap();
        let r = downloader(1 << 20)
            .fetch(
                &format!("http://{addr}/png"),
                ContentType::Png,
                &dir.path().join("missing"),
                "j",
            )
            .await;
        assert!(matches!(r, Err(DownloadError::Io(_))), "{r:?}");
    }

    #[tokio::test]
    async fn path_traversal_in_stem_stays_in_dir() {
        let addr = serve(app()).await;
        let dir = tempfile::tempdir().unwrap();
        let f = downloader(1 << 20)
            .fetch(
                &format!("http://{addr}/png"),
                ContentType::Png,
                dir.path(),
                "../../evil",
            )
            .await
            .unwrap();
        assert_eq!(f.path().parent().unwrap(), dir.path());
    }
}
