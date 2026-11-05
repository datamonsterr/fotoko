//! Fotoko domain: orders, the status machine, pricing, payOS signing, ports and the
//! [`OrderService`] use cases. No IO: AWS, HTTP and MQTT adapters live in `fotoko-api`; the
//! in-memory adapters in [`testing`] (feature `testing`) back tests and the simulation e2e.

pub mod domain;
pub mod error;
pub mod payos;
pub mod ports;
pub mod service;
pub mod state;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use domain::{Order, Photo, Printer};
pub use error::CoreError;
pub use ports::{
    Clock, IdGen, JobPublisher, ObjectStore, OrderRepo, PaymentGateway, PaymentLink,
    PaymentLinkRequest, PresignedPut, PrinterRepo, PutSpec, SystemClock,
};
pub use service::{OrderService, Ports, ServiceConfig, StatusOutcome, WebhookOutcome};
pub use state::{OrderEvent, transition};
