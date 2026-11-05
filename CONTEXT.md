# Fotoko vocabulary

| Term               | Meaning                                                                                                                                                   |
| ------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Stand              | Physical kiosk: one Printer + one Pi, a QR sticker with its `printer_id`.                                                                                 |
| Printer            | Photo printer driven by CUPS + Gutenprint, wired (USB) to the Pi. Never on the network.                                                                   |
| Pi / Pi agent      | Raspberry Pi running `fotoko-pi-agent`: MQTT client, downloads job images, prints via CUPS, reports status.                                               |
| Printer sim        | `fotoko-printer-sim`: simulated Gutenprint/CUPS printer used in tests, CI and local dev. Writes rendered pages to disk and can inject faults.             |
| Order              | One customer purchase at one Stand: N photos x copies, a price, a payOS payment.                                                                          |
| Order code         | payOS numeric `orderCode` (i64, unique), mapped 1:1 to an Order.                                                                                          |
| Photo              | One uploaded image (S3 object key) inside an Order.                                                                                                       |
| Print job          | MQTT message sent to a Pi after an Order is Paid; contains presigned GET URLs.                                                                            |
| Order status       | `awaiting_upload -> awaiting_payment -> paid -> printing -> completed`, or `failed`, `cancelled`, `expired`.                                              |
| Guest session      | Cognito identity pool unauthenticated identity; no login needed at a Stand.                                                                               |
| Device certificate | Per-Pi X.509 cert registered in AWS IoT Core; IoT policy limits it to its own topics.                                                                     |
| Ports / adapters   | `fotoko-core` traits (`OrderRepo`, `ObjectStore`, `JobPublisher`, `PaymentGateway`, `Clock`); AWS adapters in `fotoko-api`, in-memory adapters for tests. |
| Simulation e2e     | Full flow in one process or compose: API + embedded MQTT broker + payOS mock + Pi agent + Printer sim.                                                    |
