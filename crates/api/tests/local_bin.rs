//! `fotoko-local` as a process.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Kills the child when dropped, so a failing assertion cannot leak a server.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn local_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_fotoko-local"));
    cmd.env_clear();
    cmd
}

#[test]
fn local_server_fails_fast_on_missing_credentials() {
    let output = local_bin().output().unwrap();
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("PAYOS_CLIENT_ID"), "{text}");
}

#[test]
fn local_server_serves_healthz() {
    let mut child = local_bin()
        .env("FOTOKO_LOCAL_ADDR", "127.0.0.1:0")
        .env("PAYOS_CLIENT_ID", "c")
        .env("PAYOS_API_KEY", "k")
        .env("PAYOS_CHECKSUM_KEY", "s")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let guard = Guard(child);

    // Read the announcement on a thread so a hung server fails the test instead of blocking it.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("announcement");
    let addr = line
        .trim()
        .strip_prefix("fotoko-local listening on http://")
        .expect("announcement format")
        .to_owned();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let body = runtime.block_on(async {
        reqwest::get(format!("http://{addr}/healthz"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    });
    drop(guard);
    assert_eq!(body, r#"{"ok":true}"#);
}
