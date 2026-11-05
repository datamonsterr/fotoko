//! Print job handling: the contract rules from `docs/contracts.md` in one place.
//!
//! For each message on the jobs topic: ignore malformed payloads, other printers' jobs and
//! duplicates; then report `received`, `printing`, and finally `completed` or `failed` while
//! downloading every photo first and printing afterwards, so a bad photo fails the job before
//! any paper is used.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use fotoko_protocol::{
    COPIES_RANGE, JobState, JobStatus, MAX_PHOTOS_PER_ORDER, PiError, PiErrorCode, PrintJob,
    PrinterId,
};
use tracing::{info, warn};

use crate::dedupe::DedupeStore;
use crate::download::{Downloader, TempFile};
use crate::mqtt::Sink;
use crate::printer::Printer;

/// What happened to one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handled {
    /// Not a valid print job; dropped.
    Malformed,
    /// Addressed to another printer; dropped.
    WrongPrinter,
    /// Job id seen before; dropped.
    Duplicate,
    /// All copies printed.
    Completed {
        /// Copies printed.
        printed: u32,
    },
    /// The job failed.
    Failed {
        /// Copies printed before the failure.
        printed: u32,
        /// Reported error.
        error: PiError,
    },
}

/// Processes print jobs one at a time.
pub struct JobHandler {
    printer_id: PrinterId,
    printer: Arc<dyn Printer>,
    sink: Arc<dyn Sink>,
    downloader: Downloader,
    dedupe: DedupeStore,
    tmp_dir: PathBuf,
}

impl std::fmt::Debug for JobHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobHandler")
            .field("printer_id", &self.printer_id)
            .finish_non_exhaustive()
    }
}

fn failure(code: PiErrorCode, message: impl Into<String>) -> PiError {
    PiError {
        code,
        message: message.into(),
    }
}

impl JobHandler {
    /// Create a handler. Downloads go to `tmp_dir`, which must exist.
    pub fn new(
        printer_id: PrinterId,
        printer: Arc<dyn Printer>,
        sink: Arc<dyn Sink>,
        downloader: Downloader,
        dedupe: DedupeStore,
        tmp_dir: PathBuf,
    ) -> Self {
        Self {
            printer_id,
            printer,
            sink,
            downloader,
            dedupe,
            tmp_dir,
        }
    }

    async fn report(&self, job: &PrintJob, state: JobState, printed: u32, error: Option<PiError>) {
        let status = JobStatus {
            job_id: job.job_id.clone(),
            order_id: job.order_id.clone(),
            printer_id: self.printer_id.clone(),
            state,
            printed_copies: printed,
            error,
            at: Utc::now(),
        };
        if let Err(error) = self.sink.status(&status).await {
            warn!(job_id = %job.job_id, ?state, %error, "cannot publish job status");
        }
    }

    /// Handle one message from the jobs topic.
    pub async fn handle(&mut self, payload: &[u8]) -> Handled {
        let job: PrintJob = match serde_json::from_slice(payload) {
            Ok(job) => job,
            Err(error) => {
                warn!(%error, "ignoring malformed job message");
                return Handled::Malformed;
            }
        };
        if job.printer_id != self.printer_id {
            warn!(job_id = %job.job_id, target = %job.printer_id, "ignoring job for another printer");
            return Handled::WrongPrinter;
        }
        match self.dedupe.record(job.job_id.as_str()) {
            Ok(true) => {}
            Ok(false) => {
                info!(job_id = %job.job_id, "ignoring duplicate job");
                return Handled::Duplicate;
            }
            Err(error) => warn!(job_id = %job.job_id, %error, "cannot persist job id, continuing"),
        }
        info!(job_id = %job.job_id, order_id = %job.order_id, photos = job.photos.len(), "job received");
        self.report(&job, JobState::Received, 0, None).await;

        if let Some(error) = validate(&job) {
            self.report(&job, JobState::Failed, 0, Some(error.clone()))
                .await;
            return Handled::Failed { printed: 0, error };
        }
        self.report(&job, JobState::Printing, 0, None).await;

        let mut printed = 0;
        match self.run(&job, &mut printed).await {
            Ok(()) => {
                info!(job_id = %job.job_id, printed, "job completed");
                self.report(&job, JobState::Completed, printed, None).await;
                Handled::Completed { printed }
            }
            Err(error) => {
                warn!(job_id = %job.job_id, code = ?error.code, message = %error.message, "job failed");
                self.report(&job, JobState::Failed, printed, Some(error.clone()))
                    .await;
                Handled::Failed { printed, error }
            }
        }
    }

    async fn run(&self, job: &PrintJob, printed: &mut u32) -> Result<(), PiError> {
        let mut files: Vec<(TempFile, u32)> = Vec::with_capacity(job.photos.len());
        for photo in &job.photos {
            let stem = format!("{}-{}", job.job_id, photo.photo_id);
            let file = self
                .downloader
                .fetch(&photo.url, photo.content_type, &self.tmp_dir, &stem)
                .await
                .map_err(|e| failure(e.code(), e.to_string()))?;
            files.push((file, photo.copies));
        }
        for (file, copies) in &files {
            self.printer
                .print(file.path(), *copies, job.media)
                .await
                .map_err(|e| e.to_pi_error())?;
            *printed += copies;
        }
        Ok(())
    }
}

/// Reject jobs the cloud should never send.
fn validate(job: &PrintJob) -> Option<PiError> {
    if job.photos.is_empty() {
        return Some(failure(PiErrorCode::InvalidImage, "job has no photos"));
    }
    if job.photos.len() > MAX_PHOTOS_PER_ORDER {
        return Some(failure(
            PiErrorCode::InvalidImage,
            format!(
                "job has {} photos, at most {MAX_PHOTOS_PER_ORDER} allowed",
                job.photos.len()
            ),
        ));
    }
    job.photos
        .iter()
        .find(|p| !COPIES_RANGE.contains(&p.copies))
        .map(|p| {
            failure(
                PiErrorCode::InvalidImage,
                format!(
                    "photo {} has {} copies, expected {}..={}",
                    p.photo_id,
                    p.copies,
                    COPIES_RANGE.start(),
                    COPIES_RANGE.end()
                ),
            )
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::mqtt::MqttError;
    use crate::printer::PrintError;
    use async_trait::async_trait;
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::get;
    use fotoko_protocol::{ContentType, Heartbeat, JobPhoto, Media, PrinterState};
    use image::{ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    pub struct Recorder {
        pub statuses: Mutex<Vec<JobStatus>>,
        pub fail: Mutex<bool>,
    }

    #[async_trait]
    impl Sink for Recorder {
        async fn status(&self, status: &JobStatus) -> Result<(), MqttError> {
            if *self.fail.lock().unwrap() {
                return Err(MqttError::Publish("down".into()));
            }
            self.statuses.lock().unwrap().push(status.clone());
            Ok(())
        }

        fn heartbeat(&self, _: &Heartbeat) -> Result<(), MqttError> {
            Ok(())
        }
    }

    impl Recorder {
        fn states(&self) -> Vec<JobState> {
            self.statuses
                .lock()
                .unwrap()
                .iter()
                .map(|s| s.state)
                .collect()
        }
    }

    #[derive(Default)]
    struct FakePrinter {
        printed: Mutex<Vec<(PathBuf, u32)>>,
        fail_on_call: Mutex<Option<(usize, PrintError)>>,
    }

    #[async_trait]
    impl Printer for FakePrinter {
        async fn print(&self, file: &Path, copies: u32, _: Media) -> Result<(), PrintError> {
            let mut printed = self.printed.lock().unwrap();
            let call = printed.len();
            if let Some((n, error)) = self.fail_on_call.lock().unwrap().clone() {
                if n == call {
                    return Err(error);
                }
            }
            assert!(file.exists(), "file must exist while printing");
            printed.push((file.to_owned(), copies));
            Ok(())
        }

        async fn state(&self) -> PrinterState {
            PrinterState::Idle
        }
    }

    fn png() -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        RgbImage::from_pixel(16, 12, Rgb([1, 1, 1]))
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    struct Fixture {
        handler: JobHandler,
        sink: Arc<Recorder>,
        printer: Arc<FakePrinter>,
        tmp: tempfile::TempDir,
        base: String,
        _state: tempfile::TempDir,
    }

    async fn fixture() -> Fixture {
        let png = png();
        let app = Router::new()
            .route(
                "/a.png",
                get({
                    let b = png.clone();
                    move || async move { b }
                }),
            )
            .route("/b.png", get(move || async move { png }))
            .route("/gone", get(|| async { StatusCode::NOT_FOUND }))
            .route("/text", get(|| async { "hello hello hello" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let sink = Arc::new(Recorder::default());
        let printer = Arc::new(FakePrinter::default());
        let handler = JobHandler::new(
            PrinterId::new("stand-001").unwrap(),
            printer.clone(),
            sink.clone(),
            Downloader::new(true, 1 << 20, Duration::from_secs(5)).unwrap(),
            DedupeStore::open(state.path()).unwrap(),
            tmp.path().to_owned(),
        );
        Fixture {
            handler,
            sink,
            printer,
            tmp,
            base,
            _state: state,
        }
    }

    fn job(base: &str, id: &str, printer: &str, photos: &[(&str, ContentType, u32)]) -> Vec<u8> {
        let job = PrintJob {
            job_id: id.into(),
            order_id: "o1".into(),
            printer_id: PrinterId::new(printer).unwrap(),
            media: Media::Photo4x6,
            created_at: Utc::now(),
            photos: photos
                .iter()
                .enumerate()
                .map(|(i, (path, ct, copies))| JobPhoto {
                    photo_id: format!("p{i}").into(),
                    url: format!("{base}{path}"),
                    content_type: *ct,
                    copies: *copies,
                })
                .collect(),
        };
        serde_json::to_vec(&job).unwrap()
    }

    fn tmp_is_empty(f: &Fixture) -> bool {
        std::fs::read_dir(f.tmp.path()).unwrap().next().is_none()
    }

    #[tokio::test]
    async fn happy_path_reports_full_sequence() {
        let mut f = fixture().await;
        let payload = job(
            &f.base,
            "j1",
            "stand-001",
            &[
                ("/a.png", ContentType::Png, 2),
                ("/b.png", ContentType::Png, 3),
            ],
        );
        assert_eq!(
            f.handler.handle(&payload).await,
            Handled::Completed { printed: 5 }
        );
        assert_eq!(
            f.sink.states(),
            [JobState::Received, JobState::Printing, JobState::Completed]
        );
        let statuses = f.sink.statuses.lock().unwrap().clone();
        assert_eq!(statuses[2].printed_copies, 5);
        assert_eq!(statuses[0].printed_copies, 0);
        assert!(
            statuses
                .iter()
                .all(|s| s.error.is_none() && s.printer_id.as_str() == "stand-001")
        );
        assert_eq!(statuses[0].job_id.as_str(), "j1");
        assert_eq!(statuses[0].order_id.as_str(), "o1");
        let printed = f.printer.printed.lock().unwrap().clone();
        assert_eq!(printed.iter().map(|(_, c)| *c).collect::<Vec<_>>(), [2, 3]);
        assert!(printed[0].0.ends_with("j1-p0.png"));
        assert!(tmp_is_empty(&f), "temp files are cleaned");
    }

    #[tokio::test]
    async fn malformed_payloads_are_dropped_silently() {
        let mut f = fixture().await;
        for payload in [&b"not json"[..], b"{}", b"[]", b""] {
            assert_eq!(f.handler.handle(payload).await, Handled::Malformed);
        }
        assert!(f.sink.statuses.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn other_printers_jobs_are_ignored() {
        let mut f = fixture().await;
        let payload = job(
            &f.base,
            "j1",
            "stand-002",
            &[("/a.png", ContentType::Png, 1)],
        );
        assert_eq!(f.handler.handle(&payload).await, Handled::WrongPrinter);
        assert!(f.sink.statuses.lock().unwrap().is_empty());
        assert!(f.printer.printed.lock().unwrap().is_empty());
        // A wrong-printer job does not burn its job id.
        let right = job(
            &f.base,
            "j1",
            "stand-001",
            &[("/a.png", ContentType::Png, 1)],
        );
        assert_eq!(
            f.handler.handle(&right).await,
            Handled::Completed { printed: 1 }
        );
    }

    #[tokio::test]
    async fn duplicate_job_ids_are_ignored_even_after_failure() {
        let mut f = fixture().await;
        let payload = job(
            &f.base,
            "j1",
            "stand-001",
            &[("/gone", ContentType::Png, 1)],
        );
        assert!(matches!(
            f.handler.handle(&payload).await,
            Handled::Failed { .. }
        ));
        let seen = f.sink.statuses.lock().unwrap().len();
        assert_eq!(f.handler.handle(&payload).await, Handled::Duplicate);
        assert_eq!(f.sink.statuses.lock().unwrap().len(), seen);
    }

    #[tokio::test]
    async fn download_failure_reports_failed_after_printing() {
        let mut f = fixture().await;
        let payload = job(
            &f.base,
            "j1",
            "stand-001",
            &[
                ("/a.png", ContentType::Png, 1),
                ("/gone", ContentType::Png, 1),
            ],
        );
        let Handled::Failed { printed, error } = f.handler.handle(&payload).await else {
            panic!()
        };
        assert_eq!(printed, 0);
        assert_eq!(error.code, PiErrorCode::DownloadFailed);
        assert!(error.message.contains("404"));
        assert_eq!(
            f.sink.states(),
            [JobState::Received, JobState::Printing, JobState::Failed]
        );
        assert!(
            f.printer.printed.lock().unwrap().is_empty(),
            "nothing prints when a later photo fails"
        );
        assert!(tmp_is_empty(&f));
        let last = f.sink.statuses.lock().unwrap().pop().unwrap();
        assert_eq!(last.error.unwrap().code, PiErrorCode::DownloadFailed);
    }

    #[tokio::test]
    async fn wrong_content_is_invalid_image() {
        let mut f = fixture().await;
        let payload = job(
            &f.base,
            "j1",
            "stand-001",
            &[("/text", ContentType::Jpeg, 1)],
        );
        let Handled::Failed { error, .. } = f.handler.handle(&payload).await else {
            panic!()
        };
        assert_eq!(error.code, PiErrorCode::InvalidImage);
    }

    #[tokio::test]
    async fn print_failure_reports_copies_printed_so_far() {
        let mut f = fixture().await;
        *f.printer.fail_on_call.lock().unwrap() = Some((1, PrintError::PaperOut));
        let payload = job(
            &f.base,
            "j1",
            "stand-001",
            &[
                ("/a.png", ContentType::Png, 2),
                ("/b.png", ContentType::Png, 1),
            ],
        );
        let Handled::Failed { printed, error } = f.handler.handle(&payload).await else {
            panic!()
        };
        assert_eq!(printed, 2);
        assert_eq!(error.code, PiErrorCode::PaperOut);
        let last = f.sink.statuses.lock().unwrap().pop().unwrap();
        assert_eq!((last.state, last.printed_copies), (JobState::Failed, 2));
        assert!(tmp_is_empty(&f));
    }

    #[tokio::test]
    async fn invalid_jobs_fail_without_printing() {
        let mut f = fixture().await;
        let cases: [(&str, Vec<(&str, ContentType, u32)>, &str); 4] = [
            ("empty", vec![], "no photos"),
            (
                "many",
                (0..11).map(|_| ("/a.png", ContentType::Png, 1)).collect(),
                "11 photos",
            ),
            ("zero", vec![("/a.png", ContentType::Png, 0)], "0 copies"),
            ("six", vec![("/a.png", ContentType::Png, 6)], "6 copies"),
        ];
        for (id, photos, expect) in cases {
            let Handled::Failed { printed, error } = f
                .handler
                .handle(&job(&f.base, id, "stand-001", &photos))
                .await
            else {
                panic!("{id}")
            };
            assert_eq!(printed, 0);
            assert_eq!(error.code, PiErrorCode::InvalidImage);
            assert!(error.message.contains(expect), "{}", error.message);
        }
        assert!(f.printer.printed.lock().unwrap().is_empty());
        assert_eq!(
            f.sink
                .states()
                .iter()
                .filter(|s| **s == JobState::Printing)
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn publish_failures_do_not_stop_the_job() {
        let mut f = fixture().await;
        *f.sink.fail.lock().unwrap() = true;
        let payload = job(
            &f.base,
            "j1",
            "stand-001",
            &[("/a.png", ContentType::Png, 1)],
        );
        assert_eq!(
            f.handler.handle(&payload).await,
            Handled::Completed { printed: 1 }
        );
    }

    #[tokio::test]
    async fn unpersistable_dedupe_does_not_block_printing() {
        let mut f = fixture().await;
        f.handler.dedupe = DedupeStore::unwritable();
        let payload = job(
            &f.base,
            "j1",
            "stand-001",
            &[("/a.png", ContentType::Png, 1)],
        );
        assert_eq!(
            f.handler.handle(&payload).await,
            Handled::Completed { printed: 1 }
        );
    }
}
