//! The `fotoko-pi-agent` executable: config errors, SIGTERM handling.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::process::{Command, Stdio};

use common::*;
use fotoko_protocol::{JobState, PrinterState};

const EXE: &str = env!("CARGO_BIN_EXE_fotoko-pi-agent");

#[test]
fn missing_config_exits_with_an_error() {
    let out = Command::new(EXE)
        .args(["--config", "/nonexistent/agent.toml"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot read config /nonexistent/agent.toml"),
        "{stderr}"
    );
}

#[test]
fn invalid_config_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.toml");
    std::fs::write(&path, "printer_id = \"\"\n").unwrap();
    let out = Command::new(EXE)
        .arg("--config")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid config"));
}

#[test]
fn help_and_version() {
    let out = Command::new(EXE).arg("--help").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("--config"));
    let out = Command::new(EXE).arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains(env!("CARGO_PKG_VERSION")));
}

#[tokio::test]
async fn sigterm_shuts_down_cleanly_after_serving_a_job() {
    let port = broker().await;
    let base = image_server().await;
    let state = tempfile::tempdir().unwrap();
    let mut cloud = Cloud::connect(port).await;
    let config = format!(
        "printer_id = \"{PRINTER}\"\n[mqtt]\nhost = \"127.0.0.1\"\nport = {port}\ntls = false\n[printer]\nbackend = \"sim\"\n[sim]\nout_dir = \"{s}/sim\"\n[storage]\nstate_dir = \"{s}\"\nallow_insecure_http = true\n",
        s = state.path().display()
    );
    let path = state.path().join("agent.toml");
    std::fs::write(&path, config).unwrap();
    let mut child = Command::new(EXE)
        .arg("--config")
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    cloud.heartbeat_where(PrinterState::Idle).await;
    cloud
        .publish_job(&job(
            &base,
            "job-bin",
            PRINTER,
            &[("/land.jpg", fotoko_protocol::ContentType::Jpeg, 1)],
        ))
        .await;
    assert_eq!(
        cloud.job_statuses().await.last().unwrap().state,
        JobState::Completed
    );
    assert!(state.path().join("sim/job-bin-0-ph0-1.png").exists());

    let kill = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill.success());
    cloud.heartbeat_where(PrinterState::Offline).await;
    let status = tokio::task::spawn_blocking(move || child.wait().unwrap())
        .await
        .unwrap();
    assert_eq!(status.code(), Some(0));
}
