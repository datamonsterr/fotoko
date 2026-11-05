//! Process-wide rustls setup. The agent uses the `ring` provider only (no C toolchain beyond
//! what `ring` needs, friendly to cross-compiling for the Pi).

/// Install the `ring` crypto provider as the process default. Safe to call repeatedly; if some
/// other provider was installed first it is left alone.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent_and_sets_a_default() {
        install_crypto_provider();
        install_crypto_provider();
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
}
