//! The agent: MQTT event loop, job worker, heartbeat, graceful shutdown.
//!
//! Three tasks run side by side. The network task owns the rumqttc event loop (it must keep
//! polling for keep alives) and forwards job messages to the worker over an unbounded queue;
//! the worker handles one job at a time; the heartbeat task reports the printer state. On
//! shutdown the worker may finish its current job (bounded by `shutdown_grace_secs`), then the
//! agent announces `offline` and disconnects cleanly.

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fotoko_protocol::{MAX_PHOTO_BYTES, PrinterId, PrinterState, jobs_topic};
use rumqttc::{AsyncClient, Event, Incoming, Outgoing, QoS};
use thiserror::Error;
use tokio::sync::{Notify, Semaphore, mpsc};
use tokio::time::{Instant, MissedTickBehavior, interval_at, sleep, timeout};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::{Backend, Config};
use crate::cups::{CupsPrinter, CupsSettings};
use crate::dedupe::{DedupeStore, StoreError};
use crate::download::UrlPolicy;
use crate::download::{DownloadError, Downloader};
use crate::handler::{JobHandler, STATUS_TIMEOUT, reject};
use crate::mqtt::{MqttError, MqttSink, Sink, heartbeat, mqtt_options};
use crate::printer::{PrintError, Printer};
use crate::sim_backend::SimBackend;

/// Startup failure.
#[derive(Debug, Error)]
pub enum AgentError {
    /// MQTT options could not be built.
    #[error(transparent)]
    Mqtt(#[from] MqttError),
    /// The dedupe file cannot be opened.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The HTTP client cannot be built.
    #[error(transparent)]
    Download(#[from] DownloadError),
    /// The printer backend cannot be created.
    #[error("cannot create printer: {0}")]
    Printer(#[from] PrintError),
    /// The state directory is unusable.
    #[error("state directory {path}: {source}")]
    StateDir {
        /// Directory.
        path: PathBuf,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

const MIN_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Time given to flush the final messages before the connection is dropped.
const DISCONNECT_WAIT: Duration = Duration::from_secs(5);
/// Time a cancelled job gets to report itself failed.
const CANCEL_WAIT: Duration = Duration::from_secs(10);
/// Jobs waiting for the worker; more are rejected with `failed`.
pub const JOB_QUEUE: usize = 32;
/// Rejections being published at once; beyond this a rejected job gets no status at all.
pub const MAX_REJECTS_IN_FLIGHT: usize = 8;

/// Build the printer a config asks for.
pub fn build_printer(config: &Config) -> Result<Arc<dyn Printer>, PrintError> {
    match (config.printer.backend, &config.sim) {
        (Backend::Cups, _) => Ok(Arc::new(CupsPrinter::new(CupsSettings::from(
            &config.printer,
        )))),
        (Backend::Sim, Some(sim)) => Ok(Arc::new(SimBackend::from_config(sim)?)),
        (Backend::Sim, None) => Err(PrintError::Driver(
            "sim backend needs a [sim] section".to_owned(),
        )),
    }
}

/// The Pi agent.
pub struct Agent {
    config: Config,
    printer: Arc<dyn Printer>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("printer_id", &self.config.printer_id)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// Agent printing through `printer`. The e2e tests pass a `SimBackend`.
    pub fn new(config: Config, printer: Arc<dyn Printer>) -> Self {
        Self { config, printer }
    }

    /// Agent with the backend named in the config.
    pub fn from_config(config: Config) -> Result<Self, AgentError> {
        let printer = build_printer(&config)?;
        Ok(Self::new(config, printer))
    }

    /// Connect and serve until `shutdown` completes. Dropping the returned future (or aborting
    /// its task) closes the connection without a DISCONNECT, which makes the broker publish
    /// the last will.
    pub async fn run<F>(self, shutdown: F) -> Result<(), AgentError>
    where
        F: Future<Output = ()> + Send,
    {
        crate::tls::install_crypto_provider();
        let Self { config, printer } = self;
        let state_dir = &config.storage.state_dir;
        let tmp_dir = state_dir.join("tmp");
        prepare_tmp_dir(&tmp_dir)?;
        let store = DedupeStore::open(state_dir)?;
        let policy = if config.storage.allow_insecure_http {
            UrlPolicy::permissive()
        } else {
            UrlPolicy::strict(config.storage.allowed_hosts.clone())
        };
        let downloader = Downloader::new(
            policy,
            MAX_PHOTO_BYTES,
            Duration::from_secs(config.agent.download_timeout_secs),
        )?;
        let options = mqtt_options(&config)?;
        let (client, eventloop) = AsyncClient::new(options, 64);
        let sink: Arc<dyn Sink> = Arc::new(MqttSink::new(client.clone(), &config.printer_id));
        let job_cancel = CancellationToken::new();
        let handler = JobHandler::new(
            config.printer_id.clone(),
            Arc::clone(&printer),
            Arc::clone(&sink),
            downloader,
            store,
            tmp_dir,
            job_cancel.clone(),
        );

        let stop = CancellationToken::new();
        let net_stop = CancellationToken::new();
        let connected = Arc::new(Notify::new());
        let link_up = Arc::new(AtomicBool::new(false));
        let (jobs_tx, jobs_rx) = mpsc::channel(JOB_QUEUE);

        let mut net = tokio::spawn(network_loop(Network {
            eventloop,
            client: client.clone(),
            jobs_topic: jobs_topic(&config.printer_id),
            jobs: jobs_tx,
            connected: Arc::clone(&connected),
            link_up: Arc::clone(&link_up),
            sink: Arc::clone(&sink),
            printer_id: config.printer_id.clone(),
            reject_slots: Arc::new(Semaphore::new(MAX_REJECTS_IN_FLIGHT)),
            stop: net_stop.clone(),
        }));
        let mut worker = tokio::spawn(worker_loop(handler, jobs_rx, stop.clone()));
        let beat = tokio::spawn(heartbeat_loop(Heartbeats {
            printer_id: config.printer_id.clone(),
            printer: Arc::clone(&printer),
            sink: Arc::clone(&sink),
            period: config.heartbeat_interval(),
            connected,
            link_up,
            stop: stop.clone(),
        }));
        // Dropping `run` (for example by aborting its task) must not leave tasks holding the
        // connection open.
        let _guard = AbortOnDrop(vec![
            net.abort_handle(),
            worker.abort_handle(),
            beat.abort_handle(),
        ]);
        info!(printer_id = %config.printer_id, "agent started");

        shutdown.await;
        info!("shutting down");
        stop.cancel();
        let grace = Duration::from_secs(config.agent.shutdown_grace_secs);
        if timeout(grace, &mut worker).await.is_err() {
            // Withdraw the job in progress: it is reported as failed and its CUPS job cancelled.
            warn!("job still running after the grace period, cancelling it");
            job_cancel.cancel();
            if timeout(CANCEL_WAIT, &mut worker).await.is_err() {
                warn!("job did not stop, abandoning it");
                worker.abort();
            }
        }
        let _ = beat.await;
        // Announce `offline` ourselves: a clean DISCONNECT suppresses the last will.
        if let Err(error) = sink.heartbeat(&heartbeat(&config.printer_id, PrinterState::Offline)) {
            warn!(%error, "cannot publish final heartbeat");
        }
        if let Err(error) = client.try_disconnect() {
            warn!(%error, "cannot request disconnect");
        }
        if timeout(DISCONNECT_WAIT, &mut net).await.is_err() {
            net_stop.cancel();
            let _ = net.await;
        }
        info!("agent stopped");
        Ok(())
    }
}

/// Aborts the tasks it was given when dropped.
struct AbortOnDrop(Vec<tokio::task::AbortHandle>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        for handle in &self.0 {
            handle.abort();
        }
    }
}

/// Create `dir` empty with mode 0700 (downloads hold customers' photos).
fn prepare_tmp_dir(dir: &Path) -> Result<(), AgentError> {
    let err = |source| AgentError::StateDir {
        path: dir.to_owned(),
        source,
    };
    if dir.exists() {
        // Leftovers of a killed run.
        fs::remove_dir_all(dir).map_err(err)?;
    }
    fs::create_dir_all(dir).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(err)?;
    }
    Ok(())
}

/// `delay` spread by up to +-25% so a fleet of Pis does not reconnect in lockstep.
fn jittered(delay: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    delay.mul_f64(0.75 + f64::from(nanos % 1000) / 2000.0)
}

struct Network {
    eventloop: rumqttc::EventLoop,
    client: AsyncClient,
    jobs_topic: String,
    jobs: mpsc::Sender<Vec<u8>>,
    connected: Arc<Notify>,
    link_up: Arc<AtomicBool>,
    sink: Arc<dyn Sink>,
    printer_id: PrinterId,
    reject_slots: Arc<Semaphore>,
    stop: CancellationToken,
}

/// Publish a `queue full` failure in the background, at most [`MAX_REJECTS_IN_FLIGHT`] at a time
/// and each for at most [`STATUS_TIMEOUT`]. Returns `false` (dropping the status) at the cap.
fn spawn_reject(
    slots: &Arc<Semaphore>,
    sink: Arc<dyn Sink>,
    printer_id: PrinterId,
    payload: Vec<u8>,
) -> bool {
    let Ok(permit) = Arc::clone(slots).try_acquire_owned() else {
        warn!("too many rejections in flight, dropping a status");
        return false;
    };
    tokio::spawn(async move {
        reject(
            sink.as_ref(),
            &printer_id,
            &payload,
            "queue full",
            STATUS_TIMEOUT,
        )
        .await;
        drop(permit);
    });
    true
}

async fn network_loop(mut net: Network) {
    let mut backoff = MIN_BACKOFF;
    loop {
        let event = tokio::select! {
            biased;
            () = net.stop.cancelled() => break,
            event = net.eventloop.poll() => event,
        };
        match event {
            Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                info!("connected to broker");
                backoff = MIN_BACKOFF;
                net.link_up.store(true, Ordering::SeqCst);
                if let Err(error) = net.client.try_subscribe(&net.jobs_topic, QoS::AtLeastOnce) {
                    warn!(%error, "cannot subscribe to jobs topic");
                }
                net.connected.notify_one();
            }
            Ok(Event::Incoming(Incoming::Publish(publish))) => {
                if publish.topic != net.jobs_topic {
                    debug!(topic = %publish.topic, "ignoring message on unexpected topic");
                    continue;
                }
                let payload = publish.payload.to_vec();
                match net.jobs.try_send(payload) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(payload)) => {
                        warn!("job queue full, rejecting job");
                        // Publishing may wait for this very loop to drain the request queue, so
                        // it happens in its own task.
                        spawn_reject(
                            &net.reject_slots,
                            Arc::clone(&net.sink),
                            net.printer_id.clone(),
                            payload,
                        );
                    }
                    // The worker only goes away during shutdown.
                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                }
            }
            Ok(Event::Outgoing(Outgoing::Disconnect)) => break,
            Ok(_) => {}
            Err(error) => {
                net.link_up.store(false, Ordering::SeqCst);
                let wait = jittered(backoff);
                warn!(%error, retry_in = ?wait, "mqtt connection error");
                tokio::select! {
                    () = net.stop.cancelled() => break,
                    () = sleep(wait) => {}
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
    net.link_up.store(false, Ordering::SeqCst);
}

async fn worker_loop(
    mut handler: JobHandler,
    mut jobs: mpsc::Receiver<Vec<u8>>,
    stop: CancellationToken,
) {
    handler.recover().await;
    loop {
        let payload = tokio::select! {
            biased;
            () = stop.cancelled() => break,
            payload = jobs.recv() => match payload {
                Some(payload) => payload,
                None => break,
            },
        };
        let _ = handler.handle(&payload).await;
    }
}

struct Heartbeats {
    printer_id: PrinterId,
    printer: Arc<dyn Printer>,
    sink: Arc<dyn Sink>,
    period: Duration,
    connected: Arc<Notify>,
    link_up: Arc<AtomicBool>,
    stop: CancellationToken,
}

async fn heartbeat_loop(beats: Heartbeats) {
    let mut ticker = interval_at(Instant::now() + beats.period, beats.period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            () = beats.stop.cancelled() => break,
            _ = ticker.tick() => {}
            () = beats.connected.notified() => {}
        }
        if !beats.link_up.load(Ordering::SeqCst) {
            // Nobody is listening; do not queue stale heartbeats for the reconnect.
            continue;
        }
        let beat = heartbeat(&beats.printer_id, beats.printer.state().await);
        if let Err(error) = beats.sink.heartbeat(&beat) {
            warn!(%error, "cannot publish heartbeat");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(backend: &str, sim: bool) -> Config {
        let sim = if sim {
            "[sim]\nout_dir = \"/tmp/fotoko-agent-test-sim\"\n"
        } else {
            ""
        };
        Config::from_toml(&format!(
            "printer_id = \"stand-001\"\n[mqtt]\nhost = \"127.0.0.1\"\ntls = false\n[printer]\nbackend = \"{backend}\"\nlpstat_bin = \"/nonexistent/lpstat\"\n{sim}[storage]\nstate_dir = \"/tmp/x\"\n"
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn build_printer_picks_the_backend() {
        // Cups: lpstat does not exist, so the state is Error.
        let cups = build_printer(&config("cups", false)).unwrap();
        assert_eq!(cups.state().await, PrinterState::Error);
        let dir = tempfile::tempdir().unwrap();
        let mut c = config("sim", true);
        c.sim.as_mut().unwrap().out_dir = dir.path().join("o");
        let sim = build_printer(&c).unwrap();
        assert_eq!(sim.state().await, PrinterState::Idle);
    }

    #[test]
    fn build_printer_reports_sim_problems() {
        // `Config::from_toml` rejects sim without a section, so build the inconsistent value.
        let mut c = config("sim", true);
        c.sim = None;
        assert!(matches!(build_printer(&c), Err(PrintError::Driver(m)) if m.contains("[sim]")));
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("f");
        fs::write(&blocker, "x").unwrap();
        let mut c = config("sim", true);
        c.sim.as_mut().unwrap().out_dir = blocker.join("o");
        assert!(build_printer(&c).is_err());
    }

    #[test]
    fn from_config_surfaces_printer_errors_and_debug_hides_internals() {
        let mut c = config("sim", true);
        c.sim = None;
        assert!(matches!(Agent::from_config(c), Err(AgentError::Printer(_))));
        let ok = Agent::from_config(config("cups", false)).unwrap();
        let text = format!("{ok:?}");
        assert!(text.contains("stand-001") && text.contains(".."), "{text}");
    }

    #[derive(Default)]
    struct Beats(std::sync::Mutex<Vec<fotoko_protocol::Heartbeat>>);

    #[async_trait::async_trait]
    impl Sink for Beats {
        async fn status(&self, _: &fotoko_protocol::JobStatus) -> Result<(), MqttError> {
            Ok(())
        }

        fn heartbeat(&self, beat: &fotoko_protocol::Heartbeat) -> Result<(), MqttError> {
            self.0.lock().unwrap().push(beat.clone());
            Ok(())
        }
    }

    struct StuckSink;

    #[async_trait::async_trait]
    impl Sink for StuckSink {
        async fn status(&self, _: &fotoko_protocol::JobStatus) -> Result<(), MqttError> {
            std::future::pending::<()>().await;
            Ok(())
        }

        fn heartbeat(&self, _: &fotoko_protocol::Heartbeat) -> Result<(), MqttError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn rejections_in_flight_are_capped() {
        let slots = Arc::new(Semaphore::new(MAX_REJECTS_IN_FLIGHT));
        let sink: Arc<dyn Sink> = Arc::new(StuckSink);
        let id = PrinterId::new("stand-001").unwrap();
        let payload = br#"{"jobId":"j","orderId":"o","printerId":"stand-001","media":"4x6","createdAt":"2026-01-01T00:00:00Z","photos":[]}"#.to_vec();
        for _ in 0..MAX_REJECTS_IN_FLIGHT {
            assert!(spawn_reject(
                &slots,
                Arc::clone(&sink),
                id.clone(),
                payload.clone()
            ));
        }
        assert!(!spawn_reject(
            &slots,
            Arc::clone(&sink),
            id.clone(),
            payload.clone()
        ));
        assert_eq!(slots.available_permits(), 0);
    }

    #[tokio::test]
    async fn heartbeats_are_skipped_while_disconnected() {
        let dir = tempfile::tempdir().unwrap();
        let mut sim = config("sim", true);
        sim.sim.as_mut().unwrap().out_dir = dir.path().join("o");
        let printer = build_printer(&sim).unwrap();
        let sink = Arc::new(Beats::default());
        let (connected, link_up, stop) = (
            Arc::new(Notify::new()),
            Arc::new(AtomicBool::new(false)),
            CancellationToken::new(),
        );
        let task = tokio::spawn(heartbeat_loop(Heartbeats {
            printer_id: sim.printer_id.clone(),
            printer,
            sink: sink.clone(),
            period: Duration::from_millis(30),
            connected,
            link_up: Arc::clone(&link_up),
            stop: stop.clone(),
        }));
        sleep(Duration::from_millis(200)).await;
        assert!(
            sink.0.lock().unwrap().is_empty(),
            "nothing while the link is down"
        );
        link_up.store(true, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while sink.0.lock().unwrap().len() < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "no heartbeats after reconnect"
            );
            sleep(Duration::from_millis(10)).await;
        }
        let beat = sink.0.lock().unwrap()[0].clone();
        assert_eq!(beat.printer_state, PrinterState::Idle);
        stop.cancel();
        task.await.unwrap();
    }

    #[test]
    fn jitter_stays_within_a_quarter() {
        let base = Duration::from_secs(8);
        for _ in 0..50 {
            let d = jittered(base);
            assert!(d >= base.mul_f64(0.75) && d <= base.mul_f64(1.25), "{d:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn tmp_dir_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        prepare_tmp_dir(&tmp).unwrap();
        assert_eq!(
            fs::metadata(&tmp).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn tmp_dir_is_recreated_empty() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        prepare_tmp_dir(&tmp).unwrap();
        fs::write(tmp.join("stale.jpg"), "x").unwrap();
        prepare_tmp_dir(&tmp).unwrap();
        assert!(tmp.is_dir());
        assert_eq!(fs::read_dir(&tmp).unwrap().count(), 0);
        let blocker = dir.path().join("f");
        fs::write(&blocker, "x").unwrap();
        let err = prepare_tmp_dir(&blocker.join("tmp")).unwrap_err();
        assert!(matches!(err, AgentError::StateDir { .. }));
        assert!(err.to_string().contains("state directory"));
    }
}
