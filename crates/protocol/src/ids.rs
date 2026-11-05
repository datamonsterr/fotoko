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

/// Why a string is not a valid [`PrinterId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrinterIdError {
    /// The id is empty.
    #[error("printer id is empty")]
    Empty,
    /// The id contains `/`, `+` or `#`, which would break MQTT topics.
    #[error("printer id contains a reserved character: {0:?}")]
    ReservedChar(String),
}

/// Printer (IoT Thing) id, for example `stand-001`. Non-empty and free of `/`, `+` and `#`, so it
/// is always safe inside an MQTT topic. Invalid ids cannot be constructed or deserialized.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PrinterId(String);

impl PrinterId {
    /// Validate and wrap an id.
    pub fn new(id: impl Into<String>) -> Result<Self, PrinterIdError> {
        let id = id.into();
        if id.is_empty() {
            return Err(PrinterIdError::Empty);
        }
        if id.contains(['/', '+', '#']) {
            return Err(PrinterIdError::ReservedChar(id));
        }
        Ok(Self(id))
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PrinterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for PrinterId {
    type Err = PrinterIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for PrinterId {
    type Error = PrinterIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PrinterId> for String {
    fn from(value: PrinterId) -> Self {
        value.0
    }
}

impl AsRef<str> for PrinterId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

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
pub(crate) mod tests {
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

    id_tests!(OrderId, order_id);
    id_tests!(PhotoId, photo_id);
    id_tests!(JobId, job_id);

    /// Valid printer id for tests.
    pub(crate) fn pid(id: &str) -> PrinterId {
        PrinterId::new(id).expect("valid printer id")
    }

    #[test]
    fn printer_id_valid() {
        let id = pid("stand-001");
        assert_eq!(id.as_str(), "stand-001");
        assert_eq!(id.as_ref(), "stand-001");
        assert_eq!(id.to_string(), "stand-001");
        assert_eq!("stand-001".parse::<PrinterId>(), Ok(id.clone()));
        assert_eq!(PrinterId::try_from("stand-001".to_owned()), Ok(id.clone()));
        assert_eq!(String::from(id.clone()), "stand-001");
        assert_eq!(
            serde_json::to_string(&id).expect("serialize"),
            "\"stand-001\""
        );
        let back: PrinterId = serde_json::from_str("\"stand-001\"").expect("deserialize");
        assert_eq!(back, id);
    }

    #[test]
    fn printer_id_rejects_invalid() {
        assert_eq!(PrinterId::new(""), Err(PrinterIdError::Empty));
        for bad in ["a/b", "+", "a#", "#"] {
            assert_eq!(
                PrinterId::new(bad),
                Err(PrinterIdError::ReservedChar(bad.to_owned()))
            );
            assert!(bad.parse::<PrinterId>().is_err());
            assert!(serde_json::from_value::<PrinterId>(serde_json::json!(bad)).is_err());
        }
        assert!(serde_json::from_str::<PrinterId>("\"\"").is_err());
        assert_eq!(PrinterIdError::Empty.to_string(), "printer id is empty");
        assert_eq!(
            PrinterIdError::ReservedChar("a/b".into()).to_string(),
            "printer id contains a reserved character: \"a/b\""
        );
    }

    #[test]
    fn non_string_json_is_rejected() {
        assert!(serde_json::from_str::<PrinterId>("42").is_err());
        assert!(serde_json::from_str::<OrderId>("42").is_err());
    }
}
