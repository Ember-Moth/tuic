use crate::utils::{self, CongestionControl, UdpRelayMode};
use lexopt::{Arg, Error as ArgumentError, Parser};
use serde::{de::Error as DeError, Deserialize, Deserializer};
use serde_json::Error as SerdeError;
use std::{
    env::ArgsOs,
    fs::File,
    io::Error as IoError,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use thiserror::Error;

const HELP_MSG: &str = r#"
Usage tuic-client [arguments]

Arguments:
    -c, --config <path>     Path to the config file (required)
    -v, --version           Print the version
    -h, --help              Print this help message
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub relay: Relay,
    pub local: Local,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relay {
    #[serde(deserialize_with = "deserialize_server")]
    pub server: (String, u16),
    pub token: String,
    pub ip: Option<IpAddr>,
    #[serde(default = "default::relay::certificates")]
    pub certificates: Vec<String>,
    #[serde(
        default = "default::relay::udp_relay_mode",
        deserialize_with = "utils::deserialize_from_str"
    )]
    pub udp_relay_mode: UdpRelayMode,
    #[serde(
        default = "default::relay::congestion_control",
        deserialize_with = "utils::deserialize_from_str"
    )]
    pub congestion_control: CongestionControl,
    #[serde(default = "default::relay::alpn")]
    pub alpn: Vec<String>,
    #[serde(default = "default::relay::zero_rtt_handshake")]
    pub zero_rtt_handshake: bool,
    #[serde(default = "default::relay::timeout")]
    pub timeout: Duration,
    #[serde(default = "default::relay::heartbeat")]
    pub heartbeat: Duration,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Local {
    pub server: SocketAddr,
    pub username: Option<String>,
    pub password: Option<String>,
    pub dual_stack: Option<bool>,
    #[serde(default = "default::local::max_packet_size")]
    pub max_packet_size: usize,
}

mod default {
    pub mod relay {
        use crate::utils::{CongestionControl, UdpRelayMode};
        use std::time::Duration;

        pub const fn certificates() -> Vec<String> {
            Vec::new()
        }

        pub const fn udp_relay_mode() -> UdpRelayMode {
            UdpRelayMode::Native
        }

        pub const fn congestion_control() -> CongestionControl {
            CongestionControl::Cubic
        }

        pub const fn alpn() -> Vec<String> {
            Vec::new()
        }

        pub const fn zero_rtt_handshake() -> bool {
            false
        }

        pub const fn timeout() -> Duration {
            Duration::from_secs(8)
        }

        pub const fn heartbeat() -> Duration {
            Duration::from_secs(3)
        }
    }

    pub mod local {
        pub const fn max_packet_size() -> usize {
            1500
        }
    }
}

impl Config {
    pub fn parse(args: ArgsOs) -> Result<Self, ConfigError> {
        let mut parser = Parser::from_iter(args);
        let mut path = None;

        while let Some(arg) = parser.next()? {
            match arg {
                Arg::Short('c') | Arg::Long("config") => {
                    if path.is_none() {
                        path = Some(parser.value()?);
                    } else {
                        return Err(ConfigError::Argument(arg.unexpected()));
                    }
                }
                Arg::Short('v') | Arg::Long("version") => {
                    return Err(ConfigError::Version(env!("CARGO_PKG_VERSION")))
                }
                Arg::Short('h') | Arg::Long("help") => return Err(ConfigError::Help(HELP_MSG)),
                _ => return Err(ConfigError::Argument(arg.unexpected())),
            }
        }

        if path.is_none() {
            return Err(ConfigError::NoConfig);
        }

        let file = File::open(path.unwrap())?;

        Ok(serde_json::from_reader(file)?)
    }
}

pub fn deserialize_server<'de, D>(deserializer: D) -> Result<(String, u16), D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let mut parts = s.split(':');

    match (parts.next(), parts.next(), parts.next()) {
        (Some(domain), Some(port), None) => port.parse().map_or_else(
            |e| Err(DeError::custom(e)),
            |port| Ok((domain.to_owned(), port)),
        ),
        _ => Err(DeError::custom("invalid server address")),
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(transparent)]
    Argument(#[from] ArgumentError),
    #[error("no config file specified")]
    NoConfig,
    #[error("{0}")]
    Version(&'static str),
    #[error("{0}")]
    Help(&'static str),
    #[error(transparent)]
    Io(#[from] IoError),
    #[error(transparent)]
    Serde(#[from] SerdeError),
}
