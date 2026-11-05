//! A direct MQTT client on the broker: records every job, status and heartbeat that flows by
//! and publishes jobs the API would never send.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fotoko_protocol::{Heartbeat, JobStatus, PrintJob};
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const FILTERS: [&str; 3] = [
    "fotoko/printers/+/jobs",
    "fotoko/printers/+/status",
    "fotoko/printers/+/heartbeat",
];
const SENTINEL: &[u8] = br#"{"e2eBarrier":true}"#;

#[derive(Default)]
struct Seen {
    jobs: Vec<PrintJob>,
    statuses: Vec<JobStatus>,
    heartbeats: Vec<Heartbeat>,
    barriers: u32,
}

/// Recording MQTT client.
pub struct Cloud {
    client: AsyncClient,
    seen: Arc<Mutex<Seen>>,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for Cloud {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cloud").finish_non_exhaustive()
    }
}

impl Drop for Cloud {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Cloud {
    /// Connect to the broker on `port` and wait until the three subscriptions are active.
    pub async fn connect(port: u16) -> Self {
        let mut options = MqttOptions::new("e2e-cloud-tap", "127.0.0.1", port);
        options.set_keep_alive(Duration::from_secs(5));
        let (client, mut eventloop) = AsyncClient::new(options, 64);
        for filter in FILTERS {
            client
                .subscribe(filter, QoS::AtLeastOnce)
                .await
                .expect("subscribe");
        }
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (ready_tx, ready_rx) = oneshot::channel();
        let mut ready_tx = Some(ready_tx);
        let record = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            let mut acks = 0;
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Incoming::SubAck(_))) => {
                        acks += 1;
                        if acks == FILTERS.len()
                            && let Some(tx) = ready_tx.take()
                        {
                            let _ = tx.send(());
                        }
                    }
                    Ok(Event::Incoming(Incoming::Publish(message))) => {
                        let mut seen = record.lock().expect("seen lock");
                        if message.payload.as_ref() == SENTINEL {
                            seen.barriers += 1;
                        } else if message.topic.ends_with("/jobs") {
                            if let Ok(job) = serde_json::from_slice(&message.payload) {
                                seen.jobs.push(job);
                            }
                        } else if message.topic.ends_with("/status") {
                            if let Ok(status) = serde_json::from_slice(&message.payload) {
                                seen.statuses.push(status);
                            }
                        } else if let Ok(beat) = serde_json::from_slice(&message.payload) {
                            seen.heartbeats.push(beat);
                        }
                    }
                    Ok(_) => {}
                    Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                }
            }
        });
        tokio::time::timeout(Duration::from_secs(10), ready_rx)
            .await
            .expect("the tap subscribed in time")
            .expect("tap task alive");
        Self { client, seen, task }
    }

    /// Every job published so far.
    pub fn jobs(&self) -> Vec<PrintJob> {
        self.seen.lock().expect("seen lock").jobs.clone()
    }

    /// Every job status published so far.
    pub fn statuses(&self) -> Vec<JobStatus> {
        self.seen.lock().expect("seen lock").statuses.clone()
    }

    /// Every heartbeat published so far.
    pub fn heartbeats(&self) -> Vec<Heartbeat> {
        self.seen.lock().expect("seen lock").heartbeats.clone()
    }

    /// Publish `job` on the jobs topic of `printer_id`, whatever the job's own `printerId` says.
    pub async fn publish_job_on(&self, printer_id: &str, job: &PrintJob) {
        self.client
            .publish(
                format!("fotoko/printers/{printer_id}/jobs"),
                QoS::AtLeastOnce,
                false,
                serde_json::to_vec(job).expect("serialize job"),
            )
            .await
            .expect("publish job");
    }

    /// Wait until a marker published now has come back through the broker. Messages the API
    /// queued before the caller got its HTTP response are in the tap by then, which lets a test
    /// assert that something was *not* published without sleeping.
    pub async fn barrier(&self) {
        let before = self.seen.lock().expect("seen lock").barriers;
        self.client
            .publish(
                "fotoko/printers/e2e-barrier/jobs",
                QoS::AtLeastOnce,
                false,
                SENTINEL,
            )
            .await
            .expect("publish barrier");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while self.seen.lock().expect("seen lock").barriers == before {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the barrier did not come back"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
