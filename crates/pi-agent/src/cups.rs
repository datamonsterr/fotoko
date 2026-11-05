//! `backend = "cups"`: print through the CUPS command line tools.
//!
//! `lp` submits the job. CUPS does not report completion to the submitter, so the backend polls
//! `lpstat -o <queue>` until the job leaves the queue. With the USB backend's `stop-printer`
//! error policy a fault (no paper, no ribbon, unplugged) leaves the job queued and the printer
//! `disabled`, so each poll also reads `lpstat -p <queue>`; a disabled printer fails the print
//! with the matching Pi error code, and the job is cancelled so it cannot print later.

use std::path::Path;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use fotoko_protocol::{Media, PrinterState};
use tokio::process::Command;
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::config::PrinterConfig;
use crate::printer::{PrintError, Printer};

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

struct Output {
    success: bool,
    stdout: String,
    stderr: String,
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

    async fn run(&self, bin: &str, args: &[&str]) -> Result<Output, PrintError> {
        // A freshly written executable can be briefly busy while another thread forks; retry.
        let mut attempts = 0;
        let output = loop {
            let mut command = Command::new(bin);
            command
                .args(args)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true);
            match tokio::time::timeout(self.settings.command_timeout, command.output()).await {
                Err(_) => return Err(PrintError::Driver(format!("{bin} timed out"))),
                Ok(Err(e))
                    if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 5 =>
                {
                    attempts += 1;
                    sleep(Duration::from_millis(20)).await;
                }
                Ok(Err(e)) => return Err(PrintError::Driver(format!("cannot run {bin}: {e}"))),
                Ok(Ok(output)) => break output,
            }
        };
        Ok(Output {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    async fn health(&self) -> Result<Health, PrintError> {
        let out = self
            .run(&self.settings.lpstat_bin, &["-p", &self.settings.queue])
            .await?;
        if !out.success {
            return Err(map_reason(&out.stderr));
        }
        Ok(classify_printer_status(&out.stdout))
    }

    async fn queued(&self, job: &str) -> Result<bool, PrintError> {
        let out = self
            .run(&self.settings.lpstat_bin, &["-o", &self.settings.queue])
            .await?;
        if !out.success {
            return Err(map_reason(&out.stderr));
        }
        Ok(job_in_listing(&out.stdout, job))
    }

    async fn cancel(&self, job: &str) {
        match self.run(&self.settings.cancel_bin, &[job]).await {
            Ok(out) if out.success => debug!(job, "cancelled CUPS job"),
            Ok(out) => warn!(job, stderr = out.stderr.trim(), "cancel failed"),
            Err(error) => warn!(job, %error, "cancel failed"),
        }
    }
}

#[async_trait]
impl Printer for CupsPrinter {
    async fn print(&self, file: &Path, copies: u32, media: Media) -> Result<(), PrintError> {
        // Only 4x6 exists; the CUPS media name comes from the config.
        let Media::Photo4x6 = media;
        let copies = copies.to_string();
        let media_option = format!("media={}", self.settings.media);
        let file = file.to_string_lossy();
        let out = self
            .run(
                &self.settings.lp_bin,
                &[
                    "-d",
                    &self.settings.queue,
                    "-n",
                    &copies,
                    "-o",
                    &media_option,
                    "-o",
                    "fit-to-page",
                    &file,
                ],
            )
            .await?;
        if !out.success {
            return Err(map_reason(&out.stderr));
        }
        let job = parse_request_id(&out.stdout).ok_or_else(|| {
            PrintError::Driver(format!("unexpected lp output: {}", out.stdout.trim()))
        })?;
        debug!(job, "submitted to CUPS");

        let deadline = Instant::now() + self.settings.print_timeout;
        loop {
            let result = match self.queued(&job).await {
                Ok(false) => return Ok(()),
                Ok(true) => match self.health().await {
                    Ok(Health::Stopped(error)) => Err(error),
                    Ok(Health::Idle | Health::Printing) => Ok(()),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                self.cancel(&job).await;
                return Err(error);
            }
            if Instant::now() >= deadline {
                self.cancel(&job).await;
                return Err(PrintError::Driver(format!(
                    "timed out after {}s waiting for CUPS job {job}",
                    self.settings.print_timeout.as_secs()
                )));
            }
            sleep(self.settings.poll_interval).await;
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

/// Whether `lpstat -o` output lists the job (first column equals the job id).
pub fn job_in_listing(stdout: &str, job: &str) -> bool {
    stdout
        .lines()
        .any(|line| line.split_whitespace().next() == Some(job))
}

/// Read `lpstat -p` text: `printer q is idle.`, `printer q now printing q-3.`, or
/// `printer q disabled since ... -` followed by the reason.
pub fn classify_printer_status(text: &str) -> Health {
    let lower = text.to_lowercase();
    if lower.contains(" disabled since") || lower.contains(" stopped") {
        let reason: Vec<&str> = text
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let reason = reason.join("; ");
        return Health::Stopped(map_reason(if reason.is_empty() { text } else { &reason }));
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
    "ink cartridge",
    "ink is empty",
    "cartridge",
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

/// Map CUPS status or error text to a Pi error: paper, ribbon, then unreachable printer; anything
/// else is a driver error carrying the text.
pub fn map_reason(text: &str) -> PrintError {
    let lower = text.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
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
    fn listing_matches_first_column_only() {
        let listing =
            "fotoko-12   root   1024   Thu 05 Nov 2026 10:00:00 AM UTC\nfotoko-123  root  5  x\n";
        assert!(job_in_listing(listing, "fotoko-12"));
        assert!(job_in_listing(listing, "fotoko-123"));
        assert!(!job_in_listing(listing, "fotoko-1"));
        assert!(!job_in_listing("", "fotoko-12"));
        assert!(!job_in_listing("other root fotoko-12\n", "fotoko-12"));
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
    fn classify_disabled_uses_reason_line() {
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
    }

    #[test]
    fn classify_disabled_without_reason_and_stopped_wording() {
        assert!(matches!(
            classify_printer_status("printer q disabled since x -"),
            Health::Stopped(PrintError::Driver(m)) if m.contains("disabled")
        ));
        assert_eq!(
            classify_printer_status("printer q stopped\n\tmedia-empty-error"),
            Health::Stopped(PrintError::PaperOut)
        );
    }

    #[test]
    fn reason_mapping_table() {
        assert_eq!(map_reason("media-empty-error"), PrintError::PaperOut);
        assert_eq!(map_reason("Media-Needed"), PrintError::PaperOut);
        assert_eq!(map_reason("No paper in tray"), PrintError::PaperOut);
        assert_eq!(
            map_reason("marker-supply-empty-error"),
            PrintError::RibbonOut
        );
        assert_eq!(map_reason("Ribbon exhausted"), PrintError::RibbonOut);
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

    /// `lp` stub body, `lpstat` stub body (sees `$1` = -o or -p), `cancel` stub body.
    fn stubs(lp: &str, lpstat: &str, cancel: &str) -> Stubs {
        let dir = tempfile::tempdir().unwrap();
        let settings = CupsSettings {
            queue: "q".into(),
            media: "w288h432".into(),
            lp_bin: script(dir.path(), "lp", lp),
            lpstat_bin: script(dir.path(), "lpstat", lpstat),
            cancel_bin: script(dir.path(), "cancel", cancel),
            print_timeout: Duration::from_millis(400),
            poll_interval: Duration::from_millis(20),
            command_timeout: Duration::from_secs(10),
        };
        Stubs { dir, settings }
    }

    fn file(dir: &Path) -> PathBuf {
        dir.join("a.jpg")
    }

    const LP_OK: &str = "echo 'request id is q-7 (1 file(s))'";

    #[tokio::test]
    async fn passes_documented_lp_arguments() {
        let s = stubs(
            "echo \"$@\" > \"$0.args\"; echo 'request id is q-7 (1 file(s))'",
            "exit 0",
            "exit 0",
        );
        let p = CupsPrinter::new(s.settings.clone());
        p.print(&file(s.dir.path()), 3, Media::Photo4x6)
            .await
            .unwrap();
        let args = std::fs::read_to_string(format!("{}.args", s.settings.lp_bin)).unwrap();
        assert_eq!(
            args.trim(),
            format!(
                "-d q -n 3 -o media=w288h432 -o fit-to-page {}",
                file(s.dir.path()).display()
            )
        );
    }

    #[tokio::test]
    async fn succeeds_when_job_leaves_queue() {
        // First -o poll lists the job, the second does not.
        let lpstat = r#"
case "$1" in
  -o) if [ -e "$0.seen" ]; then exit 0; else touch "$0.seen"; echo "q-7 root 1 now"; fi ;;
  -p) echo "printer q now printing q-7.  enabled since x" ;;
esac"#;
        let s = stubs(LP_OK, lpstat, "exit 0");
        CupsPrinter::new(s.settings.clone())
            .print(&file(s.dir.path()), 1, Media::Photo4x6)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn lp_failure_maps_stderr() {
        let s = stubs(
            "echo 'lp: The printer or class does not exist.' >&2; exit 1",
            "exit 0",
            "exit 0",
        );
        let e = CupsPrinter::new(s.settings.clone())
            .print(&file(s.dir.path()), 1, Media::Photo4x6)
            .await
            .unwrap_err();
        assert!(matches!(e, PrintError::Offline(m) if m.contains("does not exist")));
    }

    #[tokio::test]
    async fn unparsable_lp_output_is_driver_error() {
        let s = stubs("echo hello", "exit 0", "exit 0");
        let e = CupsPrinter::new(s.settings.clone())
            .print(&file(s.dir.path()), 1, Media::Photo4x6)
            .await
            .unwrap_err();
        assert!(matches!(e, PrintError::Driver(m) if m.contains("unexpected lp output")));
    }

    #[tokio::test]
    async fn missing_binary_is_driver_error() {
        let s = stubs(LP_OK, "exit 0", "exit 0");
        let mut settings = s.settings.clone();
        settings.lp_bin = "/nonexistent/lp".into();
        let e = CupsPrinter::new(settings)
            .print(&file(s.dir.path()), 1, Media::Photo4x6)
            .await
            .unwrap_err();
        assert!(matches!(e, PrintError::Driver(m) if m.contains("cannot run")));
    }

    #[tokio::test]
    async fn disabled_printer_fails_and_cancels_job() {
        let lpstat = r#"
case "$1" in
  -o) echo "q-7 root 1 now" ;;
  -p) printf 'printer q disabled since x -\n\tOut of paper.\n' ;;
esac"#;
        let s = stubs(LP_OK, lpstat, "echo \"$@\" > \"$0.args\"");
        let e = CupsPrinter::new(s.settings.clone())
            .print(&file(s.dir.path()), 1, Media::Photo4x6)
            .await
            .unwrap_err();
        assert_eq!(e, PrintError::PaperOut);
        let cancelled = std::fs::read_to_string(format!("{}.args", s.settings.cancel_bin)).unwrap();
        assert_eq!(cancelled.trim(), "q-7");
    }

    #[tokio::test]
    async fn timeout_cancels_job() {
        let lpstat = r#"
case "$1" in
  -o) echo "q-7 root 1 now" ;;
  -p) echo "printer q now printing q-7.  enabled since x" ;;
esac"#;
        let s = stubs(LP_OK, lpstat, "touch \"$0.ran\"");
        let e = CupsPrinter::new(s.settings.clone())
            .print(&file(s.dir.path()), 1, Media::Photo4x6)
            .await
            .unwrap_err();
        assert!(matches!(e, PrintError::Driver(m) if m.contains("timed out")));
        assert!(Path::new(&format!("{}.ran", s.settings.cancel_bin)).exists());
    }

    #[tokio::test]
    async fn lpstat_failures_fail_the_print() {
        for failing in ["-o", "-p"] {
            let lpstat = format!(
                "if [ \"$1\" = \"{failing}\" ]; then echo 'lpstat: Unable to connect to server' >&2; exit 1; fi; echo 'q-7 root 1 now'; echo 'printer q is idle.'"
            );
            let s = stubs(LP_OK, &lpstat, "exit 1");
            let e = CupsPrinter::new(s.settings.clone())
                .print(&file(s.dir.path()), 1, Media::Photo4x6)
                .await
                .unwrap_err();
            assert!(
                matches!(e, PrintError::Offline(_) | PrintError::Driver(_)),
                "{failing}: {e}"
            );
        }
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
            assert_eq!(
                CupsPrinter::new(s.settings.clone()).state().await,
                expected,
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn hung_command_is_killed() {
        let mut s = stubs("sleep 5", "exit 0", "exit 0");
        s.settings.command_timeout = Duration::from_millis(100);
        let started = Instant::now();
        let e = CupsPrinter::new(s.settings.clone())
            .print(&file(s.dir.path()), 1, Media::Photo4x6)
            .await
            .unwrap_err();
        assert!(matches!(e, PrintError::Driver(m) if m.contains("timed out")));
        assert!(started.elapsed() < Duration::from_secs(3));
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
    }
}
