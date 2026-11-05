//! Agent configuration (`/etc/fotoko/agent.toml`). See `docs/contracts.md`.
//!
//! Beyond the contract's keys, a few optional tuning keys exist (all with defaults):
//! `mqtt.keep_alive_secs`, `printer.cancel_bin`, `printer.print_timeout_secs`,
//! `printer.poll_interval_ms`, `sim.latency_ms` and the whole `[agent]` section.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fotoko_printer_sim::Fault;
use fotoko_protocol::PrinterId;
use serde::Deserialize;
use thiserror::Error;

/// Configuration problem.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The config file cannot be read.
    #[error("cannot read config {path}: {source}")]
    Read {
        /// Config path.
        path: PathBuf,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// The TOML is malformed or has unknown or mistyped keys.
    #[error("invalid config: {0}")]
    Parse(#[from] toml::de::Error),
    /// A value is missing or out of range.
    #[error("invalid config: {field}: {reason}")]
    Invalid {
        /// Dotted key.
        field: &'static str,
        /// What is wrong.
        reason: String,
    },
    /// A file named in the config does not exist.
    #[error("{field}: file not found: {path}")]
    FileMissing {
        /// Dotted key.
        field: &'static str,
        /// The path.
        path: PathBuf,
    },
    /// The private key is accessible to group or others.
    #[error(
        "{path}: private key must not be accessible to group or others (mode {mode:04o}), run: chmod 600"
    )]
    KeyPermissions {
        /// Key path.
        path: PathBuf,
        /// Current mode bits.
        mode: u32,
    },
}

fn invalid(field: &'static str, reason: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        field,
        reason: reason.into(),
    }
}

/// `[mqtt]`
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MqttConfig {
    /// Broker host.
    pub host: String,
    /// Broker port; default 8883 with TLS, 1883 without.
    pub port: Option<u16>,
    /// CA bundle (PEM). Required with TLS.
    pub ca: Option<PathBuf>,
    /// Device certificate (PEM). Required with TLS.
    pub cert: Option<PathBuf>,
    /// Device private key (PEM, mode 0600). Required with TLS.
    pub key: Option<PathBuf>,
    /// Use TLS with client certificates (AWS IoT) instead of plain TCP.
    #[serde(default = "default_true")]
    pub tls: bool,
    /// MQTT keep alive in seconds.
    #[serde(default = "default_keep_alive")]
    pub keep_alive_secs: u64,
}

impl MqttConfig {
    /// Effective port.
    pub fn port(&self) -> u16 {
        self.port.unwrap_or(if self.tls { 8883 } else { 1883 })
    }
}

/// Which printer implementation to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// CUPS through `lp` and `lpstat`.
    Cups,
    /// In-process printer simulation.
    Sim,
}

/// `[printer]`
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrinterConfig {
    /// Printer implementation.
    pub backend: Backend,
    /// CUPS queue.
    #[serde(default = "default_queue")]
    pub queue: String,
    /// CUPS `media` option for 4x6.
    #[serde(default = "default_media")]
    pub media: String,
    /// `lp` executable.
    #[serde(default = "default_lp")]
    pub lp_bin: String,
    /// `lpstat` executable.
    #[serde(default = "default_lpstat")]
    pub lpstat_bin: String,
    /// `cancel` executable, used to withdraw a job that failed.
    #[serde(default = "default_cancel")]
    pub cancel_bin: String,
    /// Give up on a CUPS job after this many seconds.
    #[serde(default = "default_print_timeout")]
    pub print_timeout_secs: u64,
    /// Delay between `lpstat` polls in milliseconds.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_ms: u64,
}

/// `[sim]`
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimSection {
    /// Where rendered pages go.
    pub out_dir: PathBuf,
    /// Sheets before `paper_out`.
    #[serde(default = "default_paper")]
    pub paper: u32,
    /// Injected fault: `""`, `offline`, `ribbon_out` or `driver_error`.
    #[serde(default)]
    pub fail: Fault,
    /// Milliseconds each page takes.
    #[serde(default)]
    pub latency_ms: u64,
}

/// `[storage]`
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    /// Directory for the dedupe file and temporary downloads.
    pub state_dir: PathBuf,
    /// Allow `http://` image URLs (simulation only).
    #[serde(default)]
    pub allow_insecure_http: bool,
}

/// `[agent]`
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSection {
    /// Heartbeat period in seconds.
    #[serde(default = "default_heartbeat")]
    pub heartbeat_secs: u64,
    /// Whole-download timeout per image in seconds.
    #[serde(default = "default_download_timeout")]
    pub download_timeout_secs: u64,
    /// On shutdown, wait this long for the running job before abandoning it.
    #[serde(default = "default_shutdown_grace")]
    pub shutdown_grace_secs: u64,
}

impl Default for AgentSection {
    fn default() -> Self {
        Self {
            heartbeat_secs: default_heartbeat(),
            download_timeout_secs: default_download_timeout(),
            shutdown_grace_secs: default_shutdown_grace(),
        }
    }
}

/// Whole agent configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// This stand's printer id; also the MQTT client id.
    pub printer_id: PrinterId,
    /// `[mqtt]`
    pub mqtt: MqttConfig,
    /// `[printer]`
    pub printer: PrinterConfig,
    /// `[sim]`, required when `printer.backend = "sim"`.
    pub sim: Option<SimSection>,
    /// `[storage]`
    pub storage: StorageConfig,
    /// `[agent]`
    #[serde(default)]
    pub agent: AgentSection,
}

fn default_true() -> bool {
    true
}
fn default_keep_alive() -> u64 {
    30
}
fn default_queue() -> String {
    "fotoko".to_owned()
}
fn default_media() -> String {
    "w288h432".to_owned()
}
fn default_lp() -> String {
    "lp".to_owned()
}
fn default_lpstat() -> String {
    "lpstat".to_owned()
}
fn default_cancel() -> String {
    "cancel".to_owned()
}
fn default_print_timeout() -> u64 {
    300
}
fn default_poll_interval() -> u64 {
    1000
}
fn default_paper() -> u32 {
    50
}
fn default_heartbeat() -> u64 {
    30
}
fn default_download_timeout() -> u64 {
    60
}
fn default_shutdown_grace() -> u64 {
    20
}

/// A CUPS queue or media name: ASCII alphanumerics, `_`, `-`, `.`; not starting with `-`.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('-')
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

impl Config {
    /// Parse and validate TOML. Does not touch the filesystem; see [`Config::check_files`].
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    /// Read, parse, validate and check the files a config names.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let config = Self::from_toml(&text)?;
        config.check_files()?;
        Ok(config)
    }

    /// Value checks that need no filesystem.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.mqtt.host.trim().is_empty() {
            return Err(invalid("mqtt.host", "must not be empty"));
        }
        if self.mqtt.port == Some(0) {
            return Err(invalid("mqtt.port", "must not be 0"));
        }
        if self.mqtt.keep_alive_secs == 0 {
            return Err(invalid("mqtt.keep_alive_secs", "must be at least 1"));
        }
        if self.mqtt.tls {
            for (field, value) in [
                ("mqtt.ca", &self.mqtt.ca),
                ("mqtt.cert", &self.mqtt.cert),
                ("mqtt.key", &self.mqtt.key),
            ] {
                if value.is_none() {
                    return Err(invalid(field, "required when mqtt.tls = true"));
                }
            }
        }
        if !is_token(&self.printer.queue) {
            return Err(invalid(
                "printer.queue",
                "must be letters, digits, '_', '-' or '.'",
            ));
        }
        if !is_token(&self.printer.media) {
            return Err(invalid(
                "printer.media",
                "must be letters, digits, '_', '-' or '.'",
            ));
        }
        for (field, value) in [
            ("printer.lp_bin", &self.printer.lp_bin),
            ("printer.lpstat_bin", &self.printer.lpstat_bin),
            ("printer.cancel_bin", &self.printer.cancel_bin),
        ] {
            if value.trim().is_empty() {
                return Err(invalid(field, "must not be empty"));
            }
        }
        if self.printer.print_timeout_secs == 0 {
            return Err(invalid("printer.print_timeout_secs", "must be at least 1"));
        }
        if self.printer.poll_interval_ms == 0 {
            return Err(invalid("printer.poll_interval_ms", "must be at least 1"));
        }
        if self.printer.backend == Backend::Sim && self.sim.is_none() {
            return Err(invalid(
                "sim",
                "section required when printer.backend = \"sim\"",
            ));
        }
        if self.storage.state_dir.as_os_str().is_empty() {
            return Err(invalid("storage.state_dir", "must not be empty"));
        }
        for (field, value) in [
            ("agent.heartbeat_secs", self.agent.heartbeat_secs),
            (
                "agent.download_timeout_secs",
                self.agent.download_timeout_secs,
            ),
        ] {
            if value == 0 {
                return Err(invalid(field, "must be at least 1"));
            }
        }
        Ok(())
    }

    /// With TLS, the CA, certificate and key must exist and the key must be private (no group
    /// or other permission bits; 0600 or stricter).
    pub fn check_files(&self) -> Result<(), ConfigError> {
        if !self.mqtt.tls {
            return Ok(());
        }
        for (field, value) in [
            ("mqtt.ca", &self.mqtt.ca),
            ("mqtt.cert", &self.mqtt.cert),
            ("mqtt.key", &self.mqtt.key),
        ] {
            let path = value
                .as_ref()
                .ok_or_else(|| invalid(field, "required when mqtt.tls = true"))?;
            if !path.is_file() {
                return Err(ConfigError::FileMissing {
                    field,
                    path: path.clone(),
                });
            }
        }
        if let Some(key) = &self.mqtt.key {
            check_key_permissions(key)?;
        }
        Ok(())
    }

    /// Heartbeat period.
    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(self.agent.heartbeat_secs)
    }
}

#[cfg(unix)]
fn check_key_permissions(path: &Path) -> Result<(), ConfigError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        return Err(ConfigError::KeyPermissions {
            path: path.to_owned(),
            mode,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_key_permissions(_path: &Path) -> Result<(), ConfigError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
printer_id = "stand-001"
[mqtt]
host = "xxxx-ats.iot.ap-southeast-1.amazonaws.com"
port = 8883
ca = "/etc/fotoko/AmazonRootCA1.pem"
cert = "/etc/fotoko/device.pem.crt"
key = "/etc/fotoko/private.pem.key"
tls = true
[printer]
backend = "cups"
queue = "fotoko"
media = "w288h432"
lp_bin = "lp"
lpstat_bin = "lpstat"
[sim]
out_dir = "/var/lib/fotoko/sim-out"
paper = 50
fail = ""
[storage]
state_dir = "/var/lib/fotoko"
allow_insecure_http = false
"#;

    const LOCAL: &str = r#"
printer_id = "stand-001"
[mqtt]
host = "127.0.0.1"
tls = false
[printer]
backend = "sim"
[sim]
out_dir = "/tmp/out"
[storage]
state_dir = "/tmp/state"
"#;

    fn with(text: &str, from: &str, to: &str) -> String {
        assert!(text.contains(from), "{from} not in text");
        text.replacen(from, to, 1)
    }

    fn err(text: &str) -> String {
        Config::from_toml(text).unwrap_err().to_string()
    }

    #[test]
    fn parses_contract_example() {
        let c = Config::from_toml(FULL).unwrap();
        assert_eq!(c.printer_id.as_str(), "stand-001");
        assert_eq!(c.mqtt.port(), 8883);
        assert!(c.mqtt.tls);
        assert_eq!(c.mqtt.keep_alive_secs, 30);
        assert_eq!(c.printer.backend, Backend::Cups);
        assert_eq!(c.printer.media, "w288h432");
        assert_eq!(c.printer.cancel_bin, "cancel");
        assert_eq!(c.printer.print_timeout_secs, 300);
        assert_eq!(c.printer.poll_interval_ms, 1000);
        let sim = c.sim.as_ref().unwrap();
        assert_eq!((sim.paper, sim.fail, sim.latency_ms), (50, Fault::None, 0));
        assert!(!c.storage.allow_insecure_http);
        assert_eq!(c.agent, AgentSection::default());
        assert_eq!(c.heartbeat_interval(), Duration::from_secs(30));
    }

    #[test]
    fn parses_minimal_local_config_with_defaults() {
        let c = Config::from_toml(LOCAL).unwrap();
        assert!(!c.mqtt.tls);
        assert_eq!(c.mqtt.port(), 1883);
        assert_eq!(c.printer.queue, "fotoko");
        assert_eq!(c.printer.lp_bin, "lp");
        assert_eq!(c.printer.lpstat_bin, "lpstat");
        assert_eq!(c.sim.unwrap().paper, 50);
    }

    #[test]
    fn tls_defaults_to_true_and_port_to_8883() {
        let text = with(
            &with(LOCAL, "tls = false\n", ""),
            "[printer]",
            "ca = \"a\"\ncert = \"b\"\nkey = \"c\"\n[printer]",
        );
        // The extra keys land in [mqtt] because [printer] follows it.
        let c = Config::from_toml(&text).unwrap();
        assert!(c.mqtt.tls);
        assert_eq!(c.mqtt.port(), 8883);
    }

    #[test]
    fn explicit_port_wins() {
        let c = Config::from_toml(&with(LOCAL, "tls = false", "tls = false\nport = 1884")).unwrap();
        assert_eq!(c.mqtt.port(), 1884);
    }

    #[test]
    fn sim_fail_values() {
        let c = Config::from_toml(&with(
            LOCAL,
            "[storage]",
            "fail = \"ribbon_out\"\nlatency_ms = 5\n[storage]",
        ))
        .unwrap();
        let sim = c.sim.unwrap();
        assert_eq!((sim.fail, sim.latency_ms), (Fault::RibbonOut, 5));
        assert!(
            err(&with(LOCAL, "[storage]", "fail = \"explode\"\n[storage]")).contains("explode")
        );
    }

    #[test]
    fn parse_errors() {
        assert!(err("not toml [[[").starts_with("invalid config:"));
        assert!(err(&with(LOCAL, "printer_id = \"stand-001\"", "")).contains("printer_id"));
        assert!(err(&with(LOCAL, "stand-001", "a/b")).contains("reserved"));
        assert!(err(&with(LOCAL, "stand-001", "")).contains("empty"));
        assert!(err(&with(LOCAL, "[storage]", "mystery = 1\n[storage]")).contains("mystery"));
        assert!(err(&with(LOCAL, "\"sim\"", "\"laser\"")).contains("laser"));
    }

    #[test]
    fn mqtt_validation() {
        assert!(err(&with(LOCAL, "127.0.0.1", " ")).contains("mqtt.host"));
        assert!(err(&with(LOCAL, "tls = false", "tls = false\nport = 0")).contains("mqtt.port"));
        assert!(
            err(&with(
                LOCAL,
                "tls = false",
                "tls = false\nkeep_alive_secs = 0"
            ))
            .contains("keep_alive")
        );
    }

    #[test]
    fn tls_requires_ca_cert_and_key() {
        for missing in ["ca", "cert", "key"] {
            let text: String = FULL
                .lines()
                .filter(|l| !l.starts_with(&format!("{missing} =")))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(err(&text).contains(&format!("mqtt.{missing}")), "{missing}");
        }
    }

    #[test]
    fn printer_validation() {
        assert!(
            err(&with(FULL, "queue = \"fotoko\"", "queue = \"-evil\"")).contains("printer.queue")
        );
        assert!(
            err(&with(FULL, "queue = \"fotoko\"", "queue = \"a b\"")).contains("printer.queue")
        );
        assert!(err(&with(FULL, "queue = \"fotoko\"", "queue = \"\"")).contains("printer.queue"));
        assert!(err(&with(FULL, "w288h432", "w 288")).contains("printer.media"));
        assert!(err(&with(FULL, "lp_bin = \"lp\"", "lp_bin = \" \"")).contains("lp_bin"));
        assert!(
            err(&with(FULL, "lpstat_bin = \"lpstat\"", "lpstat_bin = \"\"")).contains("lpstat_bin")
        );
        assert!(
            err(&with(FULL, "lpstat_bin = \"lpstat\"", "cancel_bin = \"\"")).contains("cancel_bin")
        );
        assert!(
            err(&with(
                FULL,
                "lpstat_bin = \"lpstat\"",
                "print_timeout_secs = 0"
            ))
            .contains("print_timeout")
        );
        assert!(
            err(&with(
                FULL,
                "lpstat_bin = \"lpstat\"",
                "poll_interval_ms = 0"
            ))
            .contains("poll_interval")
        );
    }

    #[test]
    fn sim_backend_requires_sim_section_but_cups_does_not() {
        let no_sim = LOCAL.replace("[sim]\nout_dir = \"/tmp/out\"\n", "");
        assert!(err(&no_sim).contains("sim"));
        let cups_no_sim = FULL.replace(
            "[sim]\nout_dir = \"/var/lib/fotoko/sim-out\"\npaper = 50\nfail = \"\"\n",
            "",
        );
        assert!(Config::from_toml(&cups_no_sim).unwrap().sim.is_none());
    }

    #[test]
    fn storage_and_agent_validation() {
        assert!(err(&with(LOCAL, "/tmp/state", "")).contains("storage.state_dir"));
        let agent = |body: &str| format!("{LOCAL}[agent]\n{body}\n");
        assert!(err(&agent("heartbeat_secs = 0")).contains("heartbeat_secs"));
        assert!(err(&agent("download_timeout_secs = 0")).contains("download_timeout"));
        let c = Config::from_toml(&agent("heartbeat_secs = 1\nshutdown_grace_secs = 2")).unwrap();
        assert_eq!(c.heartbeat_interval(), Duration::from_secs(1));
        assert_eq!(c.agent.shutdown_grace_secs, 2);
    }

    #[test]
    fn is_token_cases() {
        assert!(is_token("w288h432"));
        assert!(is_token("a.b_c-d"));
        assert!(!is_token(""));
        assert!(!is_token("-x"));
        assert!(!is_token("a/b"));
        assert!(!is_token("a b"));
    }

    #[cfg(unix)]
    mod files {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn tls_config(dir: &Path, key_mode: u32) -> Config {
            for name in ["ca.pem", "cert.pem", "key.pem"] {
                fs::write(dir.join(name), "x").unwrap();
            }
            fs::set_permissions(dir.join("key.pem"), fs::Permissions::from_mode(key_mode)).unwrap();
            let text = FULL
                .replace(
                    "/etc/fotoko/AmazonRootCA1.pem",
                    dir.join("ca.pem").to_str().unwrap(),
                )
                .replace(
                    "/etc/fotoko/device.pem.crt",
                    dir.join("cert.pem").to_str().unwrap(),
                )
                .replace(
                    "/etc/fotoko/private.pem.key",
                    dir.join("key.pem").to_str().unwrap(),
                );
            Config::from_toml(&text).unwrap()
        }

        #[test]
        fn accepts_private_key() {
            let dir = tempfile::tempdir().unwrap();
            tls_config(dir.path(), 0o600).check_files().unwrap();
            tls_config(dir.path(), 0o400).check_files().unwrap();
        }

        #[test]
        fn rejects_group_or_world_access() {
            let dir = tempfile::tempdir().unwrap();
            for mode in [0o640, 0o604, 0o644, 0o660, 0o777] {
                let e = tls_config(dir.path(), mode).check_files().unwrap_err();
                assert!(
                    matches!(e, ConfigError::KeyPermissions { mode: m, .. } if m == mode),
                    "{mode:o}"
                );
                assert!(e.to_string().contains("chmod 600"));
            }
        }

        #[test]
        fn reports_missing_files() {
            let dir = tempfile::tempdir().unwrap();
            let c = tls_config(dir.path(), 0o600);
            for (name, field) in [
                ("ca.pem", "mqtt.ca"),
                ("cert.pem", "mqtt.cert"),
                ("key.pem", "mqtt.key"),
            ] {
                let path = dir.path().join(name);
                let backup = fs::read(&path).unwrap();
                fs::remove_file(&path).unwrap();
                let e = c.check_files().unwrap_err();
                assert!(
                    matches!(&e, ConfigError::FileMissing { field: f, .. } if *f == field),
                    "{e}"
                );
                fs::write(&path, backup).unwrap();
                fs::set_permissions(
                    dir.path().join("key.pem"),
                    fs::Permissions::from_mode(0o600),
                )
                .unwrap();
            }
        }

        #[test]
        fn plain_tcp_skips_file_checks() {
            Config::from_toml(LOCAL).unwrap().check_files().unwrap();
        }

        #[test]
        fn load_reads_validates_and_checks_files() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("agent.toml");
            fs::write(&path, LOCAL).unwrap();
            assert_eq!(
                Config::load(&path).unwrap().printer_id.as_str(),
                "stand-001"
            );
            let e = Config::load(&dir.path().join("missing.toml")).unwrap_err();
            assert!(matches!(e, ConfigError::Read { .. }));
            assert!(e.to_string().contains("missing.toml"));
            fs::write(&path, FULL).unwrap();
            assert!(matches!(
                Config::load(&path).unwrap_err(),
                ConfigError::FileMissing { .. }
            ));
            fs::write(&path, "garbage [[").unwrap();
            assert!(matches!(
                Config::load(&path).unwrap_err(),
                ConfigError::Parse(_)
            ));
        }
    }
}
