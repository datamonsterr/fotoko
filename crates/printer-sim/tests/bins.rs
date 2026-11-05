//! The `fake-lp`, `fake-lpstat` and `fake-cancel` binaries as separate processes sharing one
//! `FAKE_CUPS_DIR`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use image::{ImageFormat, Rgb, RgbImage};

struct Cups {
    dir: tempfile::TempDir,
}

impl Cups {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn state(&self) -> PathBuf {
        self.dir.path().join("cups")
    }

    fn run(&self, exe: &str, args: &[&str], envs: &[(&str, &str)]) -> Output {
        Command::new(exe)
            .args(args)
            .env("FAKE_CUPS_DIR", self.state())
            .env_remove("PRINTER")
            .env_remove("LPDEST")
            .envs(envs.iter().copied())
            .output()
            .unwrap()
    }

    fn lp(&self, args: &[&str]) -> Output {
        self.run(env!("CARGO_BIN_EXE_fake-lp"), args, &[])
    }

    fn lpstat(&self, args: &[&str]) -> Output {
        self.run(env!("CARGO_BIN_EXE_fake-lpstat"), args, &[])
    }

    fn cancel(&self, args: &[&str]) -> Output {
        self.run(env!("CARGO_BIN_EXE_fake-cancel"), args, &[])
    }

    fn image(&self, name: &str) -> String {
        let path = self.dir.path().join(name);
        RgbImage::from_pixel(60, 40, Rgb([9, 9, 9]))
            .save_with_format(&path, ImageFormat::Png)
            .unwrap();
        path.to_string_lossy().into_owned()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn lp_then_lpstat_across_processes() {
    let c = Cups::new();
    let file = c.image("job1-ph1.png");
    let out = c.lp(&[
        "-d",
        "fotoko",
        "-n",
        "2",
        "-o",
        "media=w288h432",
        "-o",
        "fit-to-page",
        &file,
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "request id is fotoko-1 (1 file(s))\n");
    for n in [1, 2] {
        let page = c.state().join(format!("out/job1-ph1-{n}.png"));
        let img = image::open(&page).unwrap();
        assert_eq!((img.width(), img.height()), (1800, 1200));
    }
    let jobs = c.lpstat(&["-o", "fotoko"]);
    assert!(jobs.status.success());
    assert_eq!(text(&jobs.stdout), "");
    let printer = c.lpstat(&["-p", "fotoko"]);
    assert!(text(&printer.stdout).starts_with("printer fotoko is idle.  enabled since "));
    let done = c.lpstat(&["-W", "completed", "-o", "fotoko"]);
    assert!(text(&done.stdout).starts_with("fotoko-1 "));
}

#[test]
fn fault_file_flips_behaviour_between_invocations() {
    let c = Cups::new();
    let file = c.image("j-p.png");
    fs::create_dir_all(c.state()).unwrap();
    fs::write(c.state().join("fault"), "ribbon_out\n").unwrap();
    let out = c.lp(&["-d", "q", "-s", &file]);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout), "");
    let printer = text(&c.lpstat(&["-p", "q"]).stdout);
    assert!(printer.contains("disabled since"), "{printer}");
    assert!(printer.contains("Ribbon or ink cartridge is empty."));
    assert!(text(&c.lpstat(&["-o", "q"]).stdout).starts_with("q-1 "));
    fs::write(c.state().join("fault"), "").unwrap();
    assert_eq!(text(&c.lpstat(&["-o", "q"]).stdout), "");
    assert!(c.state().join("out/j-p-1.png").exists());
}

#[test]
fn cancel_removes_the_job() {
    let c = Cups::new();
    let file = c.image("j-p.png");
    fs::create_dir_all(c.state()).unwrap();
    fs::write(c.state().join("fault"), "offline").unwrap();
    c.lp(&["-d", "q", &file]);
    let out = c.cancel(&["q-1"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&c.lpstat(&["-o", "q"]).stdout), "");
    let again = c.cancel(&["q-1"]);
    assert_eq!(again.status.code(), Some(1));
    assert!(text(&again.stderr).starts_with("cancel: cancel-job failed:"));
}

#[test]
fn lp_reads_stdin() {
    let c = Cups::new();
    let file = c.image("x.png");
    let bytes = fs::read(file).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fake-lp"))
        .args(["-d", "q"])
        .env("FAKE_CUPS_DIR", c.state())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(text(&out.stdout), "request id is q-1 (1 file(s))\n");
    assert!(c.state().join("out/job-1-1.png").exists());
}

#[test]
fn exit_codes_and_messages() {
    let c = Cups::new();
    let file = c.image("a.png");
    // No FAKE_CUPS_DIR.
    let out = Command::new(env!("CARGO_BIN_EXE_fake-lp"))
        .env_remove("FAKE_CUPS_DIR")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(text(&out.stderr), "lp: FAKE_CUPS_DIR is not set\n");
    // No destination, bad option, missing file, unknown queue.
    assert_eq!(
        text(&c.lp(&[&file]).stderr),
        "lp: Error - no default destination available.\n"
    );
    assert_eq!(c.lp(&["-z"]).status.code(), Some(1));
    let missing = c.lp(&["-d", "q", "/definitely/not/here.png"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(text(&missing.stderr).contains("unable to access"));
    let unknown = c.run(
        env!("CARGO_BIN_EXE_fake-lp"),
        &["-d", "other", &file],
        &[("FAKE_CUPS_QUEUES", "fotoko")],
    );
    assert_eq!(
        text(&unknown.stderr),
        "lp: The printer or class does not exist.\n"
    );
    let bad = c.lpstat(&["-W", "nope"]);
    assert_eq!(bad.status.code(), Some(1));
    // PRINTER supplies the default destination.
    let ok = c.run(
        env!("CARGO_BIN_EXE_fake-lp"),
        &[&file],
        &[("PRINTER", "dflt")],
    );
    assert_eq!(text(&ok.stdout), "request id is dflt-1 (1 file(s))\n");
}

#[test]
fn env_controls_paper_and_latency() {
    let c = Cups::new();
    let file = c.image("j-p.png");
    let lp = env!("CARGO_BIN_EXE_fake-lp");
    c.run(
        lp,
        &["-d", "q", "-n", "3", &file],
        &[("FAKE_CUPS_PAPER", "1")],
    );
    let printer = text(&c.lpstat(&["-l", "-p", "q"]).stdout);
    assert!(printer.contains("Out of paper."), "{printer}");
    assert!(printer.contains("Alerts: media-empty-error"));
    let slow = Cups::new();
    slow.run(
        lp,
        &["-d", "q", &file],
        &[("FAKE_CUPS_LATENCY_MS", "60000")],
    );
    let status = text(&slow.lpstat(&["-p", "q"]).stdout);
    assert!(
        status.starts_with("printer q now printing q-1."),
        "{status}"
    );
}

#[test]
fn concurrent_invocations_do_not_lose_jobs() {
    let c = Cups::new();
    let file = c.image("j-p.png");
    let children: Vec<_> = (0..6)
        .map(|_| {
            Command::new(env!("CARGO_BIN_EXE_fake-lp"))
                .args(["-d", "q", &file])
                .env("FAKE_CUPS_DIR", c.state())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut ids: Vec<String> = children
        .into_iter()
        .map(|ch| text(&ch.wait_with_output().unwrap().stdout))
        .collect();
    ids.sort();
    let expected: Vec<String> = (1..=6)
        .map(|n| format!("request id is q-{n} (1 file(s))\n"))
        .collect();
    assert_eq!(ids, expected);
}
