//! The binaries fail fast on bad configuration, and `fotoko-local` serves.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;

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
fn expire_lambda_fails_fast() {
    let bin = env!("CARGO_BIN_EXE_fotoko-expire");
    assert!(run_failing(bin, &[]).contains("TABLE_NAME"));
    assert!(
        run_failing(bin, &[("FOTOKO_MODE", "local")]).contains("only runs with FOTOKO_MODE=aws")
    );
}
