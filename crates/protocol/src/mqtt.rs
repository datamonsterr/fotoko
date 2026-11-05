//! MQTT payloads (JSON, camelCase, QoS 1). See `docs/contracts.md`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::http::{ContentType, Media};

/// One photo to print inside a [`PrintJob`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobPhoto {
    /// Photo id.
    pub photo_id: String,
    /// Presigned GET URL (15 minutes).
    pub url: String,
    /// Expected image format.
    pub content_type: ContentType,
    /// Copies to print.
    pub copies: u32,
}

/// Cloud to Pi: `fotoko/printers/{printerId}/jobs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintJob {
    /// Job id (dedupe key on the Pi).
    pub job_id: String,
    /// Order id.
    pub order_id: String,
    /// Target printer.
    pub printer_id: String,
    /// Media size.
    pub media: Media,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Photos.
    pub photos: Vec<JobPhoto>,
}

/// Progress of a job on the Pi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Accepted by the agent.
    Received,
    /// Being printed.
    Printing,
    /// All copies printed.
    Completed,
    /// Failed; see the error.
    Failed,
}

/// Error codes the Pi reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiErrorCode {
    /// Out of paper.
    PaperOut,
    /// Out of ribbon.
    RibbonOut,
    /// Printer not reachable.
    PrinterOffline,
    /// Image download failed.
    DownloadFailed,
    /// Image failed validation.
    InvalidImage,
    /// CUPS or driver failure.
    DriverError,
}

/// Error attached to a failed [`JobStatus`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiError {
    /// Error code.
    pub code: PiErrorCode,
    /// Human readable message.
    pub message: String,
}

/// Pi to cloud: `fotoko/printers/{printerId}/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    /// Job id.
    pub job_id: String,
    /// Order id.
    pub order_id: String,
    /// Reporting printer.
    pub printer_id: String,
    /// Job state.
    pub state: JobState,
    /// Copies printed so far.
    pub printed_copies: u32,
    /// Failure details when `state` is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<PiError>,
    /// Event time.
    pub at: DateTime<Utc>,
}

/// Printer state reported by the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrinterState {
    /// Ready.
    Idle,
    /// Printing.
    Printing,
    /// Faulted.
    Error,
    /// Gone (also the MQTT last will).
    Offline,
}

/// Pi to cloud: `fotoko/printers/{printerId}/heartbeat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Heartbeat {
    /// Reporting printer.
    pub printer_id: String,
    /// Printer state.
    pub printer_state: PrinterState,
    /// Agent version.
    pub agent_version: String,
    /// Event time.
    pub at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().expect("valid timestamp")
    }

    fn check<T>(value: &T, wire: Value)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        assert_eq!(serde_json::to_value(value).expect("serialize"), wire);
        assert_eq!(
            &serde_json::from_value::<T>(wire).expect("deserialize"),
            value
        );
    }

    #[test]
    fn print_job() {
        let job = PrintJob {
            job_id: "j1".into(),
            order_id: "o1".into(),
            printer_id: "stand-001".into(),
            media: Media::Photo4x6,
            created_at: ts("2026-01-01T00:00:00Z"),
            photos: vec![JobPhoto {
                photo_id: "p1".into(),
                url: "https://s3.example/get".into(),
                content_type: ContentType::Jpeg,
                copies: 3,
            }],
        };
        check(
            &job,
            json!({"jobId": "j1", "orderId": "o1", "printerId": "stand-001", "media": "4x6",
                   "createdAt": "2026-01-01T00:00:00Z",
                   "photos": [{"photoId": "p1", "url": "https://s3.example/get",
                               "contentType": "image/jpeg", "copies": 3}]}),
        );
    }

    #[test]
    fn job_state_variants() {
        check(&JobState::Received, json!("received"));
        check(&JobState::Printing, json!("printing"));
        check(&JobState::Completed, json!("completed"));
        check(&JobState::Failed, json!("failed"));
        assert!(serde_json::from_value::<JobState>(json!("queued")).is_err());
    }

    #[test]
    fn pi_error_code_variants() {
        let all = [
            (PiErrorCode::PaperOut, "paper_out"),
            (PiErrorCode::RibbonOut, "ribbon_out"),
            (PiErrorCode::PrinterOffline, "printer_offline"),
            (PiErrorCode::DownloadFailed, "download_failed"),
            (PiErrorCode::InvalidImage, "invalid_image"),
            (PiErrorCode::DriverError, "driver_error"),
        ];
        for (code, wire) in all {
            check(&code, json!(wire));
        }
    }

    #[test]
    fn job_status_without_error_omits_field() {
        let status = JobStatus {
            job_id: "j1".into(),
            order_id: "o1".into(),
            printer_id: "stand-001".into(),
            state: JobState::Printing,
            printed_copies: 1,
            error: None,
            at: ts("2026-01-01T00:00:05Z"),
        };
        check(
            &status,
            json!({"jobId": "j1", "orderId": "o1", "printerId": "stand-001",
                   "state": "printing", "printedCopies": 1, "at": "2026-01-01T00:00:05Z"}),
        );
    }

    #[test]
    fn job_status_with_error() {
        let status = JobStatus {
            job_id: "j1".into(),
            order_id: "o1".into(),
            printer_id: "stand-001".into(),
            state: JobState::Failed,
            printed_copies: 0,
            error: Some(PiError {
                code: PiErrorCode::PaperOut,
                message: "out of paper".into(),
            }),
            at: ts("2026-01-01T00:00:05Z"),
        };
        check(
            &status,
            json!({"jobId": "j1", "orderId": "o1", "printerId": "stand-001",
                   "state": "failed", "printedCopies": 0,
                   "error": {"code": "paper_out", "message": "out of paper"},
                   "at": "2026-01-01T00:00:05Z"}),
        );
    }

    #[test]
    fn printer_state_variants() {
        check(&PrinterState::Idle, json!("idle"));
        check(&PrinterState::Printing, json!("printing"));
        check(&PrinterState::Error, json!("error"));
        check(&PrinterState::Offline, json!("offline"));
    }

    #[test]
    fn heartbeat() {
        let hb = Heartbeat {
            printer_id: "stand-001".into(),
            printer_state: PrinterState::Offline,
            agent_version: "0.1.0".into(),
            at: ts("2026-01-01T00:00:00Z"),
        };
        check(
            &hb,
            json!({"printerId": "stand-001", "printerState": "offline",
                   "agentVersion": "0.1.0", "at": "2026-01-01T00:00:00Z"}),
        );
    }
}
