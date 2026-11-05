//! Image download: https only (http in simulation), allowed hosts, no private addresses, size
//! cap while streaming, magic bytes and header checks, temporary files that clean up after
//! themselves.

use std::error::Error as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fotoko_printer_sim::probe_file;
use fotoko_protocol::{ContentType, PiErrorCode};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::{Client, Url, redirect};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use url::Host;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const JPEG_SIGNATURE: [u8; 3] = [0xFF, 0xD8, 0xFF];
/// Bytes needed to decide a signature.
const SIGNATURE_LEN: usize = PNG_SIGNATURE.len();
/// Longest file stem kept (jobs and photos have uuid-like ids; this only bounds hostile input).
const MAX_STEM_LEN: usize = 120;

/// Why a photo could not be fetched or accepted.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// The URL does not parse or has no host.
    #[error("invalid image url")]
    BadUrl,
    /// The scheme is not allowed.
    #[error("image url must use https")]
    InsecureScheme,
    /// The host is not on the allow list or is a non-public address.
    #[error("image host not allowed: {0}")]
    ForbiddenHost(String),
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
            | Self::ForbiddenHost(_)
            | Self::Client(_)
            | Self::Network(_)
            | Self::Status(_)
            | Self::Io(_) => PiErrorCode::DownloadFailed,
        }
    }

    /// Message safe to send to the cloud: no hosts, addresses, paths or library text. The full
    /// error is for the local log only.
    pub fn public_message(&self) -> &'static str {
        match self.code() {
            PiErrorCode::InvalidImage => "image rejected",
            _ => "download failed",
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

/// Which URLs may be fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlPolicy {
    /// Simulation mode: plain `http` and any host, including loopback, are allowed.
    pub allow_insecure: bool,
    /// Host suffixes (`.amazonaws.com` matches subdomains) or exact host names.
    pub allowed_hosts: Vec<String>,
}

impl UrlPolicy {
    /// Production policy: https only, the given hosts only.
    pub fn strict(allowed_hosts: Vec<String>) -> Self {
        Self {
            allow_insecure: false,
            allowed_hosts,
        }
    }

    /// Simulation policy: anything goes.
    pub fn permissive() -> Self {
        Self {
            allow_insecure: true,
            allowed_hosts: Vec::new(),
        }
    }
}

/// Whether `host` matches an allow-list entry: `.example.com` matches subdomains only,
/// anything else must be equal (case-insensitive).
pub fn host_allowed(host: &str, allowed: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    allowed.iter().any(|entry| {
        let entry = entry.to_ascii_lowercase();
        if entry.starts_with('.') {
            host.ends_with(&entry) && host.len() > entry.len()
        } else {
            host == entry
        }
    })
}

/// Whether an address is publicly routable (not loopback, private, link-local, shared,
/// reserved, multicast or documentation space).
pub fn is_global(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_global_v4(v4),
        IpAddr::V6(v6) => is_global_v6(v6),
    }
}

fn is_global_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // shared address space
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || a >= 240) // reserved
}

fn is_global_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_global_v4(v4);
    }
    let first = ip.segments()[0];
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00 // unique local
        || (first & 0xffc0) == 0xfe80 // link local
        || (first == 0x2001 && ip.segments()[1] == 0x0db8) // documentation
        || (first == 0x0064 && ip.segments()[1] == 0xff9b)) // NAT64 can reach private v4
}

/// Parse an image URL and enforce the policy: scheme, host allow list and IP literals. Host
/// names are checked again for the addresses they resolve to (see [`Downloader`]).
pub fn check_url(url: &str, policy: &UrlPolicy) -> Result<Url, DownloadError> {
    let parsed = Url::parse(url).map_err(|_| DownloadError::BadUrl)?;
    let host = parsed.host().ok_or(DownloadError::BadUrl)?;
    match parsed.scheme() {
        "https" => {}
        "http" if policy.allow_insecure => {}
        _ => return Err(DownloadError::InsecureScheme),
    }
    if policy.allow_insecure {
        return Ok(parsed);
    }
    match host {
        Host::Domain(name) => {
            if !host_allowed(name, &policy.allowed_hosts) {
                return Err(DownloadError::ForbiddenHost(name.to_owned()));
            }
        }
        Host::Ipv4(ip) => {
            if !is_global(IpAddr::V4(ip)) || !host_allowed(&ip.to_string(), &policy.allowed_hosts) {
                return Err(DownloadError::ForbiddenHost(ip.to_string()));
            }
        }
        Host::Ipv6(ip) => {
            if !is_global(IpAddr::V6(ip)) || !host_allowed(&ip.to_string(), &policy.allowed_hosts) {
                return Err(DownloadError::ForbiddenHost(ip.to_string()));
            }
        }
    }
    Ok(parsed)
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

/// Make text safe as a file name component: ASCII alphanumerics, `-` and `_` stay, everything
/// else becomes `_`; the result is never empty and at most `max` characters.
pub fn sanitize_component(text: &str, max: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(max)
        .collect();
    if cleaned.is_empty() {
        "_".to_owned()
    } else {
        cleaned
    }
}

/// File stem for photo number `index` of a job. The index keeps two photos with the same
/// (hostile or buggy) id apart; each id part is cut to a sane length.
pub fn photo_stem(job_id: &str, index: usize, photo_id: &str) -> String {
    let half = (MAX_STEM_LEN - 8) / 2;
    format!(
        "{}-{index}-{}",
        sanitize_component(job_id, half),
        sanitize_component(photo_id, half)
    )
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

/// DNS resolver that refuses to hand out non-public addresses, so a public name that points
/// at the LAN (or flips there after the allow-list check) cannot be fetched.
#[derive(Debug)]
struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let found: Vec<_> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let public: Vec<_> = found
                .iter()
                .copied()
                .filter(|a| is_global(a.ip()))
                .collect();
            if public.is_empty() {
                let reason = if found.is_empty() {
                    "no addresses"
                } else {
                    "only non-public addresses"
                };
                return Err(format!("{host}: {reason}").into());
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

/// HTTP client for presigned image URLs.
#[derive(Debug, Clone)]
pub struct Downloader {
    client: Client,
    policy: UrlPolicy,
    max_bytes: u64,
}

impl Downloader {
    /// `timeout` bounds each whole download. Redirects and proxies are not used.
    pub fn new(
        policy: UrlPolicy,
        max_bytes: u64,
        timeout: Duration,
    ) -> Result<Self, DownloadError> {
        crate::tls::install_crypto_provider();
        let mut builder = Client::builder()
            .no_proxy()
            .redirect(redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10).min(timeout))
            .timeout(timeout)
            .user_agent(concat!("fotoko-pi-agent/", env!("CARGO_PKG_VERSION")));
        if !policy.allow_insecure {
            builder = builder.dns_resolver(Arc::new(PublicOnly));
        }
        let client = builder
            .build()
            .map_err(|e| DownloadError::Client(describe(e)))?;
        Ok(Self {
            client,
            policy,
            max_bytes,
        })
    }

    /// Download `url` into `dir` as `{stem}.{jpg|png}` (never overwriting, mode 0600) and
    /// validate it against `content_type`. On any error nothing is left on disk.
    pub async fn fetch(
        &self,
        url: &str,
        content_type: ContentType,
        dir: &Path,
        stem: &str,
    ) -> Result<TempFile, DownloadError> {
        let url = check_url(url, &self.policy)?;
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
            sanitize_component(stem, MAX_STEM_LEN),
            extension(content_type)
        ));
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&path)
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
        let probe_path = temp.path().to_owned();
        let info = tokio::task::spawn_blocking(move || probe_file(&probe_path))
            .await
            .map_err(|e| DownloadError::Io(e.to_string()))?
            .map_err(|e| DownloadError::Invalid(e.to_string()))?;
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
        Downloader::new(UrlPolicy::permissive(), max, Duration::from_secs(5)).unwrap()
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

    fn strict() -> UrlPolicy {
        UrlPolicy::strict(vec![".amazonaws.com".into(), "files.example.org".into()])
    }

    #[test]
    fn url_rules_in_production() {
        let ok = |u: &str| check_url(u, &strict()).is_ok();
        assert!(ok(
            "https://bucket.s3.ap-southeast-1.amazonaws.com/k?X-Amz-Signature=abc"
        ));
        assert!(ok("https://files.example.org/x"));
        assert!(ok("https://BUCKET.S3.AMAZONAWS.COM./x"));
        let err = |u: &str| check_url(u, &strict()).unwrap_err();
        assert!(matches!(
            err("http://bucket.s3.amazonaws.com/x"),
            DownloadError::InsecureScheme
        ));
        assert!(matches!(err("ftp://h/x"), DownloadError::InsecureScheme));
        assert!(matches!(err("file:///etc/passwd"), DownloadError::BadUrl));
        assert!(matches!(err("data:text/plain,hi"), DownloadError::BadUrl));
        assert!(matches!(err("not a url"), DownloadError::BadUrl));
        assert!(matches!(err("https://"), DownloadError::BadUrl));
        for forbidden in [
            "https://evil.example.com/x",
            "https://amazonaws.com/x",
            "https://notamazonaws.com/x",
            "https://amazonaws.com.evil.io/x",
            "https://s3.amazonaws.com@evil.io/x",
            "https://127.0.0.1/x",
            "https://10.0.0.5/x",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/x",
            "https://[fe80::1]/x",
            "https://8.8.8.8/x",
            "https://localhost/x",
        ] {
            assert!(
                matches!(err(forbidden), DownloadError::ForbiddenHost(_)),
                "{forbidden}"
            );
        }
    }

    #[test]
    fn global_address_classification() {
        let global = [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
            "100.63.0.1",
            "172.32.0.1",
        ];
        let local = [
            "0.0.0.0",
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.255",
            "192.0.0.8",
            "198.18.0.1",
            "198.19.255.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "192.0.2.1",
            "198.51.100.7",
            "203.0.113.9",
            "::",
            "::1",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
        ];
        for ip in global {
            assert!(is_global(ip.parse().unwrap()), "{ip}");
        }
        for ip in local {
            assert!(!is_global(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_global("::ffff:8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn host_matching() {
        let allowed = vec![".amazonaws.com".to_owned(), "Files.Example.org".to_owned()];
        assert!(host_allowed("a.amazonaws.com", &allowed));
        assert!(host_allowed("a.b.amazonaws.com.", &allowed));
        assert!(host_allowed("files.example.org", &allowed));
        assert!(!host_allowed("amazonaws.com", &allowed));
        assert!(!host_allowed("xamazonaws.com", &allowed));
        assert!(!host_allowed("evil.files.example.org", &allowed));
        assert!(!host_allowed("a.amazonaws.com", &[]));
    }

    #[test]
    fn permissive_policy_allows_http_and_loopback() {
        let p = UrlPolicy::permissive();
        assert!(check_url("http://127.0.0.1:1/x", &p).is_ok());
        assert!(check_url("https://localhost/x", &p).is_ok());
        assert!(matches!(
            check_url("ftp://h/x", &p),
            Err(DownloadError::InsecureScheme)
        ));
        assert!(matches!(
            check_url("data:x", &p),
            Err(DownloadError::BadUrl)
        ));
    }

    #[test]
    fn public_messages_hide_details() {
        let secret = "https://bucket.s3.amazonaws.com/k?X-Amz-Signature=SECRET 10.0.0.1";
        for error in [
            DownloadError::Network(secret.into()),
            DownloadError::ForbiddenHost(secret.into()),
            DownloadError::Io(secret.into()),
            DownloadError::Status(500),
            DownloadError::BadUrl,
            DownloadError::InsecureScheme,
            DownloadError::Client(secret.into()),
        ] {
            assert_eq!(error.public_message(), "download failed");
        }
        for error in [
            DownloadError::Invalid(secret.into()),
            DownloadError::TooLarge(1),
            DownloadError::BadMagic("image/png"),
        ] {
            assert_eq!(error.public_message(), "image rejected");
        }
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
        assert_eq!(sanitize_component("job-1_ph", 50), "job-1_ph");
        assert_eq!(
            sanitize_component("../../etc/passwd", 50),
            "______etc_passwd"
        );
        assert_eq!(sanitize_component("", 50), "_");
        assert_eq!(sanitize_component("ünï", 50), "_n_");
        assert_eq!(sanitize_component("abcdef", 3), "abc");
    }

    #[test]
    fn photo_stems_are_unique_bounded_and_safe() {
        assert_eq!(photo_stem("job1", 0, "p1"), "job1-0-p1");
        assert_ne!(photo_stem("job1", 0, "same"), photo_stem("job1", 1, "same"));
        let long = "x".repeat(500);
        assert!(photo_stem(&long, 12, &long).len() <= MAX_STEM_LEN);
        assert_eq!(photo_stem("../a", 3, "b/c"), "___a-3-b_c");
    }

    #[test]
    fn error_codes() {
        use PiErrorCode::{DownloadFailed, InvalidImage};
        let cases = [
            (DownloadError::ForbiddenHost("x".into()), DownloadFailed),
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
        let d =
            Downloader::new(UrlPolicy::permissive(), 1 << 20, Duration::from_millis(200)).unwrap();
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
        let strict = Downloader::new(
            UrlPolicy::strict(vec![".amazonaws.com".into()]),
            1 << 20,
            Duration::from_secs(1),
        )
        .unwrap();
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
    async fn never_overwrites_an_existing_file_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let addr = serve(app()).await;
        let dir = tempfile::tempdir().unwrap();
        let d = downloader(1 << 20);
        let url = format!("http://{addr}/png");
        let first = d
            .fetch(&url, ContentType::Png, dir.path(), "same")
            .await
            .unwrap();
        let mode = std::fs::metadata(first.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let second = d.fetch(&url, ContentType::Png, dir.path(), "same").await;
        assert!(matches!(second, Err(DownloadError::Io(_))), "{second:?}");
        assert!(
            first.path().exists(),
            "the first file must survive the collision"
        );
    }

    #[tokio::test]
    async fn resolver_refuses_names_that_resolve_to_private_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let policy = UrlPolicy::strict(vec!["localhost".into()]);
        let d = Downloader::new(policy, 1 << 20, Duration::from_secs(5)).unwrap();
        let r = d
            .fetch("https://localhost:9/x", ContentType::Png, dir.path(), "j")
            .await;
        let Err(DownloadError::Network(message)) = r else {
            panic!("{r:?}")
        };
        assert!(message.contains("non-public"), "{message}");
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
