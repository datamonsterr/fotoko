//! `CupsPrinter` from the Pi agent against the fake `lp`, `lpstat` and `cancel` binaries.
//!
//! This lives here, not in `fotoko-pi-agent`, because `CARGO_BIN_EXE_*` is only defined for
//! the package that owns the binaries. Cargo allows the dev-dependency cycle.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fotoko_pi_agent::cups::{CupsPrinter, CupsSettings};
use fotoko_pi_agent::printer::PrintContext;
use fotoko_pi_agent::{PrintError, Printer};
use fotoko_printer_sim::Fault;
use fotoko_printer_sim::fake_cups::{FakeCups, FakeCupsEnv};
use fotoko_protocol::{Media, PrinterState};
use image::{ImageFormat, Rgb, RgbImage};

struct Fixture {
    dir: tempfile::TempDir,
    cups: FakeCups,
    printer: CupsPrinter,
}

fn fixture_with(paper: u32, latency_ms: u64) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("cups");
    // The fake binaries read these from their environment; set them for the child processes
    // by wrapping the binaries in tiny scripts.
    let wrap = |name: &str, exe: &str| -> String {
        let path = dir.path().join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nFAKE_CUPS_DIR='{}' FAKE_CUPS_PAPER={paper} FAKE_CUPS_LATENCY_MS={latency_ms} exec '{exe}' \"$@\"\n",
                state.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    };
    let settings = CupsSettings {
        queue: "fotoko".into(),
        media: "w288h432".into(),
        lp_bin: wrap("lp", env!("CARGO_BIN_EXE_fake-lp")),
        lpstat_bin: wrap("lpstat", env!("CARGO_BIN_EXE_fake-lpstat")),
        cancel_bin: wrap("cancel", env!("CARGO_BIN_EXE_fake-cancel")),
        print_timeout: Duration::from_secs(10),
        poll_interval: Duration::from_millis(30),
        command_timeout: Duration::from_secs(10),
    };
    let mut env = FakeCupsEnv::new(&state);
    env.paper = paper;
    Fixture {
        dir,
        cups: FakeCups::new(env),
        printer: CupsPrinter::new(settings),
    }
}

fn fixture() -> Fixture {
    fixture_with(50, 0)
}

impl Fixture {
    fn lpstat(&self, args: &[&str]) -> String {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_fake-lpstat"))
            .args(args)
            .env("FAKE_CUPS_DIR", self.dir.path().join("cups"))
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn image(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        RgbImage::from_pixel(60, 40, Rgb([5, 5, 5]))
            .save_with_format(&path, ImageFormat::Png)
            .unwrap();
        path
    }

    fn out(&self, name: &str) -> PathBuf {
        self.cups.out_dir().join(name)
    }
}

fn ctx() -> PrintContext {
    PrintContext::new("job-1")
}

fn exists(path: &Path) -> bool {
    path.exists()
}

#[tokio::test]
async fn prints_through_lp_and_waits_for_the_queue_to_drain() {
    let f = fixture();
    let file = f.image("job1-ph1.png");
    assert_eq!(f.printer.state().await, PrinterState::Idle);
    f.printer
        .print(&file, 2, Media::Photo4x6, &ctx())
        .await
        .unwrap();
    assert!(exists(&f.out("job1-ph1-1.png")) && exists(&f.out("job1-ph1-2.png")));
    assert_eq!(f.cups.paper().unwrap(), 48);
    assert_eq!(f.printer.state().await, PrinterState::Idle);
}

#[tokio::test]
async fn waits_while_the_job_is_still_printing() {
    let f = fixture_with(50, 400);
    let file = f.image("j-p.png");
    let started = std::time::Instant::now();
    let printing = {
        let printer = f.printer.clone();
        let file = file.clone();
        tokio::spawn(async move { printer.print(&file, 1, Media::Photo4x6, &ctx()).await })
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while f.printer.state().await != PrinterState::Printing {
        assert!(std::time::Instant::now() < deadline, "never printing");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    printing.await.unwrap().unwrap();
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(exists(&f.out("j-p-1.png")));
}

#[tokio::test]
async fn faults_become_pi_errors_and_the_job_is_cancelled() {
    let f = fixture();
    let file = f.image("j-p.png");
    for (fault, check, state) in [
        (
            Fault::Offline,
            (|e: &PrintError| matches!(e, PrintError::Offline(_))) as fn(&PrintError) -> bool,
            PrinterState::Offline,
        ),
        (
            Fault::RibbonOut,
            |e| *e == PrintError::RibbonOut,
            PrinterState::Error,
        ),
        (
            Fault::DriverError,
            |e| matches!(e, PrintError::Driver(_)),
            PrinterState::Error,
        ),
    ] {
        f.cups.set_fault(fault).unwrap();
        let error = f
            .printer
            .print(&file, 1, Media::Photo4x6, &ctx())
            .await
            .unwrap_err();
        assert!(check(&error), "{fault}: {error}");
        assert_eq!(f.printer.state().await, state, "{fault}");
        // The cancelled job must not print once the fault is gone.
        f.cups.set_fault(Fault::None).unwrap();
        assert_eq!(f.printer.state().await, PrinterState::Idle);
        assert!(
            !exists(&f.out("j-p-1.png")),
            "{fault}: cancelled job printed"
        );
    }
}

#[tokio::test]
async fn out_of_paper_is_reported_and_recovers_after_reloading() {
    let f = fixture_with(1, 0);
    let file = f.image("j-p.png");
    let error = f
        .printer
        .print(&file, 2, Media::Photo4x6, &ctx())
        .await
        .unwrap_err();
    assert_eq!(error, PrintError::PaperOut);
    assert_eq!(f.printer.state().await, PrinterState::Error);
    f.cups.set_paper(5).unwrap();
    assert_eq!(f.printer.state().await, PrinterState::Idle);
    f.printer
        .print(&file, 1, Media::Photo4x6, &ctx())
        .await
        .unwrap();
}

#[tokio::test]
async fn unknown_queue_is_reported_as_offline() {
    let f = fixture();
    let file = f.image("j-p.png");
    let dir = f.dir.path().join("cups");
    let lp = f.dir.path().join("lp-strict");
    fs::write(
        &lp,
        format!(
            "#!/bin/sh\nFAKE_CUPS_DIR='{}' FAKE_CUPS_QUEUES=other exec '{}' \"$@\"\n",
            dir.display(),
            env!("CARGO_BIN_EXE_fake-lp")
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&lp, fs::Permissions::from_mode(0o755)).unwrap();
    let strict = CupsPrinter::new(CupsSettings {
        queue: "fotoko".into(),
        media: "w288h432".into(),
        lp_bin: lp.to_string_lossy().into_owned(),
        lpstat_bin: "false".into(),
        cancel_bin: "false".into(),
        print_timeout: Duration::from_secs(5),
        poll_interval: Duration::from_millis(20),
        command_timeout: Duration::from_secs(5),
    });
    let error = strict
        .print(&file, 1, Media::Photo4x6, &ctx())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, PrintError::Offline(m) if m.contains("does not exist")),
        "{error}"
    );
}

#[tokio::test]
async fn undecodable_image_is_aborted_by_cups_and_reported_as_a_driver_error() {
    // CUPS aborts the job when the filter fails and it leaves the queue; only the history
    // (`lpstat -l -W completed -o`) tells it apart from a successful print.
    let f = fixture();
    let bad = f.dir.path().join("bad.jpg");
    fs::write(&bad, b"nope").unwrap();
    let error = f
        .printer
        .print(&bad, 1, Media::Photo4x6, &ctx())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, PrintError::Driver(m) if m.contains("aborted")),
        "{error}"
    );
    assert!(!exists(&f.out("bad-1.png")));
}

#[tokio::test]
async fn cancelling_withdraws_the_queued_job_so_it_never_prints() {
    let f = fixture_with(50, 60_000);
    let file = f.image("j-p.png");
    let ctx = PrintContext::new("job-42");
    let token = ctx.cancel.clone();
    let printer = f.printer.clone();
    let path = file.clone();
    let task = tokio::spawn(async move { printer.print(&path, 1, Media::Photo4x6, &ctx).await });
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while f.printer.state().await != PrinterState::Printing {
        assert!(std::time::Instant::now() < deadline, "never printing");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    token.cancel();
    assert_eq!(task.await.unwrap().unwrap_err(), PrintError::Cancelled);
    assert_eq!(f.printer.state().await, PrinterState::Idle);
    let history = f.lpstat(&["-l", "-W", "completed", "-o", "fotoko"]);
    assert!(
        history.contains("Alerts: job-canceled-by-user"),
        "{history}"
    );
}
