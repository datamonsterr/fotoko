//! Agent integration tests: real agent + embedded rumqttd broker + local HTTP image server +
//! SimPrinter. The CUPS backend against the fake `lp`/`lpstat` lives in
//! `crates/printer-sim/tests/cups.rs` (it needs those binaries built).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::*;
use fotoko_printer_sim::Fault;
use fotoko_protocol::{ContentType, JobState, PiErrorCode, PrinterState};

fn states(statuses: &[fotoko_protocol::JobStatus]) -> Vec<JobState> {
    statuses.iter().map(|s| s.state).collect()
}

struct Env {
    port: u16,
    base: String,
    state: tempfile::TempDir,
}

async fn env() -> Env {
    Env {
        port: broker().await,
        base: image_server().await,
        state: tempfile::tempdir().unwrap(),
    }
}

async fn online(env: &Env, paper: u32) -> (RunningAgent, Cloud) {
    let mut cloud = Cloud::connect(env.port).await;
    let agent = start_agent(
        agent_config(env.port, env.state.path(), 30),
        paper,
        Duration::ZERO,
    );
    // The agent announces itself when it connects, after subscribing.
    cloud.heartbeat_where(PrinterState::Idle).await;
    (agent, cloud)
}

#[tokio::test]
async fn happy_path_prints_every_copy_and_reports_status() {
    let env = env().await;
    let (agent, mut cloud) = online(&env, 20).await;
    let job = job(
        &env.base,
        "job-1",
        PRINTER,
        &[
            ("/land.jpg", ContentType::Jpeg, 2),
            ("/port.png", ContentType::Png, 1),
        ],
    );
    cloud.publish_job(&job).await;
    let statuses = cloud.job_statuses().await;
    assert_eq!(
        states(&statuses),
        [JobState::Received, JobState::Printing, JobState::Completed]
    );
    assert_eq!(statuses[2].printed_copies, 3);
    assert!(
        statuses
            .iter()
            .all(|s| s.job_id.as_str() == "job-1" && s.order_id.as_str() == "order-job-1")
    );
    assert!(
        statuses
            .iter()
            .all(|s| s.printer_id.as_str() == PRINTER && s.error.is_none())
    );

    assert_eq!(
        page_size(&agent.out_dir.join("job-1-0-ph0-1.png")),
        (1800, 1200)
    );
    assert_eq!(
        page_size(&agent.out_dir.join("job-1-0-ph0-2.png")),
        (1800, 1200)
    );
    assert_eq!(
        page_size(&agent.out_dir.join("job-1-1-ph1-1.png")),
        (1200, 1800)
    );
    assert_eq!(agent.sim.state().paper_remaining, 17);
    // Temp downloads are gone.
    let leftovers = std::fs::read_dir(env.state.path().join("tmp"))
        .unwrap()
        .count();
    assert_eq!(leftovers, 0);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn paper_out_fails_the_job_and_heartbeat_turns_to_error() {
    let env = env().await;
    let mut cloud = Cloud::connect(env.port).await;
    let agent = start_agent(
        agent_config(env.port, env.state.path(), 1),
        1,
        Duration::ZERO,
    );
    cloud.heartbeat_where(PrinterState::Idle).await;
    cloud
        .publish_job(&job(
            &env.base,
            "job-p",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 3)],
        ))
        .await;
    let statuses = cloud.job_statuses().await;
    assert_eq!(
        states(&statuses),
        [JobState::Received, JobState::Printing, JobState::Failed]
    );
    let error = statuses[2].error.clone().unwrap();
    assert_eq!(error.code, PiErrorCode::PaperOut);
    assert_eq!(statuses[2].printed_copies, 0);
    // Out of paper: the periodic heartbeat reports the error state.
    let hb = cloud.heartbeat_where(PrinterState::Error).await;
    assert_eq!(hb.printer_id.as_str(), PRINTER);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn printer_faults_map_to_pi_error_codes() {
    let env = env().await;
    let (agent, mut cloud) = online(&env, 50).await;
    for (fault, code) in [
        (Fault::Offline, PiErrorCode::PrinterOffline),
        (Fault::RibbonOut, PiErrorCode::RibbonOut),
        (Fault::DriverError, PiErrorCode::DriverError),
    ] {
        agent.sim.set_fault(fault);
        let id = format!("job-{fault}");
        cloud
            .publish_job(&job(
                &env.base,
                &id,
                PRINTER,
                &[("/land.jpg", ContentType::Jpeg, 1)],
            ))
            .await;
        let statuses = cloud.job_statuses().await;
        assert_eq!(
            statuses.last().unwrap().error.as_ref().unwrap().code,
            code,
            "{fault}"
        );
    }
    agent.sim.set_fault(Fault::None);
    cloud
        .publish_job(&job(
            &env.base,
            "job-ok",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .await;
    assert_eq!(
        cloud.job_statuses().await.last().unwrap().state,
        JobState::Completed
    );
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn jobs_for_another_printer_are_ignored() {
    let env = env().await;
    let (agent, mut cloud) = online(&env, 20).await;
    // Wrong printerId inside the payload, delivered on this printer's topic.
    cloud
        .publish_job_on(
            PRINTER,
            &job(
                &env.base,
                "job-x",
                "stand-002",
                &[("/land.jpg", ContentType::Jpeg, 1)],
            ),
        )
        .await;
    cloud
        .publish_job(&job(
            &env.base,
            "job-ok",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .await;
    let statuses = cloud.job_statuses().await;
    assert!(
        statuses.iter().all(|s| s.job_id.as_str() == "job-ok"),
        "{statuses:?}"
    );
    assert!(!agent.out_dir.join("job-x-0-ph0-1.png").exists());
    assert_eq!(agent.sim.state().paper_remaining, 19);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn duplicate_job_ids_print_once() {
    let env = env().await;
    let (agent, mut cloud) = online(&env, 20).await;
    let first = job(
        &env.base,
        "job-d",
        PRINTER,
        &[("/land.jpg", ContentType::Jpeg, 1)],
    );
    cloud.publish_job(&first).await;
    cloud.publish_job(&first).await;
    cloud
        .publish_job(&job(
            &env.base,
            "job-after",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .await;
    let a = cloud.job_statuses().await;
    let b = cloud.job_statuses().await;
    assert!(a.iter().all(|s| s.job_id.as_str() == "job-d"));
    assert!(b.iter().all(|s| s.job_id.as_str() == "job-after"), "{b:?}");
    assert_eq!(agent.sim.state().paper_remaining, 18);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn dedupe_survives_an_agent_restart() {
    let env = env().await;
    let (agent, mut cloud) = online(&env, 20).await;
    let first = job(
        &env.base,
        "job-r",
        PRINTER,
        &[("/land.jpg", ContentType::Jpeg, 1)],
    );
    cloud.publish_job(&first).await;
    assert_eq!(
        cloud.job_statuses().await.last().unwrap().state,
        JobState::Completed
    );
    agent.stop().await.unwrap();
    cloud.heartbeat_where(PrinterState::Offline).await;

    let agent = start_agent(
        agent_config(env.port, env.state.path(), 30),
        20,
        Duration::ZERO,
    );
    cloud.heartbeat_where(PrinterState::Idle).await;
    cloud.publish_job(&first).await;
    cloud
        .publish_job(&job(
            &env.base,
            "job-next",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .await;
    let statuses = cloud.job_statuses().await;
    assert!(
        statuses.iter().all(|s| s.job_id.as_str() == "job-next"),
        "{statuses:?}"
    );
    assert_eq!(
        agent.sim.state().paper_remaining,
        19,
        "only job-next printed"
    );
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn bad_downloads_fail_with_the_right_code() {
    let env = env().await;
    let (agent, mut cloud) = online(&env, 20).await;
    let cases = [
        (
            "job-404",
            "/gone",
            ContentType::Jpeg,
            PiErrorCode::DownloadFailed,
        ),
        (
            "job-big",
            "/huge.png",
            ContentType::Png,
            PiErrorCode::InvalidImage,
        ),
        (
            "job-magic",
            "/mislabeled.jpg",
            ContentType::Jpeg,
            PiErrorCode::InvalidImage,
        ),
        (
            "job-text",
            "/text.jpg",
            ContentType::Jpeg,
            PiErrorCode::InvalidImage,
        ),
    ];
    for (id, path, content_type, code) in cases {
        cloud
            .publish_job(&job(&env.base, id, PRINTER, &[(path, content_type, 1)]))
            .await;
        let statuses = cloud.job_statuses().await;
        assert_eq!(
            states(&statuses),
            [JobState::Received, JobState::Printing, JobState::Failed],
            "{id}"
        );
        assert_eq!(statuses[2].error.as_ref().unwrap().code, code, "{id}");
        assert_eq!(statuses[2].printed_copies, 0);
    }
    assert_eq!(agent.sim.state().paper_remaining, 20, "no paper used");
    assert_eq!(
        std::fs::read_dir(env.state.path().join("tmp"))
            .unwrap()
            .count(),
        0
    );
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn one_bad_photo_fails_the_job_before_anything_prints() {
    let env = env().await;
    let (agent, mut cloud) = online(&env, 20).await;
    cloud
        .publish_job(&job(
            &env.base,
            "job-mix",
            PRINTER,
            &[
                ("/land.jpg", ContentType::Jpeg, 2),
                ("/gone", ContentType::Jpeg, 1),
            ],
        ))
        .await;
    let statuses = cloud.job_statuses().await;
    assert_eq!(
        statuses.last().unwrap().error.as_ref().unwrap().code,
        PiErrorCode::DownloadFailed
    );
    assert_eq!(agent.sim.state().paper_remaining, 20);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn malformed_messages_are_ignored_and_the_agent_keeps_working() {
    let env = env().await;
    let (agent, mut cloud) = online(&env, 20).await;
    cloud.publish_raw(PRINTER, b"{ not json".to_vec()).await;
    cloud
        .publish_job(&job(
            &env.base,
            "job-after",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .await;
    let statuses = cloud.job_statuses().await;
    assert!(statuses.iter().all(|s| s.job_id.as_str() == "job-after"));
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn jobs_are_processed_one_at_a_time_in_order() {
    let env = env().await;
    let mut cloud = Cloud::connect(env.port).await;
    let agent = start_agent(
        agent_config(env.port, env.state.path(), 30),
        20,
        Duration::from_millis(150),
    );
    cloud.heartbeat_where(PrinterState::Idle).await;
    for id in ["job-a", "job-b", "job-c"] {
        cloud
            .publish_job(&job(
                &env.base,
                id,
                PRINTER,
                &[("/land.jpg", ContentType::Jpeg, 1)],
            ))
            .await;
    }
    let mut order = Vec::new();
    for _ in 0..9 {
        let s = cloud.next_status().await;
        order.push((s.job_id.to_string(), s.state));
    }
    let expected: Vec<(String, JobState)> = ["job-a", "job-b", "job-c"]
        .iter()
        .flat_map(|id| {
            [JobState::Received, JobState::Printing, JobState::Completed]
                .map(|s| ((*id).to_owned(), s))
        })
        .collect();
    assert_eq!(order, expected);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn heartbeat_reports_printing_while_a_page_is_in_progress() {
    let env = env().await;
    let mut cloud = Cloud::connect(env.port).await;
    let agent = start_agent(
        agent_config(env.port, env.state.path(), 1),
        20,
        Duration::from_millis(2500),
    );
    cloud.heartbeat_where(PrinterState::Idle).await;
    cloud
        .publish_job(&job(
            &env.base,
            "job-slow",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .await;
    let hb = cloud.heartbeat_where(PrinterState::Printing).await;
    assert_eq!(hb.agent_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        cloud.job_statuses().await.last().unwrap().state,
        JobState::Completed
    );
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn last_will_announces_offline_when_the_agent_dies() {
    let env = env().await;
    let (agent, mut cloud) = online(&env, 20).await;
    // Abort without a clean shutdown: the TCP connection drops with no DISCONNECT.
    agent.task.abort();
    let hb = cloud.heartbeat_where(PrinterState::Offline).await;
    assert_eq!(hb.printer_id.as_str(), PRINTER);
}

#[tokio::test]
async fn graceful_shutdown_announces_offline_and_returns() {
    let env = env().await;
    let (agent, mut cloud) = online(&env, 20).await;
    let started = std::time::Instant::now();
    agent.stop().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(10));
    cloud.heartbeat_where(PrinterState::Offline).await;
}

#[tokio::test]
async fn shutdown_lets_the_running_job_finish() {
    let env = env().await;
    let mut cloud = Cloud::connect(env.port).await;
    let agent = start_agent(
        agent_config(env.port, env.state.path(), 30),
        20,
        Duration::from_millis(800),
    );
    cloud.heartbeat_where(PrinterState::Idle).await;
    cloud
        .publish_job(&job(
            &env.base,
            "job-fin",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .await;
    assert_eq!(cloud.next_status().await.state, JobState::Received);
    assert_eq!(cloud.next_status().await.state, JobState::Printing);
    let stopper = tokio::spawn(agent.stop());
    assert_eq!(cloud.next_status().await.state, JobState::Completed);
    stopper.await.unwrap().unwrap();
    cloud.heartbeat_where(PrinterState::Offline).await;
}

#[tokio::test]
async fn agent_started_before_the_broker_connects_when_it_appears() {
    let port = free_port();
    let state = tempfile::tempdir().unwrap();
    let base = image_server().await;
    let agent = start_agent(agent_config(port, state.path(), 30), 20, Duration::ZERO);
    tokio::time::sleep(Duration::from_millis(700)).await;
    start_broker(port);
    wait_for_port(port).await;
    let mut cloud = Cloud::connect(port).await;
    // Retry with backoff: the agent may need a moment; publish until a status shows up.
    let mut statuses = None;
    for attempt in 0..30 {
        cloud
            .publish_job(&job(
                &base,
                &format!("job-late-{attempt}"),
                PRINTER,
                &[("/land.jpg", ContentType::Jpeg, 1)],
            ))
            .await;
        if let Ok(s) = tokio::time::timeout(Duration::from_millis(500), cloud.next_status()).await {
            statuses = Some(s);
            break;
        }
    }
    assert_eq!(
        statuses.expect("agent never connected").state,
        JobState::Received
    );
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn startup_fails_cleanly_on_an_unusable_state_dir() {
    let state = tempfile::tempdir().unwrap();
    let blocker = state.path().join("file");
    std::fs::write(&blocker, "x").unwrap();
    let mut config = agent_config(free_port(), &blocker.join("state"), 30);
    config.sim.as_mut().unwrap().out_dir = state.path().join("sim-out");
    let agent = start_agent(config, 5, Duration::ZERO);
    let result = agent.stop().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn jobs_interrupted_by_a_restart_are_reported_failed() {
    let env = env().await;
    // A previous run accepted this job and died before finishing it.
    {
        let mut ledger = fotoko_pi_agent::dedupe::DedupeStore::open(env.state.path()).unwrap();
        ledger.begin("job-crashed", "order-job-crashed").unwrap();
    }
    let mut cloud = Cloud::connect(env.port).await;
    let agent = start_agent(
        agent_config(env.port, env.state.path(), 30),
        20,
        Duration::ZERO,
    );
    let status = cloud.next_status().await;
    assert_eq!(status.job_id.as_str(), "job-crashed");
    assert_eq!(status.order_id.as_str(), "order-job-crashed");
    assert_eq!(status.state, JobState::Failed);
    let error = status.error.unwrap();
    assert_eq!(error.code, PiErrorCode::DriverError);
    assert_eq!(error.message, "agent restarted during job");
    cloud.heartbeat_where(PrinterState::Idle).await;
    // It is closed now: redelivery of that job id is a duplicate.
    cloud
        .publish_job(&job(
            &env.base,
            "job-crashed",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .await;
    cloud
        .publish_job(&job(
            &env.base,
            "job-next",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .await;
    let statuses = cloud.job_statuses().await;
    assert!(
        statuses.iter().all(|s| s.job_id.as_str() == "job-next"),
        "{statuses:?}"
    );
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn a_job_that_failed_to_download_can_be_republished_under_the_same_id() {
    let env = env().await;
    let (agent, mut cloud) = online(&env, 20).await;
    let payload = job(
        &env.base,
        "job-order-7",
        PRINTER,
        &[("/flaky.jpg", ContentType::Jpeg, 1)],
    );
    cloud.publish_job(&payload).await;
    let first = cloud.job_statuses().await;
    assert_eq!(
        first.last().unwrap().error.as_ref().unwrap().code,
        PiErrorCode::DownloadFailed
    );
    cloud.publish_job(&payload).await;
    let second = cloud.job_statuses().await;
    assert_eq!(second.last().unwrap().state, JobState::Completed);
    // Completed: now it is a duplicate.
    cloud.publish_job(&payload).await;
    cloud
        .publish_job(&job(
            &env.base,
            "job-after",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .await;
    assert!(
        cloud
            .job_statuses()
            .await
            .iter()
            .all(|s| s.job_id.as_str() == "job-after")
    );
    assert_eq!(agent.sim.state().paper_remaining, 18);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn outside_simulation_mode_private_and_plain_http_urls_are_refused() {
    let env = env().await;
    let mut cloud = Cloud::connect(env.port).await;
    let mut config = agent_config(env.port, env.state.path(), 30);
    config.storage.allow_insecure_http = false;
    let agent = start_agent(config, 20, Duration::ZERO);
    cloud.heartbeat_where(PrinterState::Idle).await;
    for (id, url) in [
        ("job-http", format!("{}/land.jpg", env.base)),
        (
            "job-meta",
            "https://169.254.169.254/latest/meta-data".to_owned(),
        ),
        ("job-host", "https://evil.example.com/x.jpg".to_owned()),
    ] {
        let mut value: serde_json::Value = serde_json::to_value(job(
            &env.base,
            id,
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .unwrap();
        value["photos"][0]["url"] = url.into();
        cloud
            .publish_raw(PRINTER, serde_json::to_vec(&value).unwrap())
            .await;
        let statuses = cloud.job_statuses().await;
        let error = statuses.last().unwrap().error.clone().unwrap();
        assert_eq!(error.code, PiErrorCode::DownloadFailed, "{id}");
        assert_eq!(error.message, "download failed", "{id}");
    }
    assert_eq!(agent.sim.state().paper_remaining, 20);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn a_job_still_running_after_the_grace_period_is_cancelled_and_reported() {
    let env = env().await;
    let mut cloud = Cloud::connect(env.port).await;
    let mut config = agent_config(env.port, env.state.path(), 30);
    config.agent.shutdown_grace_secs = 1;
    let agent = start_agent(config, 20, Duration::from_secs(4));
    cloud.heartbeat_where(PrinterState::Idle).await;
    cloud
        .publish_job(&job(
            &env.base,
            "job-stuck",
            PRINTER,
            &[("/land.jpg", ContentType::Jpeg, 1)],
        ))
        .await;
    assert_eq!(cloud.next_status().await.state, JobState::Received);
    assert_eq!(cloud.next_status().await.state, JobState::Printing);
    let started = std::time::Instant::now();
    let stopper = tokio::spawn(agent.stop());
    let failed = cloud.next_status().await;
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.error.unwrap().code, PiErrorCode::DriverError);
    stopper.await.unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(15));
    cloud.heartbeat_where(PrinterState::Offline).await;
}

#[tokio::test]
async fn jobs_beyond_the_queue_limit_are_rejected_with_failed() {
    let env = env().await;
    let mut cloud = Cloud::connect(env.port).await;
    let mut config = agent_config(env.port, env.state.path(), 30);
    config.agent.shutdown_grace_secs = 1;
    let agent = start_agent(config, 500, Duration::from_millis(400));
    cloud.heartbeat_where(PrinterState::Idle).await;
    let total = 60;
    for n in 0..total {
        cloud
            .publish_job(&job(
                &env.base,
                &format!("job-q{n}"),
                PRINTER,
                &[("/land.jpg", ContentType::Jpeg, 1)],
            ))
            .await;
    }
    let mut rejected = Vec::new();
    while rejected.len() < total - 1 - fotoko_pi_agent::agent::JOB_QUEUE - 2 {
        let status = cloud.next_status().await;
        if let Some(error) = &status.error
            && error.message == "queue full"
        {
            assert_eq!(
                (status.state, error.code),
                (JobState::Failed, PiErrorCode::DriverError)
            );
            rejected.push(status.job_id.to_string());
        }
    }
    assert!(
        !rejected.contains(&"job-q0".to_owned()),
        "the first job was accepted"
    );
    agent.stop().await.unwrap();
}
