//! Domain types: orders, photos and printers, plus pricing, validation and expiry rules.

use std::time::Duration;

use chrono::{DateTime, Utc};
use fotoko_protocol::{
    COPIES_RANGE, ContentType, MAX_PHOTO_BYTES, MAX_PHOTOS_PER_ORDER, Media, OrderError, OrderId,
    OrderPhoto, OrderStatus, PhotoId, PhotoSpec, PrinterId, PrinterInfo, PrinterStatus,
};

use crate::error::CoreError;
use crate::state::{OrderEvent, transition};

/// One image of an order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Photo {
    /// Id within the order.
    pub photo_id: PhotoId,
    /// Image format.
    pub content_type: ContentType,
    /// Declared size in bytes.
    pub size_bytes: u64,
    /// Copies to print.
    pub copies: u32,
    /// Set at checkout, once the object is known to exist in the store.
    pub uploaded: bool,
}

impl Photo {
    /// S3 key `orders/{orderId}/{photoId}.{jpg|png}`.
    pub fn object_key(&self, order_id: &OrderId) -> String {
        let ext = match self.content_type {
            ContentType::Jpeg => "jpg",
            ContentType::Png => "png",
        };
        format!("orders/{order_id}/{}.{ext}", self.photo_id)
    }
}

/// One customer purchase at one Stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    /// Order id.
    pub order_id: OrderId,
    /// Guest identity (Cognito identity id) that created the order.
    pub owner: String,
    /// The Stand's printer.
    pub printer_id: PrinterId,
    /// Current status.
    pub status: OrderStatus,
    /// `sum(copies) * pricePerPrintVnd`.
    pub amount_vnd: u64,
    /// payOS order code, set at checkout.
    pub order_code: Option<i64>,
    /// payOS checkout page, set at checkout.
    pub checkout_url: Option<String>,
    /// payOS QR code payload, set at checkout.
    pub qr_code: Option<String>,
    /// The images.
    pub photos: Vec<Photo>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last change.
    pub updated_at: DateTime<Utc>,
    /// Unpaid orders expire at this time.
    pub expires_at: DateTime<Utc>,
    /// Why printing failed.
    pub error: Option<OrderError>,
}

impl Order {
    /// Whether the order is unpaid and past `expires_at` (strictly after) at `now`.
    pub fn is_due_for_expiry(&self, now: DateTime<Utc>) -> bool {
        matches!(
            self.status,
            OrderStatus::AwaitingUpload | OrderStatus::AwaitingPayment
        ) && now > self.expires_at
    }

    /// A copy after `event` happened at `now`, or `Conflict` when the status machine forbids it.
    pub fn advance(&self, event: OrderEvent, now: DateTime<Utc>) -> Result<Self, CoreError> {
        let status = transition(self.status, event)?;
        Ok(Self {
            status,
            updated_at: now,
            ..self.clone()
        })
    }

    /// The HTTP `Order` body. `uploaded` per photo comes from the photo flags.
    pub fn to_dto(&self) -> fotoko_protocol::Order {
        fotoko_protocol::Order {
            order_id: self.order_id.clone(),
            printer_id: self.printer_id.clone(),
            status: self.status,
            amount_vnd: self.amount_vnd,
            order_code: self.order_code,
            photos: self
                .photos
                .iter()
                .map(|p| OrderPhoto {
                    photo_id: p.photo_id.clone(),
                    copies: p.copies,
                    content_type: p.content_type,
                    uploaded: p.uploaded,
                })
                .collect(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            expires_at: self.expires_at,
            error: self.error.clone(),
        }
    }
}

/// A Stand's printer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Printer {
    /// Printer (IoT Thing) id.
    pub printer_id: PrinterId,
    /// Display name.
    pub name: String,
    /// Price of one 4x6 print in VND.
    pub price_per_print_vnd: u64,
    /// Last reported status.
    pub status: PrinterStatus,
    /// Time of the last heartbeat.
    pub last_seen_at: Option<DateTime<Utc>>,
}

impl Printer {
    /// The HTTP `GET /v1/printers/{id}` body.
    pub fn to_info(&self) -> PrinterInfo {
        PrinterInfo {
            printer_id: self.printer_id.clone(),
            name: self.name.clone(),
            status: self.status,
            price_per_print_vnd: self.price_per_print_vnd,
            media: vec![Media::Photo4x6],
        }
    }

    /// `Validation` unless the printer is online.
    pub fn require_online(&self) -> Result<(), CoreError> {
        if self.status == PrinterStatus::Online {
            Ok(())
        } else {
            Err(CoreError::Validation(format!(
                "printer {} is not online",
                self.printer_id
            )))
        }
    }
}

/// Check photo count, copies and sizes against the contract limits.
pub fn validate_photos(photos: &[PhotoSpec]) -> Result<(), CoreError> {
    if photos.is_empty() {
        return Err(CoreError::Validation("order needs at least 1 photo".into()));
    }
    if photos.len() > MAX_PHOTOS_PER_ORDER {
        return Err(CoreError::Validation(format!(
            "order has {} photos, maximum is {MAX_PHOTOS_PER_ORDER}",
            photos.len()
        )));
    }
    for (i, photo) in photos.iter().enumerate() {
        if !COPIES_RANGE.contains(&photo.copies) {
            return Err(CoreError::Validation(format!(
                "photo {i}: copies must be {}..={}",
                COPIES_RANGE.start(),
                COPIES_RANGE.end()
            )));
        }
        if photo.size_bytes == 0 || photo.size_bytes > MAX_PHOTO_BYTES {
            return Err(CoreError::Validation(format!(
                "photo {i}: sizeBytes must be 1..={MAX_PHOTO_BYTES}"
            )));
        }
    }
    Ok(())
}

/// `amountVnd = sum(copies) * pricePerPrintVnd`. `Validation` on arithmetic overflow.
pub fn price(photos: &[PhotoSpec], price_per_print_vnd: u64) -> Result<u64, CoreError> {
    let copies: u64 = photos.iter().map(|p| u64::from(p.copies)).sum();
    copies
        .checked_mul(price_per_print_vnd)
        .ok_or_else(|| CoreError::Validation("order amount overflows".into()))
}

/// `created_at + ttl`. `Internal` when the TTL does not fit a calendar duration.
pub fn expiry(created_at: DateTime<Utc>, ttl: Duration) -> Result<DateTime<Utc>, CoreError> {
    chrono::Duration::from_std(ttl)
        .ok()
        .and_then(|d| created_at.checked_add_signed(d))
        .ok_or_else(|| CoreError::Internal("order expiry out of range".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fotoko_protocol::ORDER_TTL;

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().expect("valid timestamp")
    }

    fn spec(copies: u32, size_bytes: u64) -> PhotoSpec {
        PhotoSpec {
            content_type: ContentType::Jpeg,
            size_bytes,
            copies,
        }
    }

    fn photo(content_type: ContentType) -> Photo {
        Photo {
            photo_id: "p1".into(),
            content_type,
            size_bytes: 10,
            copies: 2,
            uploaded: false,
        }
    }

    pub(crate) fn order(status: OrderStatus) -> Order {
        Order {
            order_id: "o1".into(),
            owner: "me".into(),
            printer_id: PrinterId::new("stand-001").expect("id"),
            status,
            amount_vnd: 20_000,
            order_code: Some(7),
            checkout_url: None,
            qr_code: None,
            photos: vec![photo(ContentType::Png)],
            created_at: ts("2026-01-01T00:00:00Z"),
            updated_at: ts("2026-01-01T00:00:00Z"),
            expires_at: ts("2026-01-01T00:15:00Z"),
            error: None,
        }
    }

    #[test]
    fn object_key_uses_extension_per_content_type() {
        let id = OrderId::from("o1");
        assert_eq!(photo(ContentType::Jpeg).object_key(&id), "orders/o1/p1.jpg");
        assert_eq!(photo(ContentType::Png).object_key(&id), "orders/o1/p1.png");
    }

    #[test]
    fn validate_accepts_boundaries() {
        assert_eq!(validate_photos(&[spec(1, 1)]), Ok(()));
        assert_eq!(validate_photos(&[spec(5, MAX_PHOTO_BYTES)]), Ok(()));
        let ten = vec![spec(1, 100); 10];
        assert_eq!(validate_photos(&ten), Ok(()));
    }

    #[test]
    fn validate_rejects_photo_count() {
        let err = validate_photos(&[]).expect_err("empty");
        assert_eq!(err.message(), "order needs at least 1 photo");
        let eleven = vec![spec(1, 100); 11];
        let err = validate_photos(&eleven).expect_err("too many");
        assert_eq!(err.message(), "order has 11 photos, maximum is 10");
    }

    #[test]
    fn validate_rejects_copies_out_of_range() {
        for copies in [0, 6] {
            let err = validate_photos(&[spec(copies, 100)]).expect_err("copies");
            assert!(matches!(err, CoreError::Validation(_)));
            assert_eq!(err.message(), "photo 0: copies must be 1..=5");
        }
    }

    #[test]
    fn validate_rejects_size_out_of_range() {
        for size in [0, MAX_PHOTO_BYTES + 1] {
            let err = validate_photos(&[spec(1, 100), spec(1, size)]).expect_err("size");
            assert!(matches!(err, CoreError::Validation(_)));
            assert_eq!(err.message(), "photo 1: sizeBytes must be 1..=15728640");
        }
    }

    #[test]
    fn price_sums_copies_times_unit_price() {
        assert_eq!(price(&[spec(2, 1), spec(3, 1)], 10_000), Ok(50_000));
        assert_eq!(price(&[spec(1, 1)], 0), Ok(0));
    }

    #[test]
    fn price_overflow_is_validation_error() {
        let err = price(&[spec(2, 1)], u64::MAX).expect_err("overflow");
        assert!(matches!(err, CoreError::Validation(_)));
    }

    #[test]
    fn expiry_adds_ttl() {
        assert_eq!(
            expiry(ts("2026-01-01T00:00:00Z"), ORDER_TTL),
            Ok(ts("2026-01-01T00:15:00Z"))
        );
    }

    #[test]
    fn expiry_out_of_range_is_internal() {
        let err = expiry(ts("2026-01-01T00:00:00Z"), Duration::MAX).expect_err("too large");
        assert!(matches!(err, CoreError::Internal(_)));
        // Representable duration, but the sum leaves the calendar range.
        let big = Duration::from_secs(u64::from(u32::MAX) * 100_000);
        let err = expiry(ts("2026-01-01T00:00:00Z"), big).expect_err("overflow");
        assert!(matches!(err, CoreError::Internal(_)));
    }

    #[test]
    fn due_for_expiry_only_when_unpaid_and_strictly_late() {
        let at_limit = ts("2026-01-01T00:15:00Z");
        let late = ts("2026-01-01T00:15:01Z");
        for status in [OrderStatus::AwaitingUpload, OrderStatus::AwaitingPayment] {
            assert!(!order(status).is_due_for_expiry(at_limit));
            assert!(order(status).is_due_for_expiry(late));
        }
        for status in [
            OrderStatus::Paid,
            OrderStatus::Printing,
            OrderStatus::Completed,
            OrderStatus::Failed,
            OrderStatus::Cancelled,
            OrderStatus::Expired,
        ] {
            assert!(!order(status).is_due_for_expiry(late), "{status:?}");
        }
    }

    #[test]
    fn advance_sets_status_and_timestamp() {
        let now = ts("2026-01-01T00:05:00Z");
        let next = order(OrderStatus::AwaitingUpload)
            .advance(OrderEvent::Checkout, now)
            .expect("allowed");
        assert_eq!(next.status, OrderStatus::AwaitingPayment);
        assert_eq!(next.updated_at, now);
        assert_eq!(next.created_at, ts("2026-01-01T00:00:00Z"));
    }

    #[test]
    fn advance_rejects_forbidden_event() {
        let err = order(OrderStatus::Paid)
            .advance(OrderEvent::Cancel, ts("2026-01-01T00:05:00Z"))
            .expect_err("rejected");
        assert!(matches!(err, CoreError::Conflict(_)));
    }

    #[test]
    fn order_dto_matches_contract() {
        let mut o = order(OrderStatus::Paid);
        o.photos[0].uploaded = true;
        let wire = serde_json::to_value(o.to_dto()).expect("serialize");
        assert_eq!(
            wire,
            serde_json::json!({
                "orderId": "o1", "printerId": "stand-001", "status": "paid",
                "amountVnd": 20000, "orderCode": 7,
                "photos": [{"photoId": "p1", "copies": 2, "contentType": "image/png",
                            "uploaded": true}],
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
                "expiresAt": "2026-01-01T00:15:00Z"
            })
        );
    }

    fn printer(status: PrinterStatus) -> Printer {
        Printer {
            printer_id: PrinterId::new("stand-001").expect("id"),
            name: "Stand 1".into(),
            price_per_print_vnd: 10_000,
            status,
            last_seen_at: None,
        }
    }

    #[test]
    fn printer_info_and_online_check() {
        let info = printer(PrinterStatus::Online).to_info();
        assert_eq!(info.media, vec![Media::Photo4x6]);
        assert_eq!(info.price_per_print_vnd, 10_000);
        assert_eq!(printer(PrinterStatus::Online).require_online(), Ok(()));
        for status in [
            PrinterStatus::Offline,
            PrinterStatus::Busy,
            PrinterStatus::Error,
        ] {
            let err = printer(status).require_online().expect_err("not online");
            assert!(matches!(err, CoreError::Validation(_)));
        }
    }
}
