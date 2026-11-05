//! Simulated Gutenprint/CUPS printer for tests, CI and local development.
//!
//! - [`SimPrinter`]: decodes JPEG/PNG, fits it to 4x6 @300dpi and writes one PNG per copy, with
//!   a paper counter and injectable faults.
//! - [`fake_cups`]: `lp`, `lpstat` and `cancel` behaviour over a persisted state directory,
//!   exposed as the `fake-lp`, `fake-lpstat` and `fake-cancel` binaries.

pub mod cups_cli;
pub mod fake_cups;
pub mod fit;
pub mod sim;

pub use sim::{Fault, ParseFaultError, SimConfig, SimError, SimPrinter, SimState, decode};
