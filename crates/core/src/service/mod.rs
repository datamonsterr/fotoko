//! Use cases. [`OrderService`] holds the ports as `Arc<dyn Port>` and owns no state of its own,
//! so it is cheap to clone and safe to share between Lambda invocations.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use fotoko_protocol::{
    CheckoutResponse, CreateOrderRequest, CreateOrderResponse, Heartbeat, JobPhoto, JobState,
    JobStatus, Media, ORDER_TTL, OrderError, OrderId, OrderStatus, PayosWebhook, PiErrorCode,
    PrintJob, PrinterId, PrinterInfo, PrinterState, PrinterStatus, UploadMethod, UploadTarget,
};

use crate::domain::{self, Order, Photo};
use crate::error::CoreError;
use crate::payos;
use crate::ports::{
    Clock, IdGen, JobPublisher, ObjectStore, OrderRepo, PaymentGateway, PaymentLinkRequest,
    PrinterRepo, PutSpec,
};
use crate::state::OrderEvent;

#[cfg(test)]
mod tests;

/// How often a taken payOS order code is redrawn before giving up.
const ORDER_CODE_ATTEMPTS: usize = 5;
/// Default for [`ServiceConfig::expire_batch_size`].
const EXPIRE_BATCH_SIZE: usize = 100;
/// payOS limits the payment description to 25 characters. `Fotoko ` plus a valid order code (at
/// most 16 digits) is 23 characters, so the cap is only a guard against oversized codes.
const DESCRIPTION_MAX_CHARS: usize = 25;
/// Presigned GET URLs handed to the Pi stay valid this long.
const DOWNLOAD_TTL: Duration = Duration::from_secs(15 * 60);

/// Settings of the service.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceConfig {
    /// `PUBLIC_APP_URL`; payOS return and cancel URLs are `{public_app_url}/orders/{orderId}`.
    pub public_app_url: String,
    /// payOS checksum key used to verify webhooks.
    pub payos_checksum_key: String,
    /// Orders expire this long after creation unless paid.
    pub order_ttl: Duration,
    /// Validity of the presigned GET URLs in a print job.
    pub download_ttl: Duration,
    /// How many overdue orders [`OrderService::expire_due`] loads per batch.
    pub expire_batch_size: usize,
}

impl ServiceConfig {
    /// Config with the contract defaults: 15 minute order TTL and 15 minute download URLs.
    /// `Internal` (a configuration error) when the checksum key is empty: an empty key would make
    /// every webhook signature forgeable.
    pub fn new(
        public_app_url: impl Into<String>,
        payos_checksum_key: impl Into<String>,
    ) -> Result<Self, CoreError> {
        let payos_checksum_key = payos_checksum_key.into();
        if payos_checksum_key.trim().is_empty() {
            return Err(CoreError::Internal(
                "payOS checksum key must not be empty".into(),
            ));
        }
        Ok(Self::unchecked(public_app_url, payos_checksum_key))
    }

    /// [`new`](Self::new) without the key check, for the in-memory test harness.
    pub(crate) fn unchecked(
        public_app_url: impl Into<String>,
        payos_checksum_key: impl Into<String>,
    ) -> Self {
        Self {
            public_app_url: public_app_url.into(),
            payos_checksum_key: payos_checksum_key.into(),
            order_ttl: ORDER_TTL,
            download_ttl: DOWNLOAD_TTL,
            expire_batch_size: EXPIRE_BATCH_SIZE,
        }
    }
}

impl fmt::Debug for ServiceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceConfig")
            .field("public_app_url", &self.public_app_url)
            .field("payos_checksum_key", &"<redacted>")
            .field("order_ttl", &self.order_ttl)
            .field("download_ttl", &self.download_ttl)
            .field("expire_batch_size", &self.expire_batch_size)
            .finish()
    }
}

/// The adapters a service runs on.
#[derive(Clone)]
pub struct Ports {
    /// Order storage.
    pub orders: Arc<dyn OrderRepo>,
    /// Printer storage.
    pub printers: Arc<dyn PrinterRepo>,
    /// Photo storage.
    pub store: Arc<dyn ObjectStore>,
    /// Job delivery to Pis.
    pub publisher: Arc<dyn JobPublisher>,
    /// payOS.
    pub payments: Arc<dyn PaymentGateway>,
    /// Time.
    pub clock: Arc<dyn Clock>,
    /// Ids and order codes.
    pub ids: Arc<dyn IdGen>,
}

impl fmt::Debug for Ports {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ports").finish_non_exhaustive()
    }
}

/// What a payOS webhook delivery did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookOutcome {
    /// The order moved to `paid` and its print job was published.
    Paid,
    /// The order was already paid (or further along): nothing changed. While the order is still
    /// `paid`, the job is published again so a delivery that failed to publish heals on payOS's
    /// retry; the job id is derived from the order id and the Pi dedupes by it.
    Duplicate,
    /// The webhook does not report a successful payment: the signed `data.code` is not `"00"`,
    /// or the unsigned outer `code` disagrees with it. Nothing changed.
    NotPaid,
    /// A valid, matching payment arrived for an order that is already `cancelled` or `expired`.
    /// The customer paid for nothing and needs a manual refund; the order is unchanged. The
    /// caller acknowledges the webhook (200) so payOS stops retrying, and **must log this at
    /// warn level** with the order id and code.
    LatePayment,
}

/// What a Pi job status did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusOutcome {
    /// The order changed status.
    Applied,
    /// Unknown order, or a stale or repeated report: nothing changed.
    Ignored,
}

/// Result of one try at applying a Pi status report.
enum Attempt {
    Applied,
    Ignored,
    /// The conditional write lost a race.
    Raced,
}

/// The use cases of the Fotoko backend.
#[derive(Debug, Clone)]
pub struct OrderService {
    ports: Ports,
    config: ServiceConfig,
}

/// The checkout response of an order that already has its payment link.
fn stored_checkout(order: &Order) -> Option<CheckoutResponse> {
    match (
        order.status,
        order.order_code,
        &order.checkout_url,
        &order.qr_code,
    ) {
        (OrderStatus::AwaitingPayment, Some(code), Some(url), Some(qr)) => Some(CheckoutResponse {
            order_id: order.order_id.clone(),
            status: OrderStatus::AwaitingPayment,
            order_code: code,
            checkout_url: url.clone(),
            qr_code: qr.clone(),
        }),
        _ => None,
    }
}

fn pi_error_code(code: PiErrorCode) -> &'static str {
    match code {
        PiErrorCode::PaperOut => "paper_out",
        PiErrorCode::RibbonOut => "ribbon_out",
        PiErrorCode::PrinterOffline => "printer_offline",
        PiErrorCode::DownloadFailed => "download_failed",
        PiErrorCode::InvalidImage => "invalid_image",
        PiErrorCode::DriverError => "driver_error",
    }
}

fn printer_status(state: PrinterState) -> PrinterStatus {
    match state {
        PrinterState::Idle => PrinterStatus::Online,
        PrinterState::Printing => PrinterStatus::Busy,
        PrinterState::Error => PrinterStatus::Error,
        PrinterState::Offline => PrinterStatus::Offline,
    }
}

impl OrderService {
    /// Service over `ports`.
    pub fn new(ports: Ports, config: ServiceConfig) -> Self {
        Self { ports, config }
    }

    /// `GET /v1/printers/{printerId}`.
    pub async fn get_printer(&self, printer_id: &PrinterId) -> Result<PrinterInfo, CoreError> {
        let printer = self.ports.printers.get(printer_id).await?;
        printer
            .map(|p| p.to_info())
            .ok_or_else(|| CoreError::NotFound(format!("printer {printer_id} not found")))
    }

    /// `POST /v1/orders`: validate, price, presign one PUT per photo and store the order in
    /// `awaiting_upload`.
    pub async fn create_order(
        &self,
        owner: &str,
        request: &CreateOrderRequest,
    ) -> Result<CreateOrderResponse, CoreError> {
        domain::validate_photos(&request.photos)?;
        let printer = self
            .ports
            .printers
            .get(&request.printer_id)
            .await?
            .ok_or_else(|| {
                CoreError::NotFound(format!("printer {} not found", request.printer_id))
            })?;
        printer.require_bookable()?;
        let amount_vnd = domain::price(&request.photos, printer.price_per_print_vnd)?;

        let now = self.ports.clock.now();
        let expires_at = domain::expiry(now, self.config.order_ttl)?;
        let order_id = self.ports.ids.order_id();

        let photos: Vec<Photo> = request
            .photos
            .iter()
            .map(|spec| Photo {
                photo_id: self.ports.ids.photo_id(),
                content_type: spec.content_type,
                size_bytes: spec.size_bytes,
                copies: spec.copies,
                uploaded: false,
            })
            .collect();

        let mut uploads = Vec::with_capacity(photos.len());
        for photo in &photos {
            let presigned = self
                .ports
                .store
                .presign_put(&PutSpec {
                    key: photo.object_key(&order_id),
                    content_type: photo.content_type,
                    size_bytes: photo.size_bytes,
                    ttl: self.config.order_ttl,
                })
                .await?;
            uploads.push(UploadTarget {
                photo_id: photo.photo_id.clone(),
                url: presigned.url,
                method: UploadMethod::Put,
                headers: presigned.headers,
                expires_at,
            });
        }

        let order = Order {
            order_id: order_id.clone(),
            owner: owner.to_owned(),
            printer_id: request.printer_id.clone(),
            status: OrderStatus::AwaitingUpload,
            amount_vnd,
            order_code: None,
            checkout_url: None,
            qr_code: None,
            photos,
            created_at: now,
            updated_at: now,
            expires_at,
            error: None,
        };
        self.ports.orders.insert(&order).await?;

        Ok(CreateOrderResponse {
            order_id,
            status: OrderStatus::AwaitingUpload,
            amount_vnd,
            expires_at,
            uploads,
        })
    }

    /// `POST /v1/orders/{orderId}/checkout`: all photos must be in the store; allocates the
    /// payOS order code, creates the payment link and moves the order to `awaiting_payment`.
    /// Repeating the call returns the stored link.
    pub async fn checkout(
        &self,
        owner: &str,
        order_id: &OrderId,
    ) -> Result<CheckoutResponse, CoreError> {
        let order = self.load_owned(owner, order_id).await?;
        let order = self.expire_if_due(order).await?;

        if let Some(stored) = stored_checkout(&order) {
            return Ok(stored);
        }

        let now = self.ports.clock.now();
        let mut next = order.advance(OrderEvent::Checkout, now)?;

        for photo in &mut next.photos {
            let key = photo.object_key(order_id);
            if !self.ports.store.exists(&key).await? {
                return Err(CoreError::Conflict(format!(
                    "photo {} is not uploaded",
                    photo.photo_id
                )));
            }
            photo.uploaded = true;
        }

        let order_code = self.allocate_order_code().await?;
        let return_url = format!(
            "{}/orders/{order_id}",
            self.config.public_app_url.trim_end_matches('/')
        );
        let link = self
            .ports
            .payments
            .create_payment_link(&PaymentLinkRequest {
                order_code,
                amount_vnd: order.amount_vnd,
                description: format!("Fotoko {order_code}")
                    .chars()
                    .take(DESCRIPTION_MAX_CHARS)
                    .collect(),
                cancel_url: return_url.clone(),
                return_url,
                expires_at: order.expires_at,
            })
            .await?;

        next.order_code = Some(order_code);
        next.checkout_url = Some(link.checkout_url.clone());
        next.qr_code = Some(link.qr_code.clone());
        match self.ports.orders.update(&next, order.status).await {
            Ok(()) => {}
            Err(CoreError::Conflict(_)) => {
                // A concurrent checkout won. Its link is the order's link; ours stays unused at
                // payOS and lapses with `expires_at`.
                let fresh = self.load(order_id).await?;
                return stored_checkout(&fresh).ok_or_else(|| {
                    CoreError::Conflict(format!("order {order_id} changed during checkout"))
                });
            }
            Err(error) => return Err(error),
        }

        Ok(CheckoutResponse {
            order_id: next.order_id,
            status: next.status,
            order_code,
            checkout_url: link.checkout_url,
            qr_code: link.qr_code,
        })
    }

    /// `GET /v1/orders/{orderId}` (owner only). An unpaid order past its expiry is expired on
    /// the spot; `uploaded` reflects the store while the order awaits uploads.
    pub async fn get_order(
        &self,
        owner: &str,
        order_id: &OrderId,
    ) -> Result<fotoko_protocol::Order, CoreError> {
        let order = self.load_owned(owner, order_id).await?;
        let mut order = self.expire_if_due(order).await?;
        if order.status == OrderStatus::AwaitingUpload {
            for photo in &mut order.photos {
                if !photo.uploaded {
                    photo.uploaded = self.ports.store.exists(&photo.object_key(order_id)).await?;
                }
            }
        }
        Ok(order.to_dto())
    }

    /// `POST /v1/orders/{orderId}/cancel` (owner only): only before `paid`.
    pub async fn cancel(
        &self,
        owner: &str,
        order_id: &OrderId,
    ) -> Result<fotoko_protocol::Order, CoreError> {
        let order = self.load_owned(owner, order_id).await?;
        let cancelled = order.advance(OrderEvent::Cancel, self.ports.clock.now())?;
        self.ports.orders.update(&cancelled, order.status).await?;
        Ok(cancelled.to_dto())
    }

    /// `POST /v1/payments/payos/webhook`.
    ///
    /// 1. Verifies the signature (`Validation` when bad). The signature covers only `data`, so
    ///    the payment result is read from the signed `data.code` (`Validation` when absent). The
    ///    unsigned outer `code` is only a consistency check: if it disagrees, nothing is paid.
    /// 2. Finds the order by order code (`NotFound`).
    /// 3. `data.code != "00"` or an inconsistent outer code: [`WebhookOutcome::NotPaid`].
    /// 4. The amount must equal the order's (`Validation`, order unchanged).
    /// 5. `awaiting_payment` moves to `paid` and the print job is published with presigned GET
    ///    URLs. A repeat delivery succeeds without changing the order; while the order is still
    ///    `paid` it republishes the job. A lost race against a concurrent delivery is re-read and
    ///    treated the same way. Cancelled or expired orders give
    ///    [`WebhookOutcome::LatePayment`]; an `awaiting_upload` order is a `Conflict`.
    pub async fn handle_payos_webhook(
        &self,
        webhook: &PayosWebhook,
    ) -> Result<WebhookOutcome, CoreError> {
        if !payos::verify_webhook(&self.config.payos_checksum_key, webhook) {
            return Err(CoreError::Validation("invalid webhook signature".into()));
        }
        let Some(signed_code) = webhook.data.code() else {
            return Err(CoreError::Validation("webhook data has no code".into()));
        };
        let order_code = webhook.data.order_code;
        let order = self
            .ports
            .orders
            .get_by_order_code(order_code)
            .await?
            .ok_or_else(|| CoreError::NotFound(format!("no order for code {order_code}")))?;

        if signed_code != "00" || webhook.code != "00" {
            return Ok(WebhookOutcome::NotPaid);
        }
        if webhook.data.amount != order.amount_vnd {
            return Err(CoreError::Validation(format!(
                "paid amount {} does not match order amount {}",
                webhook.data.amount, order.amount_vnd
            )));
        }

        if let Some(outcome) = self.already_settled(&order).await? {
            return Ok(outcome);
        }
        let paid = order.advance(OrderEvent::PaymentConfirmed, self.ports.clock.now())?;
        match self.ports.orders.update(&paid, order.status).await {
            Ok(()) => {}
            Err(CoreError::Conflict(_)) => {
                let fresh = self.load(&order.order_id).await?;
                return self.already_settled(&fresh).await?.ok_or_else(|| {
                    CoreError::Conflict(format!("order {} changed during payment", order.order_id))
                });
            }
            Err(error) => return Err(error),
        }
        self.publish_job(&paid).await?;
        Ok(WebhookOutcome::Paid)
    }

    /// Outcome of a valid payment for an order that is not `awaiting_payment`; `None` when it is.
    async fn already_settled(&self, order: &Order) -> Result<Option<WebhookOutcome>, CoreError> {
        match order.status {
            OrderStatus::AwaitingPayment => Ok(None),
            OrderStatus::Paid => {
                self.publish_job(order).await?;
                Ok(Some(WebhookOutcome::Duplicate))
            }
            OrderStatus::Printing | OrderStatus::Completed | OrderStatus::Failed => {
                Ok(Some(WebhookOutcome::Duplicate))
            }
            OrderStatus::Cancelled | OrderStatus::Expired => Ok(Some(WebhookOutcome::LatePayment)),
            OrderStatus::AwaitingUpload => Err(CoreError::Conflict(
                "order is awaiting_upload: payment not allowed".into(),
            )),
        }
    }

    /// Pi job status from `fotoko/printers/{printerId}/status`.
    ///
    /// `received` and `printing` move `paid` to `printing`, `completed` moves to `completed`
    /// (through `printing` when the `received` report was lost) and `failed` moves `paid` or
    /// `printing` to `failed` with the Pi's error. Unknown orders and stale or repeated reports
    /// are ignored. A report from another printer than the order's is `Forbidden`. When the
    /// conditional write loses a race, the order is re-read and the report applied once more;
    /// a second loss, or a report that no longer applies, is ignored.
    pub async fn handle_job_status(&self, status: &JobStatus) -> Result<StatusOutcome, CoreError> {
        let Some(order) = self.ports.orders.get(&status.order_id).await? else {
            return Ok(StatusOutcome::Ignored);
        };
        match self.try_apply_status(&order, status).await? {
            Attempt::Applied => Ok(StatusOutcome::Applied),
            Attempt::Ignored => Ok(StatusOutcome::Ignored),
            Attempt::Raced => {
                let Some(fresh) = self.ports.orders.get(&status.order_id).await? else {
                    return Ok(StatusOutcome::Ignored);
                };
                match self.try_apply_status(&fresh, status).await? {
                    Attempt::Applied => Ok(StatusOutcome::Applied),
                    Attempt::Ignored | Attempt::Raced => Ok(StatusOutcome::Ignored),
                }
            }
        }
    }

    async fn try_apply_status(
        &self,
        order: &Order,
        status: &JobStatus,
    ) -> Result<Attempt, CoreError> {
        if order.printer_id != status.printer_id {
            return Err(CoreError::Forbidden(format!(
                "printer {} does not own order {}",
                status.printer_id, order.order_id
            )));
        }

        let events: &[OrderEvent] = match (status.state, order.status) {
            (JobState::Received | JobState::Printing, OrderStatus::Paid) => {
                &[OrderEvent::JobStarted]
            }
            (JobState::Completed, OrderStatus::Paid) => {
                &[OrderEvent::JobStarted, OrderEvent::JobCompleted]
            }
            (JobState::Completed, OrderStatus::Printing) => &[OrderEvent::JobCompleted],
            (JobState::Failed, OrderStatus::Paid | OrderStatus::Printing) => {
                &[OrderEvent::JobFailed]
            }
            _ => return Ok(Attempt::Ignored),
        };

        let now = self.ports.clock.now();
        let mut next = order.clone();
        for event in events {
            next = next.advance(*event, now)?;
        }
        if status.state == JobState::Failed {
            next.error = Some(match &status.error {
                Some(error) => OrderError {
                    code: pi_error_code(error.code).to_owned(),
                    message: error.message.clone(),
                },
                None => OrderError {
                    code: "print_failed".to_owned(),
                    message: "the printer reported a failure".to_owned(),
                },
            });
        }
        match self.ports.orders.update(&next, order.status).await {
            Ok(()) => Ok(Attempt::Applied),
            Err(CoreError::Conflict(_)) => Ok(Attempt::Raced),
            Err(error) => Err(error),
        }
    }

    /// Pi heartbeat: sets the printer's status and `lastSeenAt` (server time). `NotFound` for
    /// an unknown printer.
    pub async fn handle_heartbeat(&self, heartbeat: &Heartbeat) -> Result<(), CoreError> {
        self.ports
            .printers
            .update_status(
                &heartbeat.printer_id,
                printer_status(heartbeat.printer_state),
                self.ports.clock.now(),
            )
            .await
    }

    /// Expire every unpaid order past its `expiresAt`, in batches of
    /// [`ServiceConfig::expire_batch_size`] until none are left. Returns how many orders this
    /// call expired. Orders that changed concurrently are skipped; a batch in which nothing could
    /// be expired ends the sweep, which the next run resumes.
    pub async fn expire_due(&self) -> Result<usize, CoreError> {
        let now = self.ports.clock.now();
        let batch_size = self.config.expire_batch_size.max(1);
        let mut total = 0;
        loop {
            let due = self
                .ports
                .orders
                .list_due_for_expiry(now, batch_size)
                .await?;
            if due.is_empty() {
                return Ok(total);
            }
            let mut expired = 0;
            for order in due {
                let next = order.advance(OrderEvent::Expire, now)?;
                match self.ports.orders.update(&next, order.status).await {
                    Ok(()) => expired += 1,
                    Err(CoreError::Conflict(_)) => {}
                    Err(error) => return Err(error),
                }
            }
            if expired == 0 {
                return Ok(total);
            }
            total += expired;
        }
    }

    async fn load_owned(&self, owner: &str, order_id: &OrderId) -> Result<Order, CoreError> {
        let order = self.load(order_id).await?;
        if order.owner != owner {
            return Err(CoreError::Forbidden(format!(
                "order {order_id} belongs to someone else"
            )));
        }
        Ok(order)
    }

    async fn load(&self, order_id: &OrderId) -> Result<Order, CoreError> {
        self.ports
            .orders
            .get(order_id)
            .await?
            .ok_or_else(|| CoreError::NotFound(format!("order {order_id} not found")))
    }

    /// Expire `order` if it is unpaid and overdue; a concurrent change wins and is re-read.
    async fn expire_if_due(&self, order: Order) -> Result<Order, CoreError> {
        let now = self.ports.clock.now();
        if !order.is_due_for_expiry(now) {
            return Ok(order);
        }
        let expired = order.advance(OrderEvent::Expire, now)?;
        match self.ports.orders.update(&expired, order.status).await {
            Ok(()) => Ok(expired),
            Err(CoreError::Conflict(_)) => self.load(&order.order_id).await,
            Err(error) => Err(error),
        }
    }

    async fn allocate_order_code(&self) -> Result<i64, CoreError> {
        for _ in 0..ORDER_CODE_ATTEMPTS {
            let code = self.ports.ids.order_code();
            if self.ports.orders.get_by_order_code(code).await?.is_none() {
                return Ok(code);
            }
        }
        Err(CoreError::Internal(
            "could not allocate a unique order code".into(),
        ))
    }

    /// Publish the print job of a paid order. The job id is `job-{orderId}`: stable, so a
    /// republish is deduplicated by the Pi.
    async fn publish_job(&self, order: &Order) -> Result<(), CoreError> {
        let mut photos = Vec::with_capacity(order.photos.len());
        for photo in &order.photos {
            let url = self
                .ports
                .store
                .presign_get(&photo.object_key(&order.order_id), self.config.download_ttl)
                .await?;
            photos.push(JobPhoto {
                photo_id: photo.photo_id.clone(),
                url,
                content_type: photo.content_type,
                copies: photo.copies,
            });
        }
        let job = PrintJob {
            job_id: format!("job-{}", order.order_id).into(),
            order_id: order.order_id.clone(),
            printer_id: order.printer_id.clone(),
            media: Media::Photo4x6,
            created_at: self.ports.clock.now(),
            photos,
        };
        self.ports.publisher.publish_job(&job).await
    }
}
