//! The printer abstraction the job handler prints through.

use std::path::Path;

use async_trait::async_trait;
use fotoko_protocol::{Media, PiError, PiErrorCode, PrinterState};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Why a print failed. Each variant maps to one Pi error code.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PrintError {
    /// Out of paper (`paper_out`).
    #[error("printer is out of paper")]
    PaperOut,
    /// Out of ribbon or ink (`ribbon_out`).
    #[error("printer is out of ribbon")]
    RibbonOut,
    /// Printer not reachable (`printer_offline`).
    #[error("printer is offline: {0}")]
    Offline(String),
    /// The printer stack rejected the image (`invalid_image`).
    #[error("invalid image: {0}")]
    InvalidImage(String),
    /// CUPS or driver failure (`driver_error`).
    #[error("driver error: {0}")]
    Driver(String),
    /// The print was withdrawn because the agent is shutting down (`driver_error`).
    #[error("print cancelled")]
    Cancelled,
}

impl PrintError {
    /// The Pi error code reported to the cloud.
    pub fn code(&self) -> PiErrorCode {
        match self {
            Self::PaperOut => PiErrorCode::PaperOut,
            Self::RibbonOut => PiErrorCode::RibbonOut,
            Self::Offline(_) => PiErrorCode::PrinterOffline,
            Self::InvalidImage(_) => PiErrorCode::InvalidImage,
            Self::Driver(_) | Self::Cancelled => PiErrorCode::DriverError,
        }
    }

    /// Code and message as sent in a failed job status.
    pub fn to_pi_error(&self) -> PiError {
        PiError {
            code: self.code(),
            message: self.to_string(),
        }
    }
}

/// Per-print context.
#[derive(Debug, Clone)]
pub struct PrintContext {
    /// Human readable job title (the job id); CUPS shows it in its queue.
    pub title: String,
    /// Fires when the print must be withdrawn. Implementations stop waiting, cancel any queued
    /// work and return [`PrintError::Cancelled`].
    pub cancel: CancellationToken,
}

impl PrintContext {
    /// A context that is never cancelled.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            cancel: CancellationToken::new(),
        }
    }
}

/// A printer that prints one image file at a time.
#[async_trait]
pub trait Printer: Send + Sync {
    /// Print `copies` copies of the image at `file` on `media`; returns when all are printed.
    async fn print(
        &self,
        file: &Path,
        copies: u32,
        media: Media,
        ctx: &PrintContext,
    ) -> Result<(), PrintError>;

    /// Current state for the heartbeat.
    async fn state(&self) -> PrinterState;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_messages() {
        let cases = [
            (
                PrintError::PaperOut,
                PiErrorCode::PaperOut,
                "printer is out of paper",
            ),
            (
                PrintError::RibbonOut,
                PiErrorCode::RibbonOut,
                "printer is out of ribbon",
            ),
            (
                PrintError::Offline("usb".into()),
                PiErrorCode::PrinterOffline,
                "printer is offline: usb",
            ),
            (
                PrintError::InvalidImage("bad".into()),
                PiErrorCode::InvalidImage,
                "invalid image: bad",
            ),
            (
                PrintError::Driver("x".into()),
                PiErrorCode::DriverError,
                "driver error: x",
            ),
        ];
        for (error, code, message) in cases {
            assert_eq!(error.code(), code);
            assert_eq!(
                error.to_pi_error(),
                PiError {
                    code,
                    message: message.to_owned()
                }
            );
        }
    }

    #[test]
    fn context_defaults_to_not_cancelled() {
        let ctx = PrintContext::new("job-1");
        assert_eq!(ctx.title, "job-1");
        assert!(!ctx.cancel.is_cancelled());
    }
}
