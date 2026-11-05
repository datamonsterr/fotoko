//! The one error type of the crate. Each variant maps 1:1 to a contract `ErrorCode`.

use fotoko_protocol::{ErrorBody, ErrorCode};
use thiserror::Error;

/// Failure of a use case or a port. The payload is a human-readable message that is safe to show
/// to API clients.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreError {
    /// The addressed printer or order does not exist.
    #[error("{0}")]
    NotFound(String),
    /// The request is malformed or breaks a limit.
    #[error("{0}")]
    Validation(String),
    /// The caller does not own the resource.
    #[error("{0}")]
    Forbidden(String),
    /// The resource is in a state that does not allow the operation.
    #[error("{0}")]
    Conflict(String),
    /// The payment provider failed or refused.
    #[error("{0}")]
    PaymentProvider(String),
    /// Anything else: storage, publishing, bugs.
    #[error("{0}")]
    Internal(String),
}

impl CoreError {
    /// The contract error code.
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Validation(_) => ErrorCode::Validation,
            Self::Forbidden(_) => ErrorCode::Forbidden,
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::PaymentProvider(_) => ErrorCode::PaymentProvider,
            Self::Internal(_) => ErrorCode::Internal,
        }
    }

    /// The message without the code.
    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(m)
            | Self::Validation(m)
            | Self::Forbidden(m)
            | Self::Conflict(m)
            | Self::PaymentProvider(m)
            | Self::Internal(m) => m,
        }
    }

    /// The HTTP error body `{"error":{"code","message"}}`.
    pub fn to_body(&self) -> ErrorBody {
        ErrorBody::new(self.code(), self.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> [(CoreError, ErrorCode, &'static str); 6] {
        [
            (
                CoreError::NotFound("a".into()),
                ErrorCode::NotFound,
                "not_found",
            ),
            (
                CoreError::Validation("a".into()),
                ErrorCode::Validation,
                "validation",
            ),
            (
                CoreError::Forbidden("a".into()),
                ErrorCode::Forbidden,
                "forbidden",
            ),
            (
                CoreError::Conflict("a".into()),
                ErrorCode::Conflict,
                "conflict",
            ),
            (
                CoreError::PaymentProvider("a".into()),
                ErrorCode::PaymentProvider,
                "payment_provider",
            ),
            (
                CoreError::Internal("a".into()),
                ErrorCode::Internal,
                "internal",
            ),
        ]
    }

    #[test]
    fn every_variant_maps_to_its_contract_code() {
        for (err, code, wire) in all() {
            assert_eq!(err.code(), code);
            assert_eq!(err.message(), "a");
            assert_eq!(err.to_string(), "a");
            let body = serde_json::to_value(err.to_body()).expect("serialize");
            assert_eq!(
                body,
                serde_json::json!({"error": {"code": wire, "message": "a"}})
            );
        }
    }
}
