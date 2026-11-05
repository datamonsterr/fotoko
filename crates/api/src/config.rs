//! Configuration read from environment variables, validated up front so a bad deployment fails
//! at cold start instead of on the first request. Every constructor takes a lookup function so
//! tests do not touch the process environment; the `*_from_env` helpers pass `std::env::var`.

use std::net::SocketAddr;
use std::path::PathBuf;

use fotoko_protocol::DEFAULT_PRICE_VND;
use thiserror::Error;

/// Default `PAYOS_BASE_URL`.
pub const DEFAULT_PAYOS_BASE_URL: &str = "https://api-merchant.payos.vn";
/// Default `FOTOKO_LOCAL_ADDR`.
pub const DEFAULT_LOCAL_ADDR: &str = "127.0.0.1:8787";

/// An invalid or missing setting.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// A required variable is unset or empty.
    #[error("environment variable {0} is required")]
    Missing(&'static str),
    /// A variable has a value that cannot be used.
    #[error("environment variable {name} is invalid: {reason}")]
    Invalid {
        /// Variable name.
        name: &'static str,
        /// Why the value is rejected.
        reason: String,
    },
}

/// How the process runs: on AWS, or as the local/sim server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Lambda behind API Gateway: identity comes from the request context only.
    Aws,
    /// Local/sim server: identity comes from the `x-fotoko-identity` header.
    Local,
}

/// Lookup of one variable: `None` when unset.
pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The process environment as a [`Lookup`] function.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn get(lookup: Lookup<'_>, name: &str) -> Option<String> {
    lookup(name)
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn required(lookup: Lookup<'_>, name: &'static str) -> Result<String, ConfigError> {
    get(lookup, name).ok_or(ConfigError::Missing(name))
}

fn http_url(name: &'static str, value: String) -> Result<String, ConfigError> {
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"));
    match rest {
        Some(host) if !host.is_empty() && !host.contains(char::is_whitespace) => {
            Ok(value.trim_end_matches('/').to_owned())
        }
        _ => Err(ConfigError::Invalid {
            name,
            reason: "must be an http(s) URL".into(),
        }),
    }
}

fn price(lookup: Lookup<'_>) -> Result<u64, ConfigError> {
    match get(lookup, "DEFAULT_PRICE_VND") {
        None => Ok(DEFAULT_PRICE_VND),
        Some(raw) => match raw.parse::<u64>() {
            Ok(v) if v > 0 => Ok(v),
            _ => Err(ConfigError::Invalid {
                name: "DEFAULT_PRICE_VND",
                reason: "must be a positive integer".into(),
            }),
        },
    }
}

fn payos_base_url(lookup: Lookup<'_>) -> Result<String, ConfigError> {
    let raw = get(lookup, "PAYOS_BASE_URL").unwrap_or_else(|| DEFAULT_PAYOS_BASE_URL.to_owned());
    http_url("PAYOS_BASE_URL", raw)
}

impl Mode {
    /// `FOTOKO_MODE`: `aws` (default when unset, so a missing variable can never enable the
    /// header-based identity) or `local`.
    pub fn from_lookup(lookup: Lookup<'_>) -> Result<Self, ConfigError> {
        match get(lookup, "FOTOKO_MODE").as_deref() {
            None | Some("aws") => Ok(Self::Aws),
            Some("local") => Ok(Self::Local),
            Some(other) => Err(ConfigError::Invalid {
                name: "FOTOKO_MODE",
                reason: format!("expected `aws` or `local`, got `{other}`"),
            }),
        }
    }
}

/// Settings of the `fotoko-http` Lambda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConfig {
    /// DynamoDB table.
    pub table_name: String,
    /// S3 bucket for photos.
    pub bucket_name: String,
    /// IoT data endpoint host (the scheme is optional in the variable).
    pub iot_data_endpoint: String,
    /// payOS API base URL.
    pub payos_base_url: String,
    /// Secrets Manager secret with the payOS credentials.
    pub payos_secret_arn: String,
    /// Base of the payOS return and cancel URLs.
    pub public_app_url: String,
    /// Price used when a printer item has none.
    pub default_price_vnd: u64,
}

impl HttpConfig {
    /// Parse and validate.
    pub fn from_lookup(lookup: Lookup<'_>) -> Result<Self, ConfigError> {
        let endpoint = required(lookup, "IOT_DATA_ENDPOINT")?;
        let endpoint = endpoint
            .strip_prefix("https://")
            .unwrap_or(&endpoint)
            .trim_end_matches('/')
            .to_owned();
        if endpoint.is_empty() || endpoint.contains(char::is_whitespace) || endpoint.contains('/') {
            return Err(ConfigError::Invalid {
                name: "IOT_DATA_ENDPOINT",
                reason: "must be a host name".into(),
            });
        }
        Ok(Self {
            table_name: required(lookup, "TABLE_NAME")?,
            bucket_name: required(lookup, "BUCKET_NAME")?,
            iot_data_endpoint: endpoint,
            payos_base_url: payos_base_url(lookup)?,
            payos_secret_arn: required(lookup, "PAYOS_SECRET_ARN")?,
            public_app_url: http_url("PUBLIC_APP_URL", required(lookup, "PUBLIC_APP_URL")?)?,
            default_price_vnd: price(lookup)?,
        })
    }
}

/// Settings of the `fotoko-iot-status` Lambda: it only touches the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IotStatusConfig {
    /// DynamoDB table.
    pub table_name: String,
    /// Price used when a printer item has none.
    pub default_price_vnd: u64,
}

impl IotStatusConfig {
    /// Parse and validate.
    pub fn from_lookup(lookup: Lookup<'_>) -> Result<Self, ConfigError> {
        Ok(Self {
            table_name: required(lookup, "TABLE_NAME")?,
            default_price_vnd: price(lookup)?,
        })
    }
}

/// Settings of the `fotoko-local` server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalConfig {
    /// Listen address (`FOTOKO_LOCAL_ADDR`).
    pub addr: SocketAddr,
    /// Public base URL of this server, used in presigned-style object URLs.
    pub public_url: String,
    /// Directory for uploaded photos (`FOTOKO_LOCAL_DIR`).
    pub object_dir: PathBuf,
    /// Secret that signs object URLs (`FOTOKO_LOCAL_SECRET`); random per process when unset.
    pub url_secret: Option<String>,
    /// MQTT broker (`FOTOKO_MQTT_URL`, e.g. `mqtt://127.0.0.1:1883`); none disables MQTT.
    pub mqtt_url: Option<String>,
    /// payOS API base URL.
    pub payos_base_url: String,
    /// payOS client id.
    pub payos_client_id: String,
    /// payOS API key.
    pub payos_api_key: String,
    /// payOS checksum key.
    pub payos_checksum_key: String,
    /// Base of the payOS return and cancel URLs.
    pub public_app_url: String,
    /// Price of one print for seeded printers.
    pub default_price_vnd: u64,
    /// Printer ids to seed as online (`FOTOKO_LOCAL_PRINTERS`, comma separated).
    pub printers: Vec<String>,
}

impl LocalConfig {
    /// Parse and validate.
    pub fn from_lookup(lookup: Lookup<'_>) -> Result<Self, ConfigError> {
        let addr_raw =
            get(lookup, "FOTOKO_LOCAL_ADDR").unwrap_or_else(|| DEFAULT_LOCAL_ADDR.to_owned());
        let addr: SocketAddr = addr_raw.parse().map_err(|_| ConfigError::Invalid {
            name: "FOTOKO_LOCAL_ADDR",
            reason: "must be host:port with an IP address".into(),
        })?;
        let public_url = match get(lookup, "FOTOKO_LOCAL_PUBLIC_URL") {
            Some(url) => http_url("FOTOKO_LOCAL_PUBLIC_URL", url)?,
            None => format!("http://{addr}"),
        };
        let mqtt_url = match get(lookup, "FOTOKO_MQTT_URL") {
            Some(url) if url.starts_with("mqtt://") || url.starts_with("tcp://") => Some(url),
            Some(_) => {
                return Err(ConfigError::Invalid {
                    name: "FOTOKO_MQTT_URL",
                    reason: "must start with mqtt:// or tcp://".into(),
                });
            }
            None => None,
        };
        let printers: Vec<String> = get(lookup, "FOTOKO_LOCAL_PRINTERS")
            .unwrap_or_else(|| "stand-001".to_owned())
            .split(',')
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .collect();
        if printers.is_empty() {
            return Err(ConfigError::Invalid {
                name: "FOTOKO_LOCAL_PRINTERS",
                reason: "must list at least one printer id".into(),
            });
        }
        Ok(Self {
            addr,
            public_url,
            object_dir: get(lookup, "FOTOKO_LOCAL_DIR")
                .map_or_else(|| std::env::temp_dir().join("fotoko-local"), PathBuf::from),
            url_secret: get(lookup, "FOTOKO_LOCAL_SECRET"),
            mqtt_url,
            payos_base_url: payos_base_url(lookup)?,
            payos_client_id: required(lookup, "PAYOS_CLIENT_ID")?,
            payos_api_key: required(lookup, "PAYOS_API_KEY")?,
            payos_checksum_key: required(lookup, "PAYOS_CHECKSUM_KEY")?,
            public_app_url: http_url(
                "PUBLIC_APP_URL",
                get(lookup, "PUBLIC_APP_URL").unwrap_or_else(|| "http://localhost:8081".into()),
            )?,
            default_price_vnd: price(lookup)?,
            printers,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    const HTTP_OK: &[(&str, &str)] = &[
        ("TABLE_NAME", "t"),
        ("BUCKET_NAME", "b"),
        (
            "IOT_DATA_ENDPOINT",
            "abc-ats.iot.ap-southeast-1.amazonaws.com",
        ),
        ("PAYOS_SECRET_ARN", "arn:aws:secretsmanager:x"),
        ("PUBLIC_APP_URL", "https://app.example.com/"),
    ];

    fn without(name: &str) -> Vec<(&'static str, &'static str)> {
        HTTP_OK
            .iter()
            .filter(|(k, _)| *k != name)
            .copied()
            .collect()
    }

    #[test]
    fn mode_parsing() {
        assert_eq!(Mode::from_lookup(&env(&[])), Ok(Mode::Aws));
        assert_eq!(
            Mode::from_lookup(&env(&[("FOTOKO_MODE", "aws")])),
            Ok(Mode::Aws)
        );
        assert_eq!(
            Mode::from_lookup(&env(&[("FOTOKO_MODE", " local ")])),
            Ok(Mode::Local)
        );
        assert_eq!(
            Mode::from_lookup(&env(&[("FOTOKO_MODE", "prod")])),
            Err(ConfigError::Invalid {
                name: "FOTOKO_MODE",
                reason: "expected `aws` or `local`, got `prod`".into()
            })
        );
    }

    #[test]
    fn http_config_defaults_and_normalisation() {
        let cfg = HttpConfig::from_lookup(&env(HTTP_OK)).expect("valid");
        assert_eq!(cfg.table_name, "t");
        assert_eq!(cfg.bucket_name, "b");
        assert_eq!(cfg.payos_base_url, DEFAULT_PAYOS_BASE_URL);
        assert_eq!(cfg.public_app_url, "https://app.example.com");
        assert_eq!(cfg.default_price_vnd, 10_000);
        assert_eq!(
            cfg.iot_data_endpoint,
            "abc-ats.iot.ap-southeast-1.amazonaws.com"
        );
    }

    #[test]
    fn http_config_accepts_scheme_on_endpoint_and_overrides() {
        let mut pairs = HTTP_OK.to_vec();
        pairs.retain(|(k, _)| *k != "IOT_DATA_ENDPOINT");
        pairs.push(("IOT_DATA_ENDPOINT", "https://host.example/"));
        pairs.push(("PAYOS_BASE_URL", "http://127.0.0.1:9999/"));
        pairs.push(("DEFAULT_PRICE_VND", "15000"));
        let cfg = HttpConfig::from_lookup(&env(&pairs)).expect("valid");
        assert_eq!(cfg.iot_data_endpoint, "host.example");
        assert_eq!(cfg.payos_base_url, "http://127.0.0.1:9999");
        assert_eq!(cfg.default_price_vnd, 15_000);
    }

    #[test]
    fn http_config_requires_each_variable() {
        for name in [
            "TABLE_NAME",
            "BUCKET_NAME",
            "IOT_DATA_ENDPOINT",
            "PAYOS_SECRET_ARN",
            "PUBLIC_APP_URL",
        ] {
            let pairs = without(name);
            let err = HttpConfig::from_lookup(&env(&pairs)).expect_err(name);
            assert_eq!(err, ConfigError::Missing(name_static(name)));
        }
        // Empty counts as unset.
        let mut pairs = without("TABLE_NAME");
        pairs.push(("TABLE_NAME", "  "));
        assert_eq!(
            HttpConfig::from_lookup(&env(&pairs)),
            Err(ConfigError::Missing("TABLE_NAME"))
        );
    }

    fn name_static(name: &str) -> &'static str {
        [
            "TABLE_NAME",
            "BUCKET_NAME",
            "IOT_DATA_ENDPOINT",
            "PAYOS_SECRET_ARN",
            "PUBLIC_APP_URL",
        ]
        .into_iter()
        .find(|n| *n == name)
        .unwrap_or("")
    }

    #[test]
    fn http_config_rejects_bad_values() {
        for (name, value) in [
            ("PUBLIC_APP_URL", "ftp://x"),
            ("PUBLIC_APP_URL", "https://"),
            ("PUBLIC_APP_URL", "https://a b"),
            ("PAYOS_BASE_URL", "payos"),
            ("DEFAULT_PRICE_VND", "0"),
            ("DEFAULT_PRICE_VND", "abc"),
            ("DEFAULT_PRICE_VND", "-5"),
            ("IOT_DATA_ENDPOINT", "host/path"),
            ("IOT_DATA_ENDPOINT", "https://"),
        ] {
            let mut pairs = HTTP_OK.to_vec();
            pairs.push((name, value));
            // Later duplicates win in the map built by `env`.
            let err = HttpConfig::from_lookup(&env(&pairs)).expect_err(value);
            assert!(
                matches!(&err, ConfigError::Invalid { name: n, .. } if *n == name),
                "{name}={value}: {err:?}"
            );
        }
    }

    #[test]
    fn iot_status_config_needs_only_the_table() {
        let cfg = IotStatusConfig::from_lookup(&env(&[("TABLE_NAME", "t")])).expect("valid");
        assert_eq!(cfg.table_name, "t");
        assert_eq!(cfg.default_price_vnd, 10_000);
        assert_eq!(
            IotStatusConfig::from_lookup(&env(&[])),
            Err(ConfigError::Missing("TABLE_NAME"))
        );
        assert!(
            IotStatusConfig::from_lookup(&env(&[("TABLE_NAME", "t"), ("DEFAULT_PRICE_VND", "x")]))
                .is_err()
        );
    }

    const LOCAL_OK: &[(&str, &str)] = &[
        ("PAYOS_CLIENT_ID", "cid"),
        ("PAYOS_API_KEY", "key"),
        ("PAYOS_CHECKSUM_KEY", "sum"),
    ];

    #[test]
    fn local_config_defaults() {
        let cfg = LocalConfig::from_lookup(&env(LOCAL_OK)).expect("valid");
        assert_eq!(cfg.addr.to_string(), "127.0.0.1:8787");
        assert_eq!(cfg.public_url, "http://127.0.0.1:8787");
        assert_eq!(cfg.mqtt_url, None);
        assert_eq!(cfg.printers, vec!["stand-001"]);
        assert_eq!(cfg.payos_base_url, DEFAULT_PAYOS_BASE_URL);
        assert_eq!(cfg.public_app_url, "http://localhost:8081");
        assert_eq!(cfg.url_secret, None);
        assert!(cfg.object_dir.ends_with("fotoko-local"));
    }

    #[test]
    fn local_config_overrides() {
        let mut pairs = LOCAL_OK.to_vec();
        pairs.extend([
            ("FOTOKO_LOCAL_ADDR", "0.0.0.0:9000"),
            (
                "FOTOKO_LOCAL_PUBLIC_URL",
                "http://host.docker.internal:9000/",
            ),
            ("FOTOKO_LOCAL_DIR", "/data/objs"),
            ("FOTOKO_LOCAL_SECRET", "s3cret"),
            ("FOTOKO_MQTT_URL", "mqtt://127.0.0.1:1883"),
            ("FOTOKO_LOCAL_PRINTERS", "a, b,,"),
            ("PUBLIC_APP_URL", "http://localhost:19006"),
        ]);
        let cfg = LocalConfig::from_lookup(&env(&pairs)).expect("valid");
        assert_eq!(cfg.addr.port(), 9000);
        assert_eq!(cfg.public_url, "http://host.docker.internal:9000");
        assert_eq!(cfg.object_dir, PathBuf::from("/data/objs"));
        assert_eq!(cfg.url_secret.as_deref(), Some("s3cret"));
        assert_eq!(cfg.mqtt_url.as_deref(), Some("mqtt://127.0.0.1:1883"));
        assert_eq!(cfg.printers, vec!["a", "b"]);
        assert_eq!(cfg.public_app_url, "http://localhost:19006");
    }

    #[test]
    fn local_config_rejects_bad_values() {
        for (name, value) in [
            ("FOTOKO_LOCAL_ADDR", "nope"),
            ("FOTOKO_LOCAL_PUBLIC_URL", "x"),
            ("FOTOKO_MQTT_URL", "http://x"),
            ("FOTOKO_LOCAL_PRINTERS", " , "),
            ("PUBLIC_APP_URL", "x"),
            ("PAYOS_BASE_URL", "x"),
            ("DEFAULT_PRICE_VND", "0"),
        ] {
            let mut pairs = LOCAL_OK.to_vec();
            pairs.push((name, value));
            let err = LocalConfig::from_lookup(&env(&pairs)).expect_err(name);
            assert!(matches!(&err, ConfigError::Invalid { name: n, .. } if *n == name));
        }
        let tcp = [LOCAL_OK, &[("FOTOKO_MQTT_URL", "tcp://h:1")]].concat();
        assert!(LocalConfig::from_lookup(&env(&tcp)).is_ok());
    }

    #[test]
    fn local_config_requires_payos_credentials() {
        for name in ["PAYOS_CLIENT_ID", "PAYOS_API_KEY", "PAYOS_CHECKSUM_KEY"] {
            let pairs: Vec<_> = LOCAL_OK
                .iter()
                .filter(|(k, _)| *k != name)
                .copied()
                .collect();
            assert!(matches!(
                LocalConfig::from_lookup(&env(&pairs)),
                Err(ConfigError::Missing(n)) if n == name
            ));
        }
    }

    #[test]
    fn error_messages() {
        assert_eq!(
            ConfigError::Missing("X").to_string(),
            "environment variable X is required"
        );
        assert_eq!(
            ConfigError::Invalid {
                name: "X",
                reason: "bad".into()
            }
            .to_string(),
            "environment variable X is invalid: bad"
        );
    }

    #[test]
    fn process_env_reads_the_environment() {
        assert!(process_env("FOTOKO_DEFINITELY_NOT_SET_12345").is_none());
        assert!(process_env("PATH").is_some());
    }
}
