//! The whole simulation in one process: `fotoko-local` on a free port, an embedded MQTT broker,
//! a payOS mock, a Pi agent on the printer simulation, and helpers that drive the customer and
//! payOS sides over real HTTP.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use fotoko_api::local::LocalApp;
use fotoko_core::payos::sign_webhook_data;
use fotoko_core::testing::{ManualClock, signed_webhook};
use fotoko_pi_agent::{Agent, AgentError, Config, SimBackend};
use fotoko_printer_sim::{SimConfig, SimPrinter};
use fotoko_protocol::{
    CheckoutResponse, CreateOrderResponse, Order, OrderStatus, PayosWebhook, PrinterId,
    PrinterStatus, UploadTarget,
};
use reqwest::Method;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use wiremock::MockServer;

use crate::cloud::Cloud;
use crate::fixtures::Photo;
use crate::{broker, payos};

/// The only printer of the stack.
pub const PRINTER: &str = "stand-001";
/// Default deadline of the `wait_*` helpers.
pub const WAIT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(20);

/// How to start a [`Stack`].
#[derive(Debug, Clone)]
pub struct StackOptions {
    /// Sheets in the simulated printer.
    pub paper: u32,
    /// Milliseconds each simulated page takes.
    pub page_latency_ms: u64,
    /// Whether the Pi agent starts with the stack.
    pub agent: bool,
}

impl Default for StackOptions {
    fn default() -> Self {
        Self {
            paper: 50,
            page_latency_ms: 0,
            agent: true,
        }
    }
}

struct RunningAgent {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<Result<(), AgentError>>,
}

/// A page the simulated printer wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// File name, `{jobId}-{idx}-{photoId}-{n}.png`.
    pub name: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// An order that has been paid.
#[derive(Debug, Clone)]
pub struct PaidOrder {
    /// Order id.
    pub order_id: String,
    /// payOS order code.
    pub order_code: i64,
    /// Amount in VND.
    pub amount_vnd: u64,
}

/// The running simulation.
pub struct Stack {
    /// The local API, with its in-memory repos.
    pub app: LocalApp,
    /// Base URL of the API.
    pub base: String,
    /// HTTP client for the API.
    pub http: reqwest::Client,
    /// The payOS mock.
    pub payos: MockServer,
    /// The clock the API runs on; move it to expire orders.
    pub clock: ManualClock,
    /// Recorder on the broker.
    pub cloud: Cloud,
    /// Port of the broker.
    pub broker_port: u16,
    /// Handle on the running agent's simulated printer.
    pub printer: Arc<SimPrinter>,
    opts: StackOptions,
    dir: tempfile::TempDir,
    agent: Option<RunningAgent>,
}

impl std::fmt::Debug for Stack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stack")
            .field("base", &self.base)
            .field("broker_port", &self.broker_port)
            .finish_non_exhaustive()
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.take() {
            agent.task.abort();
        }
    }
}

/// Poll `check` every few milliseconds until it returns `Some`, or panic after `deadline`.
pub async fn eventually<T, F, Fut>(what: &str, deadline: Duration, mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let end = tokio::time::Instant::now() + deadline;
    loop {
        if let Some(value) = check().await {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < end,
            "timed out after {deadline:?} waiting for {what}"
        );
        tokio::time::sleep(POLL).await;
    }
}

fn printer_id() -> PrinterId {
    PrinterId::new(PRINTER).expect("printer id")
}

impl Stack {
    /// Start the API, the broker, the payOS mock and (by default) the Pi agent, and return once
    /// the agent's first heartbeat has marked the printer online.
    pub async fn start(opts: StackOptions) -> Self {
        let port = broker::start_on_free_port();
        let payos = payos::start_mock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the API");
        let base = format!("http://{}", listener.local_addr().expect("API address"));
        let clock = ManualClock::new(Utc::now());
        let mut app = LocalApp::builder()
            .public_url(&base)
            .object_dir(dir.path().join("objects"))
            .url_secret("e2e-url-secret")
            .payos(payos.uri(), payos::credentials())
            .public_app_url("http://app.test")
            .mqtt_url(Some(format!("mqtt://127.0.0.1:{port}")))
            .clock(Arc::new(clock.clone()))
            .expire_every(None)
            .build()
            .await
            .expect("build the API");
        app.serve(listener).expect("serve the API");
        let cloud = Cloud::connect(port).await;
        let printer = new_sim_printer(&dir.path().join("sim-out"), &opts);
        let mut stack = Self {
            app,
            base,
            http: reqwest::Client::new(),
            payos,
            clock,
            cloud,
            broker_port: port,
            printer,
            opts,
            dir,
            agent: None,
        };
        if stack.opts.agent {
            stack.start_agent().await;
        }
        stack
    }

    /// Directory the simulated printer writes its pages to.
    pub fn sim_out(&self) -> PathBuf {
        self.dir.path().join("sim-out")
    }

    fn agent_config(&self) -> Config {
        let text = format!(
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
out_dir = "{out}"
paper = {paper}
latency_ms = {latency}
[storage]
state_dir = "{state}"
allow_insecure_http = true
[agent]
heartbeat_secs = 1
shutdown_grace_secs = 5
"#,
            port = self.broker_port,
            out = self.sim_out().display(),
            paper = self.opts.paper,
            latency = self.opts.page_latency_ms,
            state = self.dir.path().join("pi-state").display(),
        );
        Config::from_toml(&text).expect("agent config")
    }

    /// Start the Pi agent (a fresh simulated printer on the same output directory, the same
    /// state directory) and wait for its first heartbeat to mark the printer online.
    pub async fn start_agent(&mut self) {
        assert!(self.agent.is_none(), "the agent is already running");
        let config = self.agent_config();
        self.printer = new_sim_printer(&self.sim_out(), &self.opts);
        let backend = Arc::new(SimBackend::new(Arc::clone(&self.printer)));
        let beats_before = self.cloud.heartbeats().len();
        let (shutdown, stop) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            Agent::new(config, backend)
                .run(async move {
                    let _ = stop.await;
                })
                .await
        });
        self.agent = Some(RunningAgent { shutdown, task });
        eventually("the agent's first heartbeat", WAIT, || async {
            self.cloud.heartbeats()[beats_before..]
                .iter()
                .any(|beat| beat.printer_state != fotoko_protocol::PrinterState::Offline)
                .then_some(())
        })
        .await;
        self.wait_printer(PrinterStatus::Online).await;
    }

    /// Stop the agent the way systemd does: it announces `offline` and disconnects.
    pub async fn stop_agent(&mut self) {
        let agent = self.agent.take().expect("the agent is running");
        let _ = agent.shutdown.send(());
        let result = tokio::time::timeout(Duration::from_secs(30), agent.task)
            .await
            .expect("the agent stops in time")
            .expect("the agent task");
        result.expect("the agent stops cleanly");
    }

    /// Kill the agent without a goodbye (power loss): the broker publishes the last will.
    pub async fn crash_agent(&mut self) {
        let agent = self.agent.take().expect("the agent is running");
        agent.task.abort();
        let _ = agent.task.await;
    }

    /// Wait until the API sees the printer in `status`.
    pub async fn wait_printer(&self, status: PrinterStatus) {
        let id = printer_id();
        eventually(&format!("the printer to be {status:?}"), WAIT, || async {
            (self.app.printers.snapshot(&id)?.status == status).then_some(())
        })
        .await;
    }

    /// Request `method path` as `who` (or anonymous); returns the status and the JSON body.
    pub async fn call(
        &self,
        method: Method,
        path: &str,
        who: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(who) = who {
            request = request.header("x-fotoko-identity", who);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("response");
        let status = response.status().as_u16();
        let text = response.text().await.expect("body");
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// `POST /v1/orders` with `photos`; returns the raw status and body.
    pub async fn try_create_order(&self, who: &str, photos: &[Photo]) -> (u16, Value) {
        let specs: Vec<Value> = photos
            .iter()
            .map(|photo| {
                json!({"contentType": photo.content_type, "sizeBytes": photo.bytes.len(),
                       "copies": photo.copies})
            })
            .collect();
        self.call(
            Method::POST,
            "/v1/orders",
            Some(who),
            Some(json!({"printerId": PRINTER, "photos": specs})),
        )
        .await
    }

    /// `POST /v1/orders` expecting `201`.
    pub async fn create_order(&self, who: &str, photos: &[Photo]) -> CreateOrderResponse {
        let (status, body) = self.try_create_order(who, photos).await;
        assert_eq!(status, 201, "create order: {body}");
        serde_json::from_value(body).expect("create order response")
    }

    /// PUT `photo` to its presigned `target`.
    pub async fn upload_photo(&self, target: &UploadTarget, photo: &Photo) {
        let mut put = self.http.put(&target.url);
        for (name, value) in &target.headers {
            put = put.header(name, value);
        }
        let response = put.body(photo.bytes.clone()).send().await.expect("upload");
        assert_eq!(
            response.status().as_u16(),
            200,
            "upload {}",
            target.photo_id
        );
    }

    /// Create an order and upload every photo.
    pub async fn create_uploaded_order(&self, who: &str, photos: &[Photo]) -> CreateOrderResponse {
        let created = self.create_order(who, photos).await;
        assert_eq!(created.uploads.len(), photos.len());
        for (target, photo) in created.uploads.iter().zip(photos) {
            self.upload_photo(target, photo).await;
        }
        created
    }

    /// `POST /v1/orders/{id}/checkout` expecting `200`.
    pub async fn checkout(&self, who: &str, order_id: &str) -> CheckoutResponse {
        let (status, body) = self
            .call(
                Method::POST,
                &format!("/v1/orders/{order_id}/checkout"),
                Some(who),
                None,
            )
            .await;
        assert_eq!(status, 200, "checkout: {body}");
        serde_json::from_value(body).expect("checkout response")
    }

    /// `GET /v1/orders/{id}`.
    pub async fn order(&self, who: &str, order_id: &str) -> Order {
        let (status, body) = self
            .call(
                Method::GET,
                &format!("/v1/orders/{order_id}"),
                Some(who),
                None,
            )
            .await;
        assert_eq!(status, 200, "get order: {body}");
        serde_json::from_value(body).expect("order")
    }

    /// Poll the order until it is `expected`. Panics at the deadline, and as soon as the order
    /// reaches a different final status.
    pub async fn wait_order_status(
        &self,
        who: &str,
        order_id: &str,
        expected: OrderStatus,
    ) -> Order {
        eventually(
            &format!("order {order_id} to be {expected:?}"),
            WAIT,
            || async {
                let order = self.order(who, order_id).await;
                if order.status == expected {
                    return Some(order);
                }
                assert!(
                    !matches!(
                        order.status,
                        OrderStatus::Completed
                            | OrderStatus::Failed
                            | OrderStatus::Cancelled
                            | OrderStatus::Expired
                    ),
                    "order {order_id} ended as {:?} while waiting for {expected:?}: {order:?}",
                    order.status
                );
                None
            },
        )
        .await
    }

    /// A correctly signed webhook for `order_code`.
    pub fn signed_webhook(&self, code: &str, order_code: i64, amount_vnd: u64) -> PayosWebhook {
        signed_webhook(payos::CHECKSUM_KEY, code, order_code, amount_vnd).expect("sign webhook")
    }

    /// Deliver `webhook` to the API; returns the status and body.
    pub async fn send_webhook(&self, webhook: &PayosWebhook) -> (u16, Value) {
        self.call(
            Method::POST,
            "/v1/payments/payos/webhook",
            None,
            Some(serde_json::to_value(webhook).expect("webhook json")),
        )
        .await
    }

    /// Deliver a correctly signed payment result (`"00"` = paid).
    pub async fn webhook(&self, code: &str, order_code: i64, amount_vnd: u64) -> (u16, Value) {
        self.send_webhook(&self.signed_webhook(code, order_code, amount_vnd))
            .await
    }

    /// Deliver a paid webhook expecting `200`.
    pub async fn pay(&self, order_code: i64, amount_vnd: u64) {
        let (status, body) = self.webhook("00", order_code, amount_vnd).await;
        assert_eq!(status, 200, "paid webhook: {body}");
        assert_eq!(body, json!({"success": true}));
    }

    /// Upload, check out and pay an order for `who`.
    pub async fn place_paid_order(&self, who: &str, photos: &[Photo]) -> PaidOrder {
        let created = self.create_uploaded_order(who, photos).await;
        let order_id = created.order_id.to_string();
        let checkout = self.checkout(who, &order_id).await;
        self.pay(checkout.order_code, created.amount_vnd).await;
        PaidOrder {
            order_id,
            order_code: checkout.order_code,
            amount_vnd: created.amount_vnd,
        }
    }

    /// Move the API's clock forward.
    pub fn advance_clock(&self, by: Duration) {
        self.clock
            .advance(ChronoDuration::from_std(by).expect("duration"));
    }

    /// The pages written so far, sorted by name.
    pub fn sim_pages(&self) -> Vec<Page> {
        pages_in(&self.sim_out())
    }

    /// Wait until the printer wrote exactly `count` pages (and no more shortly after is not
    /// checked: call [`Self::wait_order_status`] first when the count must be final).
    pub async fn wait_pages(&self, count: usize) -> Vec<Page> {
        eventually(&format!("{count} printed pages"), WAIT, || async {
            let pages = self.sim_pages();
            (pages.len() == count).then_some(pages)
        })
        .await
    }

    /// A webhook with a valid shape but the wrong signature.
    pub fn forged_webhook(&self, order_code: i64, amount_vnd: u64) -> PayosWebhook {
        let mut webhook = self.signed_webhook("00", order_code, amount_vnd);
        webhook.signature = sign_webhook_data("not-the-checksum-key", &webhook.data)
            .expect("sign with a wrong key");
        webhook
    }
}

fn new_sim_printer(out_dir: &Path, opts: &StackOptions) -> Arc<SimPrinter> {
    let mut config = SimConfig::new(out_dir);
    config.paper = opts.paper;
    config.latency = Duration::from_millis(opts.page_latency_ms);
    Arc::new(SimPrinter::new(config).expect("sim printer"))
}

/// The PNG pages in `dir`, sorted by name; unfinished `.tmp` files are not pages.
pub fn pages_in(dir: &Path) -> Vec<Page> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut pages: Vec<Page> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !name.ends_with(".png") {
                return None;
            }
            let (width, height) = image::image_dimensions(entry.path()).ok()?;
            Some(Page {
                name,
                width,
                height,
            })
        })
        .collect();
    pages.sort_by(|a, b| a.name.cmp(&b.name));
    pages
}
