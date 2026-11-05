//! `CupsPrinter` from the Pi agent against the fake `lp`, `lpstat` and `cancel` binaries.
//!
//! This lives here, not in `fotoko-pi-agent`, because `CARGO_BIN_EXE_*` is only defined for
//! the package that owns the binaries. Cargo allows the dev-dependency cycle.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fotoko_pi_agent::cups::{CupsPrinter, CupsSettings};
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

fn exists(path: &Path) -> bool {
    path.exists()
}

#[tokio::test]
async fn prints_through_lp_and_waits_for_the_queue_to_drain() {
    let f = fixture();
    let file = f.image("job1-ph1.png");
    assert_eq!(f.printer.state().await, PrinterState::Idle);
    f.printer.print(&file, 2, Media::Photo4x6).await.unwrap();
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
        tokio::spawn(async move { printer.print(&file, 1, Media::Photo4x6).await })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(f.printer.state().await, PrinterState::Printing);
    printing.await.unwrap().unwrap();
    assert!(started.elapsed() >= Duration::from_millis(350));
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
            .print(&file, 1, Media::Photo4x6)
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
        .print(&file, 2, Media::Photo4x6)
        .await
        .unwrap_err();
    assert_eq!(error, PrintError::PaperOut);
    assert_eq!(f.printer.state().await, PrinterState::Error);
    f.cups.set_paper(5).unwrap();
    assert_eq!(f.printer.state().await, PrinterState::Idle);
    f.printer.print(&file, 1, Media::Photo4x6).await.unwrap();
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
    let error = strict.print(&file, 1, Media::Photo4x6).await.unwrap_err();
    assert!(
        matches!(&error, PrintError::Offline(m) if m.contains("does not exist")),
        "{error}"
    );
}

#[tokio::test]
async fn undecodable_image_leaves_the_queue_like_a_cups_filter_failure() {
    // CUPS aborts the job when the filter fails, so lpstat cannot tell it from success. The
    // agent therefore validates headers before printing (see `download`).
    let f = fixture();
    let bad = f.dir.path().join("bad.jpg");
    fs::write(&bad, b"nope").unwrap();
    f.printer.print(&bad, 1, Media::Photo4x6).await.unwrap();
    assert!(!exists(&f.out("bad-1.png")));
}
