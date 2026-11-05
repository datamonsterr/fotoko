//! HTTP error mapping: [`CoreError`] to status code and the contract body
//! `{"error":{"code","message"}}`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use fotoko_core::CoreError;
use fotoko_protocol::{ErrorBody, ErrorCode};

/// Message shown for `internal` errors: the real cause only goes to the logs.
pub const INTERNAL_MESSAGE: &str = "internal server error";

/// HTTP status of a contract error code.
pub const fn status_for(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Validation => StatusCode::BAD_REQUEST,
        ErrorCode::Forbidden => StatusCode::FORBIDDEN,
        ErrorCode::Conflict => StatusCode::CONFLICT,
        ErrorCode::PaymentProvider => StatusCode::BAD_GATEWAY,
        ErrorCode::Internal | ErrorCode::Unknown => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// An error on its way to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    status: StatusCode,
    error: CoreError,
}

impl ApiError {
    /// Same status as the code's default, with a different one (413, 405).
    pub fn with_status(status: StatusCode, error: CoreError) -> Self {
        Self { status, error }
    }

    /// The HTTP status.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The wrapped error.
    pub fn error(&self) -> &CoreError {
        &self.error
    }

    /// The body sent to clients. `internal` errors lose their message.
    pub fn body(&self) -> ErrorBody {
        match &self.error {
            CoreError::Internal(_) => ErrorBody::new(ErrorCode::Internal, INTERNAL_MESSAGE),
            other => other.to_body(),
        }
    }
}

impl From<CoreError> for ApiError {
    fn from(error: CoreError) -> Self {
        Self {
            status: status_for(error.code()),
            error,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let CoreError::Internal(cause) = &self.error {
            tracing::error!(cause = %cause, "internal error");
        } else if self.status.is_server_error() {
            tracing::warn!(code = ?self.error.code(), message = %self.error.message(), "upstream failure");
        }
        (self.status, Json(self.body())).into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    #[test]
    fn status_table() {
        let table = [
            (ErrorCode::NotFound, 404),
            (ErrorCode::Validation, 400),
            (ErrorCode::Forbidden, 403),
            (ErrorCode::Conflict, 409),
            (ErrorCode::PaymentProvider, 502),
            (ErrorCode::Internal, 500),
            (ErrorCode::Unknown, 500),
        ];
        for (code, status) in table {
            assert_eq!(status_for(code).as_u16(), status, "{code:?}");
        }
    }

    async fn render(error: ApiError) -> (u16, serde_json::Value) {
        let response = error.into_response();
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.expect("body");
        (status, serde_json::from_slice(&bytes).expect("json"))
    }

    #[tokio::test]
    async fn every_core_error_renders_its_contract_body() {
        let cases = [
            (CoreError::NotFound("nf".into()), 404, "not_found", "nf"),
            (CoreError::Validation("v".into()), 400, "validation", "v"),
            (CoreError::Forbidden("f".into()), 403, "forbidden", "f"),
            (CoreError::Conflict("c".into()), 409, "conflict", "c"),
            (
                CoreError::PaymentProvider("p".into()),
                502,
                "payment_provider",
                "p",
            ),
            (
                CoreError::Internal("table fotoko-prod exploded".into()),
                500,
                "internal",
                INTERNAL_MESSAGE,
            ),
        ];
        for (error, status, code, message) in cases {
            let (got_status, body) = render(ApiError::from(error)).await;
            assert_eq!(got_status, status);
            assert_eq!(
                body,
                serde_json::json!({"error": {"code": code, "message": message}})
            );
        }
    }

    #[tokio::test]
    async fn status_override_keeps_the_code() {
        let error = ApiError::with_status(
            StatusCode::PAYLOAD_TOO_LARGE,
            CoreError::Validation("too big".into()),
        );
        assert_eq!(error.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(error.error(), &CoreError::Validation("too big".into()));
        let (status, body) = render(error).await;
        assert_eq!(status, 413);
        assert_eq!(body["error"]["code"], "validation");
    }
}
