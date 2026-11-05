//! The local server against an embedded `rumqttd` broker: the paid job reaches the Pi topic and
//! Pi statuses and heartbeats flow back into the orders and printers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::net::TcpListener;
use std::time::Duration;

use common::{CHECKSUM_KEY, eventually, stack};
use fotoko_core::testing::signed_webhook;
use fotoko_protocol::{OrderId, OrderStatus, PrintJob, PrinterId, PrinterStatus};
use reqwest::Method;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde_json::json;

/// Start a broker on a free port and return that port.
fn start_broker() -> u16 {
    let port = {
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let config: rumqttd::Config = serde_json::from_value(json!({
        "id": 0,
        "router": {
            "max_connections": 100,
            "max_outgoing_packet_count": 200,
            "max_segment_size": 1_048_576,
            "max_segment_count": 10
        },
        "v4": {"1": {
            "name": "v4-1",
            "listen": format!("127.0.0.1:{port}"),
            "next_connection_delay_ms": 1,
            "connections": {
                "connection_timeout_ms": 60_000,
                "max_payload_size": 65_536,
                "max_inflight_count": 100,
                "dynamic_filters": true
            }
        }}
    }))
    .unwrap();
    std::thread::spawn(move || {
        let mut broker = rumqttd::Broker::new(config);
        let _ = broker.start();
    });
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return port;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("broker did not start");
}

#[tokio::test]
async fn paid_job_is_published_and_pi_reports_flow_back() {
    let port = start_broker();
    let s = stack(|b| b.mqtt_url(Some(format!("mqtt://127.0.0.1:{port}")))).await;

    // A stand-in Pi: subscribes to its jobs topic and later reports status.
    let mut options = MqttOptions::new("pi-stand-001", "127.0.0.1", port);
    options.set_keep_alive(Duration::from_secs(5));
    let (pi, mut pi_loop) = AsyncClient::new(options, 16);
    pi.subscribe("fotoko/printers/stand-001/jobs", QoS::AtLeastOnce)
        .await
        .unwrap();
    let (jobs_tx, mut jobs_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (subscribed_tx, subscribed_rx) = tokio::sync::oneshot::channel::<()>();
    let mut subscribed_tx = Some(subscribed_tx);
    tokio::spawn(async move {
        loop {
            match pi_loop.poll().await {
                Ok(Event::Incoming(Packet::SubAck(_))) => {
                    if let Some(tx) = subscribed_tx.take() {
                        let _ = tx.send(());
                    }
                }
                Ok(Event::Incoming(Packet::Publish(p))) => {
                    let _ = jobs_tx.send(p.payload.to_vec());
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    });
    tokio::time::timeout(Duration::from_secs(10), subscribed_rx)
        .await
        .unwrap()
        .unwrap();

    // Customer flow up to payment.
    let created = s.order_with_uploads("guest-1").await;
    let order_id = created["orderId"].as_str().unwrap().to_owned();
    let (_, checkout) = s
        .call(
            Method::POST,
            &format!("/v1/orders/{order_id}/checkout"),
            Some("guest-1"),
            None,
        )
        .await;
    let order_code = checkout["orderCode"].as_i64().unwrap();
    let hook = signed_webhook(CHECKSUM_KEY, "00", order_code, 30_000).unwrap();
    let (status, _) = s
        .call(
            Method::POST,
            "/v1/payments/payos/webhook",
            None,
            Some(serde_json::to_value(hook).unwrap()),
        )
        .await;
    assert_eq!(status, 200);

    // The job arrives on the Pi's topic with downloadable photos.
    let payload = tokio::time::timeout(Duration::from_secs(10), jobs_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let job: PrintJob = serde_json::from_slice(&payload).unwrap();
    assert_eq!(job.order_id.as_str(), order_id);
    assert_eq!(job.printer_id.as_str(), "stand-001");
    assert_eq!(job.photos.len(), 2);
    let photo = s.http.get(&job.photos[0].url).send().await.unwrap();
    assert_eq!(photo.bytes().await.unwrap().as_ref(), b"jpeg-bytes!");

    // Heartbeat feeds the printer; retried until the app's subscription is live.
    let stand = PrinterId::new("stand-001").unwrap();
    eventually("the heartbeat to reach the printer", || async {
        pi.publish(
            "fotoko/printers/stand-001/heartbeat",
            QoS::AtLeastOnce,
            false,
            json!({"printerId": "stand-001", "printerState": "error", "agentVersion": "t",
                   "at": "2026-01-01T00:00:00Z"})
            .to_string(),
        )
        .await
        .unwrap();
        s.app.printers.snapshot(&stand).unwrap().status == PrinterStatus::Error
    })
    .await;

    // Status reports move the order along.
    let id = OrderId::from(order_id.as_str());
    for (state, expected) in [
        ("received", OrderStatus::Printing),
        ("completed", OrderStatus::Completed),
    ] {
        eventually(state, || async {
            pi.publish(
                "fotoko/printers/stand-001/status",
                QoS::AtLeastOnce,
                false,
                json!({"jobId": job.job_id, "orderId": order_id, "printerId": "stand-001",
                       "state": state, "printedCopies": 3, "at": "2026-01-01T00:01:00Z"})
                .to_string(),
            )
            .await
            .unwrap();
            s.app.orders.snapshot(&id).unwrap().status == expected
        })
        .await;
    }
    let (_, order) = s
        .call(
            Method::GET,
            &format!("/v1/orders/{order_id}"),
            Some("guest-1"),
            None,
        )
        .await;
    assert_eq!(order["status"], "completed");
}
