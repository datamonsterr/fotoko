//! Shared harness: embedded rumqttd broker, "cloud" MQTT client, local image server, agent
//! runner. Everything binds 127.0.0.1.
#![allow(dead_code)]

use std::io::Cursor;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use chrono::Utc;
use fotoko_pi_agent::{Agent, AgentError, Config, SimBackend};
use fotoko_printer_sim::{SimConfig, SimPrinter};
use fotoko_protocol::{
    ContentType, Heartbeat, JobPhoto, JobStatus, Media, PrintJob, PrinterId, heartbeat_topic,
    jobs_topic, status_topic,
};
use image::{ImageFormat, Rgb, RgbImage};
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::timeout;

pub const WAIT: Duration = Duration::from_secs(20);
pub const PRINTER: &str = "stand-001";

pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Start an embedded broker on `port` in a background thread (it lives until the process ends).
pub fn start_broker(port: u16) {
    let text = format!(
        r#"
id = 0
[router]
id = 0
max_connections = 100
max_outgoing_packet_count = 200
max_segment_size = 1048576
max_segment_count = 10
[v4.1]
name = "v4-1"
listen = "127.0.0.1:{port}"
next_connection_delay_ms = 1
[v4.1.connections]
connection_timeout_ms = 60000
max_payload_size = 262144
max_inflight_count = 100
dynamic_filters = true
"#
    );
    let config: rumqttd::Config = toml::from_str(&text).unwrap();
    let mut broker = rumqttd::Broker::new(config);
    thread::spawn(move || {
        let _ = broker.start();
    });
}

/// Wait until something accepts TCP connections on `port`.
pub async fn wait_for_port(port: u16) {
    timeout(WAIT, async {
        while tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("broker did not start");
}

pub async fn broker() -> u16 {
    let port = free_port();
    start_broker(port);
    wait_for_port(port).await;
    port
}

/// The cloud side: publishes jobs, observes status and heartbeat topics.
pub struct Cloud {
    client: AsyncClient,
    statuses: mpsc::UnboundedReceiver<JobStatus>,
    heartbeats: mpsc::UnboundedReceiver<Heartbeat>,
}

impl Cloud {
    pub async fn connect(port: u16) -> Self {
        let id = PrinterId::new(PRINTER).unwrap();
        let client_id = format!("cloud-{}", uuid_like());
        let mut options = MqttOptions::new(client_id, "127.0.0.1", port);
        options.set_keep_alive(Duration::from_secs(5));
        let (client, mut eventloop) = AsyncClient::new(options, 32);
        client
            .subscribe(status_topic(&id), QoS::AtLeastOnce)
            .await
            .unwrap();
        client
            .subscribe(heartbeat_topic(&id), QoS::AtLeastOnce)
            .await
            .unwrap();
        let (status_tx, statuses) = mpsc::unbounded_channel();
        let (hb_tx, heartbeats) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        let mut ready_tx = Some(ready_tx);
        tokio::spawn(async move {
            let mut acks = 0;
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Incoming::SubAck(_))) => {
                        acks += 1;
                        if acks == 2 {
                            if let Some(tx) = ready_tx.take() {
                                let _ = tx.send(());
                            }
                        }
                    }
                    Ok(Event::Incoming(Incoming::Publish(p))) => {
                        if p.topic.ends_with("/status") {
                            let _ = status_tx.send(serde_json::from_slice(&p.payload).unwrap());
                        } else if p.topic.ends_with("/heartbeat") {
                            let _ = hb_tx.send(serde_json::from_slice(&p.payload).unwrap());
                        }
                    }
                    Ok(_) => {}
                    Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                }
            }
        });
        timeout(WAIT, ready_rx)
            .await
            .expect("cloud subscribe")
            .unwrap();
        Self {
            client,
            statuses,
            heartbeats,
        }
    }

    pub async fn publish_raw(&self, printer_id: &str, payload: Vec<u8>) {
        let id = PrinterId::new(printer_id).unwrap();
        self.client
            .publish(jobs_topic(&id), QoS::AtLeastOnce, false, payload)
            .await
            .unwrap();
    }

    /// Publish `job` on `printer_id`'s jobs topic whatever its own `printerId` says.
    pub async fn publish_job_on(&self, printer_id: &str, job: &PrintJob) {
        self.publish_raw(printer_id, serde_json::to_vec(job).unwrap())
            .await;
    }

    pub async fn publish_job(&self, job: &PrintJob) {
        self.publish_raw(job.printer_id.as_str(), serde_json::to_vec(job).unwrap())
            .await;
    }

    pub async fn next_status(&mut self) -> JobStatus {
        timeout(WAIT, self.statuses.recv())
            .await
            .expect("timed out waiting for status")
            .unwrap()
    }

    /// Statuses up to and including the first `completed` or `failed`.
    pub async fn job_statuses(&mut self) -> Vec<JobStatus> {
        let mut out = Vec::new();
        loop {
            let status = self.next_status().await;
            let done = matches!(
                status.state,
                fotoko_protocol::JobState::Completed | fotoko_protocol::JobState::Failed
            );
            out.push(status);
            if done {
                return out;
            }
        }
    }

    pub async fn no_status_within(&mut self, wait: Duration) -> bool {
        timeout(wait, self.statuses.recv()).await.is_err()
    }

    pub async fn next_heartbeat(&mut self) -> Heartbeat {
        timeout(WAIT, self.heartbeats.recv())
            .await
            .expect("timed out waiting for heartbeat")
            .unwrap()
    }

    /// Next heartbeat whose state matches.
    pub async fn heartbeat_where(&mut self, want: fotoko_protocol::PrinterState) -> Heartbeat {
        loop {
            let hb = self.next_heartbeat().await;
            if hb.printer_state == want {
                return hb;
            }
        }
    }
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

pub fn jpeg_landscape() -> Vec<u8> {
    encode(120, 80, ImageFormat::Jpeg)
}

pub fn png_portrait() -> Vec<u8> {
    encode(80, 120, ImageFormat::Png)
}

fn encode(w: u32, h: u32, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    RgbImage::from_pixel(w, h, Rgb([30, 140, 220]))
        .write_to(&mut out, format)
        .unwrap();
    out.into_inner()
}

/// Local image host. Paths: `/land.jpg`, `/port.png`, `/gone` (404), `/huge.png` (16 MiB),
/// `/mislabeled.jpg` (PNG bytes), `/text.jpg`.
pub async fn image_server() -> String {
    let mut huge = vec![0u8; 16 * 1024 * 1024];
    huge[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let (jpg, png) = (jpeg_landscape(), png_portrait());
    let (png2, huge) = (png.clone(), Arc::new(huge));
    let app = Router::new()
        .route("/land.jpg", get(move || async move { jpg }))
        .route("/port.png", get(move || async move { png }))
        .route("/mislabeled.jpg", get(move || async move { png2 }))
        .route("/text.jpg", get(|| async { "this is not an image at all" }))
        .route("/gone", get(|| async { StatusCode::NOT_FOUND }))
        .route("/huge.png", get(move || async move { huge.to_vec() }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

pub fn job(
    base: &str,
    job_id: &str,
    printer_id: &str,
    photos: &[(&str, ContentType, u32)],
) -> PrintJob {
    PrintJob {
        job_id: job_id.into(),
        order_id: format!("order-{job_id}").into(),
        printer_id: PrinterId::new(printer_id).unwrap(),
        media: Media::Photo4x6,
        created_at: Utc::now(),
        photos: photos
            .iter()
            .enumerate()
            .map(|(i, (path, content_type, copies))| JobPhoto {
                photo_id: format!("ph{i}").into(),
                url: format!("{base}{path}"),
                content_type: *content_type,
                copies: *copies,
            })
            .collect(),
    }
}

pub fn agent_config(port: u16, state_dir: &Path, heartbeat_secs: u64) -> Config {
    Config::from_toml(&format!(
        r#"
printer_id = "{PRINTER}"
[mqtt]
host = "127.0.0.1"
port = {port}
tls = false
keep_alive_secs = 5
[printer]
backend = "sim"
[sim]
out_dir = "{state}/sim-out"
[storage]
state_dir = "{state}"
allow_insecure_http = true
[agent]
heartbeat_secs = {heartbeat_secs}
shutdown_grace_secs = 10
"#,
        state = state_dir.display()
    ))
    .unwrap()
}

pub struct RunningAgent {
    pub task: JoinHandle<Result<(), AgentError>>,
    pub shutdown: oneshot::Sender<()>,
    pub sim: Arc<SimPrinter>,
    pub out_dir: PathBuf,
}

impl RunningAgent {
    pub async fn stop(self) -> Result<(), AgentError> {
        let _ = self.shutdown.send(());
        timeout(WAIT, self.task)
            .await
            .expect("agent did not stop")
            .unwrap()
    }
}

/// Start an agent with a sim printer holding `paper` sheets and `latency` per page.
pub fn start_agent(config: Config, paper: u32, latency: Duration) -> RunningAgent {
    let out_dir = config.sim.as_ref().unwrap().out_dir.clone();
    let mut sim_config = SimConfig::new(&out_dir);
    sim_config.paper = paper;
    sim_config.latency = latency;
    let sim = Arc::new(SimPrinter::new(sim_config).unwrap());
    let printer = Arc::new(SimBackend::new(Arc::clone(&sim)));
    let (shutdown, rx) = oneshot::channel::<()>();
    let agent = Agent::new(config, printer);
    let task = tokio::spawn(async move {
        agent
            .run(async move {
                let _ = rx.await;
            })
            .await
    });
    RunningAgent {
        task,
        shutdown,
        sim,
        out_dir,
    }
}

pub fn page_size(path: &Path) -> (u32, u32) {
    let img = image::open(path).unwrap();
    (img.width(), img.height())
}
