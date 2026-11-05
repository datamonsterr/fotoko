//! `backend = "cups"`: print through the CUPS command line tools.
//!
//! `lp` submits the job. CUPS does not report completion to the submitter, so the backend polls
//! `lpstat -o <queue>` until the job leaves the queue, then confirms with
//! `lpstat -l -W completed -o <queue>` how it ended (`job-completed-successfully`, aborted or
//! cancelled). With the USB backend's `stop-printer` error policy a fault (no paper, no ribbon,
//! unplugged) leaves the job queued and the printer `disabled`, so each poll also reads
//! `lpstat -p <queue>`; a disabled printer fails the print with the matching Pi error code.
//!
//! A job we stop caring about (error, timeout, cancellation, dropped future) is cancelled with
//! `cancel` so it cannot print later. If `lp` itself hangs, jobs that appeared in the queue since
//! before the call are cancelled too.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use fotoko_protocol::{Media, PrinterState};
use tokio::process::Command;
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::config::PrinterConfig;
use crate::printer::{PrintContext, PrintError, Printer};

/// Default for how long one CUPS command may run before it is killed.
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// CUPS printer settings.
#[derive(Debug, Clone)]
pub struct CupsSettings {
    /// Queue name.
    pub queue: String,
    /// `media=` option value.
    pub media: String,
    /// `lp` executable.
    pub lp_bin: String,
    /// `lpstat` executable.
    pub lpstat_bin: String,
    /// `cancel` executable.
    pub cancel_bin: String,
    /// Maximum time to wait for a job to leave the queue.
    pub print_timeout: Duration,
    /// Delay between polls.
    pub poll_interval: Duration,
    /// Kill any single CUPS command that runs longer than this.
    pub command_timeout: Duration,
    /// The user `lp` submits as (the `lpstat -o` owner column). Only that user's jobs are ever
    /// swept after a hung `lp`; without it no sweep happens.
    pub owner: Option<String>,
}

impl From<&PrinterConfig> for CupsSettings {
    fn from(config: &PrinterConfig) -> Self {
        Self {
            queue: config.queue.clone(),
            media: config.media.clone(),
            lp_bin: config.lp_bin.clone(),
            lpstat_bin: config.lpstat_bin.clone(),
            cancel_bin: config.cancel_bin.clone(),
            print_timeout: Duration::from_secs(config.print_timeout_secs),
            poll_interval: Duration::from_millis(config.poll_interval_ms),
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
            owner: std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .ok()
                .filter(|u| !u.is_empty()),
        }
    }
}

/// Printer state read from `lpstat -p`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// Enabled and waiting.
    Idle,
    /// Enabled and working on a job.
    Printing,
    /// Disabled; the error explains why.
    Stopped(PrintError),
}

/// How CUPS says a job that left the queue ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobEnd {
    /// `job-completed-successfully` (or with warnings).
    Completed,
    /// Aborted by the system or completed with errors.
    Aborted,
    /// Cancelled by someone.
    Canceled,
}

/// Consecutive failed `lpstat` polls tolerated before the print fails.
const MAX_TRANSIENT_FAILURES: u32 = 3;

/// Why one poll could not tell whether the job is done.
enum Poll {
    /// `lpstat` itself failed; worth another try.
    Transient(PrintError),
    /// The printer or the job is in a failed state.
    Fatal(PrintError),
}

struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

enum RunError {
    TimedOut(String),
    Failed(PrintError),
}

impl RunError {
    fn into_print_error(self) -> PrintError {
        match self {
            Self::TimedOut(bin) => PrintError::Driver(format!("{bin} timed out")),
            Self::Failed(error) => error,
        }
    }
}

/// Cancels a CUPS job when dropped while armed, so an abandoned print never leaves a job that
/// prints later. Best effort: a detached `cancel` process.
struct JobGuard {
    cancel_bin: String,
    job: String,
    armed: bool,
}

impl JobGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (bin, job) = (self.cancel_bin.clone(), self.job.clone());
        // Detached: Drop cannot await, and the runtime may be shutting down.
        std::thread::spawn(move || {
            let _ = std::process::Command::new(bin)
                .arg(job)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }
}

/// Printer driven through `lp` and `lpstat`.
#[derive(Debug, Clone)]
pub struct CupsPrinter {
    settings: CupsSettings,
}

impl CupsPrinter {
    /// Create a printer for a queue.
    pub fn new(settings: CupsSettings) -> Self {
        Self { settings }
    }

    async fn run(&self, bin: &str, args: &[&str]) -> Result<Output, RunError> {
        // A freshly written executable can be briefly busy while another thread forks; retry.
        let mut attempts = 0;
        let output = loop {
            let mut command = Command::new(bin);
            command
                .args(args)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true);
            match tokio::time::timeout(self.settings.command_timeout, command.output()).await {
                Err(_) => return Err(RunError::TimedOut(bin.to_owned())),
                Ok(Err(e))
                    if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 5 =>
                {
                    attempts += 1;
                    sleep(Duration::from_millis(20)).await;
                }
                Ok(Err(e)) => {
                    return Err(RunError::Failed(PrintError::Driver(format!(
                        "cannot run {bin}: {e}"
                    ))));
                }
                Ok(Ok(output)) => break output,
            }
        };
        Ok(Output {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Run an `lpstat` query; a non-zero exit becomes the mapped error.
    async fn lpstat(&self, args: &[&str]) -> Result<String, PrintError> {
        let out = self
            .run(&self.settings.lpstat_bin, args)
            .await
            .map_err(RunError::into_print_error)?;
        if out.success {
            Ok(out.stdout)
        } else {
            Err(map_reason(&out.stderr))
        }
    }

    async fn health(&self) -> Result<Health, PrintError> {
        let text = self.lpstat(&["-p", &self.settings.queue]).await?;
        Ok(classify_printer_status(&text))
    }

    async fn queued_entries(&self) -> Result<Vec<(String, Option<String>)>, PrintError> {
        let text = self.lpstat(&["-o", &self.settings.queue]).await?;
        Ok(listed_entries(&text))
    }

    async fn queued_jobs(&self) -> Result<Vec<String>, PrintError> {
        Ok(self
            .queued_entries()
            .await?
            .into_iter()
            .map(|(job, _)| job)
            .collect())
    }

    async fn cancel(&self, job: &str) {
        match self.run(&self.settings.cancel_bin, &[job]).await {
            Ok(out) if out.success => debug!(job, "cancelled CUPS job"),
            Ok(out) => warn!(job, stderr = out.stderr.trim(), "cancel failed"),
            Err(_) => warn!(job, "cancel failed"),
        }
    }

    /// `lp` hung and was killed: it may still have queued the job. Cancel the jobs that appeared
    /// since `before` and belong to our user. Without a good snapshot or a known owner nothing is
    /// cancelled, because other users' (or earlier) jobs must never be touched.
    async fn cancel_new_jobs(&self, before: Option<&HashSet<String>>) {
        let (Some(before), Some(owner)) = (before, self.settings.owner.as_deref()) else {
            warn!("lp hung and the queue before it is unknown, not sweeping the queue");
            return;
        };
        match self.queued_entries().await {
            Ok(now) => {
                for (job, job_owner) in &now {
                    if !before.contains(job) && job_owner.as_deref() == Some(owner) {
                        warn!(job, "cancelling job queued by a hung lp");
                        self.cancel(job).await;
                    }
                }
            }
            Err(error) => warn!(%error, "cannot list queue after lp timeout"),
        }
    }

    /// Whether the job already left the queue and CUPS says it printed.
    async fn finished_ok(&self, job: &str) -> bool {
        matches!(self.queued_jobs().await, Ok(jobs) if !jobs.iter().any(|j| j == job))
            && self.confirm_finished(job).await.is_ok()
    }

    /// One poll: `Ok(true)` when the job has finished, `Ok(false)` while it is still working.
    async fn poll_once(&self, job: &str) -> Result<bool, Poll> {
        let queued = self.queued_jobs().await.map_err(Poll::Transient)?;
        if !queued.iter().any(|j| j == job) {
            self.confirm_finished(job).await.map_err(Poll::Fatal)?;
            return Ok(true);
        }
        match self.health().await.map_err(Poll::Transient)? {
            Health::Stopped(error) => Err(Poll::Fatal(error)),
            Health::Idle | Health::Printing => Ok(false),
        }
    }

    /// The job left the queue: check CUPS says it printed. If the history cannot say (disabled,
    /// pruned, query failed) the print is assumed good.
    async fn confirm_finished(&self, job: &str) -> Result<(), PrintError> {
        let listing = self
            .lpstat(&["-l", "-W", "completed", "-o", &self.settings.queue])
            .await;
        match listing.map(|text| job_end(&text, job)) {
            Ok(Some(JobEnd::Completed)) => Ok(()),
            Ok(Some(JobEnd::Aborted)) => Err(PrintError::Driver(format!("CUPS aborted job {job}"))),
            Ok(Some(JobEnd::Canceled)) => Err(PrintError::Driver(format!(
                "job {job} was cancelled in CUPS"
            ))),
            Ok(None) => {
                warn!(job, "job not in CUPS history, assuming it printed");
                Ok(())
            }
            Err(error) => {
                warn!(job, %error, "cannot read CUPS history, assuming the job printed");
                Ok(())
            }
        }
    }
}

#[async_trait]
impl Printer for CupsPrinter {
    async fn print(
        &self,
        file: &Path,
        copies: u32,
        media: Media,
        ctx: &PrintContext,
    ) -> Result<(), PrintError> {
        // Only 4x6 exists; the CUPS media name comes from the config.
        let Media::Photo4x6 = media;
        // The queue before lp, to tell its job apart if lp hangs; `None` if it cannot be read.
        let before: Option<HashSet<String>> = self
            .queued_jobs()
            .await
            .ok()
            .map(|jobs| jobs.into_iter().collect());
        let copies = copies.to_string();
        let media_option = format!("media={}", self.settings.media);
        let file = file.to_string_lossy();
        let submitted = self
            .run(
                &self.settings.lp_bin,
                &[
                    "-d",
                    &self.settings.queue,
                    "-t",
                    &ctx.title,
                    "-n",
                    &copies,
                    "-o",
                    &media_option,
                    "-o",
                    "fit-to-page",
                    &file,
                ],
            )
            .await;
        let out = match submitted {
            Ok(out) => out,
            Err(error) => {
                if matches!(error, RunError::TimedOut(_)) {
                    self.cancel_new_jobs(before.as_ref()).await;
                }
                return Err(error.into_print_error());
            }
        };
        if !out.success {
            return Err(map_reason(&out.stderr));
        }
        let job = parse_request_id(&out.stdout).ok_or_else(|| {
            PrintError::Driver(format!("unexpected lp output: {}", out.stdout.trim()))
        })?;
        debug!(job, "submitted to CUPS");
        let mut guard = JobGuard {
            cancel_bin: self.settings.cancel_bin.clone(),
            job: job.clone(),
            armed: true,
        };

        let deadline = Instant::now() + self.settings.print_timeout;
        let mut failures = 0;
        loop {
            let step = tokio::select! {
                biased;
                () = ctx.cancel.cancelled() => {
                    // One last look: a job that already printed must not be reported failed.
                    let printed = self.finished_ok(&job).await;
                    if !printed {
                        self.cancel(&job).await;
                    }
                    guard.disarm();
                    return if printed { Ok(()) } else { Err(PrintError::Cancelled) };
                }
                step = self.poll_once(&job) => step,
            };
            match step {
                Ok(true) => {
                    guard.disarm();
                    return Ok(());
                }
                Ok(false) => failures = 0,
                Err(Poll::Transient(error)) => {
                    failures += 1;
                    warn!(%job, failures, %error, "lpstat failed while waiting for the job");
                    if failures > MAX_TRANSIENT_FAILURES {
                        self.cancel(&job).await;
                        guard.disarm();
                        return Err(error);
                    }
                }
                Err(Poll::Fatal(error)) => {
                    self.cancel(&job).await;
                    guard.disarm();
                    return Err(error);
                }
            }
            if Instant::now() >= deadline {
                self.cancel(&job).await;
                guard.disarm();
                return Err(PrintError::Driver(format!(
                    "timed out after {}s waiting for CUPS job {job}",
                    self.settings.print_timeout.as_secs()
                )));
            }
            tokio::select! {
                () = ctx.cancel.cancelled() => {}
                () = sleep(self.settings.poll_interval) => {}
            }
        }
    }

    async fn state(&self) -> PrinterState {
        match self.health().await {
            Ok(Health::Idle) => PrinterState::Idle,
            Ok(Health::Printing) => PrinterState::Printing,
            Ok(Health::Stopped(PrintError::Offline(_))) => PrinterState::Offline,
            Ok(Health::Stopped(_)) | Err(_) => PrinterState::Error,
        }
    }
}

/// Job id from `lp` output: `request id is fotoko-12 (1 file(s))` gives `fotoko-12`.
pub fn parse_request_id(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("request id is ")?;
        let id = rest.split_whitespace().next()?;
        Some(id.to_owned())
    })
}

/// Job ids listed by `lpstat -o`: the first column of every unindented line.
pub fn listed_jobs(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// Job ids and owners from `lpstat -o` (`id owner size date`); the owner is the second column.
pub fn listed_entries(stdout: &str) -> Vec<(String, Option<String>)> {
    stdout
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let job = cols.next()?.to_owned();
            Some((job, cols.next().map(str::to_owned)))
        })
        .collect()
}

/// Whether `lpstat -o` output lists the job (first column equals the job id).
pub fn job_in_listing(stdout: &str, job: &str) -> bool {
    listed_jobs(stdout).iter().any(|j| j == job)
}

/// How a finished job ended, from `lpstat -l -W completed -o`: the job's block (its line plus the
/// indented lines below) carries an `Alerts:` line with the IPP job-state-reasons. `None` when
/// the job or its reasons are not listed.
pub fn job_end(stdout: &str, job: &str) -> Option<JobEnd> {
    let mut in_block = false;
    for line in stdout.lines() {
        if !line.starts_with(char::is_whitespace) {
            in_block = line.split_whitespace().next() == Some(job);
            continue;
        }
        if !in_block {
            continue;
        }
        let Some(reasons) = line.trim().strip_prefix("Alerts:") else {
            continue;
        };
        let reasons = reasons.to_lowercase();
        return if reasons.contains("abort") || reasons.contains("job-completed-with-errors") {
            Some(JobEnd::Aborted)
        } else if reasons.contains("cancel") {
            Some(JobEnd::Canceled)
        } else if reasons.contains("job-completed") {
            Some(JobEnd::Completed)
        } else {
            None
        };
    }
    None
}

/// Read `lpstat -p` text: `printer q is idle.`, `printer q now printing q-3.`, or
/// `printer q disabled since ... -` followed by the reason lines.
pub fn classify_printer_status(text: &str) -> Health {
    let lower = text.to_lowercase();
    if lower.contains(" disabled since") || lower.contains(" stopped") {
        // Only the reason lines count: the first line carries the queue name.
        let reason: Vec<&str> = text
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let reason = reason.join("; ");
        return Health::Stopped(map_reason(if reason.is_empty() {
            "printer disabled"
        } else {
            &reason
        }));
    }
    if lower.contains("now printing") {
        Health::Printing
    } else {
        Health::Idle
    }
}

const PAPER: &[&str] = &[
    "media-empty",
    "media-needed",
    "out of paper",
    "paper out",
    "no paper",
];
const RIBBON: &[&str] = &[
    "marker-supply-empty",
    "ribbon",
    "out of ink",
    "ink empty",
    "ink is empty",
];
const OFFLINE: &[&str] = &[
    "connecting-to-device",
    "waiting for printer to become available",
    "offline",
    "unable to open",
    "not connected",
    "unplugged",
    "no such device",
    "cannot open",
    "does not exist",
    "unknown destination",
    "not accepting",
    "no default destination",
    "invalid destination",
];

/// Lowercase words of a message; letters, digits and `-` stay inside a word.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether the words contain the phrase. A single dashed keyword such as `media-empty` also
/// matches its IPP variants (`media-empty-error`, `media-empty-warning`).
fn has_phrase(words: &[String], phrase: &str) -> bool {
    let parts: Vec<&str> = phrase.split(' ').collect();
    if let [single] = parts[..] {
        let prefix = format!("{single}-");
        return words.iter().any(|w| w == single || w.starts_with(&prefix));
    }
    words
        .windows(parts.len())
        .any(|w| w.iter().map(String::as_str).eq(parts.iter().copied()))
}

/// Map CUPS status or error text to a Pi error: paper, ribbon, then unreachable printer; anything
/// else is a driver error carrying the text. Keywords match whole words, not substrings.
pub fn map_reason(text: &str) -> PrintError {
    let words = words(text);
    let has = |keywords: &[&str]| keywords.iter().any(|k| has_phrase(&words, k));
    let message = {
        let t = text.trim();
        if t.is_empty() {
            "unknown CUPS failure".to_owned()
        } else {
            t.to_owned()
        }
    };
    if has(PAPER) {
        PrintError::PaperOut
    } else if has(RIBBON) {
        PrintError::RibbonOut
    } else if has(OFFLINE) {
        PrintError::Offline(message)
    } else {
        PrintError::Driver(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    #[test]
    fn request_id_parsing() {
        assert_eq!(
            parse_request_id("request id is fotoko-12 (1 file(s))\n").as_deref(),
            Some("fotoko-12")
        );
        assert_eq!(
            parse_request_id("noise\nrequest id is my-q-3 (2 file(s))").as_deref(),
            Some("my-q-3")
        );
        assert_eq!(parse_request_id(""), None);
        assert_eq!(parse_request_id("request id is "), None);
        assert_eq!(parse_request_id("something else"), None);
    }

    #[test]
    fn listing_matches_first_column_of_unindented_lines_only() {
        let listing = "fotoko-12   root   1024   Thu 05 Nov 2026 10:00:00 AM UTC\n\tAlerts: x\nfotoko-123  root  5  x\n";
        assert_eq!(listed_jobs(listing), ["fotoko-12", "fotoko-123"]);
        assert!(job_in_listing(listing, "fotoko-12"));
        assert!(job_in_listing(listing, "fotoko-123"));
        assert!(!job_in_listing(listing, "fotoko-1"));
        assert!(!job_in_listing("", "fotoko-12"));
        assert!(!job_in_listing("other root fotoko-12\n", "fotoko-12"));
        assert!(!job_in_listing("\tfotoko-12 indented\n", "fotoko-12"));
    }

    #[test]
    fn job_end_reads_the_alerts_of_the_right_job() {
        let listing = "q-1 root 1 now\n\tAlerts: aborted-by-system\nq-2 root 1 now\n\tqueued for q\n\tAlerts: job-completed-successfully\nq-3 root 1 now\n\tAlerts: job-canceled-by-user\nq-4 root 1 now\n\tAlerts: job-completed-with-errors\nq-5 root 1 now\n\tAlerts: job-completed-with-warnings\nq-6 root 1 now\n\tAlerts: processing-to-stop-point\nq-7 root 1 now\n";
        assert_eq!(job_end(listing, "q-1"), Some(JobEnd::Aborted));
        assert_eq!(job_end(listing, "q-2"), Some(JobEnd::Completed));
        assert_eq!(job_end(listing, "q-3"), Some(JobEnd::Canceled));
        assert_eq!(job_end(listing, "q-4"), Some(JobEnd::Aborted));
        assert_eq!(job_end(listing, "q-5"), Some(JobEnd::Completed));
        assert_eq!(job_end(listing, "q-6"), None, "unrecognised reason");
        assert_eq!(job_end(listing, "q-7"), None, "no alerts");
        assert_eq!(job_end(listing, "q-8"), None, "not listed");
        assert_eq!(job_end("", "q-1"), None);
    }

    #[test]
    fn entries_carry_the_owner_column() {
        let listing = "q-1  alice  10  Thu\n\tAlerts: x\nq-2\n";
        assert_eq!(
            listed_entries(listing),
            [
                ("q-1".to_owned(), Some("alice".to_owned())),
                ("q-2".to_owned(), None)
            ]
        );
    }

    #[test]
    fn classify_idle_and_printing() {
        assert_eq!(
            classify_printer_status("printer fotoko is idle.  enabled since Thu 05 Nov 2026"),
            Health::Idle
        );
        assert_eq!(
            classify_printer_status("printer fotoko now printing fotoko-3.  enabled since x"),
            Health::Printing
        );
        assert_eq!(classify_printer_status(""), Health::Idle);
    }

    #[test]
    fn classify_disabled_uses_reason_lines_only() {
        let text =
            |reason: &str| format!("printer fotoko disabled since Thu 05 Nov 2026 -\n\t{reason}\n");
        assert_eq!(
            classify_printer_status(&text("Out of paper.")),
            Health::Stopped(PrintError::PaperOut)
        );
        assert_eq!(
            classify_printer_status(&text("Ribbon or ink cartridge is empty.")),
            Health::Stopped(PrintError::RibbonOut)
        );
        assert_eq!(
            classify_printer_status(&text("Waiting for printer to become available.")),
            Health::Stopped(PrintError::Offline(
                "Waiting for printer to become available.".into()
            ))
        );
        assert_eq!(
            classify_printer_status(&text("Filter failed.")),
            Health::Stopped(PrintError::Driver("Filter failed.".into()))
        );
        // A queue called "ribbon" must not turn every fault into ribbon_out.
        assert_eq!(
            classify_printer_status("printer ribbon disabled since x -\n\tFilter failed.\n"),
            Health::Stopped(PrintError::Driver("Filter failed.".into()))
        );
    }

    #[test]
    fn classify_disabled_without_reason_and_stopped_wording() {
        assert_eq!(
            classify_printer_status("printer paper-queue disabled since x -"),
            Health::Stopped(PrintError::Driver("printer disabled".into()))
        );
        assert_eq!(
            classify_printer_status("printer q stopped\n\tmedia-empty-error"),
            Health::Stopped(PrintError::PaperOut)
        );
    }

    #[test]
    fn reason_mapping_table() {
        assert_eq!(map_reason("media-empty-error"), PrintError::PaperOut);
        assert_eq!(map_reason("media-empty"), PrintError::PaperOut);
        assert_eq!(map_reason("Media-Needed"), PrintError::PaperOut);
        assert_eq!(map_reason("No paper in tray"), PrintError::PaperOut);
        assert_eq!(map_reason("Out of paper."), PrintError::PaperOut);
        assert_eq!(
            map_reason("marker-supply-empty-error"),
            PrintError::RibbonOut
        );
        assert_eq!(map_reason("Ribbon exhausted"), PrintError::RibbonOut);
        assert_eq!(map_reason("The ink is empty"), PrintError::RibbonOut);
        assert!(matches!(
            map_reason("Unable to open USB device"),
            PrintError::Offline(_)
        ));
        assert!(matches!(
            map_reason("lp: The printer or class does not exist."),
            PrintError::Offline(_)
        ));
        assert!(matches!(
            map_reason("lp: Destination \"q\" is not accepting jobs"),
            PrintError::Offline(_)
        ));
        assert!(matches!(
            map_reason("connecting-to-device"),
            PrintError::Offline(_)
        ));
        assert_eq!(
            map_reason("scheduler not responding"),
            PrintError::Driver("scheduler not responding".into())
        );
        assert_eq!(
            map_reason("  \n"),
            PrintError::Driver("unknown CUPS failure".into())
        );
        // Paper wins over a message that also mentions the printer being offline.
        assert_eq!(map_reason("offline: out of paper"), PrintError::PaperOut);
    }

    #[test]
    fn keywords_match_whole_words_only() {
        for text in [
            "ribbonfactory.ppd failed",
            "mediaempty",
            "pre-media-empty",
            "inkjet error",
            "offlined",
            "xnot accepting",
        ] {
            assert!(matches!(map_reason(text), PrintError::Driver(_)), "{text}");
        }
    }

    // ---- shell stubs ----

    fn script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    struct Stubs {
        dir: tempfile::TempDir,
        settings: CupsSettings,
    }

    /// An `lpstat` stub that answers `-l -W completed -o`, `-o` and `-p` with the given shell
    /// snippets.
    fn lpstat(completed: &str, queue: &str, printer: &str) -> String {
        format!(
            "case \"$*\" in\n*completed*) {completed} ;;\n-o*) {queue} ;;\n-p*) {printer} ;;\nesac"
        )
    }

    const LP_OK: &str = "echo 'request id is q-7 (1 file(s))'";
    const DONE: &str = "echo 'q-7 root 1 now'; printf '\\tAlerts: job-completed-successfully\\n'";
    const LISTED: &str = "echo 'q-7 root 1 now'";
    const PRINTING: &str = "echo 'printer q now printing q-7.  enabled since x'";
    const IDLE: &str = "echo 'printer q is idle.  enabled since x'";

    /// `lp` stub body, `lpstat` stub body, `cancel` stub body.
    fn stubs(lp: &str, lpstat: &str, cancel: &str) -> Stubs {
        let dir = tempfile::tempdir().unwrap();
        let settings = CupsSettings {
            queue: "q".into(),
            media: "w288h432".into(),
            lp_bin: script(dir.path(), "lp", lp),
            lpstat_bin: script(dir.path(), "lpstat", lpstat),
            cancel_bin: script(dir.path(), "cancel", cancel),
            print_timeout: Duration::from_millis(600),
            poll_interval: Duration::from_millis(20),
            command_timeout: Duration::from_secs(10),
            owner: Some("root".into()),
        };
        Stubs { dir, settings }
    }

    impl Stubs {
        fn printer(&self) -> CupsPrinter {
            CupsPrinter::new(self.settings.clone())
        }

        fn file(&self) -> PathBuf {
            self.dir.path().join("a.jpg")
        }

        async fn print(&self) -> Result<(), PrintError> {
            self.printer()
                .print(
                    &self.file(),
                    1,
                    Media::Photo4x6,
                    &PrintContext::new("job-1"),
                )
                .await
        }

        fn log(&self, bin: &str) -> String {
            std::fs::read_to_string(self.dir.path().join(format!("{bin}.log"))).unwrap_or_default()
        }
    }

    async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !check() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn passes_documented_lp_arguments() {
        let s = stubs(
            "echo \"$@\" > \"$0.args\"; echo 'request id is q-7 (1 file(s))'",
            &lpstat(DONE, "exit 0", IDLE),
            "exit 0",
        );
        s.printer()
            .print(&s.file(), 3, Media::Photo4x6, &PrintContext::new("job-9"))
            .await
            .unwrap();
        let args = std::fs::read_to_string(format!("{}.args", s.settings.lp_bin)).unwrap();
        assert_eq!(
            args.trim(),
            format!(
                "-d q -t job-9 -n 3 -o media=w288h432 -o fit-to-page {}",
                s.file().display()
            )
        );
    }

    #[tokio::test]
    async fn succeeds_when_job_leaves_queue_and_history_says_completed() {
        // The snapshot before lp and the first poll list the job, the second poll does not.
        let queue = "n=$(cat \"$0.n\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \"$0.n\"; if [ $n -le 2 ]; then echo 'q-7 root 1 now'; fi";
        let s = stubs(LP_OK, &lpstat(DONE, queue, PRINTING), "exit 0");
        s.print().await.unwrap();
    }

    #[tokio::test]
    async fn aborted_and_cancelled_jobs_are_driver_errors() {
        for (alert, expect) in [
            ("aborted-by-system", "aborted"),
            ("job-canceled-by-user", "cancelled"),
        ] {
            let completed = format!("echo 'q-7 root 1 now'; printf '\\tAlerts: {alert}\\n'");
            let s = stubs(LP_OK, &lpstat(&completed, "exit 0", IDLE), "exit 0");
            let e = s.print().await.unwrap_err();
            assert!(
                matches!(&e, PrintError::Driver(m) if m.contains(expect)),
                "{alert}: {e}"
            );
        }
    }

    #[tokio::test]
    async fn unconfirmable_history_is_assumed_good() {
        // Not listed in the history, listed without reasons, or the query fails.
        for completed in [
            "exit 0",
            "echo 'q-7 root 1 now'",
            "echo 'lpstat: boom' >&2; exit 1",
        ] {
            let s = stubs(LP_OK, &lpstat(completed, "exit 0", IDLE), "exit 0");
            s.print().await.unwrap();
        }
    }

    #[tokio::test]
    async fn lp_failure_maps_stderr() {
        let s = stubs(
            "echo 'lp: The printer or class does not exist.' >&2; exit 1",
            "exit 0",
            "exit 0",
        );
        let e = s.print().await.unwrap_err();
        assert!(matches!(e, PrintError::Offline(m) if m.contains("does not exist")));
    }

    #[tokio::test]
    async fn unparsable_lp_output_is_driver_error() {
        let s = stubs("echo hello", "exit 0", "exit 0");
        let e = s.print().await.unwrap_err();
        assert!(matches!(e, PrintError::Driver(m) if m.contains("unexpected lp output")));
    }

    #[tokio::test]
    async fn missing_binary_is_driver_error() {
        let mut s = stubs(LP_OK, "exit 0", "exit 0");
        s.settings.lp_bin = "/nonexistent/lp".into();
        let e = s.print().await.unwrap_err();
        assert!(matches!(e, PrintError::Driver(m) if m.contains("cannot run")));
    }

    #[tokio::test]
    async fn disabled_printer_fails_and_cancels_job() {
        let printer = "printf 'printer q disabled since x -\\n\\tOut of paper.\\n'";
        let s = stubs(
            LP_OK,
            &lpstat(DONE, LISTED, printer),
            "echo \"$@\" >> \"$0.log\"",
        );
        assert_eq!(s.print().await.unwrap_err(), PrintError::PaperOut);
        assert_eq!(s.log("cancel").trim(), "q-7");
    }

    #[tokio::test]
    async fn timeout_cancels_job() {
        let s = stubs(
            LP_OK,
            &lpstat(DONE, LISTED, PRINTING),
            "echo \"$@\" >> \"$0.log\"",
        );
        let e = s.print().await.unwrap_err();
        assert!(matches!(e, PrintError::Driver(m) if m.contains("timed out")));
        assert_eq!(s.log("cancel").trim(), "q-7");
    }

    #[tokio::test]
    async fn lpstat_failures_fail_the_print_and_cancel() {
        for failing in ["-o", "-p"] {
            let lpstat = format!(
                "if [ \"$1\" = \"{failing}\" ]; then echo 'lpstat: Unable to connect to server' >&2; exit 1; fi; echo 'q-7 root 1 now'; echo 'printer q is idle.'"
            );
            let s = stubs(LP_OK, &lpstat, "echo \"$@\" >> \"$0.log\"");
            let e = s.print().await.unwrap_err();
            assert!(matches!(e, PrintError::Driver(_)), "{failing}: {e}");
            if failing == "-p" {
                assert_eq!(s.log("cancel").trim(), "q-7");
            }
        }
    }

    #[tokio::test]
    async fn cancel_token_withdraws_the_job() {
        let mut s = stubs(
            LP_OK,
            &lpstat(DONE, LISTED, PRINTING),
            "echo \"$@\" >> \"$0.log\"",
        );
        s.settings.print_timeout = Duration::from_secs(30);
        let ctx = PrintContext::new("job-1");
        let token = ctx.cancel.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(150)).await;
            token.cancel();
        });
        let started = Instant::now();
        let e = s
            .printer()
            .print(&s.file(), 1, Media::Photo4x6, &ctx)
            .await
            .unwrap_err();
        assert_eq!(e, PrintError::Cancelled);
        assert_eq!(s.log("cancel").trim(), "q-7");
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[tokio::test]
    async fn dropping_a_print_cancels_the_cups_job() {
        let mut s = stubs(
            LP_OK,
            &lpstat(DONE, "touch \"$0.polled\"; echo 'q-7 root 1 now'", PRINTING),
            "echo \"$@\" >> \"$0.log\"",
        );
        s.settings.print_timeout = Duration::from_secs(30);
        let printer = s.printer();
        let file = s.file();
        let task = tokio::spawn(async move {
            printer
                .print(&file, 1, Media::Photo4x6, &PrintContext::new("job-1"))
                .await
        });
        let polled = format!("{}.polled", s.settings.lpstat_bin);
        eventually("first poll", || Path::new(&polled).exists()).await;
        task.abort();
        let _ = task.await;
        eventually("detached cancel", || s.log("cancel").trim() == "q-7").await;
    }

    #[tokio::test]
    async fn hung_lp_is_killed_and_the_job_it_queued_is_cancelled() {
        let lpstat = lpstat(
            DONE,
            "if [ -e \"$(dirname \"$0\")/queued\" ]; then echo 'q-9 root 1 now'; fi",
            IDLE,
        );
        let mut s = stubs(
            "touch \"$(dirname \"$0\")/queued\"; sleep 5",
            &lpstat,
            "echo \"$@\" >> \"$0.log\"",
        );
        s.settings.command_timeout = Duration::from_millis(300);
        let started = Instant::now();
        let e = s.print().await.unwrap_err();
        assert!(
            matches!(&e, PrintError::Driver(m) if m.contains("timed out")),
            "{e}"
        );
        assert_eq!(s.log("cancel").trim(), "q-9");
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[tokio::test]
    async fn jobs_that_were_already_queued_are_not_cancelled_after_a_hung_lp() {
        let lpstat = lpstat(DONE, "echo 'q-1 root 1 old'", IDLE);
        let mut s = stubs("sleep 5", &lpstat, "echo \"$@\" >> \"$0.log\"");
        s.settings.command_timeout = Duration::from_millis(200);
        assert!(s.print().await.is_err());
        assert_eq!(s.log("cancel"), "");
    }

    #[tokio::test]
    async fn hung_lp_sweep_needs_a_snapshot_and_only_touches_our_jobs() {
        // Snapshot fails: no sweep.
        let failing_snapshot = "n=$(cat \"$0.n\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \"$0.n\"; if [ $n -eq 1 ]; then echo 'lpstat: down' >&2; exit 1; fi; echo 'q-9 root 1 now'";
        let mut s = stubs(
            "sleep 5",
            &lpstat(DONE, failing_snapshot, IDLE),
            "echo \"$@\" >> \"$0.log\"",
        );
        s.settings.command_timeout = Duration::from_millis(200);
        assert!(s.print().await.is_err());
        assert_eq!(s.log("cancel"), "", "no sweep without a snapshot");
        // Owner unknown: no sweep.
        let queued = "echo 'q-9 root 1 now'";
        let mut s = stubs(
            "sleep 5",
            &lpstat(DONE, queued, IDLE),
            "echo \"$@\" >> \"$0.log\"",
        );
        s.settings.command_timeout = Duration::from_millis(200);
        s.settings.owner = None;
        assert!(s.print().await.is_err());
        assert_eq!(s.log("cancel"), "");
        // New job of another user: left alone; new job of ours: cancelled.
        let after = "if [ -e \"$(dirname \"$0\")/queued\" ]; then echo 'q-8 alice 1 now'; echo 'q-9 root 1 now'; fi";
        let mut s = stubs(
            "touch \"$(dirname \"$0\")/queued\"; sleep 5",
            &lpstat(DONE, after, IDLE),
            "echo \"$@\" >> \"$0.log\"",
        );
        s.settings.command_timeout = Duration::from_millis(300);
        assert!(s.print().await.is_err());
        assert_eq!(s.log("cancel").trim(), "q-9");
    }

    #[tokio::test]
    async fn a_few_lpstat_failures_in_a_row_are_tolerated() {
        // Calls: 1 snapshot ok, 2 to 4 fail, then the job is gone.
        let queue = "n=$(cat \"$0.n\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \"$0.n\"; if [ $n -ge 2 ] && [ $n -le 4 ]; then echo 'lpstat: busy' >&2; exit 1; fi; if [ $n -eq 1 ]; then exit 0; fi";
        let s = stubs(LP_OK, &lpstat(DONE, queue, IDLE), "exit 0");
        s.print().await.unwrap();
        // Four in a row is too many.
        let queue = "n=$(cat \"$0.n\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \"$0.n\"; if [ $n -ge 2 ] && [ $n -le 5 ]; then echo 'lpstat: busy' >&2; exit 1; fi";
        let s = stubs(
            LP_OK,
            &lpstat(DONE, queue, IDLE),
            "echo \"$@\" >> \"$0.log\"",
        );
        assert!(matches!(s.print().await, Err(PrintError::Driver(_))));
        assert_eq!(s.log("cancel").trim(), "q-7");
    }

    #[tokio::test]
    async fn cancelling_a_job_that_already_finished_is_not_a_failure() {
        let s = stubs(
            LP_OK,
            &lpstat(DONE, "exit 0", IDLE),
            "echo \"$@\" >> \"$0.log\"",
        );
        let ctx = PrintContext::new("job-1");
        ctx.cancel.cancel();
        s.printer()
            .print(&s.file(), 1, Media::Photo4x6, &ctx)
            .await
            .unwrap();
        assert_eq!(s.log("cancel"), "", "nothing to cancel");
    }

    #[tokio::test]
    async fn state_maps_health() {
        let cases = [
            (
                "echo 'printer q is idle.  enabled since x'",
                PrinterState::Idle,
            ),
            (
                "echo 'printer q now printing q-1.  enabled since x'",
                PrinterState::Printing,
            ),
            (
                "printf 'printer q disabled since x -\\n\\tUnable to open USB device\\n'",
                PrinterState::Offline,
            ),
            (
                "printf 'printer q disabled since x -\\n\\tOut of paper.\\n'",
                PrinterState::Error,
            ),
            ("echo 'lpstat: Bad' >&2; exit 1", PrinterState::Error),
        ];
        for (body, expected) in cases {
            let s = stubs(LP_OK, body, "exit 0");
            assert_eq!(s.printer().state().await, expected, "{body}");
        }
    }

    #[tokio::test]
    async fn hung_command_is_killed() {
        let mut s = stubs("sleep 5", "exit 0", "exit 0");
        s.settings.command_timeout = Duration::from_millis(100);
        let started = Instant::now();
        let e = s.print().await.unwrap_err();
        assert!(matches!(e, PrintError::Driver(m) if m.contains("timed out")));
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn settings_from_config() {
        let c = PrinterConfig {
            backend: crate::config::Backend::Cups,
            queue: "fotoko".into(),
            media: "w288h432".into(),
            lp_bin: "lp".into(),
            lpstat_bin: "lpstat".into(),
            cancel_bin: "cancel".into(),
            print_timeout_secs: 10,
            poll_interval_ms: 250,
        };
        let s = CupsSettings::from(&c);
        assert_eq!(s.print_timeout, Duration::from_secs(10));
        assert_eq!(s.poll_interval, Duration::from_millis(250));
        assert_eq!(s.queue, "fotoko");
        assert_eq!(s.command_timeout, Duration::from_secs(30));
        assert_eq!(
            s.owner,
            std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .ok()
                .filter(|u| !u.is_empty())
        );
    }
}
