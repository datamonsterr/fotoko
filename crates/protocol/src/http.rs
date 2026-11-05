//! HTTP API bodies (base `/v1`, JSON, camelCase). See `docs/contracts.md`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{OrderId, PhotoId, PrinterId};

/// Photo media size a printer supports. Only 4x6 inch today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Media {
    /// 4x6 inch print.
    #[serde(rename = "4x6")]
    Photo4x6,
}

/// Image formats accepted for upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentType {
    /// JPEG image.
    #[serde(rename = "image/jpeg")]
    Jpeg,
    /// PNG image.
    #[serde(rename = "image/png")]
    Png,
}

/// Printer availability as shown to customers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrinterStatus {
    /// Ready to print.
    Online,
    /// Not reachable.
    Offline,
    /// Printing now.
    Busy,
    /// Faulted (paper, ribbon, driver).
    Error,
}

/// Order lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    /// Waiting for photo uploads.
    AwaitingUpload,
    /// Checkout created, waiting for payOS.
    AwaitingPayment,
    /// Paid, job not yet received by the Pi.
    Paid,
    /// Pi is printing.
    Printing,
    /// All copies printed.
    Completed,
    /// Printing failed after payment.
    Failed,
    /// Cancelled by the customer before payment.
    Cancelled,
    /// Not paid within the TTL.
    Expired,
}

/// Machine-readable error code in an error body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Resource does not exist.
    NotFound,
    /// Request body or state failed validation.
    Validation,
    /// Caller may not access the resource.
    Forbidden,
    /// Invalid state transition.
    Conflict,
    /// payOS call failed.
    PaymentProvider,
    /// Unexpected server error.
    Internal,
    /// A code this client version does not know.
    #[serde(other)]
    Unknown,
}

/// `{"error":{"code":"...","message":"..."}}` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// Error code.
    pub code: ErrorCode,
    /// Human readable message.
    pub message: String,
}

/// Body of every non-2xx response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// The error.
    pub error: ErrorDetail,
}

impl ErrorBody {
    /// Build an error body.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            error: ErrorDetail {
                code,
                message: message.into(),
            },
        }
    }
}

/// `GET /healthz` response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// Always true when the service is up.
    pub ok: bool,
}

/// `GET /v1/printers/{printerId}` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterInfo {
    /// Printer (Thing) id.
    pub printer_id: PrinterId,
    /// Display name.
    pub name: String,
    /// Availability.
    pub status: PrinterStatus,
    /// Price of one print in VND.
    pub price_per_print_vnd: u64,
    /// Supported media.
    pub media: Vec<Media>,
}

/// One photo in a create-order request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoSpec {
    /// Image format.
    pub content_type: ContentType,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Copies to print.
    pub copies: u32,
}

/// `POST /v1/orders` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrderRequest {
    /// Target printer.
    pub printer_id: PrinterId,
    /// Photos to print.
    pub photos: Vec<PhotoSpec>,
}

/// HTTP method of an upload target. Always PUT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadMethod {
    /// HTTP PUT.
    #[serde(rename = "PUT")]
    Put,
}

/// Presigned upload for one photo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadTarget {
    /// Photo id.
    pub photo_id: PhotoId,
    /// Presigned URL.
    pub url: String,
    /// Method to use.
    pub method: UploadMethod,
    /// Headers to send with the upload.
    pub headers: BTreeMap<String, String>,
    /// URL expiry.
    pub expires_at: DateTime<Utc>,
}

/// `POST /v1/orders` response (201).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrderResponse {
    /// New order id.
    pub order_id: OrderId,
    /// Always `awaiting_upload`.
    pub status: OrderStatus,
    /// Total price in VND.
    pub amount_vnd: u64,
    /// Order expiry.
    pub expires_at: DateTime<Utc>,
    /// One upload target per photo.
    pub uploads: Vec<UploadTarget>,
}

/// Photo entry inside an [`Order`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderPhoto {
    /// Photo id.
    pub photo_id: PhotoId,
    /// Copies to print.
    pub copies: u32,
    /// Image format.
    pub content_type: ContentType,
    /// Whether the object exists in S3.
    pub uploaded: bool,
}

/// Failure details on an [`Order`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderError {
    /// Error code (for example a Pi error code).
    pub code: String,
    /// Human readable message.
    pub message: String,
}

/// `GET /v1/orders/{orderId}` response; also `cancel` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    /// Order id.
    pub order_id: OrderId,
    /// Printer id.
    pub printer_id: PrinterId,
    /// Status.
    pub status: OrderStatus,
    /// Total price in VND.
    pub amount_vnd: u64,
    /// payOS order code once checkout exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_code: Option<i64>,
    /// Photos.
    pub photos: Vec<OrderPhoto>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Expiry time.
    pub expires_at: DateTime<Utc>,
    /// Failure details, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<OrderError>,
}

/// `POST /v1/orders/{orderId}/checkout` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutResponse {
    /// Order id.
    pub order_id: OrderId,
    /// Always `awaiting_payment`.
    pub status: OrderStatus,
    /// payOS order code.
    pub order_code: i64,
    /// payOS checkout page.
    pub checkout_url: String,
    /// VietQR payload.
    pub qr_code: String,
}

/// Inner `data` object of a payOS webhook. Unlisted fields are kept in `extra` because the
/// signature covers all keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayosWebhookData {
    /// payOS order code.
    pub order_code: i64,
    /// Paid amount in VND.
    pub amount: u64,
    /// Remaining payOS fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// `POST /v1/payments/payos/webhook` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayosWebhook {
    /// `"00"` means paid.
    pub code: String,
    /// Description.
    pub desc: String,
    /// Success flag.
    pub success: bool,
    /// Payment data.
    pub data: PayosWebhookData,
    /// HMAC-SHA256 hex over sorted data.
    pub signature: String,
}

/// `POST /v1/payments/payos/webhook` response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookAck {
    /// Always true on success.
    pub success: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::tests::pid;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().expect("valid timestamp")
    }

    /// Serializes to exactly `wire` and parses back to an equal value.
    fn check<T>(value: &T, wire: Value)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        assert_eq!(serde_json::to_value(value).expect("serialize"), wire);
        assert_eq!(
            &serde_json::from_value::<T>(wire).expect("deserialize"),
            value
        );
    }

    #[test]
    fn media_and_content_type() {
        check(&Media::Photo4x6, json!("4x6"));
        check(&ContentType::Jpeg, json!("image/jpeg"));
        check(&ContentType::Png, json!("image/png"));
        assert!(serde_json::from_value::<ContentType>(json!("image/gif")).is_err());
    }

    #[test]
    fn printer_status_variants() {
        check(&PrinterStatus::Online, json!("online"));
        check(&PrinterStatus::Offline, json!("offline"));
        check(&PrinterStatus::Busy, json!("busy"));
        check(&PrinterStatus::Error, json!("error"));
    }

    #[test]
    fn order_status_variants() {
        let all = [
            (OrderStatus::AwaitingUpload, "awaiting_upload"),
            (OrderStatus::AwaitingPayment, "awaiting_payment"),
            (OrderStatus::Paid, "paid"),
            (OrderStatus::Printing, "printing"),
            (OrderStatus::Completed, "completed"),
            (OrderStatus::Failed, "failed"),
            (OrderStatus::Cancelled, "cancelled"),
            (OrderStatus::Expired, "expired"),
        ];
        for (status, wire) in all {
            check(&status, json!(wire));
        }
        assert!(serde_json::from_value::<OrderStatus>(json!("done")).is_err());
    }

    #[test]
    fn error_code_variants() {
        let all = [
            (ErrorCode::NotFound, "not_found"),
            (ErrorCode::Validation, "validation"),
            (ErrorCode::Forbidden, "forbidden"),
            (ErrorCode::Conflict, "conflict"),
            (ErrorCode::PaymentProvider, "payment_provider"),
            (ErrorCode::Internal, "internal"),
            (ErrorCode::Unknown, "unknown"),
        ];
        for (code, wire) in all {
            check(&code, json!(wire));
        }
    }

    #[test]
    fn unknown_error_code_falls_back() {
        assert_eq!(
            serde_json::from_value::<ErrorCode>(json!("rate_limited")).expect("deserialize"),
            ErrorCode::Unknown
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::Unknown).expect("serialize"),
            json!("unknown")
        );
    }

    #[test]
    fn error_body() {
        let body = ErrorBody::new(ErrorCode::NotFound, "no such order");
        check(
            &body,
            json!({"error": {"code": "not_found", "message": "no such order"}}),
        );
    }

    #[test]
    fn health_and_ack() {
        check(&Health { ok: true }, json!({"ok": true}));
        check(&WebhookAck { success: true }, json!({"success": true}));
    }

    #[test]
    fn printer_info() {
        let info = PrinterInfo {
            printer_id: pid("stand-001"),
            name: "Stand 1".into(),
            status: PrinterStatus::Online,
            price_per_print_vnd: 10_000,
            media: vec![Media::Photo4x6],
        };
        check(
            &info,
            json!({"printerId": "stand-001", "name": "Stand 1", "status": "online",
                   "pricePerPrintVnd": 10000, "media": ["4x6"]}),
        );
    }

    #[test]
    fn create_order_request() {
        let req = CreateOrderRequest {
            printer_id: pid("stand-001"),
            photos: vec![PhotoSpec {
                content_type: ContentType::Jpeg,
                size_bytes: 1234,
                copies: 2,
            }],
        };
        check(
            &req,
            json!({"printerId": "stand-001",
                   "photos": [{"contentType": "image/jpeg", "sizeBytes": 1234, "copies": 2}]}),
        );
    }

    #[test]
    fn create_order_response() {
        let res = CreateOrderResponse {
            order_id: "o1".into(),
            status: OrderStatus::AwaitingUpload,
            amount_vnd: 20_000,
            expires_at: ts("2026-01-01T00:15:00Z"),
            uploads: vec![UploadTarget {
                photo_id: "p1".into(),
                url: "https://s3.example/x".into(),
                method: UploadMethod::Put,
                headers: BTreeMap::from([("content-type".to_owned(), "image/jpeg".to_owned())]),
                expires_at: ts("2026-01-01T00:15:00Z"),
            }],
        };
        check(
            &res,
            json!({"orderId": "o1", "status": "awaiting_upload", "amountVnd": 20000,
                   "expiresAt": "2026-01-01T00:15:00Z",
                   "uploads": [{"photoId": "p1", "url": "https://s3.example/x", "method": "PUT",
                                "headers": {"content-type": "image/jpeg"},
                                "expiresAt": "2026-01-01T00:15:00Z"}]}),
        );
    }

    #[test]
    fn upload_method_rejects_other_verbs() {
        assert!(serde_json::from_value::<UploadMethod>(json!("POST")).is_err());
    }

    fn sample_order() -> Order {
        Order {
            order_id: "o1".into(),
            printer_id: pid("stand-001"),
            status: OrderStatus::Paid,
            amount_vnd: 10_000,
            order_code: Some(123),
            photos: vec![OrderPhoto {
                photo_id: "p1".into(),
                copies: 1,
                content_type: ContentType::Png,
                uploaded: true,
            }],
            created_at: ts("2026-01-01T00:00:00Z"),
            updated_at: ts("2026-01-01T00:01:00Z"),
            expires_at: ts("2026-01-01T00:15:00Z"),
            error: None,
        }
    }

    #[test]
    fn order_with_code_without_error() {
        check(
            &sample_order(),
            json!({"orderId": "o1", "printerId": "stand-001", "status": "paid",
                   "amountVnd": 10000, "orderCode": 123,
                   "photos": [{"photoId": "p1", "copies": 1, "contentType": "image/png",
                               "uploaded": true}],
                   "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:01:00Z",
                   "expiresAt": "2026-01-01T00:15:00Z"}),
        );
    }

    #[test]
    fn order_without_code_with_error() {
        let order = Order {
            order_code: None,
            status: OrderStatus::Failed,
            error: Some(OrderError {
                code: "paper_out".into(),
                message: "no paper".into(),
            }),
            ..sample_order()
        };
        let wire = serde_json::to_value(&order).expect("serialize");
        assert!(wire.get("orderCode").is_none());
        assert_eq!(
            wire["error"],
            json!({"code": "paper_out", "message": "no paper"})
        );
        assert_eq!(
            serde_json::from_value::<Order>(wire).expect("deserialize"),
            order
        );
    }

    #[test]
    fn checkout_response() {
        let res = CheckoutResponse {
            order_id: "o1".into(),
            status: OrderStatus::AwaitingPayment,
            order_code: 42,
            checkout_url: "https://pay.payos.vn/web/abc".into(),
            qr_code: "000201".into(),
        };
        check(
            &res,
            json!({"orderId": "o1", "status": "awaiting_payment", "orderCode": 42,
                   "checkoutUrl": "https://pay.payos.vn/web/abc", "qrCode": "000201"}),
        );
    }

    #[test]
    fn payos_webhook_keeps_extra_fields() {
        let hook = PayosWebhook {
            code: "00".into(),
            desc: "success".into(),
            success: true,
            data: PayosWebhookData {
                order_code: 42,
                amount: 10_000,
                extra: BTreeMap::from([
                    ("currency".to_owned(), json!("VND")),
                    ("paymentLinkId".to_owned(), json!("abc")),
                ]),
            },
            signature: "deadbeef".into(),
        };
        check(
            &hook,
            json!({"code": "00", "desc": "success", "success": true,
                   "data": {"orderCode": 42, "amount": 10000, "currency": "VND",
                            "paymentLinkId": "abc"},
                   "signature": "deadbeef"}),
        );
    }
}
