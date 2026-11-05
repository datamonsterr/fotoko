//! MQTT topic names: `fotoko/printers/{printerId}/{jobs|status|heartbeat}`.

use thiserror::Error;

const PREFIX: &str = "fotoko/printers";

/// Which of the three per-printer topics a topic names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TopicKind {
    /// Cloud to Pi: print jobs.
    Jobs,
    /// Pi to cloud: job status.
    Status,
    /// Pi to cloud: heartbeat and last will.
    Heartbeat,
}

impl TopicKind {
    /// Last topic segment for this kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jobs => "jobs",
            Self::Status => "status",
            Self::Heartbeat => "heartbeat",
        }
    }

    fn parse(segment: &str) -> Option<Self> {
        match segment {
            "jobs" => Some(Self::Jobs),
            "status" => Some(Self::Status),
            "heartbeat" => Some(Self::Heartbeat),
            _ => None,
        }
    }
}

/// Why a topic string is not a Fotoko printer topic.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TopicError {
    /// Not `fotoko/printers/{id}/{kind}` (wrong prefix or segment count).
    #[error("not a fotoko printer topic: {0}")]
    Malformed(String),
    /// The printer id segment is empty or contains an MQTT wildcard.
    #[error("invalid printer id in topic: {0}")]
    InvalidPrinterId(String),
    /// The last segment is not `jobs`, `status` or `heartbeat`.
    #[error("unknown topic kind: {0}")]
    UnknownKind(String),
}

/// Topic for one printer and kind.
pub fn topic(printer_id: &str, kind: TopicKind) -> String {
    format!("{PREFIX}/{printer_id}/{}", kind.as_str())
}

/// `fotoko/printers/{printer_id}/jobs`
pub fn jobs_topic(printer_id: &str) -> String {
    topic(printer_id, TopicKind::Jobs)
}

/// `fotoko/printers/{printer_id}/status`
pub fn status_topic(printer_id: &str) -> String {
    topic(printer_id, TopicKind::Status)
}

/// `fotoko/printers/{printer_id}/heartbeat`
pub fn heartbeat_topic(printer_id: &str) -> String {
    topic(printer_id, TopicKind::Heartbeat)
}

/// Split a topic into its printer id and kind.
pub fn parse_topic(topic: &str) -> Result<(String, TopicKind), TopicError> {
    let segments: Vec<&str> = topic.split('/').collect();
    let [fotoko, printers, id, kind] = segments.as_slice() else {
        return Err(TopicError::Malformed(topic.to_owned()));
    };
    if *fotoko != "fotoko" || *printers != "printers" {
        return Err(TopicError::Malformed(topic.to_owned()));
    }
    if id.is_empty() || id.contains(['+', '#']) {
        return Err(TopicError::InvalidPrinterId((*id).to_owned()));
    }
    let kind = TopicKind::parse(kind).ok_or_else(|| TopicError::UnknownKind((*kind).to_owned()))?;
    Ok(((*id).to_owned(), kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_produce_contract_topics() {
        assert_eq!(jobs_topic("stand-001"), "fotoko/printers/stand-001/jobs");
        assert_eq!(
            status_topic("stand-001"),
            "fotoko/printers/stand-001/status"
        );
        assert_eq!(
            heartbeat_topic("stand-001"),
            "fotoko/printers/stand-001/heartbeat"
        );
    }

    #[test]
    fn kind_strings() {
        assert_eq!(TopicKind::Jobs.as_str(), "jobs");
        assert_eq!(TopicKind::Status.as_str(), "status");
        assert_eq!(TopicKind::Heartbeat.as_str(), "heartbeat");
    }

    #[test]
    fn parse_round_trips_every_kind() {
        for kind in [TopicKind::Jobs, TopicKind::Status, TopicKind::Heartbeat] {
            let parsed = parse_topic(&topic("p-1", kind));
            assert_eq!(parsed, Ok(("p-1".to_owned(), kind)));
        }
    }

    #[test]
    fn parse_rejects_wrong_segment_count() {
        for bad in [
            "",
            "fotoko",
            "fotoko/printers/x",
            "fotoko/printers/x/jobs/extra",
        ] {
            assert_eq!(
                parse_topic(bad),
                Err(TopicError::Malformed(bad.to_owned())),
                "{bad}"
            );
        }
    }

    #[test]
    fn parse_rejects_wrong_prefix() {
        for bad in ["other/printers/x/jobs", "fotoko/devices/x/jobs"] {
            assert_eq!(
                parse_topic(bad),
                Err(TopicError::Malformed(bad.to_owned())),
                "{bad}"
            );
        }
    }

    #[test]
    fn parse_rejects_empty_or_wildcard_printer_id() {
        assert_eq!(
            parse_topic("fotoko/printers//jobs"),
            Err(TopicError::InvalidPrinterId(String::new()))
        );
        assert_eq!(
            parse_topic("fotoko/printers/+/jobs"),
            Err(TopicError::InvalidPrinterId("+".to_owned()))
        );
        assert_eq!(
            parse_topic("fotoko/printers/a#b/jobs"),
            Err(TopicError::InvalidPrinterId("a#b".to_owned()))
        );
    }

    #[test]
    fn parse_rejects_unknown_kind() {
        assert_eq!(
            parse_topic("fotoko/printers/x/logs"),
            Err(TopicError::UnknownKind("logs".to_owned()))
        );
    }

    #[test]
    fn errors_display() {
        assert_eq!(
            TopicError::Malformed("a".into()).to_string(),
            "not a fotoko printer topic: a"
        );
        assert_eq!(
            TopicError::InvalidPrinterId("+".into()).to_string(),
            "invalid printer id in topic: +"
        );
        assert_eq!(
            TopicError::UnknownKind("x".into()).to_string(),
            "unknown topic kind: x"
        );
    }
}
