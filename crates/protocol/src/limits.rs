//! Validation limits and defaults from `docs/contracts.md`.

use std::ops::RangeInclusive;
use std::time::Duration;

/// Maximum number of photos in one order.
pub const MAX_PHOTOS_PER_ORDER: usize = 10;
/// Allowed copies per photo.
pub const COPIES_RANGE: RangeInclusive<u32> = 1..=5;
/// Maximum size of one photo in bytes (15 MiB).
pub const MAX_PHOTO_BYTES: u64 = 15 * 1024 * 1024;
/// An order expires this long after creation unless paid (15 minutes).
pub const ORDER_TTL: Duration = Duration::from_secs(15 * 60);
/// Price of one print in VND when a printer has no override.
pub const DEFAULT_PRICE_VND: u64 = 10_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photo_limit_is_ten() {
        assert_eq!(MAX_PHOTOS_PER_ORDER, 10);
    }

    #[test]
    fn copies_range_is_one_to_five_inclusive() {
        assert!(!COPIES_RANGE.contains(&0));
        assert!(COPIES_RANGE.contains(&1));
        assert!(COPIES_RANGE.contains(&5));
        assert!(!COPIES_RANGE.contains(&6));
    }

    #[test]
    fn max_photo_bytes_is_15_mib() {
        assert_eq!(MAX_PHOTO_BYTES, 15_728_640);
    }

    #[test]
    fn order_ttl_is_fifteen_minutes() {
        assert_eq!(ORDER_TTL.as_secs(), 900);
    }

    #[test]
    fn default_price_is_ten_thousand_vnd() {
        assert_eq!(DEFAULT_PRICE_VND, 10_000);
    }
}
