//! Local MQTT: publishes print jobs and feeds Pi statuses and heartbeats back into the service,
//! the way AWS IoT Core plus the `fotoko-iot-status` rule do in production.

use std::time::Duration;

use async_trait::async_trait;
use fotoko_core::{CoreError, JobPublisher, OrderService};
use fotoko_protocol::{PrintJob, TopicKind, jobs_topic, parse_topic};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use serde_json::Value;

use crate::status::handle_event;

/// Filters the service listens on.
const SUBSCRIPTIONS: [&str; 2] = ["fotoko/printers/+/status", "fotoko/printers/+/heartbeat"];
const REQUEST_CAPACITY: usize = 64;
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// `mqtt://host:port` or `tcp://host:port` to `(host, port)`; the port defaults to 1883.
pub fn parse_broker_url(url: &str) -> Result<(String, u16), CoreError> {
    let bad = || CoreError::Internal(format!("invalid MQTT URL {url:?}"));
    let rest = url
        .strip_prefix("mqtt://")
        .or_else(|| url.strip_prefix("tcp://"))
        .ok_or_else(bad)?
        .trim_end_matches('/');
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().map_err(|_| bad())?),
        None => (rest, 1883),
    };
    if host.is_empty() || host.contains('/') {
        return Err(bad());
    }
    Ok((host.to_owned(), port))
}

/// [`JobPublisher`] over MQTT, QoS 1. The call returns once the packet is queued on the client;
/// the event loop (see [`MqttEvents::run`]) sends it and handles the acknowledgement.
#[derive(Debug, Clone)]
pub struct MqttJobPublisher {
    client: AsyncClient,
}

#[async_trait]
impl JobPublisher for MqttJobPublisher {
    async fn publish_job(&self, job: &PrintJob) -> Result<(), CoreError> {
        let payload = serde_json::to_vec(job)
            .map_err(|e| CoreError::Internal(format!("cannot serialize the print job: {e}")))?;
        self.client
            .publish(
                jobs_topic(&job.printer_id),
                QoS::AtLeastOnce,
                false,
                payload,
            )
            .await
            .map_err(|e| CoreError::Internal(format!("cannot queue the print job: {e}")))
    }
}

/// The receiving half of the MQTT connection.
pub struct MqttEvents {
    client: AsyncClient,
    eventloop: EventLoop,
}

impl std::fmt::Debug for MqttEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MqttEvents").finish_non_exhaustive()
    }
}

/// Connect (lazily: the connection is made when [`MqttEvents::run`] polls) to the broker.
pub fn connect(url: &str, client_id: &str) -> Result<(MqttJobPublisher, MqttEvents), CoreError> {
    let (host, port) = parse_broker_url(url)?;
    let mut options = MqttOptions::new(client_id, host, port);
    options.set_keep_alive(Duration::from_secs(30));
    let (client, eventloop) = AsyncClient::new(options, REQUEST_CAPACITY);
    Ok((
        MqttJobPublisher {
            client: client.clone(),
        },
        MqttEvents { client, eventloop },
    ))
}

/// The IoT rule event for an MQTT message: the payload plus `printerId` taken from the topic.
/// `None` for topics that are not status or heartbeat topics and for non-JSON payloads.
pub fn rule_event(topic: &str, payload: &[u8]) -> Option<Value> {
    let (printer_id, kind) = parse_topic(topic).ok()?;
    if !matches!(kind, TopicKind::Status | TopicKind::Heartbeat) {
        return None;
    }
    let mut event: Value = serde_json::from_slice(payload).ok()?;
    event
        .as_object_mut()?
        .insert("printerId".into(), Value::String(printer_id.to_string()));
    Some(event)
}

impl MqttEvents {
    /// Drive the connection forever: subscribe after every (re)connect and apply each status and
    /// heartbeat to `service`. Connection errors are logged and retried.
    pub async fn run(mut self, service: OrderService) {
        loop {
            match self.eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    for filter in SUBSCRIPTIONS {
                        if let Err(error) = self.client.try_subscribe(filter, QoS::AtLeastOnce) {
                            tracing::error!(%error, filter, "MQTT subscribe failed");
                        }
                    }
                }
                Ok(Event::Incoming(Packet::Publish(message))) => {
                    match rule_event(&message.topic, &message.payload) {
                        Some(event) => {
                            if let Err(error) = handle_event(&service, event).await {
                                tracing::error!(%error, topic = %message.topic, "MQTT event failed");
                            }
                        }
                        None => tracing::warn!(topic = %message.topic, "ignoring an MQTT message"),
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "MQTT connection error, retrying");
                    tokio::time::sleep(RECONNECT_DELAY).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn broker_urls() {
        assert_eq!(
            parse_broker_url("mqtt://127.0.0.1:1884"),
            Ok(("127.0.0.1".into(), 1884))
        );
        assert_eq!(
            parse_broker_url("tcp://broker:1883/"),
            Ok(("broker".into(), 1883))
        );
        assert_eq!(
            parse_broker_url("mqtt://broker"),
            Ok(("broker".into(), 1883))
        );
        for bad in [
            "http://x:1",
            "mqtt://",
            "mqtt://:1883",
            "mqtt://h:notaport",
            "mqtt://h/path:1",
            "x",
        ] {
            assert!(
                matches!(parse_broker_url(bad), Err(CoreError::Internal(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn rule_events_carry_the_printer_id_of_the_topic() {
        let event = rule_event(
            "fotoko/printers/stand-001/status",
            br#"{"jobId":"j","printerId":"spoofed"}"#,
        )
        .expect("event");
        assert_eq!(event["printerId"], "stand-001");
        assert_eq!(event["jobId"], "j");
        let hb = rule_event(
            "fotoko/printers/stand-001/heartbeat",
            br#"{"printerState":"idle"}"#,
        );
        assert_eq!(hb.expect("event")["printerId"], "stand-001");
    }

    #[test]
    fn other_messages_are_not_rule_events() {
        assert_eq!(rule_event("fotoko/printers/stand-001/jobs", b"{}"), None);
        assert_eq!(rule_event("other/topic", b"{}"), None);
        assert_eq!(
            rule_event("fotoko/printers/stand-001/status", b"not json"),
            None
        );
        assert_eq!(rule_event("fotoko/printers/stand-001/status", b"[1]"), None);
        assert_eq!(
            rule_event(
                "fotoko/printers/stand-001/status",
                json!("x").to_string().as_bytes()
            ),
            None
        );
    }

    #[test]
    fn connect_validates_the_url() {
        assert!(connect("mqtt://127.0.0.1:1883", "c").is_ok());
        assert!(connect("nope", "c").is_err());
    }
}
