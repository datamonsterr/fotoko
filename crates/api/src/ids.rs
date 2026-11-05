//! Production [`IdGen`]: random ids and payOS order codes.

use fotoko_core::IdGen;
use fotoko_protocol::{OrderId, PhotoId};

/// payOS order codes are at most 2^53 - 1.
const ORDER_CODE_MASK: u64 = (1 << 53) - 1;

/// Ids from random UUIDs.
#[derive(Debug, Clone, Copy, Default)]
pub struct RandomIdGen;

impl IdGen for RandomIdGen {
    fn order_id(&self) -> OrderId {
        OrderId::from(uuid::Uuid::new_v4().simple().to_string())
    }

    fn photo_id(&self) -> PhotoId {
        PhotoId::from(uuid::Uuid::new_v4().simple().to_string())
    }

    fn order_code(&self) -> i64 {
        let (high, low) = uuid::Uuid::new_v4().as_u64_pair();
        let code = (high ^ low) & ORDER_CODE_MASK;
        // Zero is not a valid code; the mask keeps the value inside i64.
        i64::try_from(code.max(1)).unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn ids_are_distinct_url_safe_strings() {
        let ids = RandomIdGen;
        let orders: HashSet<String> = (0..200).map(|_| ids.order_id().to_string()).collect();
        let photos: HashSet<String> = (0..200).map(|_| ids.photo_id().to_string()).collect();
        assert_eq!(orders.len(), 200);
        assert_eq!(photos.len(), 200);
        for id in orders.iter().chain(&photos) {
            assert_eq!(id.len(), 32);
            assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn order_codes_fit_payos() {
        let ids = RandomIdGen;
        for _ in 0..1000 {
            let code = ids.order_code();
            assert!((1..=9_007_199_254_740_991).contains(&code), "{code}");
        }
    }
}
