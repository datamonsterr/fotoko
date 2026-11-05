//! Embedded MQTT broker (`rumqttd`) for the simulation.

use std::net::TcpListener;
use std::time::Duration;

use serde_json::json;

/// Largest MQTT payload the broker accepts.
const MAX_PAYLOAD: u32 = 262_144;

/// Broker configuration listening on `listen` (`host:port`), plain MQTT 3.1.1, no auth.
pub fn config(listen: &str) -> rumqttd::Config {
    serde_json::from_value(json!({
        "id": 0,
        "router": {
            "max_connections": 100,
            "max_outgoing_packet_count": 200,
            "max_segment_size": 1_048_576,
            "max_segment_count": 10
        },
        "v4": {"1": {
            "name": "v4-1",
            "listen": listen,
            "next_connection_delay_ms": 1,
            "connections": {
                "connection_timeout_ms": 60_000,
                "max_payload_size": MAX_PAYLOAD,
                "max_inflight_count": 100,
                "dynamic_filters": true
            }
        }}
    }))
    .expect("valid broker config")
}

/// Run a broker on `listen` in the calling thread until the process ends. Returns only when
/// the broker stops (for example because the address is taken).
pub fn run(listen: &str) -> Result<(), String> {
    let mut broker = rumqttd::Broker::new(config(listen));
    broker.start().map_err(|error| error.to_string())
}

/// Start a broker on a free loopback port in a background thread and return the port once it
/// accepts connections. The port is probed first; if another process takes it before the broker
/// binds, the start is retried on a new port. The broker lives until the process ends.
pub fn start_on_free_port() -> u16 {
    for _ in 0..10 {
        let port = {
            let probe = TcpListener::bind("127.0.0.1:0").expect("probe port");
            probe.local_addr().expect("probe addr").port()
        };
        let listen = format!("127.0.0.1:{port}");
        // `start` only returns when the broker stopped, which at this point means a bind error.
        let (stopped_tx, stopped_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = run(&listen);
            let _ = stopped_tx.send(());
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return port;
            }
            if stopped_rx.try_recv().is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    panic!("the MQTT broker did not start");
}
