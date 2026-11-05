//! Print job handling: the contract rules from `docs/contracts.md` in one place.
//!
//! For each message on the jobs topic: ignore malformed payloads, other printers' jobs and
//! duplicates; then report `received`, `printing`, and finally `completed` or `failed` while
//! downloading every photo first and printing afterwards, so a bad photo fails the job before
//! any paper is used. Every accepted job is entered in the ledger and closed with its outcome.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use fotoko_protocol::{
    COPIES_RANGE, JobState, JobStatus, MAX_PHOTOS_PER_ORDER, PiError, PiErrorCode, PrintJob,
    PrinterId,
};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::dedupe::{DedupeStore, Outcome};
use crate::download::{Downloader, TempFile, photo_stem};
use crate::mqtt::Sink;
use crate::printer::{PrintContext, Printer};

/// How long publishing one status may take before it is given up on.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// What happened to one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handled {
    /// Not a valid print job; dropped.
    Malformed,
    /// Addressed to another printer; dropped.
    WrongPrinter,
    /// Job id in progress or already done; dropped.
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
    store: Arc<Mutex<DedupeStore>>,
    tmp_dir: PathBuf,
    cancel: CancellationToken,
    status_timeout: Duration,
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

fn shutting_down() -> PiError {
    failure(PiErrorCode::DriverError, "agent shutting down")
}

impl JobHandler {
    /// Create a handler. Downloads go to `tmp_dir`, which must exist. `cancel` withdraws the
    /// job in progress (downloads and prints) when it fires.
    pub fn new(
        printer_id: PrinterId,
        printer: Arc<dyn Printer>,
        sink: Arc<dyn Sink>,
        downloader: Downloader,
        store: DedupeStore,
        tmp_dir: PathBuf,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            printer_id,
            printer,
            sink,
            downloader,
            store: Arc::new(Mutex::new(store)),
            tmp_dir,
            cancel,
            status_timeout: STATUS_TIMEOUT,
        }
    }

    /// Change how long a status publish may take.
    #[must_use]
    pub fn with_status_timeout(mut self, status_timeout: Duration) -> Self {
        self.status_timeout = status_timeout;
        self
    }

    /// Run a ledger operation off the async threads (it syncs to disk). `None` if it panicked.
    async fn ledger<R: Send + 'static>(
        &self,
        op: impl FnOnce(&mut DedupeStore) -> R + Send + 'static,
    ) -> Option<R> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            let mut guard = store.lock().unwrap_or_else(|e| e.into_inner());
            op(&mut guard)
        })
        .await
        .ok()
    }

    async fn report(
        &self,
        job_id: &str,
        order_id: &str,
        state: JobState,
        printed: u32,
        error: Option<PiError>,
    ) {
        let status = JobStatus {
            job_id: job_id.into(),
            order_id: order_id.into(),
            printer_id: self.printer_id.clone(),
            state,
            printed_copies: printed,
            error,
            at: Utc::now(),
        };
        match timeout(self.status_timeout, self.sink.status(&status)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!(%job_id, ?state, %error, "cannot publish job status"),
            Err(_) => warn!(%job_id, ?state, "publishing job status timed out"),
        }
    }

    async fn finish(&self, job_id: &str, order_id: &str, outcome: Outcome) {
        let (job, order) = (job_id.to_owned(), order_id.to_owned());
        match self.ledger(move |s| s.finish(&job, &order, outcome)).await {
            Some(Ok(())) => {}
            Some(Err(error)) => warn!(%job_id, %error, "cannot record job outcome"),
            None => warn!(%job_id, "cannot record job outcome"),
        }
    }

    /// After a restart: report every job the previous run accepted but never finished as failed,
    /// and close it in the ledger. Returns how many there were.
    pub async fn recover(&self) -> usize {
        let unfinished = self.ledger(|s| s.unfinished()).await.unwrap_or_default();
        for job in &unfinished {
            warn!(job_id = %job.job_id, "job was interrupted by an agent restart");
            let error = failure(PiErrorCode::DriverError, "agent restarted during job");
            self.finish(
                &job.job_id,
                &job.order_id,
                Outcome::Failed(PiErrorCode::DriverError),
            )
            .await;
            self.report(&job.job_id, &job.order_id, JobState::Failed, 0, Some(error))
                .await;
        }
        unfinished.len()
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
        let (job_id, order_id) = (job.job_id.to_string(), job.order_id.to_string());
        let (j, o) = (job_id.clone(), order_id.clone());
        match self.ledger(move |s| s.begin(&j, &o)).await {
            Some(Ok(true)) => {}
            Some(Ok(false)) => {
                info!(%job_id, "ignoring duplicate job");
                return Handled::Duplicate;
            }
            Some(Err(error)) => warn!(%job_id, %error, "cannot persist job, continuing"),
            None => warn!(%job_id, "cannot persist job, continuing"),
        }
        info!(%job_id, %order_id, photos = job.photos.len(), "job received");
        self.report(&job_id, &order_id, JobState::Received, 0, None)
            .await;

        let mut printed = 0;
        let result = match validate(&job) {
            Some(error) => Err(error),
            None => {
                self.report(&job_id, &order_id, JobState::Printing, 0, None)
                    .await;
                self.run(&job, &mut printed).await
            }
        };
        match result {
            Ok(()) => {
                info!(%job_id, printed, "job completed");
                self.finish(&job_id, &order_id, Outcome::Completed).await;
                self.report(&job_id, &order_id, JobState::Completed, printed, None)
                    .await;
                Handled::Completed { printed }
            }
            Err(error) => {
                warn!(%job_id, code = ?error.code, message = %error.message, "job failed");
                self.finish(&job_id, &order_id, Outcome::Failed(error.code))
                    .await;
                self.report(
                    &job_id,
                    &order_id,
                    JobState::Failed,
                    printed,
                    Some(error.clone()),
                )
                .await;
                Handled::Failed { printed, error }
            }
        }
    }

    async fn run(&self, job: &PrintJob, printed: &mut u32) -> Result<(), PiError> {
        let mut files: Vec<(TempFile, u32)> = Vec::with_capacity(job.photos.len());
        for (index, photo) in job.photos.iter().enumerate() {
            let stem = photo_stem(job.job_id.as_str(), index, photo.photo_id.as_str());
            let fetched = tokio::select! {
                () = self.cancel.cancelled() => return Err(shutting_down()),
                fetched = self.downloader.fetch(&photo.url, photo.content_type, &self.tmp_dir, &stem) => fetched,
            };
            let file = fetched.map_err(|error| {
                // Details (hosts, addresses, library text) stay in the local log.
                warn!(job_id = %job.job_id, photo_id = %photo.photo_id, %error, "download rejected");
                failure(error.code(), error.public_message())
            })?;
            files.push((file, photo.copies));
        }
        let ctx = PrintContext {
            title: job.job_id.to_string(),
            cancel: self.cancel.clone(),
        };
        for (file, copies) in &files {
            self.printer
                .print(file.path(), *copies, job.media, &ctx)
                .await
                .map_err(|e| e.to_pi_error())?;
            *printed += copies;
        }
        Ok(())
    }
}

/// Publish `failed` for a job message that cannot be processed (for example the queue is full).
/// Messages that are not jobs for this printer are ignored.
pub async fn reject(sink: &dyn Sink, printer_id: &PrinterId, payload: &[u8], message: &str) {
    let Ok(job) = serde_json::from_slice::<PrintJob>(payload) else {
        return;
    };
    if job.printer_id != *printer_id {
        return;
    }
    let status = JobStatus {
        job_id: job.job_id,
        order_id: job.order_id,
        printer_id: printer_id.clone(),
        state: JobState::Failed,
        printed_copies: 0,
        error: Some(failure(PiErrorCode::DriverError, message)),
        at: Utc::now(),
    };
    if let Err(error) = sink.status(&status).await {
        warn!(%error, "cannot publish rejection");
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
    let mut seen = HashSet::new();
    for photo in &job.photos {
        if !seen.insert(photo.photo_id.as_str()) {
            return Some(failure(
                PiErrorCode::InvalidImage,
                format!("duplicate photo id {}", photo.photo_id),
            ));
        }
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
    use crate::download::UrlPolicy;
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
    use std::sync::atomic::{AtomicU32, Ordering};

    #[derive(Default)]
    pub struct Recorder {
        pub statuses: Mutex<Vec<JobStatus>>,
        pub fail: Mutex<bool>,
        pub hang: Mutex<bool>,
    }

    #[async_trait]
    impl Sink for Recorder {
        async fn status(&self, status: &JobStatus) -> Result<(), MqttError> {
            if *self.hang.lock().unwrap() {
                std::future::pending::<()>().await;
            }
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
        printed: Mutex<Vec<(PathBuf, u32, String)>>,
        fail_on_call: Mutex<Option<(usize, PrintError)>>,
        wait_for_cancel: Mutex<bool>,
    }

    #[async_trait]
    impl Printer for FakePrinter {
        async fn print(
            &self,
            file: &Path,
            copies: u32,
            _: Media,
            ctx: &PrintContext,
        ) -> Result<(), PrintError> {
            if *self.wait_for_cancel.lock().unwrap() {
                ctx.cancel.cancelled().await;
                return Err(PrintError::Cancelled);
            }
            let mut printed = self.printed.lock().unwrap();
            let call = printed.len();
            if let Some((n, error)) = self.fail_on_call.lock().unwrap().clone()
                && n == call
            {
                return Err(error);
            }
            assert!(file.exists(), "file must exist while printing");
            printed.push((file.to_owned(), copies, ctx.title.clone()));
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
        cancel: CancellationToken,
        tmp: tempfile::TempDir,
        base: String,
        state: tempfile::TempDir,
    }

    async fn fixture() -> Fixture {
        let png = png();
        let flaky = Arc::new(AtomicU32::new(0));
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
            .route(
                "/flaky",
                get({
                    let flaky = Arc::clone(&flaky);
                    let ok = self::png();
                    move || {
                        let (flaky, ok) = (Arc::clone(&flaky), ok.clone());
                        async move {
                            if flaky.fetch_add(1, Ordering::SeqCst) == 0 {
                                Err(StatusCode::FORBIDDEN)
                            } else {
                                Ok(ok)
                            }
                        }
                    }
                }),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    "late"
                }),
            )
            .route("/text", get(|| async { "hello hello hello" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let sink = Arc::new(Recorder::default());
        let printer = Arc::new(FakePrinter::default());
        let cancel = CancellationToken::new();
        let handler = JobHandler::new(
            PrinterId::new("stand-001").unwrap(),
            printer.clone(),
            sink.clone(),
            Downloader::new(UrlPolicy::permissive(), 1 << 20, Duration::from_secs(5)).unwrap(),
            DedupeStore::open(state.path()).unwrap(),
            tmp.path().to_owned(),
            cancel.clone(),
        );
        Fixture {
            handler,
            sink,
            printer,
            cancel,
            tmp,
            base,
            state,
        }
    }

    fn job(base: &str, id: &str, printer: &str, photos: &[(&str, ContentType, u32)]) -> Vec<u8> {
        let job = PrintJob {
            job_id: id.into(),
            order_id: format!("order-{id}").into(),
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

    fn one(base: &str, id: &str) -> Vec<u8> {
        job(base, id, "stand-001", &[("/a.png", ContentType::Png, 1)])
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
        assert_eq!(statuses[0].order_id.as_str(), "order-j1");
        let printed = f.printer.printed.lock().unwrap().clone();
        assert_eq!(
            printed.iter().map(|(_, c, _)| *c).collect::<Vec<_>>(),
            [2, 3]
        );
        assert!(printed[0].0.ends_with("j1-0-p0.png"), "{:?}", printed[0].0);
        assert!(printed[1].0.ends_with("j1-1-p1.png"));
        assert!(printed.iter().all(|(_, _, title)| title == "j1"));
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
        assert_eq!(
            f.handler.handle(&one(&f.base, "j1")).await,
            Handled::Completed { printed: 1 }
        );
    }

    #[tokio::test]
    async fn finished_and_in_progress_jobs_are_duplicates() {
        let mut f = fixture().await;
        assert!(matches!(
            f.handler.handle(&one(&f.base, "j1")).await,
            Handled::Completed { .. }
        ));
        let seen = f.sink.statuses.lock().unwrap().len();
        assert_eq!(
            f.handler.handle(&one(&f.base, "j1")).await,
            Handled::Duplicate
        );
        assert_eq!(f.sink.statuses.lock().unwrap().len(), seen);
        // In progress: accepted in the ledger, not finished.
        f.handler
            .ledger(|s| s.begin("busy", "order-busy"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            f.handler.handle(&one(&f.base, "busy")).await,
            Handled::Duplicate
        );
    }

    #[tokio::test]
    async fn non_download_failures_stay_duplicates() {
        let mut f = fixture().await;
        let bad = job(
            &f.base,
            "j1",
            "stand-001",
            &[("/text", ContentType::Jpeg, 1)],
        );
        assert!(matches!(
            f.handler.handle(&bad).await,
            Handled::Failed { .. }
        ));
        assert_eq!(f.handler.handle(&bad).await, Handled::Duplicate);
        *f.printer.fail_on_call.lock().unwrap() = Some((0, PrintError::PaperOut));
        let paper = one(&f.base, "j2");
        assert!(matches!(
            f.handler.handle(&paper).await,
            Handled::Failed { .. }
        ));
        assert_eq!(f.handler.handle(&paper).await, Handled::Duplicate);
    }

    #[tokio::test]
    async fn a_job_that_failed_to_download_may_be_republished() {
        let mut f = fixture().await;
        let payload = job(
            &f.base,
            "job-order-1",
            "stand-001",
            &[("/flaky", ContentType::Png, 1)],
        );
        let Handled::Failed { error, .. } = f.handler.handle(&payload).await else {
            panic!()
        };
        assert_eq!(error.code, PiErrorCode::DownloadFailed);
        // The cloud republishes the same job id with a fresh URL (here the same one works now).
        assert_eq!(
            f.handler.handle(&payload).await,
            Handled::Completed { printed: 1 }
        );
        assert_eq!(f.handler.handle(&payload).await, Handled::Duplicate);
        assert_eq!(
            f.sink.states(),
            [
                JobState::Received,
                JobState::Printing,
                JobState::Failed,
                JobState::Received,
                JobState::Printing,
                JobState::Completed,
            ]
        );
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
        assert_eq!(
            error.message, "download failed",
            "no details leak to the cloud"
        );
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
    async fn wrong_content_is_invalid_image_without_details() {
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
        assert_eq!(error.message, "image rejected");
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
        type Photos<'a> = Vec<(&'a str, ContentType, u32)>;
        let cases: [(&str, Photos, &str); 4] = [
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
    async fn duplicate_photo_ids_are_rejected() {
        let mut f = fixture().await;
        let mut value: serde_json::Value = serde_json::from_slice(&job(
            &f.base,
            "dup",
            "stand-001",
            &[
                ("/a.png", ContentType::Png, 1),
                ("/b.png", ContentType::Png, 1),
            ],
        ))
        .unwrap();
        value["photos"][1]["photoId"] = value["photos"][0]["photoId"].clone();
        let payload = serde_json::to_vec(&value).unwrap();
        let Handled::Failed { error, .. } = f.handler.handle(&payload).await else {
            panic!()
        };
        assert_eq!(error.code, PiErrorCode::InvalidImage);
        assert!(
            error.message.contains("duplicate photo id p0"),
            "{}",
            error.message
        );
        assert!(f.printer.printed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn hostile_ids_cannot_escape_the_temp_dir_or_collide() {
        let mut f = fixture().await;
        let mut value: serde_json::Value =
            serde_json::from_slice(&one(&f.base, "../../evil")).unwrap();
        value["photos"][0]["photoId"] = "../x".into();
        let payload = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            f.handler.handle(&payload).await,
            Handled::Completed { .. }
        ));
        let printed = f.printer.printed.lock().unwrap().clone();
        assert_eq!(printed[0].0.parent().unwrap(), f.tmp.path());
        assert!(
            printed[0].0.ends_with("______evil-0-___x.png"),
            "{:?}",
            printed[0].0
        );
    }

    #[tokio::test]
    async fn publish_failures_do_not_stop_the_job() {
        let mut f = fixture().await;
        *f.sink.fail.lock().unwrap() = true;
        assert_eq!(
            f.handler.handle(&one(&f.base, "j1")).await,
            Handled::Completed { printed: 1 }
        );
    }

    #[tokio::test]
    async fn a_stuck_status_publish_does_not_block_the_worker() {
        let f = fixture().await;
        *f.sink.hang.lock().unwrap() = true;
        let mut handler = f.handler.with_status_timeout(Duration::from_millis(50));
        let started = std::time::Instant::now();
        let payload = one(&f.base, "j1");
        assert_eq!(
            handler.handle(&payload).await,
            Handled::Completed { printed: 1 }
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn unpersistable_ledger_does_not_block_printing() {
        let mut f = fixture().await;
        *f.handler.store.lock().unwrap() = DedupeStore::unwritable();
        assert_eq!(
            f.handler.handle(&one(&f.base, "j1")).await,
            Handled::Completed { printed: 1 }
        );
    }

    #[tokio::test]
    async fn recover_fails_jobs_a_previous_run_left_unfinished() {
        let mut f = fixture().await;
        f.handler
            .ledger(|s| s.begin("crashed", "order-crashed"))
            .await
            .unwrap()
            .unwrap();
        f.handler
            .ledger(|s| s.begin("fine", "order-fine"))
            .await
            .unwrap()
            .unwrap();
        f.handler
            .ledger(|s| s.finish("fine", "order-fine", Outcome::Completed))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(f.handler.recover().await, 1);
        let statuses = f.sink.statuses.lock().unwrap().clone();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].job_id.as_str(), "crashed");
        assert_eq!(statuses[0].order_id.as_str(), "order-crashed");
        assert_eq!(statuses[0].state, JobState::Failed);
        let error = statuses[0].error.clone().unwrap();
        assert_eq!(
            (error.code, error.message.as_str()),
            (PiErrorCode::DriverError, "agent restarted during job")
        );
        // Closed in the ledger: no second report, and the job id now counts as failed.
        assert_eq!(f.handler.recover().await, 0);
        assert_eq!(
            f.handler.handle(&one(&f.base, "crashed")).await,
            Handled::Duplicate
        );
    }

    #[tokio::test]
    async fn cancelling_mid_print_fails_the_job_as_shutting_down() {
        let mut f = fixture().await;
        *f.printer.wait_for_cancel.lock().unwrap() = true;
        let cancel = f.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });
        let Handled::Failed { error, .. } = f.handler.handle(&one(&f.base, "j1")).await else {
            panic!()
        };
        assert_eq!(
            (error.code, error.message.as_str()),
            (PiErrorCode::DriverError, "print cancelled")
        );
        assert!(tmp_is_empty(&f));
    }

    #[tokio::test]
    async fn cancelling_mid_download_fails_the_job_as_shutting_down() {
        let mut f = fixture().await;
        let cancel = f.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });
        let slow = job(
            &f.base,
            "j1",
            "stand-001",
            &[("/slow", ContentType::Png, 1)],
        );
        let started = std::time::Instant::now();
        let Handled::Failed { error, .. } = f.handler.handle(&slow).await else {
            panic!()
        };
        assert_eq!(error.message, "agent shutting down");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(tmp_is_empty(&f));
    }

    #[tokio::test]
    async fn reject_publishes_failed_for_our_jobs_only() {
        let sink = Recorder::default();
        let me = PrinterId::new("stand-001").unwrap();
        reject(&sink, &me, &one("http://x", "j1"), "queue full").await;
        reject(
            &sink,
            &me,
            &job(
                "http://x",
                "j2",
                "stand-002",
                &[("/a", ContentType::Png, 1)],
            ),
            "queue full",
        )
        .await;
        reject(&sink, &me, b"garbage", "queue full").await;
        let statuses = sink.statuses.lock().unwrap().clone();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].job_id.as_str(), "j1");
        assert_eq!(statuses[0].state, JobState::Failed);
        assert_eq!(statuses[0].error.as_ref().unwrap().message, "queue full");
        *sink.fail.lock().unwrap() = true;
        reject(&sink, &me, &one("http://x", "j3"), "queue full").await;
        assert_eq!(sink.statuses.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn state_dir_is_used_for_the_ledger() {
        let mut f = fixture().await;
        f.handler.handle(&one(&f.base, "j1")).await;
        assert!(f.state.path().join(crate::dedupe::FILE_NAME).exists());
    }
}
