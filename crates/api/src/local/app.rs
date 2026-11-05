//! The local/sim app: in-memory repos, local-fs object store, optional MQTT, real payOS client.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use fotoko_core::testing::{InMemoryOrderRepo, InMemoryPrinterRepo, RecordingJobPublisher};
use fotoko_core::{
    Clock, CoreError, JobPublisher, OrderService, Ports, Printer, ServiceConfig, SystemClock,
};
use fotoko_protocol::{PrinterId, PrinterStatus};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::mqtt;
use super::objects::LocalObjectStore;
use crate::api::{AppState, api_routes, with_request_context};
use crate::config::{DEFAULT_PAYOS_BASE_URL, LocalConfig};
use crate::identity::IdentityMode;
use crate::ids::RandomIdGen;
use crate::payos::{PayosClient, PayosCredentials};

/// How often `expire_due` runs by default.
pub const DEFAULT_EXPIRE_INTERVAL: Duration = Duration::from_secs(30);

/// A running local app. Background tasks stop when it is dropped.
#[derive(Debug)]
pub struct LocalApp {
    /// The HTTP API plus the object routes; serve it with `axum::serve` or call it in-process.
    pub router: Router,
    /// The use cases, for tests that drive the service directly.
    pub service: OrderService,
    /// Order storage.
    pub orders: InMemoryOrderRepo,
    /// Printer storage (for seeding and for assertions).
    pub printers: InMemoryPrinterRepo,
    /// Object store behind the presigned-style URLs.
    pub objects: Arc<LocalObjectStore>,
    /// Jobs published through the default recording publisher, when MQTT is not used.
    pub recorded_jobs: RecordingJobPublisher,
    tasks: Vec<JoinHandle<()>>,
}

impl LocalApp {
    /// Start configuring an app.
    pub fn builder() -> LocalAppBuilder {
        LocalAppBuilder::default()
    }

    /// Serve [`Self::router`] on `listener` in a background task; returns the bound address.
    pub fn serve(&mut self, listener: TcpListener) -> std::io::Result<std::net::SocketAddr> {
        let addr = listener.local_addr()?;
        let router = self.router.clone();
        self.tasks.push(tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, router).await {
                tracing::error!(%error, "local server stopped");
            }
        }));
        Ok(addr)
    }
}

impl Drop for LocalApp {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Builder of a [`LocalApp`].
pub struct LocalAppBuilder {
    public_url: String,
    object_dir: Option<PathBuf>,
    url_secret: Option<String>,
    payos_base_url: String,
    payos: PayosCredentials,
    public_app_url: String,
    mqtt_url: Option<String>,
    publisher: Option<Arc<dyn JobPublisher>>,
    clock: Option<Arc<dyn Clock>>,
    printers: Vec<Printer>,
    default_price_vnd: u64,
    expire_every: Option<Duration>,
}

impl std::fmt::Debug for LocalAppBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalAppBuilder")
            .field("public_url", &self.public_url)
            .field("mqtt_url", &self.mqtt_url)
            .field("payos_base_url", &self.payos_base_url)
            .finish_non_exhaustive()
    }
}

impl Default for LocalAppBuilder {
    fn default() -> Self {
        Self {
            public_url: "http://127.0.0.1:8787".into(),
            object_dir: None,
            url_secret: None,
            payos_base_url: DEFAULT_PAYOS_BASE_URL.into(),
            payos: PayosCredentials {
                client_id: "local-client-id".into(),
                api_key: "local-api-key".into(),
                checksum_key: "local-checksum-key".into(),
            },
            public_app_url: "http://localhost:8081".into(),
            mqtt_url: None,
            publisher: None,
            clock: None,
            printers: Vec::new(),
            default_price_vnd: fotoko_protocol::DEFAULT_PRICE_VND,
            expire_every: Some(DEFAULT_EXPIRE_INTERVAL),
        }
    }
}

impl LocalAppBuilder {
    /// Everything from the validated environment settings.
    pub fn from_config(config: &LocalConfig) -> Result<Self, CoreError> {
        let mut builder = Self::default()
            .public_url(&config.public_url)
            .object_dir(&config.object_dir)
            .payos(
                &config.payos_base_url,
                PayosCredentials {
                    client_id: config.payos_client_id.clone(),
                    api_key: config.payos_api_key.clone(),
                    checksum_key: config.payos_checksum_key.clone(),
                },
            )
            .public_app_url(&config.public_app_url)
            .mqtt_url(config.mqtt_url.clone());
        builder.default_price_vnd = config.default_price_vnd;
        builder.url_secret.clone_from(&config.url_secret);
        for id in &config.printers {
            let printer_id = PrinterId::new(id.as_str())
                .map_err(|e| CoreError::Internal(format!("invalid printer id {id:?}: {e}")))?;
            builder = builder.printer(online_printer(printer_id, config.default_price_vnd));
        }
        Ok(builder)
    }

    /// Base URL clients use to reach this server; presigned-style URLs start with it.
    #[must_use]
    pub fn public_url(mut self, url: impl Into<String>) -> Self {
        self.public_url = url.into();
        self
    }

    /// Directory for uploaded photos (default: a fresh directory under the system temp dir).
    #[must_use]
    pub fn object_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.object_dir = Some(dir.into());
        self
    }

    /// Secret that signs object URLs (default: random per app).
    #[must_use]
    pub fn url_secret(mut self, secret: impl Into<String>) -> Self {
        self.url_secret = Some(secret.into());
        self
    }

    /// payOS endpoint and credentials; the checksum key also verifies webhooks.
    #[must_use]
    pub fn payos(mut self, base_url: impl Into<String>, credentials: PayosCredentials) -> Self {
        self.payos_base_url = base_url.into();
        self.payos = credentials;
        self
    }

    /// Base of the payOS return and cancel URLs.
    #[must_use]
    pub fn public_app_url(mut self, url: impl Into<String>) -> Self {
        self.public_app_url = url.into();
        self
    }

    /// Broker for jobs, statuses and heartbeats; `None` publishes to the recording publisher.
    #[must_use]
    pub fn mqtt_url(mut self, url: Option<String>) -> Self {
        self.mqtt_url = url;
        self
    }

    /// Publish jobs here instead of MQTT or the recorder.
    #[must_use]
    pub fn publisher(mut self, publisher: Arc<dyn JobPublisher>) -> Self {
        self.publisher = Some(publisher);
        self
    }

    /// Time source (default: the system clock).
    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Seed a printer. Without any, `stand-001` is seeded online.
    #[must_use]
    pub fn printer(mut self, printer: Printer) -> Self {
        self.printers.push(printer);
        self
    }

    /// How often overdue orders are expired; `None` turns the sweep off.
    #[must_use]
    pub fn expire_every(mut self, interval: Option<Duration>) -> Self {
        self.expire_every = interval;
        self
    }

    /// Wire everything and start the background tasks (MQTT, expiry sweep).
    pub async fn build(self) -> Result<LocalApp, CoreError> {
        let clock: Arc<dyn Clock> = self.clock.unwrap_or_else(|| Arc::new(SystemClock));
        let orders = InMemoryOrderRepo::new();
        let printers = InMemoryPrinterRepo::new();
        let recorded_jobs = RecordingJobPublisher::new();
        let mut printer_list = self.printers;
        if printer_list.is_empty() {
            let id = PrinterId::new("stand-001")
                .map_err(|e| CoreError::Internal(format!("invalid default printer id: {e}")))?;
            printer_list.push(online_printer(id, self.default_price_vnd));
        }
        for printer in printer_list {
            printers.insert(printer);
        }

        let object_dir = self.object_dir.unwrap_or_else(|| {
            std::env::temp_dir().join(format!("fotoko-local-{}", uuid::Uuid::new_v4().simple()))
        });
        let secret = self
            .url_secret
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        let objects = Arc::new(LocalObjectStore::new(
            object_dir,
            self.public_url,
            secret,
            Arc::clone(&clock),
        ));

        let mut tasks = Vec::new();
        let link = self
            .mqtt_url
            .as_deref()
            .map(|url| {
                mqtt::connect(
                    url,
                    &format!("fotoko-local-{}", uuid::Uuid::new_v4().simple()),
                )
            })
            .transpose()?;
        let (mqtt_publisher, mqtt_events) = match link {
            Some((publisher, events)) => (Some(publisher), Some(events)),
            None => (None, None),
        };
        let publisher: Arc<dyn JobPublisher> = match (self.publisher, mqtt_publisher) {
            (Some(custom), _) => custom,
            (None, Some(mqtt)) => Arc::new(mqtt),
            (None, None) => Arc::new(recorded_jobs.clone()),
        };

        let payments = PayosClient::new(&self.payos_base_url, self.payos.clone())?;
        let service = OrderService::new(
            Ports {
                orders: Arc::new(orders.clone()),
                printers: Arc::new(printers.clone()),
                store: objects.clone(),
                publisher,
                payments: Arc::new(payments),
                clock,
                ids: Arc::new(RandomIdGen),
            },
            ServiceConfig::new(self.public_app_url, self.payos.checksum_key.clone())?,
        );

        if let Some(events) = mqtt_events {
            tasks.push(tokio::spawn(events.run(service.clone())));
        }
        if let Some(interval) = self.expire_every {
            tasks.push(tokio::spawn(expire_loop(service.clone(), interval)));
        }

        let router = with_request_context(
            api_routes(AppState {
                service: service.clone(),
                identity_mode: IdentityMode::Local,
            })
            .merge(objects.routes()),
        );
        Ok(LocalApp {
            router,
            service,
            orders,
            printers,
            objects,
            recorded_jobs,
            tasks,
        })
    }
}

fn online_printer(printer_id: PrinterId, price_per_print_vnd: u64) -> Printer {
    Printer {
        name: printer_id.to_string(),
        printer_id,
        price_per_print_vnd,
        status: PrinterStatus::Online,
        last_seen_at: None,
    }
}

async fn expire_loop(service: OrderService, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        match service.expire_due().await {
            Ok(0) => {}
            Ok(count) => tracing::info!(count, "expired overdue orders"),
            Err(error) => tracing::error!(%error, "expiry sweep failed"),
        }
    }
}
