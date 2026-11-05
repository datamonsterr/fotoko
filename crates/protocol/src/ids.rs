//! Typed identifiers. Each serializes as a plain JSON string.

use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// The id as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = Infallible;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(s.to_owned()))
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

id_type!(
    /// Printer (IoT Thing) id, for example `stand-001`.
    PrinterId
);
id_type!(
    /// Order id.
    OrderId
);
id_type!(
    /// Photo id within an order.
    PhotoId
);
id_type!(
    /// Print job id (dedupe key on the Pi).
    JobId
);

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! id_tests {
        ($name:ident, $mod:ident) => {
            mod $mod {
                use super::*;

                #[test]
                fn serializes_as_plain_string() {
                    let id = $name::from("abc");
                    assert_eq!(serde_json::to_string(&id).expect("serialize"), "\"abc\"");
                    let back: $name = serde_json::from_str("\"abc\"").expect("deserialize");
                    assert_eq!(back, id);
                }

                #[test]
                fn conversions_and_display() {
                    let id = $name::from("abc".to_owned());
                    assert_eq!(id.as_str(), "abc");
                    assert_eq!(id.as_ref(), "abc");
                    assert_eq!(id.to_string(), "abc");
                    assert_eq!("abc".parse::<$name>(), Ok(id));
                }
            }
        };
    }

    id_tests!(PrinterId, printer_id);
    id_tests!(OrderId, order_id);
    id_tests!(PhotoId, photo_id);
    id_tests!(JobId, job_id);

    #[test]
    fn non_string_json_is_rejected() {
        assert!(serde_json::from_str::<PrinterId>("42").is_err());
    }
}
