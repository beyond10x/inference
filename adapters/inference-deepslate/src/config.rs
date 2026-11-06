//! Where a session connects and what it asks Opal for beyond the session configuration.

use std::time::Duration;

use crate::error::ConfigError;
use crate::protocol::HostedTtsMode;

/// The documented host of the realtime endpoint, which runs in the EU.
pub const BASE_URL: &str = "wss://app.deepslate.eu";

/// The environment variable [`Endpoint::from_env`] reads the vendor id from.
pub const VENDOR_VARIABLE: &str = "DEEPSLATE_VENDOR_ID";

/// The environment variable [`Endpoint::from_env`] reads the organization id from.
pub const ORGANIZATION_VARIABLE: &str = "DEEPSLATE_ORGANIZATION_ID";

/// The environment variable [`Endpoint::from_env`] reads a pinned model from, when it is set.
pub const MODEL_VARIABLE: &str = "DEEPSLATE_MODEL";

/// How long an opening may take in all: the TCP connection, TLS, the WebSocket handshake, the
/// `InitializeSessionRequest` and the `SessionReady` that answers it, which the documentation
/// says comes after the server has warmed up its speech synthesis.
pub const DEFAULT_OPEN_TIMEOUT: Duration = Duration::from_secs(15);

/// How long an open connection may send nothing, not even the `Pong` to a `Ping`, before it is
/// taken as stalled; and how long a write may take.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// How often a session sends `Ping`: the definition advises every 10 seconds or less, since a
/// session the server receives nothing from times out after about 30.
pub const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(10);

/// The longest vendor id, organization id or model the binding puts in the path.
pub const MAX_PATH_SEGMENT: usize = 128;

/// Where a session connects: the base URL, the vendor and organization the key belongs to, the
/// model when one is pinned, and the bounds on the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    url: String,
    vendor_id: String,
    organization_id: String,
    model: Option<String>,
    /// How long an opening may take in all.
    pub open_timeout: Duration,
    /// How long the connection may send nothing before it is stalled; how long a write may take.
    pub read_timeout: Duration,
    /// How often a `Ping` goes out.
    pub ping_interval: Duration,
}

impl Endpoint {
    /// The documented endpoint for `vendor_id` and `organization_id`, on the platform's default
    /// model.
    pub fn new(vendor_id: impl Into<String>, organization_id: impl Into<String>) -> Self {
        Self {
            url: BASE_URL.to_owned(),
            vendor_id: vendor_id.into(),
            organization_id: organization_id.into(),
            model: None,
            open_timeout: DEFAULT_OPEN_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
            ping_interval: DEFAULT_PING_INTERVAL,
        }
    }

    /// The endpoint for the vendor and organization in [`VENDOR_VARIABLE`] and
    /// [`ORGANIZATION_VARIABLE`], the model in [`MODEL_VARIABLE`] when it is set.
    ///
    /// # Errors
    /// [`ConfigError::Missing`] naming the variable that is not set or not Unicode.
    pub fn from_env() -> Result<Self, ConfigError> {
        let read = |name: &'static str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .ok_or(ConfigError::Missing(name))
        };
        let endpoint = Self::new(read(VENDOR_VARIABLE)?, read(ORGANIZATION_VARIABLE)?);
        Ok(match read(MODEL_VARIABLE) {
            Ok(model) => endpoint.with_model(model),
            Err(_) => endpoint,
        })
    }

    /// This endpoint at another base URL, such as a local fixture server's.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// This endpoint pinned to `model`, `opal-v2.1` for one, appended to the path.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    #[must_use]
    pub const fn with_open_timeout(mut self, timeout: Duration) -> Self {
        self.open_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn with_ping_interval(mut self, interval: Duration) -> Self {
        self.ping_interval = interval;
        self
    }

    /// The model pinned, if one is.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The URL a session connects to:
    /// `<base>/api/v1/vendors/<vendor>/organizations/<organization>/realtime[/<model>]`.
    ///
    /// # Errors
    /// [`ConfigError::InvalidUrl`] when the base is not `wss://`, or `ws://` to a loopback host
    /// (`127.0.0.0/8`, `::1`, `localhost`), or carries a path, a query, a fragment or a user
    /// part; [`ConfigError::PathSegment`] when the vendor id, the organization id or the model is
    /// empty, longer than [`MAX_PATH_SEGMENT`], or holds anything but ASCII letters, digits, `-`,
    /// `_` and `.`; [`ConfigError::PingInterval`] for a zero ping interval. No error carries the
    /// URL.
    pub fn url(&self) -> Result<String, ConfigError> {
        if self.ping_interval.is_zero() {
            return Err(ConfigError::PingInterval);
        }
        let base = checked_base(&self.url)?;
        let vendor = segment("vendor id", &self.vendor_id)?;
        let organization = segment("organization id", &self.organization_id)?;
        let mut url =
            format!("{base}/api/v1/vendors/{vendor}/organizations/{organization}/realtime");
        if let Some(model) = &self.model {
            url.push('/');
            url.push_str(segment("model", model)?);
        }
        Ok(url)
    }
}

/// What a session asks Opal for besides its [`SessionConfig`](inference_duplex::SessionConfig).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct OpalConfig {
    /// The temperature of `InferenceConfiguration`; `None` sends none, which on the wire is 0.
    pub temperature: Option<f64>,
    /// The quality and latency trade-off of the hosted voice.
    pub tts_mode: HostedTtsMode,
}

impl OpalConfig {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// `value` once it is a path segment the binding takes.
fn segment<'a>(what: &'static str, value: &'a str) -> Result<&'a str, ConfigError> {
    let plain = !value.is_empty()
        && value.len() <= MAX_PATH_SEGMENT
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && value != "."
        && value != "..";
    if plain {
        Ok(value)
    } else {
        Err(ConfigError::PathSegment(what))
    }
}

/// `url`, its trailing slashes dropped, once it is a base URL the binding takes: `wss://`, or
/// `ws://` to a loopback host, a host and port only. The error never carries the URL.
fn checked_base(url: &str) -> Result<&str, ConfigError> {
    let base = url.trim_end_matches('/');
    let rest = if let Some(rest) = base.strip_prefix("ws://") {
        if !is_loopback(rest) {
            return Err(ConfigError::InvalidUrl(
                "ws:// would send the key in the clear; it is taken only to a loopback host, use wss://",
            ));
        }
        rest
    } else if let Some(rest) = base.strip_prefix("wss://") {
        rest
    } else {
        return Err(ConfigError::InvalidUrl(
            "the scheme must be wss://, or ws:// to a loopback host",
        ));
    };
    if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
        return Err(ConfigError::InvalidUrl(
            "the base URL must be a scheme, a host and a port only; the binding adds the path",
        ));
    }
    Ok(base)
}

/// Whether the authority `rest`, what follows `ws://`, names a loopback host: `localhost`, an
/// IPv4 address in `127.0.0.0/8` or `[::1]`, with or without a port.
fn is_loopback(rest: &str) -> bool {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return false;
    }
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((host, after)) if after.is_empty() || after.starts_with(':') => host,
            _ => return false,
        },
        None => authority.split(':').next().unwrap_or_default(),
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_is_the_documented_path_with_the_model_appended() {
        let endpoint = Endpoint::new("v-1", "org_2");
        assert_eq!(
            endpoint.url().as_deref(),
            Ok("wss://app.deepslate.eu/api/v1/vendors/v-1/organizations/org_2/realtime")
        );
        assert_eq!(
            endpoint.with_model("opal-v2.1").url().as_deref(),
            Ok("wss://app.deepslate.eu/api/v1/vendors/v-1/organizations/org_2/realtime/opal-v2.1")
        );
    }

    #[test]
    fn a_base_that_would_leak_the_key_or_move_the_path_is_refused() {
        for url in [
            "ws://example.com",
            "http://127.0.0.1:1",
            "wss://app.deepslate.eu/api",
            "wss://app.deepslate.eu?x=1",
            "wss://user@app.deepslate.eu",
            "ws://127.0.0.1@example.com",
            "wss://",
        ] {
            assert!(
                matches!(
                    Endpoint::new("v", "o").with_url(url).url(),
                    Err(ConfigError::InvalidUrl(_))
                ),
                "{url}"
            );
        }
        for url in ["ws://127.0.0.1:9", "ws://localhost:9/", "ws://[::1]:9"] {
            assert!(Endpoint::new("v", "o").with_url(url).url().is_ok(), "{url}");
        }
    }

    #[test]
    fn an_id_that_is_not_a_plain_path_segment_is_refused() {
        for (vendor, organization) in [("", "o"), ("v", "a/b"), ("..", "o"), ("v", "o?x")] {
            assert!(
                matches!(
                    Endpoint::new(vendor, organization).url(),
                    Err(ConfigError::PathSegment(_))
                ),
                "{vendor} {organization}"
            );
        }
        assert_eq!(
            Endpoint::new("v", "o").with_model("a b").url(),
            Err(ConfigError::PathSegment("model"))
        );
        assert_eq!(
            Endpoint::new("v", "o")
                .with_ping_interval(Duration::ZERO)
                .url(),
            Err(ConfigError::PingInterval)
        );
    }
}
