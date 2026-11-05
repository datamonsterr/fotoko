//! DynamoDB adapters (`OrderRepo`, `PrinterRepo`) over the single table of `docs/contracts.md`.
//!
//! | item | pk | sk |
//! | --- | --- | --- |
//! | printer | `PRINTER#{printerId}` | `META` |
//! | order | `ORDER#{orderId}` | `META` |
//! | order-code reservation | `ORDERCODE#{orderCode}` | `META` (attribute `orderId`) |
//!
//! Order codes are unique because every write that sets one goes through a
//! `TransactWriteItems` that also puts the reservation item, guarded by `attribute_not_exists`
//! (or by "already reserved for this very order" for later updates). `get_by_order_code` reads
//! the reservation with a consistent read, so a webhook right after checkout cannot miss it,
//! which an eventually consistent GSI query could.
//!
//! Orders carry `gsi1pk = ORDERCODE#{code}` as the contract says, and `gsi2pk = "DUE"`,
//! `gsi2sk = expiresAt` while they are `awaiting_*`; `list_due_for_expiry` queries `gsi2`
//! (projection ALL) instead of scanning. Writes replace the whole item, so leaving the awaiting
//! states drops the `gsi2` keys by itself. Timestamps are fixed-width RFC 3339 with nanoseconds
//! so that string order is time order.

use std::collections::HashMap;

use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::put_item::PutItemError;
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use aws_sdk_dynamodb::operation::update_item::UpdateItemError;
use aws_sdk_dynamodb::types::{AttributeValue, CancellationReason, Put, TransactWriteItem};
use aws_smithy_types::error::display::DisplayErrorContext;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use fotoko_core::{CoreError, Order, OrderRepo, Photo, Printer, PrinterRepo};
use fotoko_protocol::{
    ContentType, OrderError, OrderId, OrderStatus, PhotoId, PrinterId, PrinterStatus,
};

type Item = HashMap<String, AttributeValue>;

/// Name of the index on `gsi2pk`/`gsi2sk` that [`DynamoOrderRepo`] queries for due orders.
pub const GSI_DUE: &str = "gsi2";
/// `gsi2pk` of every order that can still expire.
pub const DUE_PARTITION: &str = "DUE";
/// Items are deleted by DynamoDB TTL this long after the order's expiry.
const RETENTION: Duration = Duration::days(7);
const COND_FAILED: &str = "ConditionalCheckFailed";

fn s(value: impl Into<String>) -> AttributeValue {
    AttributeValue::S(value.into())
}

fn n(value: impl ToString) -> AttributeValue {
    AttributeValue::N(value.to_string())
}

fn ts(value: DateTime<Utc>) -> AttributeValue {
    s(value.to_rfc3339_opts(SecondsFormat::Nanos, true))
}

fn order_pk(order_id: &OrderId) -> String {
    format!("ORDER#{order_id}")
}

fn code_pk(order_code: i64) -> String {
    format!("ORDERCODE#{order_code}")
}

fn printer_pk(printer_id: &PrinterId) -> String {
    format!("PRINTER#{printer_id}")
}

fn key(pk: String) -> Item {
    Item::from([("pk".to_owned(), s(pk)), ("sk".to_owned(), s("META"))])
}

fn status_text<T: serde::Serialize>(status: T) -> Result<String, CoreError> {
    match serde_json::to_value(status) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err(CoreError::Internal("status does not serialize".into())),
    }
}

fn status_from<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, CoreError> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|_| CoreError::Internal(format!("stored status `{text}` is unknown")))
}

fn content_type_text(content_type: ContentType) -> &'static str {
    match content_type {
        ContentType::Jpeg => "image/jpeg",
        ContentType::Png => "image/png",
    }
}

fn corrupt(what: &str) -> CoreError {
    CoreError::Internal(format!("stored item is malformed: {what}"))
}

fn get_s<'a>(item: &'a Item, name: &str) -> Result<&'a str, CoreError> {
    item.get(name)
        .and_then(|v| v.as_s().ok())
        .map(String::as_str)
        .ok_or_else(|| corrupt(name))
}

fn opt_s(item: &Item, name: &str) -> Option<String> {
    item.get(name).and_then(|v| v.as_s().ok()).cloned()
}

fn get_n<T: std::str::FromStr>(item: &Item, name: &str) -> Result<T, CoreError> {
    item.get(name)
        .and_then(|v| v.as_n().ok())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| corrupt(name))
}

fn get_ts(item: &Item, name: &str) -> Result<DateTime<Utc>, CoreError> {
    DateTime::parse_from_rfc3339(get_s(item, name)?)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| corrupt(name))
}

/// The DynamoDB item of an order. `ttl` is epoch seconds.
pub fn order_to_item(order: &Order) -> Result<Item, CoreError> {
    let mut item = key(order_pk(&order.order_id));
    let photos: Vec<AttributeValue> = order
        .photos
        .iter()
        .map(|p| {
            AttributeValue::M(Item::from([
                ("photoId".to_owned(), s(p.photo_id.as_str())),
                (
                    "contentType".to_owned(),
                    s(content_type_text(p.content_type)),
                ),
                ("sizeBytes".to_owned(), n(p.size_bytes)),
                ("copies".to_owned(), n(p.copies)),
                ("uploaded".to_owned(), AttributeValue::Bool(p.uploaded)),
            ]))
        })
        .collect();
    let status = status_text(order.status)?;
    item.extend([
        ("orderId".to_owned(), s(order.order_id.as_str())),
        ("printerId".to_owned(), s(order.printer_id.as_str())),
        ("owner".to_owned(), s(&order.owner)),
        ("status".to_owned(), s(&status)),
        ("amountVnd".to_owned(), n(order.amount_vnd)),
        ("photos".to_owned(), AttributeValue::L(photos)),
        ("createdAt".to_owned(), ts(order.created_at)),
        ("updatedAt".to_owned(), ts(order.updated_at)),
        ("expiresAt".to_owned(), ts(order.expires_at)),
        (
            "ttl".to_owned(),
            n((order.expires_at + RETENTION).timestamp()),
        ),
    ]);
    if let Some(code) = order.order_code {
        item.insert("orderCode".to_owned(), n(code));
        item.insert("gsi1pk".to_owned(), s(code_pk(code)));
    }
    if let Some(url) = &order.checkout_url {
        item.insert("checkoutUrl".to_owned(), s(url));
    }
    if let Some(qr) = &order.qr_code {
        item.insert("qrCode".to_owned(), s(qr));
    }
    if let Some(error) = &order.error {
        item.insert(
            "error".to_owned(),
            AttributeValue::M(Item::from([
                ("code".to_owned(), s(&error.code)),
                ("message".to_owned(), s(&error.message)),
            ])),
        );
    }
    if matches!(
        order.status,
        OrderStatus::AwaitingUpload | OrderStatus::AwaitingPayment
    ) {
        item.insert("gsi2pk".to_owned(), s(DUE_PARTITION));
        item.insert("gsi2sk".to_owned(), ts(order.expires_at));
    }
    Ok(item)
}

/// The order stored in `item`.
pub fn order_from_item(item: &Item) -> Result<Order, CoreError> {
    let photos = item
        .get("photos")
        .and_then(|v| v.as_l().ok())
        .ok_or_else(|| corrupt("photos"))?
        .iter()
        .map(|value| {
            let photo = value.as_m().map_err(|_| corrupt("photos"))?;
            Ok(Photo {
                photo_id: PhotoId::from(get_s(photo, "photoId")?),
                content_type: match get_s(photo, "contentType")? {
                    "image/jpeg" => ContentType::Jpeg,
                    "image/png" => ContentType::Png,
                    _ => return Err(corrupt("photos.contentType")),
                },
                size_bytes: get_n(photo, "sizeBytes")?,
                copies: get_n(photo, "copies")?,
                uploaded: photo
                    .get("uploaded")
                    .and_then(|v| v.as_bool().ok())
                    .copied()
                    .ok_or_else(|| corrupt("photos.uploaded"))?,
            })
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    let error = match item.get("error") {
        None => None,
        Some(value) => {
            let m = value.as_m().map_err(|_| corrupt("error"))?;
            Some(OrderError {
                code: get_s(m, "code")?.to_owned(),
                message: get_s(m, "message")?.to_owned(),
            })
        }
    };
    Ok(Order {
        order_id: OrderId::from(get_s(item, "orderId")?),
        owner: get_s(item, "owner")?.to_owned(),
        printer_id: PrinterId::new(get_s(item, "printerId")?).map_err(|_| corrupt("printerId"))?,
        status: status_from(get_s(item, "status")?)?,
        amount_vnd: get_n(item, "amountVnd")?,
        order_code: if item.contains_key("orderCode") {
            Some(get_n(item, "orderCode")?)
        } else {
            None
        },
        checkout_url: opt_s(item, "checkoutUrl"),
        qr_code: opt_s(item, "qrCode"),
        photos,
        created_at: get_ts(item, "createdAt")?,
        updated_at: get_ts(item, "updatedAt")?,
        expires_at: get_ts(item, "expiresAt")?,
        error,
    })
}

/// The printer stored in `item`; `default_price_vnd` fills a missing price.
pub fn printer_from_item(item: &Item, default_price_vnd: u64) -> Result<Printer, CoreError> {
    let printer_id = get_s(item, "pk")?
        .strip_prefix("PRINTER#")
        .ok_or_else(|| corrupt("pk"))?;
    Ok(Printer {
        printer_id: PrinterId::new(printer_id).map_err(|_| corrupt("pk"))?,
        name: opt_s(item, "name").unwrap_or_else(|| printer_id.to_owned()),
        price_per_print_vnd: if item.contains_key("pricePerPrintVnd") {
            get_n(item, "pricePerPrintVnd")?
        } else {
            default_price_vnd
        },
        status: status_from::<PrinterStatus>(get_s(item, "status")?)?,
        last_seen_at: if item.contains_key("lastSeenAt") {
            Some(get_ts(item, "lastSeenAt")?)
        } else {
            None
        },
    })
}

fn storage_error<E, R>(op: &str, error: &SdkError<E, R>) -> CoreError
where
    E: std::error::Error + 'static,
    R: std::fmt::Debug,
{
    tracing::error!(op, error = %DisplayErrorContext(error), "DynamoDB call failed");
    CoreError::Internal(format!("storage error during {op}"))
}

fn build_error(error: &aws_sdk_dynamodb::error::BuildError) -> CoreError {
    CoreError::Internal(format!("cannot build a DynamoDB request: {error}"))
}

fn reason_failed(reason: Option<&CancellationReason>) -> bool {
    reason.and_then(CancellationReason::code) == Some(COND_FAILED)
}

/// DynamoDB-backed [`OrderRepo`].
#[derive(Debug, Clone)]
pub struct DynamoOrderRepo {
    client: Client,
    table: String,
}

impl DynamoOrderRepo {
    /// Repo on `table`.
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    async fn get_item(&self, pk: String, op: &str) -> Result<Option<Item>, CoreError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(pk)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|e| storage_error(op, &e))?;
        Ok(out.item)
    }

    fn put(&self, item: Item, condition: &str, values: Item) -> Result<Put, CoreError> {
        let names = condition
            .contains("#st")
            .then(|| ("#st".to_owned(), "status".to_owned()));
        let mut builder = Put::builder()
            .table_name(&self.table)
            .set_item(Some(item))
            .condition_expression(condition);
        if let Some((k, v)) = names {
            builder = builder.expression_attribute_names(k, v);
        }
        if !values.is_empty() {
            builder = builder.set_expression_attribute_values(Some(values));
        }
        builder.build().map_err(|e| build_error(&e))
    }

    fn reservation(&self, order: &Order, code: i64, exclusive: bool) -> Result<Put, CoreError> {
        let mut item = key(code_pk(code));
        item.insert("orderId".to_owned(), s(order.order_id.as_str()));
        item.insert(
            "ttl".to_owned(),
            n((order.expires_at + RETENTION).timestamp()),
        );
        if exclusive {
            self.put(item, "attribute_not_exists(pk)", Item::new())
        } else {
            self.put(
                item,
                "attribute_not_exists(pk) OR orderId = :oid",
                Item::from([(":oid".to_owned(), s(order.order_id.as_str()))]),
            )
        }
    }

    async fn transact(
        &self,
        writes: Vec<TransactWriteItem>,
        order: &Order,
        missing: MissingOrder,
    ) -> Result<(), CoreError> {
        let result = self
            .client
            .transact_write_items()
            .set_transact_items(Some(writes))
            .send()
            .await;
        let error = match result {
            Ok(_) => return Ok(()),
            Err(error) => error,
        };
        let reasons = match &error {
            SdkError::ServiceError(se) => match se.err() {
                TransactWriteItemsError::TransactionCanceledException(e) => {
                    Some(e.cancellation_reasons())
                }
                _ => None,
            },
            _ => None,
        };
        let Some(reasons) = reasons else {
            return Err(storage_error("transact_write_items", &error));
        };
        if reason_failed(reasons.first()) {
            return self.order_condition_failed(order, missing).await;
        }
        if reason_failed(reasons.get(1)) {
            return Err(CoreError::Conflict(format!(
                "order code {} is already taken",
                order.order_code.unwrap_or_default()
            )));
        }
        let codes: Vec<&str> = reasons
            .iter()
            .filter_map(CancellationReason::code)
            .collect();
        if codes.contains(&"TransactionConflict") {
            return Err(CoreError::Conflict(format!(
                "order {} changed concurrently",
                order.order_id
            )));
        }
        tracing::error!(?codes, "DynamoDB transaction cancelled");
        Err(CoreError::Internal("storage transaction failed".into()))
    }

    /// The order write's condition failed: the order is gone, taken, or in another status.
    async fn order_condition_failed(
        &self,
        order: &Order,
        missing: MissingOrder,
    ) -> Result<(), CoreError> {
        match missing {
            MissingOrder::Exists => Err(CoreError::Conflict(format!(
                "order {} already exists",
                order.order_id
            ))),
            MissingOrder::NotFound => {
                if self
                    .get_item(order_pk(&order.order_id), "get_item")
                    .await?
                    .is_none()
                {
                    Err(CoreError::NotFound(format!(
                        "order {} not found",
                        order.order_id
                    )))
                } else {
                    Err(CoreError::Conflict(format!(
                        "order {} changed concurrently",
                        order.order_id
                    )))
                }
            }
        }
    }

    /// Run `order_write` alone, or in one transaction with the order-code reservation.
    async fn write(
        &self,
        order: &Order,
        order_write: Put,
        exclusive_code: bool,
        missing: MissingOrder,
    ) -> Result<(), CoreError> {
        match order.order_code {
            Some(code) => {
                let reservation = self.reservation(order, code, exclusive_code)?;
                let writes = [order_write, reservation]
                    .into_iter()
                    .map(|put| TransactWriteItem::builder().put(put).build())
                    .collect();
                self.transact(writes, order, missing).await
            }
            None => {
                let put = order_write;
                let result = self
                    .client
                    .put_item()
                    .table_name(put.table_name)
                    .set_item(Some(put.item))
                    .set_condition_expression(Some(put.condition_expression.unwrap_or_default()))
                    .set_expression_attribute_names(put.expression_attribute_names)
                    .set_expression_attribute_values(put.expression_attribute_values)
                    .send()
                    .await;
                match result {
                    Ok(_) => Ok(()),
                    Err(SdkError::ServiceError(se))
                        if matches!(se.err(), PutItemError::ConditionalCheckFailedException(_)) =>
                    {
                        self.order_condition_failed(order, missing).await
                    }
                    Err(error) => Err(storage_error("put_item", &error)),
                }
            }
        }
    }
}

/// What a failed order condition means.
#[derive(Debug, Clone, Copy)]
enum MissingOrder {
    /// Insert: the order id is taken.
    Exists,
    /// Update: look the order up to tell "gone" from "other status".
    NotFound,
}

#[async_trait]
impl OrderRepo for DynamoOrderRepo {
    async fn get(&self, order_id: &OrderId) -> Result<Option<Order>, CoreError> {
        self.get_item(order_pk(order_id), "get_item")
            .await?
            .map(|item| order_from_item(&item))
            .transpose()
    }

    async fn get_by_order_code(&self, order_code: i64) -> Result<Option<Order>, CoreError> {
        let Some(reservation) = self.get_item(code_pk(order_code), "get_item").await? else {
            return Ok(None);
        };
        let order_id = OrderId::from(get_s(&reservation, "orderId")?);
        self.get(&order_id).await
    }

    async fn insert(&self, order: &Order) -> Result<(), CoreError> {
        let item = order_to_item(order)?;
        let put = self.put(item, "attribute_not_exists(pk)", Item::new())?;
        self.write(order, put, true, MissingOrder::Exists).await
    }

    async fn update(&self, order: &Order, expected: OrderStatus) -> Result<(), CoreError> {
        let item = order_to_item(order)?;
        let put = self.put(
            item,
            "#st = :expected",
            Item::from([(":expected".to_owned(), s(status_text(expected)?))]),
        )?;
        self.write(order, put, false, MissingOrder::NotFound).await
    }

    async fn list_due_for_expiry(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<Order>, CoreError> {
        let limit = i32::try_from(limit).unwrap_or(i32::MAX).max(1);
        let out = self
            .client
            .query()
            .table_name(&self.table)
            .index_name(GSI_DUE)
            .key_condition_expression("gsi2pk = :due AND gsi2sk < :now")
            .expression_attribute_values(":due", s(DUE_PARTITION))
            .expression_attribute_values(":now", ts(now))
            .limit(limit)
            .send()
            .await
            .map_err(|e| storage_error("query", &e))?;
        out.items()
            .iter()
            .map(order_from_item)
            .collect::<Result<Vec<_>, _>>()
    }
}

/// DynamoDB-backed [`PrinterRepo`].
#[derive(Debug, Clone)]
pub struct DynamoPrinterRepo {
    client: Client,
    table: String,
    default_price_vnd: u64,
}

impl DynamoPrinterRepo {
    /// Repo on `table`; `default_price_vnd` is used for printer items without a price.
    pub fn new(client: Client, table: impl Into<String>, default_price_vnd: u64) -> Self {
        Self {
            client,
            table: table.into(),
            default_price_vnd,
        }
    }
}

#[async_trait]
impl PrinterRepo for DynamoPrinterRepo {
    async fn get(&self, printer_id: &PrinterId) -> Result<Option<Printer>, CoreError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(printer_pk(printer_id))))
            .send()
            .await
            .map_err(|e| storage_error("get_item", &e))?;
        out.item
            .map(|item| printer_from_item(&item, self.default_price_vnd))
            .transpose()
    }

    async fn update_status(
        &self,
        printer_id: &PrinterId,
        status: PrinterStatus,
        last_seen_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .set_key(Some(key(printer_pk(printer_id))))
            .condition_expression("attribute_exists(pk)")
            .update_expression("SET #st = :status, lastSeenAt = :seen")
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(":status", s(status_text(status)?))
            .expression_attribute_values(":seen", ts(last_seen_at))
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(se))
                if matches!(
                    se.err(),
                    UpdateItemError::ConditionalCheckFailedException(_)
                ) =>
            {
                Err(CoreError::NotFound(format!(
                    "printer {printer_id} not found"
                )))
            }
            Err(error) => Err(storage_error("update_item", &error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().expect("ts")
    }

    fn order(status: OrderStatus, code: Option<i64>) -> Order {
        Order {
            order_id: "ord-1".into(),
            owner: "guest".into(),
            printer_id: PrinterId::new("stand-001").expect("id"),
            status,
            amount_vnd: 30_000,
            order_code: code,
            checkout_url: code.map(|_| "https://pay.test/1".to_owned()),
            qr_code: code.map(|_| "qr".to_owned()),
            photos: vec![
                Photo {
                    photo_id: "p1".into(),
                    content_type: ContentType::Jpeg,
                    size_bytes: 1000,
                    copies: 2,
                    uploaded: true,
                },
                Photo {
                    photo_id: "p2".into(),
                    content_type: ContentType::Png,
                    size_bytes: 2000,
                    copies: 1,
                    uploaded: false,
                },
            ],
            created_at: at("2026-01-01T00:00:00.123456789Z"),
            updated_at: at("2026-01-01T00:00:01Z"),
            expires_at: at("2026-01-01T00:15:00Z"),
            error: None,
        }
    }

    #[test]
    fn order_round_trips_in_every_shape() {
        let mut failed = order(OrderStatus::Failed, Some(77));
        failed.error = Some(OrderError {
            code: "paper_out".into(),
            message: "no paper".into(),
        });
        for o in [
            order(OrderStatus::AwaitingUpload, None),
            order(OrderStatus::AwaitingPayment, Some(5)),
            order(OrderStatus::Paid, Some(9_007_199_254_740_991)),
            failed,
        ] {
            let item = order_to_item(&o).expect("encode");
            assert_eq!(order_from_item(&item).expect("decode"), o);
        }
    }

    #[test]
    fn order_item_layout() {
        let item = order_to_item(&order(OrderStatus::AwaitingPayment, Some(5))).expect("encode");
        assert_eq!(get_s(&item, "pk"), Ok("ORDER#ord-1"));
        assert_eq!(get_s(&item, "sk"), Ok("META"));
        assert_eq!(get_s(&item, "status"), Ok("awaiting_payment"));
        assert_eq!(get_s(&item, "gsi1pk"), Ok("ORDERCODE#5"));
        assert_eq!(get_s(&item, "gsi2pk"), Ok("DUE"));
        assert_eq!(get_s(&item, "gsi2sk"), Ok("2026-01-01T00:15:00.000000000Z"));
        assert_eq!(get_n::<i64>(&item, "ttl"), Ok(1_767_226_500 + 7 * 86_400));
        assert_eq!(get_n::<i64>(&item, "orderCode"), Ok(5));
    }

    #[test]
    fn due_index_keys_only_while_awaiting() {
        for (status, due) in [
            (OrderStatus::AwaitingUpload, true),
            (OrderStatus::AwaitingPayment, true),
            (OrderStatus::Paid, false),
            (OrderStatus::Printing, false),
            (OrderStatus::Completed, false),
            (OrderStatus::Failed, false),
            (OrderStatus::Cancelled, false),
            (OrderStatus::Expired, false),
        ] {
            let item = order_to_item(&order(status, None)).expect("encode");
            assert_eq!(item.contains_key("gsi2pk"), due, "{status:?}");
            assert_eq!(item.contains_key("gsi2sk"), due, "{status:?}");
            assert!(!item.contains_key("gsi1pk"));
            assert!(!item.contains_key("orderCode"));
        }
    }

    #[test]
    fn malformed_orders_are_internal_errors() {
        let good = order_to_item(&order(OrderStatus::Paid, Some(5))).expect("encode");
        for name in [
            "orderId",
            "owner",
            "printerId",
            "status",
            "amountVnd",
            "photos",
            "createdAt",
            "updatedAt",
            "expiresAt",
        ] {
            let mut item = good.clone();
            item.remove(name);
            assert!(
                matches!(order_from_item(&item), Err(CoreError::Internal(_))),
                "missing {name}"
            );
        }
        let bad: [(&str, AttributeValue); 8] = [
            ("status", s("weird")),
            ("printerId", s("a/b")),
            ("createdAt", s("yesterday")),
            ("amountVnd", n("many")),
            ("orderCode", s("x")),
            ("photos", s("x")),
            ("photos", AttributeValue::L(vec![s("x")])),
            ("error", s("x")),
        ];
        for (name, value) in bad {
            let mut item = good.clone();
            item.insert(name.to_owned(), value);
            assert!(
                matches!(order_from_item(&item), Err(CoreError::Internal(_))),
                "bad {name}"
            );
        }
        let broken_photos: [(&str, AttributeValue); 4] = [
            ("contentType", s("image/gif")),
            ("copies", s("x")),
            ("uploaded", s("x")),
            ("photoId", n(1)),
        ];
        for (name, value) in broken_photos {
            let mut item = good.clone();
            let mut photo = Item::from([
                ("photoId".to_owned(), s("p")),
                ("contentType".to_owned(), s("image/png")),
                ("sizeBytes".to_owned(), n(1)),
                ("copies".to_owned(), n(1)),
                ("uploaded".to_owned(), AttributeValue::Bool(true)),
            ]);
            photo.insert(name.to_owned(), value);
            item.insert(
                "photos".to_owned(),
                AttributeValue::L(vec![AttributeValue::M(photo)]),
            );
            assert!(order_from_item(&item).is_err(), "bad photo {name}");
        }
        let mut item = good;
        item.insert(
            "error".to_owned(),
            AttributeValue::M(Item::from([("code".to_owned(), s("c"))])),
        );
        assert!(order_from_item(&item).is_err());
    }

    fn printer_item() -> Item {
        let mut item = key("PRINTER#stand-001".into());
        item.extend([
            ("name".to_owned(), s("Stand 1")),
            ("pricePerPrintVnd".to_owned(), n(12_000)),
            ("status".to_owned(), s("busy")),
            ("lastSeenAt".to_owned(), ts(at("2026-01-01T00:00:00Z"))),
        ]);
        item
    }

    #[test]
    fn printer_decoding() {
        let printer = printer_from_item(&printer_item(), 10_000).expect("decode");
        assert_eq!(printer.printer_id.as_str(), "stand-001");
        assert_eq!(printer.name, "Stand 1");
        assert_eq!(printer.price_per_print_vnd, 12_000);
        assert_eq!(printer.status, PrinterStatus::Busy);
        assert_eq!(printer.last_seen_at, Some(at("2026-01-01T00:00:00Z")));
    }

    #[test]
    fn printer_defaults() {
        let mut item = printer_item();
        for name in ["name", "pricePerPrintVnd", "lastSeenAt"] {
            item.remove(name);
        }
        let printer = printer_from_item(&item, 10_000).expect("decode");
        assert_eq!(printer.name, "stand-001");
        assert_eq!(printer.price_per_print_vnd, 10_000);
        assert_eq!(printer.last_seen_at, None);
    }

    #[test]
    fn malformed_printers_are_internal_errors() {
        let mut item = printer_item();
        item.insert("status".into(), s("sleeping"));
        assert!(printer_from_item(&item, 1).is_err());
        let mut item = printer_item();
        item.remove("status");
        assert!(printer_from_item(&item, 1).is_err());
        let mut item = printer_item();
        item.insert("pk".into(), s("ORDER#x"));
        assert!(printer_from_item(&item, 1).is_err());
        let mut item = printer_item();
        item.insert("pk".into(), s("PRINTER#a/b"));
        assert!(printer_from_item(&item, 1).is_err());
        let mut item = printer_item();
        item.insert("pricePerPrintVnd".into(), s("x"));
        assert!(printer_from_item(&item, 1).is_err());
        let mut item = printer_item();
        item.insert("lastSeenAt".into(), s("x"));
        assert!(printer_from_item(&item, 1).is_err());
    }

    #[test]
    fn reason_helper() {
        let failed = CancellationReason::builder().code(COND_FAILED).build();
        let none = CancellationReason::builder().code("None").build();
        assert!(reason_failed(Some(&failed)));
        assert!(!reason_failed(Some(&none)));
        assert!(!reason_failed(None));
    }

    #[test]
    fn key_helpers() {
        assert_eq!(code_pk(42), "ORDERCODE#42");
        assert_eq!(order_pk(&"x".into()), "ORDER#x");
        assert_eq!(content_type_text(ContentType::Png), "image/png");
    }
}
