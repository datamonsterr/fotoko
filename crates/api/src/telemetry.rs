//! JSON logging for the binaries.

use tracing_subscriber::EnvFilter;

/// Install a JSON subscriber on stdout. `RUST_LOG` overrides the `info` default. Safe to call
/// twice: the second call is a no-op.
pub fn init_json() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // An already installed subscriber is fine; there is nothing to report.
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_current_span(true)
        .try_init();
}

#[cfg(test)]
mod tests {
    #[test]
    fn init_twice_is_harmless() {
        super::init_json();
        super::init_json();
    }
}
