//! payOS request and webhook signing. Pure functions, no IO.
//!
//! Algorithm (payOS docs, "Kiem tra du lieu voi signature", and the `payOSHQ` SDKs): take the
//! fields to sign, sort the keys alphabetically, render each value as a string, join `key=value`
//! pairs with `&`, and compute `HMAC_SHA256(checksumKey, string)` as lowercase hex.
//!
//! Value rendering follows the reference Node SDK (`convertObjToQueryStr`):
//!
//! - `null`, and the strings `"null"` and `"undefined"`, render as the empty string;
//! - strings render as is (raw UTF-8, no escaping);
//! - booleans render as `true` / `false`, integers as plain decimal digits;
//! - arrays render as compact JSON in which every object element has its keys sorted
//!   (`JSON.stringify(value.map(sortObjDataByKey))`).
//!
//! Numbers are never routed through `f64`: serde_json keeps the original `Number`, so large
//! `i64` order codes are exact. Known divergences from JavaScript that payOS never produces: a
//! non-integral float inside an array renders with Rust's float formatting, and a plain nested
//! object renders as compact JSON instead of `[object Object]`.

use std::collections::BTreeMap;

use fotoko_protocol::{PayosWebhook, PayosWebhookData};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Map, Value};
use sha2::Sha256;

use crate::error::CoreError;
use crate::ports::PaymentLinkRequest;

type HmacSha256 = Hmac<Sha256>;

fn mac(checksum_key: &str) -> Result<HmacSha256, CoreError> {
    // HMAC accepts keys of any length, so this does not fail in practice.
    HmacSha256::new_from_slice(checksum_key.as_bytes())
        .map_err(|_| CoreError::Internal("invalid payOS checksum key".into()))
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks(2)
        .map(|pair| {
            let hi = char::from(pair[0]).to_digit(16)?;
            let lo = char::from(pair[1]).to_digit(16)?;
            u8::try_from(hi * 16 + lo).ok()
        })
        .collect()
}

fn sorted_object(map: &Map<String, Value>) -> Value {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut sorted = Map::new();
    for key in keys {
        if let Some(value) = map.get(key) {
            sorted.insert(key.clone(), value.clone());
        }
    }
    Value::Object(sorted)
}

fn render(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) if s == "null" || s == "undefined" => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(items) => {
            let sorted: Vec<Value> = items
                .iter()
                .map(|item| match item {
                    Value::Object(map) => sorted_object(map),
                    other => other.clone(),
                })
                .collect();
            Value::Array(sorted).to_string()
        }
        Value::Object(_) => value.to_string(),
    }
}

/// `key=value` pairs sorted by key and joined with `&`.
fn signing_string(fields: &BTreeMap<String, Value>) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{key}={}", render(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn hmac_hex(checksum_key: &str, data: &str) -> Result<String, CoreError> {
    let mut mac = mac(checksum_key)?;
    mac.update(data.as_bytes());
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

/// Signature for `POST /v2/payment-requests`:
/// `HMAC_SHA256(key, "amount=..&cancelUrl=..&description=..&orderCode=..&returnUrl=..")` as hex.
pub fn sign_payment_request(
    checksum_key: &str,
    fields: &PaymentLinkRequest,
) -> Result<String, CoreError> {
    let data = format!(
        "amount={}&cancelUrl={}&description={}&orderCode={}&returnUrl={}",
        fields.amount_vnd,
        fields.cancel_url,
        fields.description,
        fields.order_code,
        fields.return_url
    );
    hmac_hex(checksum_key, &data)
}

/// The data fields of a webhook as a key-sorted map, with the typed fields restored to their JSON
/// numbers.
fn webhook_fields(data: &PayosWebhookData) -> BTreeMap<String, Value> {
    let mut fields: BTreeMap<String, Value> = data.extra.clone().into_iter().collect();
    // `code` and `desc` are typed fields, so they are not in `extra`: each key appears once.
    for (key, value) in [("code", &data.code), ("desc", &data.desc)] {
        if let Some(value) = value {
            fields.insert(key.into(), Value::from(value.clone()));
        }
    }
    fields.insert("orderCode".into(), Value::from(data.order_code));
    fields.insert("amount".into(), Value::from(data.amount));
    fields
}

/// Signature payOS computes over a webhook's `data` object. Also used by the payOS mock in tests.
pub fn sign_webhook_data(checksum_key: &str, data: &PayosWebhookData) -> Result<String, CoreError> {
    hmac_hex(checksum_key, &signing_string(&webhook_fields(data)))
}

/// Whether `webhook.signature` is the HMAC-SHA256 hex of `webhook.data`. The comparison is
/// constant-time; a signature that is not valid hex is simply `false`.
pub fn verify_webhook(checksum_key: &str, webhook: &PayosWebhook) -> bool {
    let Some(expected) = hex_decode(&webhook.signature) else {
        return false;
    };
    let Ok(mut mac) = mac(checksum_key) else {
        return false;
    };
    mac.update(signing_string(&webhook_fields(&webhook.data)).as_bytes());
    mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Checksum key, data and signature from the payOS docs page
    /// <https://payos.vn/docs/tich-hop-webhook/kiem-tra-du-lieu-voi-signature>.
    const DOC_KEY: &str = "1a54716c8f0efb2744fb28b6e38b25da7f67a925d98bc1c18bd8faaecadd7675";
    const DOC_SIGNATURE: &str = "412e915d2871504ed31be63c8f62a149a4410d34c4c42affc9006ef9917eaa03";

    fn doc_webhook() -> PayosWebhook {
        serde_json::from_value(json!({
            "code": "00",
            "desc": "success",
            "success": true,
            "data": {
                "orderCode": 123,
                "amount": 3000,
                "description": "VQRIO123",
                "accountNumber": "12345678",
                "reference": "TF230204212323",
                "transactionDateTime": "2023-02-04 18:25:00",
                "currency": "VND",
                "paymentLinkId": "124c33293c43417ab7879e14c8d9eb18",
                "code": "00",
                "desc": "Thành công",
                "counterAccountBankId": "",
                "counterAccountBankName": "",
                "counterAccountName": "",
                "counterAccountNumber": "",
                "virtualAccountName": "",
                "virtualAccountNumber": ""
            },
            "signature": DOC_SIGNATURE
        }))
        .expect("webhook")
    }

    fn webhook_with(extra: Value, signature: &str) -> PayosWebhook {
        let mut data = json!({"orderCode": 1001, "amount": 20000, "code": "00",
                              "description": "Fotoko 1001"});
        if let (Some(base), Value::Object(more)) = (data.as_object_mut(), extra) {
            base.extend(more);
        }
        serde_json::from_value(json!({
            "code": "00", "desc": "success", "success": true,
            "data": data, "signature": signature
        }))
        .expect("webhook")
    }

    #[test]
    fn docs_vector_signs_and_verifies() {
        let webhook = doc_webhook();
        assert_eq!(
            sign_webhook_data(DOC_KEY, &webhook.data).as_deref(),
            Ok(DOC_SIGNATURE)
        );
        assert!(verify_webhook(DOC_KEY, &webhook));
    }

    #[test]
    fn signing_string_sorts_keys_and_keeps_raw_utf8() {
        let fields = webhook_fields(&doc_webhook().data);
        assert_eq!(
            signing_string(&fields),
            "accountNumber=12345678&amount=3000&code=00&counterAccountBankId=\
&counterAccountBankName=&counterAccountName=&counterAccountNumber=&currency=VND\
&desc=Thành công&description=VQRIO123&orderCode=123\
&paymentLinkId=124c33293c43417ab7879e14c8d9eb18&reference=TF230204212323\
&transactionDateTime=2023-02-04 18:25:00&virtualAccountName=&virtualAccountNumber="
        );
    }

    #[test]
    fn typed_code_and_desc_are_signed_exactly_once() {
        let data = doc_webhook().data;
        assert_eq!(data.code.as_deref(), Some("00"));
        assert_eq!(data.desc.as_deref(), Some("Thành công"));
        assert!(!data.extra.contains_key("code") && !data.extra.contains_key("desc"));
        let text = signing_string(&webhook_fields(&data));
        assert_eq!(text.matches("&code=").count(), 1);
        assert_eq!(text.matches("&desc=").count(), 1);
        // Changing either typed field breaks the signature.
        let mut tampered = doc_webhook();
        tampered.data.code = Some("01".into());
        assert!(!verify_webhook(DOC_KEY, &tampered));
        let mut tampered = doc_webhook();
        tampered.data.desc = None;
        assert!(!verify_webhook(DOC_KEY, &tampered));
    }

    #[test]
    fn verify_rejects_tampering() {
        let mut tampered = doc_webhook();
        tampered.data.amount = 1;
        assert!(!verify_webhook(DOC_KEY, &tampered));

        let mut tampered = doc_webhook();
        tampered.data.order_code = 124;
        assert!(!verify_webhook(DOC_KEY, &tampered));

        let mut tampered = doc_webhook();
        tampered.data.extra.insert("currency".into(), json!("USD"));
        assert!(!verify_webhook(DOC_KEY, &tampered));

        assert!(!verify_webhook("other-key", &doc_webhook()));
    }

    #[test]
    fn verify_rejects_malformed_signatures() {
        for bad in ["", "zz", "abc", &DOC_SIGNATURE[..62], "é1"] {
            let mut webhook = doc_webhook();
            webhook.signature = bad.to_owned();
            assert!(!verify_webhook(DOC_KEY, &webhook), "{bad:?}");
        }
        let mut longer = doc_webhook();
        longer.signature = format!("{DOC_SIGNATURE}00");
        assert!(!verify_webhook(DOC_KEY, &longer));
    }

    #[test]
    fn verify_accepts_uppercase_hex() {
        let mut webhook = doc_webhook();
        webhook.signature = DOC_SIGNATURE.to_uppercase();
        assert!(verify_webhook(DOC_KEY, &webhook));
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(hex_decode("000fA5ff"), Some(vec![0x00, 0x0f, 0xa5, 0xff]));
        assert_eq!(hex_decode("0"), None);
        assert_eq!(hex_decode("0g"), None);
        assert_eq!(hex_decode("g0"), None);
        assert_eq!(hex_decode(""), Some(vec![]));
    }

    /// Vector computed with the documented algorithm (Python `hmac` + `hashlib`, array rendered
    /// like `JSON.stringify` with sorted element keys) because the docs publish none for these
    /// shapes. Data: `items` array, `nothing` null, `flag` the string "true".
    #[test]
    fn arrays_nulls_and_strings_follow_the_sdk_rules() {
        let extra = json!({
            "items": [{"quantity": 1, "name": "Ảnh 4x6", "price": 10000}],
            "nothing": null,
            "flag": "true"
        });
        let webhook = webhook_with(
            extra,
            "5af75f7c090b54d68a28334af11eaa4141304809837e7614d750ba03bc1e21ea",
        );
        assert_eq!(
            signing_string(&webhook_fields(&webhook.data)),
            "amount=20000&code=00&description=Fotoko 1001&flag=true\
&items=[{\"name\":\"Ảnh 4x6\",\"price\":10000,\"quantity\":1}]&nothing=&orderCode=1001"
        );
        assert!(verify_webhook("test-checksum-key", &webhook));
    }

    #[test]
    fn render_covers_every_value_kind() {
        assert_eq!(render(&Value::Null), "");
        assert_eq!(render(&json!("null")), "");
        assert_eq!(render(&json!("undefined")), "");
        assert_eq!(render(&json!("plain")), "plain");
        assert_eq!(render(&json!(true)), "true");
        assert_eq!(render(&json!(false)), "false");
        assert_eq!(
            render(&json!(9_007_199_254_740_993_i64)),
            "9007199254740993"
        );
        assert_eq!(
            render(&json!([1, "a", {"b": 1, "a": 2}])),
            "[1,\"a\",{\"a\":2,\"b\":1}]"
        );
        assert_eq!(render(&json!({"k": 1})), "{\"k\":1}");
    }

    #[test]
    fn large_order_codes_are_not_lossy() {
        // 2^53 + 1 is not representable as f64.
        let data = PayosWebhookData {
            order_code: 9_007_199_254_740_993,
            amount: 1,
            code: None,
            desc: None,
            extra: BTreeMap::new(),
        };
        assert_eq!(
            signing_string(&webhook_fields(&data)),
            "amount=1&orderCode=9007199254740993"
        );
    }

    #[test]
    fn payment_request_vector() {
        // Computed with the documented algorithm (Python hmac/hashlib): see module docs.
        let request = PaymentLinkRequest {
            order_code: 1001,
            amount_vnd: 20_000,
            description: "Fotoko 1001".into(),
            return_url: "https://app.example/orders/o1".into(),
            cancel_url: "https://app.example/orders/o1".into(),
            expires_at: "2026-01-01T00:15:00Z".parse().expect("ts"),
        };
        assert_eq!(
            sign_payment_request("test-checksum-key", &request).as_deref(),
            Ok("a4b4c61a60495908486c55bd95961d4b212e123599f3809d944a3aa9cc3ad96b")
        );
        // Every signed field changes the signature; expires_at is not signed.
        let base = sign_payment_request("k", &request).expect("sign");
        let mut changed = request.clone();
        changed.amount_vnd += 1;
        assert_ne!(sign_payment_request("k", &changed).expect("sign"), base);
        let mut changed = request.clone();
        changed.expires_at += chrono::Duration::minutes(1);
        assert_eq!(sign_payment_request("k", &changed).expect("sign"), base);
    }

    #[test]
    fn empty_key_still_signs() {
        assert!(sign_webhook_data("", &doc_webhook().data).is_ok());
    }
}
