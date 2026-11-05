//! Fake CUPS: `lp`, `lpstat` and `cancel` behaviour on top of [`SimPrinter`], with state
//! persisted under `FAKE_CUPS_DIR` so separate process invocations share one printer.
//!
//! Layout of the directory:
//!
//! - `state.json`: job counter, paper, queued and finished jobs (internal; use [`FakeCups`]).
//! - `fault`: text file with `""`, `offline`, `ribbon_out` or `driver_error`. Tests flip the
//!   fault by writing this file (or with [`FakeCups::set_fault`]).
//! - `out/`: rendered pages, `{stem}-{n}.png` where stem is the input file's stem.
//! - `spool/`: copies of queued input files.
//!
//! Behaviour mirrors CUPS with a USB backend and `stop-printer` error policy: a fault does not
//! fail `lp`; the job stays queued and the printer is reported as `disabled` with a reason until
//! the fault is cleared, after which the next command prints the job. A file that cannot be
//! decoded aborts the job (a filter failure) and leaves the queue without printing.
//!
//! Environment: `FAKE_CUPS_DIR` (required), `FAKE_CUPS_PAPER` (initial sheets, default 50),
//! `FAKE_CUPS_LATENCY_MS` (time a job spends printing, default 0), `FAKE_CUPS_QUEUES`
//! (comma-separated valid queue names; any name is valid when unset), `PRINTER` / `LPDEST`
//! (default destination for `lp`). Message texts follow the CUPS tools approximately.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cups_cli::{self, Which};
use crate::sim::{Fault, SimConfig, SimError, SimPrinter};

const DEFAULT_PAPER: u32 = 50;
const USER: &str = "root";

/// Result of a fake command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// Exit code.
    pub code: i32,
}

impl Outcome {
    fn ok(stdout: String) -> Self {
        Self {
            stdout,
            stderr: String::new(),
            code: 0,
        }
    }

    pub(crate) fn fail(command: &str, message: impl std::fmt::Display) -> Self {
        Self {
            stdout: String::new(),
            stderr: format!("{command}: {message}\n"),
            code: 1,
        }
    }
}

/// Settings read from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeCupsEnv {
    /// `FAKE_CUPS_DIR`.
    pub dir: PathBuf,
    /// `FAKE_CUPS_PAPER`.
    pub paper: u32,
    /// `FAKE_CUPS_LATENCY_MS`.
    pub latency: Duration,
    /// `FAKE_CUPS_QUEUES`.
    pub queues: Option<Vec<String>>,
    /// `PRINTER`, else `LPDEST`.
    pub default_dest: Option<String>,
}

impl FakeCupsEnv {
    /// Read settings through `get` (pass `|k| std::env::var(k).ok()` for the process
    /// environment). The error text is ready to print.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let dir = get("FAKE_CUPS_DIR")
            .filter(|d| !d.is_empty())
            .ok_or("FAKE_CUPS_DIR is not set")?;
        let paper = match get("FAKE_CUPS_PAPER") {
            Some(v) => v
                .trim()
                .parse()
                .map_err(|_| format!("bad FAKE_CUPS_PAPER {v:?}"))?,
            None => DEFAULT_PAPER,
        };
        let latency = match get("FAKE_CUPS_LATENCY_MS") {
            Some(v) => Duration::from_millis(
                v.trim()
                    .parse()
                    .map_err(|_| format!("bad FAKE_CUPS_LATENCY_MS {v:?}"))?,
            ),
            None => Duration::ZERO,
        };
        let queues = get("FAKE_CUPS_QUEUES").map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .map(str::to_owned)
                .collect()
        });
        let default_dest = get("PRINTER")
            .filter(|v| !v.is_empty())
            .or_else(|| get("LPDEST").filter(|v| !v.is_empty()));
        Ok(Self {
            dir: dir.into(),
            paper,
            latency,
            queues,
            default_dest,
        })
    }

    /// Settings for tests that build the environment by hand.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            paper: DEFAULT_PAPER,
            latency: Duration::ZERO,
            queues: None,
            default_dest: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JobState {
    Queued,
    Completed,
    Aborted,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Stop {
    Offline,
    PaperOut,
    RibbonOut,
    DriverError,
}

impl Stop {
    /// Text on the line under `printer X disabled since ...`.
    fn message(self) -> &'static str {
        match self {
            Self::Offline => "Waiting for printer to become available.",
            Self::PaperOut => "Out of paper.",
            Self::RibbonOut => "Ribbon or ink cartridge is empty.",
            Self::DriverError => "Filter failed.",
        }
    }

    /// IPP printer-state-reasons keyword shown by `lpstat -l -p`.
    fn alert(self) -> &'static str {
        match self {
            Self::Offline => "connecting-to-device",
            Self::PaperOut => "media-empty-error",
            Self::RibbonOut => "marker-supply-empty-error",
            Self::DriverError => "cups-filter-error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JobFile {
    spool: PathBuf,
    stem: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Job {
    id: u64,
    queue: String,
    size: u64,
    files: Vec<JobFile>,
    copies: u32,
    printed: u32,
    state: JobState,
    submitted_ms: i64,
    ready_ms: i64,
}

impl Job {
    fn name(&self) -> String {
        format!("{}-{}", self.queue, self.id)
    }

    fn total_units(&self) -> u32 {
        self.copies
            .saturating_mul(u32::try_from(self.files.len()).unwrap_or(u32::MAX))
    }
}

/// Finished jobs kept in the state file (CUPS keeps a bounded history too).
const KEEP_FINISHED: usize = 100;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    next_job: u64,
    paper: Option<u32>,
    stop: Option<Stop>,
    jobs: Vec<Job>,
}

impl State {
    /// Drop the oldest finished jobs beyond [`KEEP_FINISHED`]; queued jobs always stay.
    fn prune(&mut self) {
        let finished = self
            .jobs
            .iter()
            .filter(|j| j.state != JobState::Queued)
            .count();
        let mut excess = finished.saturating_sub(KEEP_FINISHED);
        self.jobs.retain(|j| {
            if excess > 0 && j.state != JobState::Queued {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }
}

/// One fake CUPS installation (a directory).
#[derive(Debug, Clone)]
pub struct FakeCups {
    env: FakeCupsEnv,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn fmt_time(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%a %d %b %Y %I:%M:%S %p UTC")
        .to_string()
}

fn io_text(e: &io::Error) -> String {
    // "No such file or directory (os error 2)" -> "No such file or directory"
    let text = e.to_string();
    text.split(" (os error").next().unwrap_or(&text).to_owned()
}

/// Locked, loaded state. Dropping releases the directory lock.
struct Session {
    _lock: File,
    state: State,
}

impl FakeCups {
    /// Fake CUPS over `env.dir`.
    pub fn new(env: FakeCupsEnv) -> Self {
        Self { env }
    }

    /// Directory with rendered pages.
    pub fn out_dir(&self) -> PathBuf {
        self.env.dir.join("out")
    }

    fn fault_path(&self) -> PathBuf {
        self.env.dir.join("fault")
    }

    /// Current fault from the `fault` file (unreadable or unknown content means none).
    pub fn fault(&self) -> Fault {
        fs::read_to_string(self.fault_path())
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or_default()
    }

    /// Write the `fault` file.
    pub fn set_fault(&self, fault: Fault) -> io::Result<()> {
        fs::create_dir_all(&self.env.dir)?;
        let tmp = self.env.dir.join("fault.tmp");
        fs::write(&tmp, fault.as_str())?;
        fs::rename(tmp, self.fault_path())
    }

    /// Load sheets of paper.
    pub fn set_paper(&self, sheets: u32) -> io::Result<()> {
        let mut session = self.open()?;
        session.state.paper = Some(sheets);
        self.save(&session)
    }

    /// Sheets left.
    pub fn paper(&self) -> io::Result<u32> {
        Ok(self.open()?.state.paper.unwrap_or(self.env.paper))
    }

    fn open(&self) -> io::Result<Session> {
        fs::create_dir_all(self.env.dir.join("spool"))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.env.dir.join(".lock"))?;
        lock.lock()?;
        let state = match fs::read(self.env.dir.join("state.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e),
        };
        Ok(Session { _lock: lock, state })
    }

    fn save(&self, session: &Session) -> io::Result<()> {
        let tmp = self.env.dir.join("state.json.tmp");
        let mut state = session.state.clone();
        state.prune();
        let bytes = serde_json::to_vec(&state).map_err(io::Error::other)?;
        fs::write(&tmp, bytes)?;
        fs::rename(tmp, self.env.dir.join("state.json"))
    }

    /// Let the printer work through the queue: print what it can, record why it stopped.
    fn process(&self, state: &mut State, now: i64) {
        let fault = self.fault();
        let paper = state.paper.unwrap_or(self.env.paper);
        // Like CUPS, a stopped printer stays stopped (even with an empty queue) until the cause
        // is gone.
        let was_stopped = state.stop.take().is_some();
        if was_stopped {
            state.stop = match fault {
                Fault::Offline => Some(Stop::Offline),
                Fault::RibbonOut => Some(Stop::RibbonOut),
                Fault::DriverError => Some(Stop::DriverError),
                Fault::None if paper == 0 => Some(Stop::PaperOut),
                Fault::None => None,
            };
        }
        let mut config = SimConfig::new(self.out_dir());
        config.paper = paper;
        config.fault = fault;
        let Ok(sim) = SimPrinter::new(config) else {
            state.stop = Some(Stop::DriverError);
            state.paper = Some(paper);
            return;
        };
        state.paper = Some(paper);
        for job in state
            .jobs
            .iter_mut()
            .filter(|j| j.state == JobState::Queued)
        {
            if now < job.ready_ms {
                break;
            }
            let total = job.total_units();
            while job.printed < total {
                let file = &job.files[(job.printed / job.copies) as usize];
                let index = job.printed % job.copies + 1;
                match sim.print_copy(&file.spool, &file.stem, index) {
                    Ok(_) => {
                        job.printed += 1;
                        state.paper = Some(sim.state().paper_remaining);
                    }
                    Err(SimError::InvalidImage(_) | SimError::Io { .. }) => {
                        job.state = JobState::Aborted;
                        break;
                    }
                    Err(SimError::Offline) => state.stop = Some(Stop::Offline),
                    Err(SimError::PaperOut) => state.stop = Some(Stop::PaperOut),
                    Err(SimError::RibbonOut) => state.stop = Some(Stop::RibbonOut),
                    Err(SimError::DriverError) => state.stop = Some(Stop::DriverError),
                }
                if state.stop.is_some() {
                    return;
                }
            }
            if job.state == JobState::Queued {
                job.state = JobState::Completed;
            }
            for file in &job.files {
                let _ = fs::remove_file(&file.spool);
            }
        }
    }

    fn queue_known(&self, queue: &str) -> bool {
        self.env
            .queues
            .as_ref()
            .is_none_or(|q| q.iter().any(|k| k == queue))
    }

    /// `lp`: submit a job. `stdin` is read when no file (or `-`) is given.
    pub fn lp(&self, args: &[String], stdin: &mut dyn Read) -> Outcome {
        const CMD: &str = "lp";
        let parsed = match cups_cli::parse_lp(args) {
            Ok(p) => p,
            Err(e) => return Outcome::fail(CMD, e),
        };
        let Some(queue) = parsed
            .dest
            .clone()
            .or_else(|| self.env.default_dest.clone())
        else {
            return Outcome::fail(CMD, "Error - no default destination available.");
        };
        if !self.queue_known(&queue) {
            return Outcome::fail(CMD, "The printer or class does not exist.");
        }
        // (stem, bytes); stem None means "name it after the job".
        let mut inputs: Vec<(Option<String>, Vec<u8>)> = Vec::new();
        let sources = if parsed.files.is_empty() {
            vec!["-".to_owned()]
        } else {
            parsed.files
        };
        for source in sources {
            if source == "-" {
                let mut bytes = Vec::new();
                if let Err(e) = stdin.read_to_end(&mut bytes) {
                    return Outcome::fail(
                        CMD,
                        format!("Error - unable to read stdin - {}", io_text(&e)),
                    );
                }
                inputs.push((None, bytes));
            } else {
                match fs::read(&source) {
                    Ok(bytes) => {
                        let stem = Path::new(&source)
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned());
                        inputs.push((stem, bytes));
                    }
                    Err(e) => {
                        return Outcome::fail(
                            CMD,
                            format!("Error - unable to access \"{source}\" - {}", io_text(&e)),
                        );
                    }
                }
            }
        }
        let run = || -> io::Result<Outcome> {
            let mut session = self.open()?;
            let state = &mut session.state;
            state.next_job = state.next_job.max(1);
            let id = state.next_job;
            state.next_job += 1;
            let now = now_ms();
            let mut files = Vec::new();
            let mut size = 0;
            for (i, (stem, bytes)) in inputs.iter().enumerate() {
                let spool = self.env.dir.join("spool").join(format!("{id}-{i}.dat"));
                fs::write(&spool, bytes)?;
                size += bytes.len() as u64;
                files.push(JobFile {
                    spool,
                    stem: stem.clone().unwrap_or_else(|| format!("job-{id}")),
                });
            }
            let count = files.len();
            state.jobs.push(Job {
                id,
                queue: queue.clone(),
                size,
                files,
                copies: parsed.copies,
                printed: 0,
                state: JobState::Queued,
                submitted_ms: now,
                ready_ms: now + i64::try_from(self.env.latency.as_millis()).unwrap_or(0),
            });
            self.process(&mut session.state, now);
            self.save(&session)?;
            let stdout = if parsed.silent {
                String::new()
            } else {
                format!("request id is {queue}-{id} ({count} file(s))\n")
            };
            Ok(Outcome::ok(stdout))
        };
        run().unwrap_or_else(|e| Outcome::fail(CMD, format!("Error - {e}")))
    }

    /// `lpstat`: list jobs and printer state.
    pub fn lpstat(&self, args: &[String]) -> Outcome {
        const CMD: &str = "lpstat";
        let parsed = match cups_cli::parse_lpstat(args) {
            Ok(p) => p,
            Err(e) => return Outcome::fail(CMD, e),
        };
        let run = || -> io::Result<Outcome> {
            let mut session = self.open()?;
            let now = now_ms();
            self.process(&mut session.state, now);
            self.save(&session)?;
            let state = &session.state;
            let mut out = Outcome::ok(String::new());
            let bad = |out: &mut Outcome, name: &str| {
                out.stderr.push_str(&format!(
                    "lpstat: Invalid destination name in list \"{name}\".\n"
                ));
                out.code = 1;
            };
            if parsed.scheduler {
                out.stdout.push_str("scheduler is running\n");
            }
            if let Some(list) = &parsed.printers {
                let mut names = list.clone();
                if names.is_empty() {
                    names = self.env.queues.clone().unwrap_or_else(|| {
                        let mut seen: Vec<String> = Vec::new();
                        for job in &state.jobs {
                            if !seen.contains(&job.queue) {
                                seen.push(job.queue.clone());
                            }
                        }
                        seen
                    });
                }
                for name in names {
                    if !self.queue_known(&name) {
                        bad(&mut out, &name);
                        continue;
                    }
                    out.stdout
                        .push_str(&printer_status(state, &name, now, parsed.long));
                }
            }
            if let Some(list) = &parsed.jobs {
                for name in list.iter().filter(|n| !self.queue_known(n)) {
                    bad(&mut out, name);
                }
                for job in state.jobs.iter().filter(|j| {
                    (list.is_empty() || list.contains(&j.queue))
                        && self.queue_known(&j.queue)
                        && selected(j.state, parsed.which)
                }) {
                    out.stdout.push_str(&job_line(job, parsed.long));
                }
            }
            Ok(out)
        };
        run().unwrap_or_else(|e| Outcome::fail(CMD, format!("Error - {e}")))
    }

    /// `cancel`: cancel jobs by id, or by destination with `-a`.
    pub fn cancel(&self, args: &[String]) -> Outcome {
        const CMD: &str = "cancel";
        let parsed = match cups_cli::parse_cancel(args) {
            Ok(p) => p,
            Err(e) => return Outcome::fail(CMD, e),
        };
        let run = || -> io::Result<Outcome> {
            let mut session = self.open()?;
            let now = now_ms();
            self.process(&mut session.state, now);
            let mut out = Outcome::ok(String::new());
            let mut targets = parsed.targets.clone();
            if parsed.all && targets.is_empty() {
                targets = self.env.queues.clone().unwrap_or_else(|| {
                    let mut all: Vec<String> =
                        session.state.jobs.iter().map(|j| j.queue.clone()).collect();
                    all.sort();
                    all.dedup();
                    all
                });
            }
            for target in &targets {
                let job_ref = if self.env.queues.as_ref().is_some_and(|q| q.contains(target)) {
                    None
                } else {
                    cups_cli::parse_job_ref(target)
                };
                match job_ref {
                    Some((queue, id)) => {
                        let found = session
                            .state
                            .jobs
                            .iter_mut()
                            .find(|j| j.id == id && queue.is_none_or(|q| q == j.queue));
                        match found {
                            None => {
                                out.stderr.push_str(&format!(
                                    "cancel: cancel-job failed: Job #{id} doesn't exist.\n"
                                ));
                                out.code = 1;
                            }
                            Some(job) if job.state != JobState::Queued => {
                                out.stderr.push_str(&format!(
                                    "cancel: cancel-job failed: Job #{id} is already completed - can't cancel.\n"
                                ));
                                out.code = 1;
                            }
                            Some(job) => cancel_job(job),
                        }
                    }
                    None => {
                        let queued = session
                            .state
                            .jobs
                            .iter_mut()
                            .filter(|j| j.queue == *target && j.state == JobState::Queued);
                        if parsed.all {
                            queued.for_each(cancel_job);
                        } else if let Some(job) = queued.min_by_key(|j| j.id) {
                            cancel_job(job);
                        }
                    }
                }
            }
            self.process(&mut session.state, now);
            self.save(&session)?;
            Ok(out)
        };
        run().unwrap_or_else(|e| Outcome::fail(CMD, format!("Error - {e}")))
    }
}

fn cancel_job(job: &mut Job) {
    job.state = JobState::Canceled;
    for file in &job.files {
        let _ = fs::remove_file(&file.spool);
    }
}

fn selected(state: JobState, which: Which) -> bool {
    match which {
        Which::NotCompleted => state == JobState::Queued,
        Which::Completed => state != JobState::Queued,
        Which::Successful => state == JobState::Completed,
        Which::All => true,
    }
}

fn job_line(job: &Job, long: bool) -> String {
    let mut line = format!(
        "{:<23} {:<8} {:>8}   {}\n",
        job.name(),
        USER,
        job.size,
        fmt_time(job.submitted_ms)
    );
    if long {
        let status = match job.state {
            JobState::Queued => format!("\tQueued for {}\n", job.queue),
            JobState::Completed => "\tAlerts: job-completed-successfully\n".to_owned(),
            JobState::Aborted => "\tAlerts: aborted-by-system\n".to_owned(),
            JobState::Canceled => "\tAlerts: job-canceled-by-user\n".to_owned(),
        };
        line.push_str(&status);
    }
    line
}

fn printer_status(state: &State, queue: &str, now: i64, long: bool) -> String {
    let since = fmt_time(state.jobs.first().map_or(now, |j| j.submitted_ms));
    let mut out = match state.stop {
        Some(stop) => format!(
            "printer {queue} disabled since {since} -\n\t{}\n",
            stop.message()
        ),
        None => match state
            .jobs
            .iter()
            .find(|j| j.state == JobState::Queued && j.queue == queue && now < j.ready_ms)
        {
            Some(job) => {
                format!(
                    "printer {queue} now printing {}.  enabled since {since}\n",
                    job.name()
                )
            }
            None => format!("printer {queue} is idle.  enabled since {since}\n"),
        },
    };
    if long {
        let alerts = state.stop.map_or("none", Stop::alert);
        out.push_str(&format!(
            "\tForm mounted:\n\tContent types: any\n\tPrinter types: unknown\n\tDescription: {queue}\n\tAlerts: {alerts}\n\tLocation:\n\tConnection: direct\n\tOn fault: no alert\n\tAfter fault: continue\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage};
    use std::io::empty;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn png(dir: &Path, name: &str) -> String {
        let path = dir.join(name);
        RgbImage::from_pixel(30, 20, Rgb([1, 2, 3]))
            .save_with_format(&path, ImageFormat::Png)
            .unwrap();
        path.to_string_lossy().into_owned()
    }

    fn cups(dir: &Path) -> FakeCups {
        FakeCups::new(FakeCupsEnv::new(dir.join("cups")))
    }

    #[test]
    fn env_requires_dir() {
        let err = FakeCupsEnv::from_lookup(|_| None).unwrap_err();
        assert!(err.contains("FAKE_CUPS_DIR"));
        let err =
            FakeCupsEnv::from_lookup(|k| (k == "FAKE_CUPS_DIR").then(String::new)).unwrap_err();
        assert!(err.contains("FAKE_CUPS_DIR"));
    }

    #[test]
    fn env_defaults_and_overrides() {
        let env =
            FakeCupsEnv::from_lookup(|k| (k == "FAKE_CUPS_DIR").then(|| "/x".to_owned())).unwrap();
        assert_eq!(env, FakeCupsEnv::new("/x"));
        let env = FakeCupsEnv::from_lookup(|k| {
            Some(
                match k {
                    "FAKE_CUPS_DIR" => "/x",
                    "FAKE_CUPS_PAPER" => "3",
                    "FAKE_CUPS_LATENCY_MS" => "250",
                    "FAKE_CUPS_QUEUES" => "a, b,",
                    "LPDEST" => "b",
                    _ => return None,
                }
                .to_owned(),
            )
        })
        .unwrap();
        assert_eq!(env.paper, 3);
        assert_eq!(env.latency, Duration::from_millis(250));
        assert_eq!(env.queues, Some(v(&["a", "b"])));
        assert_eq!(env.default_dest.as_deref(), Some("b"));
    }

    #[test]
    fn env_prefers_printer_over_lpdest_and_rejects_bad_numbers() {
        let env = FakeCupsEnv::from_lookup(|k| {
            Some(match k {
                "FAKE_CUPS_DIR" => "/x".to_owned(),
                "PRINTER" => "p".to_owned(),
                "LPDEST" => "l".to_owned(),
                _ => return None,
            })
        })
        .unwrap();
        assert_eq!(env.default_dest.as_deref(), Some("p"));
        for var in ["FAKE_CUPS_PAPER", "FAKE_CUPS_LATENCY_MS"] {
            let err = FakeCupsEnv::from_lookup(|k| {
                Some(if k == "FAKE_CUPS_DIR" {
                    "/x".to_owned()
                } else if k == var {
                    "abc".to_owned()
                } else {
                    return None;
                })
            })
            .unwrap_err();
            assert!(err.contains(var));
        }
    }

    #[test]
    fn lp_prints_and_reports_request_id() {
        let dir = tempfile::tempdir().unwrap();
        let file = png(dir.path(), "j1-p1.png");
        let c = cups(dir.path());
        let out = c.lp(&v(&["-d", "fotoko", "-n", "2", &file]), &mut empty());
        assert_eq!(out.code, 0, "{out:?}");
        assert_eq!(out.stdout, "request id is fotoko-1 (1 file(s))\n");
        assert!(c.out_dir().join("j1-p1-1.png").exists());
        assert!(c.out_dir().join("j1-p1-2.png").exists());
        assert_eq!(c.paper().unwrap(), 48);
        let out = c.lp(&v(&["-d", "fotoko", "-s", &file]), &mut empty());
        assert_eq!(out.stdout, "");
        assert_eq!(c.paper().unwrap(), 47);
        // Job ids keep counting.
        let out = c.lp(&v(&["-d", "fotoko", &file]), &mut empty());
        assert_eq!(out.stdout, "request id is fotoko-3 (1 file(s))\n");
    }

    #[test]
    fn lp_reads_stdin_and_uses_default_dest() {
        let dir = tempfile::tempdir().unwrap();
        let file = png(dir.path(), "x.png");
        let bytes = fs::read(file).unwrap();
        let mut env = FakeCupsEnv::new(dir.path().join("cups"));
        env.default_dest = Some("dq".into());
        let c = FakeCups::new(env);
        let out = c.lp(&[], &mut bytes.as_slice());
        assert_eq!(out.stdout, "request id is dq-1 (1 file(s))\n");
        assert!(c.out_dir().join("job-1-1.png").exists());
        let out = c.lp(&v(&["-"]), &mut bytes.as_slice());
        assert_eq!(out.code, 0);
        assert!(c.out_dir().join("job-2-1.png").exists());
    }

    #[test]
    fn lp_multiple_files_in_one_job() {
        let dir = tempfile::tempdir().unwrap();
        let a = png(dir.path(), "a.png");
        let b = png(dir.path(), "b.png");
        let c = cups(dir.path());
        let out = c.lp(&v(&["-d", "q", "-n", "2", &a, &b]), &mut empty());
        assert_eq!(out.stdout, "request id is q-1 (2 file(s))\n");
        for name in ["a-1", "a-2", "b-1", "b-2"] {
            assert!(c.out_dir().join(format!("{name}.png")).exists(), "{name}");
        }
    }

    #[test]
    fn lp_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file = png(dir.path(), "a.png");
        let c = cups(dir.path());
        let out = c.lp(&v(&["-z"]), &mut empty());
        assert_eq!(
            (out.code, out.stderr.as_str()),
            (1, "lp: Unknown option \"-z\".\n")
        );
        let out = c.lp(&v(&[&file]), &mut empty());
        assert_eq!(out.code, 1);
        assert!(out.stderr.contains("no default destination"));
        let missing = dir.path().join("nope.png");
        let out = c.lp(&v(&["-d", "q", missing.to_str().unwrap()]), &mut empty());
        assert_eq!(out.code, 1);
        assert!(out.stderr.contains("unable to access"));
        assert!(out.stderr.contains("No such file or directory"));
        assert!(!out.stderr.contains("os error"));
        let mut env = FakeCupsEnv::new(dir.path().join("cups"));
        env.queues = Some(v(&["only"]));
        let out = FakeCups::new(env).lp(&v(&["-d", "other", &file]), &mut empty());
        assert_eq!(
            (out.code, out.stderr.as_str()),
            (1, "lp: The printer or class does not exist.\n")
        );
    }

    #[test]
    fn lp_stdin_read_failure() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("boom"))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let out = cups(dir.path()).lp(&v(&["-d", "q"]), &mut Broken);
        assert_eq!(out.code, 1);
        assert!(out.stderr.contains("unable to read stdin"));
    }

    #[test]
    fn fault_keeps_job_queued_until_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let file = png(dir.path(), "j-p.png");
        for (fault, message, alert) in [
            (
                Fault::Offline,
                "Waiting for printer to become available.",
                "connecting-to-device",
            ),
            (
                Fault::RibbonOut,
                "Ribbon or ink cartridge is empty.",
                "marker-supply-empty-error",
            ),
            (Fault::DriverError, "Filter failed.", "cups-filter-error"),
        ] {
            let c = FakeCups::new(FakeCupsEnv::new(dir.path().join(format!("c-{fault}"))));
            c.set_fault(fault).unwrap();
            assert_eq!(c.fault(), fault);
            let out = c.lp(&v(&["-d", "q", &file]), &mut empty());
            assert_eq!(out.code, 0);
            let jobs = c.lpstat(&v(&["-o", "q"]));
            assert!(jobs.stdout.starts_with("q-1 "), "{jobs:?}");
            let printer = c.lpstat(&v(&["-p", "q"]));
            assert!(
                printer.stdout.starts_with("printer q disabled since "),
                "{printer:?}"
            );
            assert!(printer.stdout.ends_with(&format!(" -\n\t{message}\n")));
            let long = c.lpstat(&v(&["-l", "-p", "q"]));
            assert!(long.stdout.contains(&format!("Alerts: {alert}")));
            c.set_fault(Fault::None).unwrap();
            let jobs = c.lpstat(&v(&["-o", "q"]));
            assert_eq!(jobs.stdout, "");
            assert!(c.out_dir().join("j-p-1.png").exists());
            let printer = c.lpstat(&v(&["-p", "q"]));
            assert!(
                printer
                    .stdout
                    .starts_with("printer q is idle.  enabled since ")
            );
        }
    }

    #[test]
    fn finished_jobs_are_pruned_but_queued_ones_stay() {
        let mut state = State::default();
        let job = |id, state| Job {
            id,
            queue: "q".into(),
            size: 1,
            files: vec![],
            copies: 1,
            printed: 0,
            state,
            submitted_ms: 0,
            ready_ms: 0,
        };
        for id in 1..=150 {
            state.jobs.push(job(id, JobState::Completed));
        }
        state.jobs.insert(0, job(999, JobState::Queued));
        state.prune();
        assert_eq!(state.jobs.len(), KEEP_FINISHED + 1);
        assert_eq!(state.jobs[0].id, 999);
        assert_eq!(state.jobs[1].id, 51);
    }

    #[test]
    fn stopped_printer_stays_stopped_after_cancel_until_fixed() {
        let dir = tempfile::tempdir().unwrap();
        let file = png(dir.path(), "j-p.png");
        let c = cups(dir.path());
        c.set_fault(Fault::Offline).unwrap();
        c.lp(&v(&["-d", "q", &file]), &mut empty());
        c.cancel(&v(&["q-1"]));
        assert!(c.lpstat(&v(&["-p", "q"])).stdout.contains("disabled since"));
        // The cause changes kind, the printer stays stopped.
        c.set_fault(Fault::RibbonOut).unwrap();
        assert!(c.lpstat(&v(&["-p", "q"])).stdout.contains("Ribbon"));
        c.set_fault(Fault::DriverError).unwrap();
        assert!(c.lpstat(&v(&["-p", "q"])).stdout.contains("Filter failed."));
        c.set_fault(Fault::None).unwrap();
        assert!(c.lpstat(&v(&["-p", "q"])).stdout.contains("is idle"));
    }

    #[test]
    fn paper_out_stops_mid_job_and_resumes_after_reload() {
        let dir = tempfile::tempdir().unwrap();
        let file = png(dir.path(), "j-p.png");
        let mut env = FakeCupsEnv::new(dir.path().join("cups"));
        env.paper = 1;
        let c = FakeCups::new(env);
        c.lp(&v(&["-d", "q", "-n", "3", &file]), &mut empty());
        assert!(c.out_dir().join("j-p-1.png").exists());
        assert!(!c.out_dir().join("j-p-2.png").exists());
        let printer = c.lpstat(&v(&["-p", "q"]));
        assert!(printer.stdout.contains("Out of paper."));
        assert!(c.lpstat(&v(&["-o", "q"])).stdout.starts_with("q-1 "));
        c.set_paper(5).unwrap();
        assert_eq!(c.lpstat(&v(&["-o", "q"])).stdout, "");
        assert!(c.out_dir().join("j-p-3.png").exists());
        assert_eq!(c.paper().unwrap(), 3);
    }

    #[test]
    fn invalid_image_aborts_job_and_next_job_prints() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.jpg");
        fs::write(&bad, b"not an image").unwrap();
        let good = png(dir.path(), "good.png");
        let c = cups(dir.path());
        c.lp(&v(&["-d", "q", bad.to_str().unwrap()]), &mut empty());
        c.lp(&v(&["-d", "q", &good]), &mut empty());
        assert_eq!(c.lpstat(&v(&["-o", "q"])).stdout, "");
        assert!(c.out_dir().join("good-1.png").exists());
        assert!(!c.out_dir().join("bad-1.png").exists());
        assert!(c.lpstat(&v(&["-p", "q"])).stdout.contains("is idle"));
        let done = c.lpstat(&v(&["-W", "completed", "-o", "q"]));
        assert_eq!(done.stdout.lines().count(), 2);
        let ok = c.lpstat(&v(&["-W", "successful", "-o", "q"]));
        assert_eq!(ok.stdout.lines().count(), 1);
        assert!(ok.stdout.starts_with("q-2 "));
        let long = c.lpstat(&v(&["-W", "all", "-l", "-o", "q"]));
        assert!(long.stdout.contains("Alerts: aborted-by-system"));
        assert!(long.stdout.contains("Alerts: job-completed-successfully"));
    }

    #[test]
    fn latency_keeps_job_queued_and_printer_busy() {
        let dir = tempfile::tempdir().unwrap();
        let file = png(dir.path(), "j-p.png");
        let mut env = FakeCupsEnv::new(dir.path().join("cups"));
        env.latency = Duration::from_millis(300);
        let c = FakeCups::new(env);
        c.lp(&v(&["-d", "q", &file]), &mut empty());
        assert!(c.lpstat(&v(&["-o", "q"])).stdout.starts_with("q-1 "));
        let printer = c.lpstat(&v(&["-p", "q"]));
        assert!(
            printer
                .stdout
                .starts_with("printer q now printing q-1.  enabled since ")
        );
        std::thread::sleep(Duration::from_millis(350));
        assert_eq!(c.lpstat(&v(&["-o", "q"])).stdout, "");
        assert!(c.out_dir().join("j-p-1.png").exists());
        let long = c.lpstat(&v(&["-l", "-W", "all", "-o", "q"]));
        assert!(long.stdout.contains("Alerts: job-completed-successfully"));
    }

    #[test]
    fn lpstat_formats_job_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = png(dir.path(), "j-p.png");
        let c = cups(dir.path());
        c.set_fault(Fault::Offline).unwrap();
        c.lp(&v(&["-d", "q", &file]), &mut empty());
        let out = c.lpstat(&v(&["-o"]));
        let line = out.stdout.lines().next().unwrap();
        let fields: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(fields[0], "q-1");
        assert_eq!(fields[1], "root");
        assert!(fields[2].parse::<u64>().unwrap() > 0);
        let long = c.lpstat(&v(&["-l", "-o", "q"]));
        assert!(long.stdout.contains("\tQueued for q\n"));
        // Plain lpstat lists jobs too; filtering by another queue shows nothing.
        assert!(c.lpstat(&[]).stdout.starts_with("q-1 "));
        assert_eq!(c.lpstat(&v(&["-o", "other"])).stdout, "");
    }

    #[test]
    fn lpstat_scheduler_and_all_printers() {
        let dir = tempfile::tempdir().unwrap();
        let file = png(dir.path(), "j-p.png");
        let c = cups(dir.path());
        assert_eq!(c.lpstat(&v(&["-r"])).stdout, "scheduler is running\n");
        assert_eq!(c.lpstat(&v(&["-p"])).stdout, "");
        c.lp(&v(&["-d", "q", &file]), &mut empty());
        c.lp(&v(&["-d", "q", &file]), &mut empty());
        assert_eq!(c.lpstat(&v(&["-p"])).stdout.lines().count(), 1);
        let mut env = FakeCupsEnv::new(dir.path().join("cups"));
        env.queues = Some(v(&["a", "b"]));
        let two = FakeCups::new(env).lpstat(&v(&["-p"]));
        assert_eq!(two.stdout.lines().count(), 2);
    }

    #[test]
    fn lpstat_invalid_destination_and_bad_args() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = FakeCupsEnv::new(dir.path().join("cups"));
        env.queues = Some(v(&["only"]));
        let c = FakeCups::new(env);
        let out = c.lpstat(&v(&["-p", "nope"]));
        assert_eq!(out.code, 1);
        assert_eq!(
            out.stderr,
            "lpstat: Invalid destination name in list \"nope\".\n"
        );
        let out = c.lpstat(&v(&["-o", "nope"]));
        assert_eq!(out.code, 1);
        let out = c.lpstat(&v(&["-x"]));
        assert_eq!(
            (out.code, out.stderr.as_str()),
            (1, "lpstat: Unknown option \"-x\".\n")
        );
    }

    #[test]
    fn cancel_by_id_dest_and_all() {
        let dir = tempfile::tempdir().unwrap();
        let file = png(dir.path(), "j-p.png");
        let c = cups(dir.path());
        c.set_fault(Fault::Offline).unwrap();
        for _ in 0..4 {
            c.lp(&v(&["-d", "q", &file]), &mut empty());
        }
        assert_eq!(c.cancel(&v(&["q-2"])).code, 0);
        assert_eq!(c.lpstat(&v(&["-o"])).stdout.lines().count(), 3);
        assert_eq!(c.cancel(&v(&["3"])).code, 0);
        // dest alone cancels the oldest job.
        assert_eq!(c.cancel(&v(&["q"])).code, 0);
        let left = c.lpstat(&v(&["-o"]));
        assert_eq!(left.stdout.lines().count(), 1);
        assert!(left.stdout.starts_with("q-4 "));
        assert_eq!(c.cancel(&v(&["-a", "q"])).code, 0);
        assert_eq!(c.lpstat(&v(&["-o"])).stdout, "");
        c.lp(&v(&["-d", "q", &file]), &mut empty());
        assert_eq!(c.cancel(&v(&["-a"])).code, 0);
        assert_eq!(c.lpstat(&v(&["-o"])).stdout, "");
        // Canceled jobs never print, even after the fault clears.
        c.set_fault(Fault::None).unwrap();
        c.lpstat(&v(&["-o"]));
        assert!(!c.out_dir().join("j-p-1.png").exists());
    }

    #[test]
    fn cancel_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file = png(dir.path(), "j-p.png");
        let c = cups(dir.path());
        c.lp(&v(&["-d", "q", &file]), &mut empty());
        let out = c.cancel(&v(&["q-9"]));
        assert_eq!(out.code, 1);
        assert_eq!(
            out.stderr,
            "cancel: cancel-job failed: Job #9 doesn't exist.\n"
        );
        let out = c.cancel(&v(&["q-1"]));
        assert_eq!(out.code, 1);
        assert!(out.stderr.contains("already completed"));
        assert_eq!(c.cancel(&v(&["-z"])).code, 1);
        // A name listed in FAKE_CUPS_QUEUES is a destination even if it looks like a job id.
        let mut env = FakeCupsEnv::new(dir.path().join("cups"));
        env.queues = Some(v(&["q-1"]));
        assert_eq!(FakeCups::new(env).cancel(&v(&["q-1"])).code, 0);
    }

    #[test]
    fn corrupt_state_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let c = cups(dir.path());
        fs::create_dir_all(dir.path().join("cups")).unwrap();
        fs::write(dir.path().join("cups/state.json"), b"{nope").unwrap();
        for out in [
            c.lp(&v(&["-d", "q"]), &mut b"x".as_slice()),
            c.lpstat(&v(&["-o"])),
            c.cancel(&v(&["-a"])),
        ] {
            assert_eq!(out.code, 1);
            assert!(out.stderr.contains("Error -"), "{out:?}");
        }
        assert!(c.paper().is_err());
        assert!(c.set_paper(1).is_err());
    }

    #[test]
    fn set_fault_and_unknown_fault_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = cups(dir.path());
        assert_eq!(c.fault(), Fault::None);
        c.set_fault(Fault::RibbonOut).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("cups/fault")).unwrap(),
            "ribbon_out"
        );
        fs::write(dir.path().join("cups/fault"), "garbage").unwrap();
        assert_eq!(c.fault(), Fault::None);
    }
}
