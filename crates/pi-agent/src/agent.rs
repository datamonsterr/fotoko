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
use std::time::Duration;

use fotoko_protocol::{MAX_PHOTO_BYTES, PrinterId, PrinterState, jobs_topic};
use rumqttc::{AsyncClient, Event, Incoming, Outgoing, QoS};
use thiserror::Error;
use tokio::sync::{Notify, mpsc};
use tokio::time::{Instant, MissedTickBehavior, interval_at, sleep, timeout};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::{Backend, Config};
use crate::cups::{CupsPrinter, CupsSettings};
use crate::dedupe::{DedupeStore, StoreError};
use crate::download::{DownloadError, Downloader};
use crate::handler::JobHandler;
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
        let dedupe = DedupeStore::open(state_dir)?;
        let downloader = Downloader::new(
            config.storage.allow_insecure_http,
            MAX_PHOTO_BYTES,
            Duration::from_secs(config.agent.download_timeout_secs),
        )?;
        let options = mqtt_options(&config)?;
        let (client, eventloop) = AsyncClient::new(options, 64);
        let sink: Arc<dyn Sink> = Arc::new(MqttSink::new(client.clone(), &config.printer_id));
        let handler = JobHandler::new(
            config.printer_id.clone(),
            Arc::clone(&printer),
            Arc::clone(&sink),
            downloader,
            dedupe,
            tmp_dir,
        );

        let stop = CancellationToken::new();
        let net_stop = CancellationToken::new();
        let connected = Arc::new(Notify::new());
        let (jobs_tx, jobs_rx) = mpsc::unbounded_channel();

        let mut net = tokio::spawn(network_loop(
            eventloop,
            client.clone(),
            jobs_topic(&config.printer_id),
            jobs_tx,
            Arc::clone(&connected),
            net_stop.clone(),
        ));
        let mut worker = tokio::spawn(worker_loop(handler, jobs_rx, stop.clone()));
        let beat = tokio::spawn(heartbeat_loop(
            config.printer_id.clone(),
            Arc::clone(&printer),
            Arc::clone(&sink),
            config.heartbeat_interval(),
            connected,
            stop.clone(),
        ));
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
            warn!("job still running after the grace period, abandoning it");
            worker.abort();
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

fn prepare_tmp_dir(dir: &Path) -> Result<(), AgentError> {
    let err = |source| AgentError::StateDir {
        path: dir.to_owned(),
        source,
    };
    if dir.exists() {
        // Leftovers of a killed run.
        fs::remove_dir_all(dir).map_err(err)?;
    }
    fs::create_dir_all(dir).map_err(err)
}

async fn network_loop(
    mut eventloop: rumqttc::EventLoop,
    client: AsyncClient,
    jobs_topic: String,
    jobs: mpsc::UnboundedSender<Vec<u8>>,
    connected: Arc<Notify>,
    stop: CancellationToken,
) {
    let mut backoff = MIN_BACKOFF;
    loop {
        let event = tokio::select! {
            biased;
            () = stop.cancelled() => break,
            event = eventloop.poll() => event,
        };
        match event {
            Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                info!("connected to broker");
                backoff = MIN_BACKOFF;
                if let Err(error) = client.try_subscribe(&jobs_topic, QoS::AtLeastOnce) {
                    warn!(%error, "cannot subscribe to jobs topic");
                }
                connected.notify_one();
            }
            Ok(Event::Incoming(Incoming::Publish(publish))) => {
                if publish.topic == jobs_topic {
                    // The receiver only disappears during shutdown.
                    let _ = jobs.send(publish.payload.to_vec());
                } else {
                    debug!(topic = %publish.topic, "ignoring message on unexpected topic");
                }
            }
            Ok(Event::Outgoing(Outgoing::Disconnect)) => break,
            Ok(_) => {}
            Err(error) => {
                warn!(%error, retry_in = ?backoff, "mqtt connection error");
                tokio::select! {
                    () = stop.cancelled() => break,
                    () = sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

async fn worker_loop(
    mut handler: JobHandler,
    mut jobs: mpsc::UnboundedReceiver<Vec<u8>>,
    stop: CancellationToken,
) {
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

async fn heartbeat_loop(
    printer_id: PrinterId,
    printer: Arc<dyn Printer>,
    sink: Arc<dyn Sink>,
    period: Duration,
    connected: Arc<Notify>,
    stop: CancellationToken,
) {
    let mut ticker = interval_at(Instant::now() + period, period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            () = stop.cancelled() => break,
            _ = ticker.tick() => {}
            () = connected.notified() => {}
        }
        let beat = heartbeat(&printer_id, printer.state().await);
        if let Err(error) = sink.heartbeat(&beat) {
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
