//! Where to connect and what to ask for: the provider-wide [`Endpoint`] and the [`ListenConfig`]
//! of the live transcription endpoint.

use std::time::Duration;

use crate::error::ConfigError;

/// The hosted API's base URL.
pub const DEFAULT_URL: &str = "wss://api.deepgram.com";

/// The EU base URL: the same keys, requests kept in the EU, which fail rather than fall back.
pub const EU_URL: &str = "wss://api.eu.deepgram.com";

/// The model the binding asks for unless told otherwise. The endpoint's own default is an older
/// one, so the binding always names a model.
pub const DEFAULT_MODEL: &str = "nova-3";

/// The language the binding asks for unless told otherwise, the endpoint's own default.
pub const DEFAULT_LANGUAGE: &str = "en";

/// How long the TCP connection, TLS and the WebSocket handshake may take together.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a recognition waits, in total, from sending `CloseStream` to the summary `Metadata`,
/// and how long a write may take.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the binding goes without sending audio before it sends a `KeepAlive`: within the
/// 3 to 5 seconds the endpoint asks for, and under the 10 seconds after which it closes a silent
/// connection.
pub const DEFAULT_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(4);

/// The shortest and longest `utterance_end_ms` the endpoint takes.
pub const UTTERANCE_END_RANGE: std::ops::RangeInclusive<u32> = 1_000..=5_000;

/// The host region of the hosted API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Region {
    /// [`DEFAULT_URL`].
    #[default]
    Default,
    /// [`EU_URL`].
    Eu,
}

impl Region {
    /// The base URL of the region.
    pub const fn url(self) -> &'static str {
        match self {
            Self::Default => DEFAULT_URL,
            Self::Eu => EU_URL,
        }
    }
}

/// Where a recognition connects: the base URL, the bound on the handshake, and the bound on the
/// whole wait from the end of input to the summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Scheme and host (and port), no path, query or fragment: `wss://` to any host, `ws://`
    /// only to a loopback host, since the key would travel in the clear.
    pub url: String,
    pub read_timeout: Duration,
    pub connect_timeout: Duration,
}

impl Default for Endpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl Endpoint {
    /// The hosted endpoint, [`DEFAULT_URL`], with the default bounds.
    pub fn new() -> Self {
        Self::region(Region::Default)
    }

    /// The hosted endpoint of the EU, [`EU_URL`], with the default bounds.
    pub fn eu() -> Self {
        Self::region(Region::Eu)
    }

    /// The hosted endpoint of `region`, with the default bounds.
    pub fn region(region: Region) -> Self {
        Self {
            url: region.url().to_owned(),
            read_timeout: DEFAULT_READ_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }

    /// This endpoint at another base URL, such as a local test server.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// This endpoint with another read timeout.
    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.read_timeout = read_timeout;
        self
    }

    /// This endpoint with another connect timeout.
    pub fn with_connect_timeout(mut self, connect_timeout: Duration) -> Self {
        self.connect_timeout = connect_timeout;
        self
    }

    /// The base URL, its trailing slashes dropped, once it is one the binding takes.
    ///
    /// # Errors
    /// [`ConfigError::InvalidUrl`] when it is not `wss://`, or `ws://` to a loopback host
    /// (`127.0.0.0/8`, `::1`, `localhost`), or carries a query or fragment. The message never
    /// carries the URL itself.
    pub fn base(&self) -> Result<&str, ConfigError> {
        let base = self.url.trim_end_matches('/');
        if let Some(rest) = base.strip_prefix("ws://") {
            if !is_loopback(rest) {
                return Err(ConfigError::InvalidUrl(
                    "ws:// would send the key in the clear; it is taken only to a loopback host, use wss://"
                        .to_owned(),
                ));
            }
        } else if !base.starts_with("wss://") {
            return Err(ConfigError::InvalidUrl(
                "the scheme must be wss://, or ws:// to a loopback host".to_owned(),
            ));
        }
        if base.contains('?') || base.contains('#') {
            return Err(ConfigError::InvalidUrl(
                "the base URL carries a query or fragment; the binding adds its own".to_owned(),
            ));
        }
        Ok(base)
    }
}

/// Whether the authority at the start of `rest`, what follows `ws://`, names a loopback host:
/// `localhost`, an IPv4 address in `127.0.0.0/8` or `[::1]`, with or without a port. A user
/// part (`user@host`) is refused, since it could make a host read as another.
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

/// The `endpointing` query parameter: the silence in milliseconds after which the endpoint marks
/// `speech_final`, or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endpointing {
    /// `endpointing=false`.
    Disabled,
    /// `endpointing=<ms>`.
    Millis(u32),
}

/// What a recognition asks the live transcription endpoint for. The encoding, sample rate and
/// channels come from the audio itself; see [`crate::listen`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenConfig {
    /// The model, [`DEFAULT_MODEL`] from [`ListenConfig::new`]: ASCII letters, digits, `-`, `_`
    /// and `.` only.
    pub model: String,
    /// The language, [`DEFAULT_LANGUAGE`] from [`ListenConfig::new`]: ASCII letters, digits and
    /// `-` only.
    pub language: String,
    /// `None` leaves `endpointing` out, so the endpoint's default applies.
    pub endpointing: Option<Endpointing>,
    /// The `utterance_end_ms` query parameter, within [`UTTERANCE_END_RANGE`]; `None` leaves it
    /// out and no `UtteranceEnd` arrives.
    pub utterance_end_ms: Option<u32>,
    /// The `smart_format` query parameter, off unless set.
    pub smart_format: bool,
    /// How long the binding goes without sending audio before it sends a `KeepAlive`.
    pub keepalive_interval: Duration,
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl ListenConfig {
    /// [`DEFAULT_MODEL`] in [`DEFAULT_LANGUAGE`], `smart_format` off, a `KeepAlive` every
    /// [`DEFAULT_KEEPALIVE_INTERVAL`] without audio, and nothing else set.
    pub fn new() -> Self {
        Self {
            model: DEFAULT_MODEL.to_owned(),
            language: DEFAULT_LANGUAGE.to_owned(),
            endpointing: None,
            utterance_end_ms: None,
            smart_format: false,
            keepalive_interval: DEFAULT_KEEPALIVE_INTERVAL,
        }
    }

    /// This configuration with another model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// This configuration with another language.
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    /// Checks what the URL and the endpoint's documented ranges need.
    ///
    /// # Errors
    /// [`ConfigError::InvalidModel`], [`ConfigError::InvalidLanguage`],
    /// [`ConfigError::UtteranceEndMs`] or [`ConfigError::KeepAliveInterval`].
    pub fn validate(&self) -> Result<(), ConfigError> {
        let token = |value: &str, extra: &[char]| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || extra.contains(&c))
        };
        if !token(&self.model, &['_', '.']) {
            return Err(ConfigError::InvalidModel);
        }
        if !token(&self.language, &[]) {
            return Err(ConfigError::InvalidLanguage);
        }
        if let Some(ms) = self.utterance_end_ms
            && !UTTERANCE_END_RANGE.contains(&ms)
        {
            return Err(ConfigError::UtteranceEndMs(ms));
        }
        if self.keepalive_interval.is_zero() {
            return Err(ConfigError::KeepAliveInterval);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_eu_switch_selects_the_eu_host() {
        assert_eq!(Endpoint::new().url, "wss://api.deepgram.com");
        assert_eq!(Endpoint::eu().url, "wss://api.eu.deepgram.com");
        assert_eq!(Endpoint::region(Region::Eu), Endpoint::eu());
        assert_eq!(Endpoint::default(), Endpoint::region(Region::Default));
    }

    #[test]
    fn plaintext_is_taken_only_to_a_loopback_host() {
        for taken in [
            "ws://127.0.0.1:9",
            "ws://127.8.9.10",
            "ws://localhost:8080/",
            "ws://[::1]:9",
            "wss://api.deepgram.com",
            "wss://api.eu.deepgram.com/",
        ] {
            assert!(Endpoint::new().with_url(taken).base().is_ok(), "{taken}");
        }
        for refused in [
            "ws://api.deepgram.com",
            "ws://10.0.0.1:9",
            "ws://[::2]:9",
            "ws://localhost.example.com",
            "ws://127.0.0.1@evil.example",
            "ws://",
        ] {
            match Endpoint::new().with_url(refused).base() {
                Err(ConfigError::InvalidUrl(detail)) => {
                    assert!(detail.contains("ws://"));
                    assert!(!detail.contains("@evil"), "the message names no URL");
                }
                other => panic!("{refused}: {other:?}"),
            }
        }
        for refused in ["http://host", "wss://host?x=1", "wss://host#f"] {
            assert!(matches!(
                Endpoint::new().with_url(refused).base(),
                Err(ConfigError::InvalidUrl(_))
            ));
        }
    }

    #[test]
    fn the_configuration_holds_to_the_documented_ranges() {
        assert_eq!(ListenConfig::new().validate(), Ok(()));
        assert_eq!(
            ListenConfig::new().with_model("nova 3").validate(),
            Err(ConfigError::InvalidModel)
        );
        assert_eq!(
            ListenConfig::new().with_language("en&x=1").validate(),
            Err(ConfigError::InvalidLanguage)
        );
        for ms in [999, 5_001] {
            let config = ListenConfig {
                utterance_end_ms: Some(ms),
                ..ListenConfig::new()
            };
            assert_eq!(config.validate(), Err(ConfigError::UtteranceEndMs(ms)));
        }
        let config = ListenConfig {
            keepalive_interval: Duration::ZERO,
            ..ListenConfig::new()
        };
        assert_eq!(config.validate(), Err(ConfigError::KeepAliveInterval));
    }
}
