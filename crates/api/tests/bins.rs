//! The binaries fail fast on bad configuration, and `fotoko-local` serves.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

fn run_failing(bin: &str, env: &[(&str, &str)]) -> String {
    let output = Command::new(bin)
        .env_clear()
        .envs(env.iter().copied())
        .output()
        .unwrap();
    assert!(!output.status.success(), "{bin} should exit non-zero");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn http_lambda_fails_fast() {
    let bin = env!("CARGO_BIN_EXE_fotoko-http");
    assert!(run_failing(bin, &[]).contains("IOT_DATA_ENDPOINT"));
    assert!(
        run_failing(bin, &[("FOTOKO_MODE", "local")]).contains("only runs with FOTOKO_MODE=aws")
    );
    assert!(run_failing(bin, &[("FOTOKO_MODE", "bogus")]).contains("FOTOKO_MODE"));
}

#[test]
fn iot_status_lambda_fails_fast() {
    let bin = env!("CARGO_BIN_EXE_fotoko-iot-status");
    assert!(run_failing(bin, &[]).contains("TABLE_NAME"));
    assert!(
        run_failing(bin, &[("FOTOKO_MODE", "local")]).contains("only runs with FOTOKO_MODE=aws")
    );
}

#[test]
fn local_server_fails_fast_on_missing_credentials() {
    let out = run_failing(env!("CARGO_BIN_EXE_fotoko-local"), &[]);
    assert!(out.contains("PAYOS_CLIENT_ID"), "{out}");
}

#[test]
fn local_server_serves_healthz() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fotoko-local"))
        .env_clear()
        .env("FOTOKO_LOCAL_ADDR", "127.0.0.1:0")
        .env("PAYOS_CLIENT_ID", "c")
        .env("PAYOS_API_KEY", "k")
        .env("PAYOS_CHECKSUM_KEY", "s")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let addr = line
        .trim()
        .strip_prefix("fotoko-local listening on http://")
        .unwrap()
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
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(body, r#"{"ok":true}"#);
}
