//! MQTT plumbing: connection options (TLS for AWS IoT, plain TCP for local brokers), the last
//! will, and the publishing side used by the job handler and the heartbeat.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use fotoko_protocol::{
    Heartbeat, JobStatus, PrinterId, PrinterState, heartbeat_topic, status_topic,
};
use rumqttc::{AsyncClient, LastWill, MqttOptions, QoS, TlsConfiguration, Transport};
use thiserror::Error;

use crate::config::Config;

/// Largest packet accepted or sent. Print jobs with ten presigned URLs exceed rumqttc's 10 KiB
/// default.
const MAX_INCOMING_PACKET: usize = 256 * 1024;
const MAX_OUTGOING_PACKET: usize = 64 * 1024;

/// Agent version reported in heartbeats.
pub const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// MQTT setup or publish failure.
#[derive(Debug, Error)]
pub enum MqttError {
    /// A certificate or key file cannot be read.
    #[error("cannot read {path}: {source}")]
    ReadFile {
        /// File path.
        path: PathBuf,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// The configuration lacks TLS files although `tls = true`.
    #[error("tls requires mqtt.ca, mqtt.cert and mqtt.key")]
    MissingTlsFiles,
    /// The client's request queue is closed or full.
    #[error("publish failed: {0}")]
    Publish(String),
    /// A payload cannot be serialized.
    #[error("cannot encode payload: {0}")]
    Encode(#[from] serde_json::Error),
}

fn read(path: &Path) -> Result<Vec<u8>, MqttError> {
    fs::read(path).map_err(|source| MqttError::ReadFile {
        path: path.to_owned(),
        source,
    })
}

/// A heartbeat for `printer_id` right now.
pub fn heartbeat(printer_id: &PrinterId, state: PrinterState) -> Heartbeat {
    Heartbeat {
        printer_id: printer_id.clone(),
        printer_state: state,
        agent_version: AGENT_VERSION.to_owned(),
        at: Utc::now(),
    }
}

/// Connection options: client id is the printer id, persistent session (so jobs published while
/// the Pi reboots are delivered), keep alive from config, and a last will announcing `offline`
/// on the heartbeat topic (QoS 1, not retained).
pub fn mqtt_options(config: &Config) -> Result<MqttOptions, MqttError> {
    let mqtt = &config.mqtt;
    let mut options = MqttOptions::new(config.printer_id.as_str(), &mqtt.host, mqtt.port());
    options
        .set_keep_alive(Duration::from_secs(mqtt.keep_alive_secs))
        .set_clean_session(false)
        .set_max_packet_size(MAX_INCOMING_PACKET, MAX_OUTGOING_PACKET);
    if mqtt.tls {
        let (Some(ca), Some(cert), Some(key)) = (&mqtt.ca, &mqtt.cert, &mqtt.key) else {
            return Err(MqttError::MissingTlsFiles);
        };
        options.set_transport(Transport::tls_with_config(TlsConfiguration::Simple {
            ca: read(ca)?,
            alpn: None,
            client_auth: Some((read(cert)?, read(key)?)),
        }));
    }
    let will = serde_json::to_vec(&heartbeat(&config.printer_id, PrinterState::Offline))?;
    options.set_last_will(LastWill::new(
        heartbeat_topic(&config.printer_id),
        will,
        QoS::AtLeastOnce,
        false,
    ));
    Ok(options)
}

/// Where the agent publishes. Implemented over MQTT; tests substitute a recorder.
#[async_trait]
pub trait Sink: Send + Sync {
    /// Publish a job status.
    async fn status(&self, status: &JobStatus) -> Result<(), MqttError>;
    /// Publish a heartbeat. Must not block when the broker is unreachable.
    fn heartbeat(&self, heartbeat: &Heartbeat) -> Result<(), MqttError>;
}

/// [`Sink`] over an rumqttc client.
#[derive(Debug, Clone)]
pub struct MqttSink {
    client: AsyncClient,
    status_topic: String,
    heartbeat_topic: String,
}

impl MqttSink {
    /// Publish for `printer_id` through `client`.
    pub fn new(client: AsyncClient, printer_id: &PrinterId) -> Self {
        Self {
            client,
            status_topic: status_topic(printer_id),
            heartbeat_topic: heartbeat_topic(printer_id),
        }
    }
}

#[async_trait]
impl Sink for MqttSink {
    async fn status(&self, status: &JobStatus) -> Result<(), MqttError> {
        let payload = serde_json::to_vec(status)?;
        self.client
            .publish(&self.status_topic, QoS::AtLeastOnce, false, payload)
            .await
            .map_err(|e| MqttError::Publish(e.to_string()))
    }

    fn heartbeat(&self, heartbeat: &Heartbeat) -> Result<(), MqttError> {
        let payload = serde_json::to_vec(heartbeat)?;
        self.client
            // QoS 0: a heartbeat that is late is useless, so it is never queued for redelivery.
            .try_publish(&self.heartbeat_topic, QoS::AtMostOnce, false, payload)
            .map_err(|e| MqttError::Publish(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fotoko_protocol::{JobId, JobState, OrderId};
    use serde_json::Value;

    const LOCAL: &str = r#"
printer_id = "stand-001"
[mqtt]
host = "broker.local"
port = 1884
tls = false
keep_alive_secs = 12
[printer]
backend = "sim"
[sim]
out_dir = "/tmp/o"
[storage]
state_dir = "/tmp/s"
"#;

    fn local() -> Config {
        Config::from_toml(LOCAL).unwrap()
    }

    #[test]
    fn local_options() {
        let o = mqtt_options(&local()).unwrap();
        assert_eq!(o.client_id(), "stand-001");
        assert_eq!(o.broker_address(), ("broker.local".to_owned(), 1884));
        assert_eq!(o.keep_alive(), Duration::from_secs(12));
        assert!(!o.clean_session());
        assert_eq!(o.max_packet_size(), MAX_INCOMING_PACKET);
        assert!(matches!(o.transport(), Transport::Tcp));
    }

    #[test]
    fn last_will_is_offline_heartbeat_qos1_not_retained() {
        let o = mqtt_options(&local()).unwrap();
        let will = o.last_will().unwrap();
        assert_eq!(will.topic, "fotoko/printers/stand-001/heartbeat");
        assert_eq!(will.qos, QoS::AtLeastOnce);
        assert!(!will.retain);
        let body: Value = serde_json::from_slice(&will.message).unwrap();
        assert_eq!(body["printerId"], "stand-001");
        assert_eq!(body["printerState"], "offline");
        assert_eq!(body["agentVersion"], AGENT_VERSION);
        assert!(body["at"].is_string());
    }

    #[test]
    fn tls_options_read_the_pem_files() {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in [("ca", "CA-PEM"), ("cert", "CERT-PEM"), ("key", "KEY-PEM")] {
            fs::write(dir.path().join(name), body).unwrap();
        }
        let text = LOCAL.replace(
            "tls = false",
            &format!(
                "tls = true\nca = \"{0}/ca\"\ncert = \"{0}/cert\"\nkey = \"{0}/key\"",
                dir.path().display()
            ),
        );
        let config = Config::from_toml(&text).unwrap();
        let o = mqtt_options(&config).unwrap();
        let Transport::Tls(TlsConfiguration::Simple {
            ca,
            alpn,
            client_auth,
        }) = o.transport()
        else {
            panic!("expected TLS transport");
        };
        assert_eq!(ca, b"CA-PEM");
        assert!(alpn.is_none());
        assert_eq!(
            client_auth,
            Some((b"CERT-PEM".to_vec(), b"KEY-PEM".to_vec()))
        );
        fs::remove_file(dir.path().join("key")).unwrap();
        let e = mqtt_options(&config).unwrap_err();
        assert!(
            matches!(&e, MqttError::ReadFile { path, .. } if path.ends_with("key")),
            "{e}"
        );
    }

    #[test]
    fn tls_without_files_is_rejected() {
        let mut config = local();
        config.mqtt.tls = true;
        assert!(matches!(
            mqtt_options(&config),
            Err(MqttError::MissingTlsFiles)
        ));
    }

    #[test]
    fn heartbeat_fields() {
        let id = PrinterId::new("stand-9").unwrap();
        let hb = heartbeat(&id, PrinterState::Printing);
        assert_eq!(hb.printer_id, id);
        assert_eq!(hb.printer_state, PrinterState::Printing);
        assert_eq!(hb.agent_version, AGENT_VERSION);
    }

    fn sample_status() -> JobStatus {
        JobStatus {
            job_id: JobId::from("j"),
            order_id: OrderId::from("o"),
            printer_id: PrinterId::new("stand-001").unwrap(),
            state: JobState::Received,
            printed_copies: 0,
            error: None,
            at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn sink_errors_when_the_client_is_gone_or_full() {
        let id = PrinterId::new("stand-001").unwrap();
        let (client, eventloop) = AsyncClient::new(MqttOptions::new("t", "127.0.0.1", 1), 1);
        let sink = MqttSink::new(client, &id);
        // Fill the queue: the second heartbeat cannot be queued and must not block.
        sink.heartbeat(&heartbeat(&id, PrinterState::Idle)).unwrap();
        assert!(matches!(
            sink.heartbeat(&heartbeat(&id, PrinterState::Idle)),
            Err(MqttError::Publish(_))
        ));
        drop(eventloop);
        assert!(matches!(
            sink.status(&sample_status()).await,
            Err(MqttError::Publish(_))
        ));
    }
}
