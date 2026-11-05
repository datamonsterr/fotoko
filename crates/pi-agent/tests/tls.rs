//! mTLS smoke test: the agent connects to a TLS server that demands a client certificate (as
//! AWS IoT does), using a CA, device certificate and key read from files. Certificates are
//! generated per run with ECDSA (cheap), signed by a throwaway CA.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use common::WAIT;
use fotoko_pi_agent::{Agent, Config, SimBackend};
use fotoko_printer_sim::{SimConfig, SimPrinter};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, SanType,
};
use rustls::RootCertStore;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;

struct Pki {
    ca_pem: String,
    server_cert: String,
    server_key: String,
    client_cert: String,
    client_key: String,
}

fn pki() -> Pki {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "fotoko test ca");
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(ca_params, &ca_key);

    let server_key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(Vec::new()).unwrap();
    params.subject_alt_names = vec![SanType::IpAddress("127.0.0.1".parse().unwrap())];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server = params.signed_by(&server_key, &issuer).unwrap();

    let client_key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(Vec::new()).unwrap();
    params
        .distinguished_name
        .push(DnType::CommonName, "stand-001");
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client = params.signed_by(&client_key, &issuer).unwrap();

    Pki {
        ca_pem: ca.pem(),
        server_cert: server.pem(),
        server_key: server_key.serialize_pem(),
        client_cert: client.pem(),
        client_key: client_key.serialize_pem(),
    }
}

fn write(dir: &Path, name: &str, body: &str, mode: u32) -> String {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path.to_string_lossy().into_owned()
}

fn acceptor(pki: &Pki) -> TlsAcceptor {
    fotoko_pi_agent::tls::install_crypto_provider();
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(pki.ca_pem.as_bytes()).unwrap())
        .unwrap();
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .unwrap();
    let chain = CertificateDer::pem_slice_iter(pki.server_cert.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_slice(pki.server_key.as_bytes()).unwrap();
    let config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)
        .unwrap();
    TlsAcceptor::from(Arc::new(config))
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[tokio::test]
async fn agent_connects_with_client_certificate_and_sends_last_will() {
    let pki = pki();
    let dir = tempfile::tempdir().unwrap();
    let ca = write(dir.path(), "ca.pem", &pki.ca_pem, 0o644);
    let cert = write(dir.path(), "device.pem.crt", &pki.client_cert, 0o644);
    let key = write(dir.path(), "private.pem.key", &pki.client_key, 0o600);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = acceptor(&pki);
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut tls = acceptor.accept(tcp).await.expect("mTLS handshake");
        let mut connect = vec![0u8; 4096];
        let n = tls.read(&mut connect).await.unwrap();
        connect.truncate(n);
        tls.write_all(&[0x20, 0x02, 0x00, 0x00]).await.unwrap(); // CONNACK, accepted
        let mut after = vec![0u8; 4096];
        let n = tls.read(&mut after).await.unwrap();
        after.truncate(n);
        (connect, after, tls)
    });

    let config = Config::from_toml(&format!(
        r#"
printer_id = "stand-001"
[mqtt]
host = "127.0.0.1"
port = {port}
ca = "{ca}"
cert = "{cert}"
key = "{key}"
[printer]
backend = "sim"
[sim]
out_dir = "{state}/sim"
[storage]
state_dir = "{state}"
"#,
        state = dir.path().display()
    ))
    .unwrap();
    config.check_files().unwrap();
    let sim = Arc::new(SimPrinter::new(SimConfig::new(dir.path().join("sim"))).unwrap());
    let agent = Agent::new(config, Arc::new(SimBackend::new(sim)));
    let running = tokio::spawn(agent.run(std::future::pending::<()>()));

    let (connect, after, _tls) = timeout(WAIT, server).await.expect("no TLS client").unwrap();
    assert!(contains(&connect, "stand-001"), "client id");
    assert!(
        contains(&connect, "fotoko/printers/stand-001/heartbeat"),
        "last will topic"
    );
    assert!(
        contains(&connect, "\"printerState\":\"offline\""),
        "last will payload"
    );
    assert!(
        contains(&after, "fotoko/printers/stand-001/jobs"),
        "subscribes to jobs"
    );
    running.abort();
}

#[tokio::test]
async fn agent_rejects_a_server_certificate_from_an_unknown_ca() {
    let trusted = pki();
    let other = pki();
    let dir = tempfile::tempdir().unwrap();
    // The agent trusts `trusted`, the server presents a certificate from `other`.
    let ca = write(dir.path(), "ca.pem", &trusted.ca_pem, 0o644);
    let cert = write(dir.path(), "device.pem.crt", &trusted.client_cert, 0o644);
    let key = write(dir.path(), "private.pem.key", &trusted.client_key, 0o600);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = acceptor(&other);
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        acceptor.accept(tcp).await.is_err()
    });
    let config = Config::from_toml(&format!(
        "printer_id = \"stand-001\"\n[mqtt]\nhost = \"127.0.0.1\"\nport = {port}\nca = \"{ca}\"\ncert = \"{cert}\"\nkey = \"{key}\"\n[printer]\nbackend = \"sim\"\n[sim]\nout_dir = \"{state}/sim\"\n[storage]\nstate_dir = \"{state}\"\n",
        state = dir.path().display()
    ))
    .unwrap();
    let sim = Arc::new(SimPrinter::new(SimConfig::new(dir.path().join("sim"))).unwrap());
    let running = tokio::spawn(
        Agent::new(config, Arc::new(SimBackend::new(sim))).run(std::future::pending::<()>()),
    );
    assert!(
        timeout(WAIT, server).await.expect("no TLS client").unwrap(),
        "handshake must fail"
    );
    running.abort();
}
