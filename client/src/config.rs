use crate::{
    certificate,
    relay::{ServerAddr, UdpMode},
    socks5::Authentication as Socks5Authentication,
};
use getopts::{Fail, Options};
use log::{LevelFilter, ParseLevelError};
use quinn::{
    congestion::{BbrConfig, CubicConfig, NewRenoConfig},
    ClientConfig, IdleTimeout, TransportConfig, VarInt,
};
use rustls::RootCertStore;
use serde::{de::Error as DeError, Deserialize, Deserializer};
use serde_json::Error as JsonError;
use std::{
    env::ArgsOs,
    fmt::Display,
    fs::File,
    io::Error as IoError,
    net::{AddrParseError, IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    num::ParseIntError,
    str::FromStr,
    sync::Arc,
};
use thiserror::Error;
use webpki::Error as WebpkiError;

pub struct Config {
    pub client_config: ClientConfig,
    pub server_addr: ServerAddr,
    pub token_digest: [u8; 32],
    pub local_addr: SocketAddr,
    pub socks5_authentication: Socks5Authentication,
    pub udp_mode: UdpMode,
    pub heartbeat_interval: u64,
    pub reduce_rtt: bool,
    pub enable_ipv6: bool,
    pub max_udp_packet_size: usize,
    pub log_level: LevelFilter,
}

impl Config {
    pub fn parse(args: ArgsOs) -> Result<Self, ConfigError> {
        let raw = RawConfig::parse(args)?;

        let client_config = {
            let mut config = if let Some(path) = raw.relay.certificate {
                let mut certs = RootCertStore::empty();

                for cert in certificate::load_certificates(&path)
                    .map_err(|err| ConfigError::Io(path, err))?
                {
                    certs.add(&cert)?;
                }

                ClientConfig::with_root_certificates(certs)
            } else {
                ClientConfig::with_native_roots()
            };

            let mut transport = TransportConfig::default();

            match raw.relay.congestion_controller {
                CongestionController::Bbr => {
                    transport.congestion_controller_factory(Arc::new(BbrConfig::default()));
                }
                CongestionController::Cubic => {
                    transport.congestion_controller_factory(Arc::new(CubicConfig::default()));
                }
                CongestionController::NewReno => {
                    transport.congestion_controller_factory(Arc::new(NewRenoConfig::default()));
                }
            }

            if raw.relay.max_idle_time as u64 <= raw.relay.heartbeat_interval {
                return Err(ConfigError::HeartbeatInterval);
            }

            transport.max_idle_timeout(Some(IdleTimeout::from(VarInt::from_u32(
                raw.relay.max_idle_time,
            ))));

            config.transport = Arc::new(transport);
            config
        };

        let server_addr = {
            let name = raw.relay.server.unwrap();
            let port = raw.relay.port.unwrap();

            if let Some(ip) = raw.relay.ip {
                ServerAddr::SocketAddr {
                    server_addr: SocketAddr::new(ip, port),
                    server_name: name,
                }
            } else {
                ServerAddr::HostnameAddr {
                    hostname: name,
                    server_port: port,
                }
            }
        };

        let token_digest = *blake3::hash(&raw.relay.token.unwrap().into_bytes()).as_bytes();

        let local_addr = {
            let local_port = raw.local.port.unwrap();

            let local_ip = match (raw.enable_ipv6, raw.local.allow_external_connection) {
                (false, false) => IpAddr::V4(Ipv4Addr::LOCALHOST),
                (false, true) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                (true, false) => IpAddr::V6(Ipv6Addr::LOCALHOST),
                (true, true) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            };

            SocketAddr::from((local_ip, local_port))
        };

        let socks5_authentication = match (raw.local.username, raw.local.password) {
            (None, None) => Socks5Authentication::None,
            (Some(username), Some(password)) => Socks5Authentication::Password {
                username: username.into_bytes(),
                password: password.into_bytes(),
            },
            _ => return Err(ConfigError::LocalAuthentication),
        };

        let udp_mode = raw.relay.udp_mode;
        let heartbeat_interval = raw.relay.heartbeat_interval;
        let reduce_rtt = raw.relay.reduce_rtt;
        let enable_ipv6 = raw.enable_ipv6;
        let max_udp_packet_size = raw.max_udp_packet_size;
        let log_level = raw.log_level;

        Ok(Self {
            client_config,
            server_addr,
            token_digest,
            local_addr,
            socks5_authentication,
            udp_mode,
            heartbeat_interval,
            reduce_rtt,
            enable_ipv6,
            max_udp_packet_size,
            log_level,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    relay: RawRelayConfig,
    local: RawLocalConfig,
    #[serde(default = "default::enable_ipv6")]
    enable_ipv6: bool,
    #[serde(default = "default::max_udp_packet_size")]
    max_udp_packet_size: usize,
    #[serde(default = "default::log_level")]
    log_level: LevelFilter,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRelayConfig {
    server: Option<String>,
    port: Option<u16>,
    ip: Option<IpAddr>,
    token: Option<String>,
    certificate: Option<String>,

    #[serde(
        default = "default::udp_mode",
        deserialize_with = "deserialize_from_str"
    )]
    udp_mode: UdpMode,

    #[serde(
        default = "default::congestion_controller",
        deserialize_with = "deserialize_from_str"
    )]
    congestion_controller: CongestionController,
    #[serde(default = "default::max_idle_time")]
    max_idle_time: u32,
    #[serde(default = "default::heartbeat_interval")]
    heartbeat_interval: u64,
    #[serde(default = "default::reduce_rtt")]
    reduce_rtt: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLocalConfig {
    port: Option<u16>,
    username: Option<String>,
    password: Option<String>,
    #[serde(default = "default::allow_external_connection")]
    allow_external_connection: bool,
}

impl Default for RawConfig {
    fn default() -> Self {
        Self {
            relay: RawRelayConfig::default(),
            local: RawLocalConfig::default(),
            enable_ipv6: default::enable_ipv6(),
            max_udp_packet_size: default::max_udp_packet_size(),
            log_level: default::log_level(),
        }
    }
}

impl Default for RawRelayConfig {
    fn default() -> Self {
        Self {
            server: None,
            port: None,
            ip: None,
            token: None,
            certificate: None,
            udp_mode: default::udp_mode(),
            congestion_controller: default::congestion_controller(),
            max_idle_time: default::max_idle_time(),
            heartbeat_interval: default::heartbeat_interval(),
            reduce_rtt: default::reduce_rtt(),
        }
    }
}

impl Default for RawLocalConfig {
    fn default() -> Self {
        Self {
            port: None,
            username: None,
            password: None,
            allow_external_connection: default::allow_external_connection(),
        }
    }
}

impl RawConfig {
    fn parse(args: ArgsOs) -> Result<Self, ConfigError> {
        let mut opts = Options::new();

        opts.optopt(
            "c",
            "config",
            "Read configuration from a file. Note that command line arguments will override the configuration file",
            "CONFIG_FILE",
        );

        opts.optopt(
            "",
            "server",
            "Set the server address. This address must be included in the certificate",
            "SERVER",
        );

        opts.optopt("", "server-port", "Set the server port", "SERVER_PORT");

        opts.optopt(
            "",
            "server-ip",
            "Set the server IP, for overwriting the DNS lookup result of the server address set in option 'server'",
            "SERVER_IP",
        );

        opts.optopt(
            "",
            "token",
            "Set the token for TUIC authentication",
            "TOKEN",
        );

        opts.optopt(
            "",
            "certificate",
            "Set the X.509 certificate for QUIC handshake. If not set, native CA roots will be trusted",
            "CERTIFICATE",
        );

        opts.optopt(
            "",
            "udp-mode",
            r#"Set the UDP relay mode. Available: "native", "quic". Default: "native""#,
            "UDP_MODE",
        );

        opts.optopt(
            "",
            "congestion-controller",
            r#"Set the congestion control algorithm. Available: "cubic", "new_reno", "bbr". Default: "cubic""#,
            "CONGESTION_CONTROLLER",
        );

        opts.optopt(
            "",
            "max-idle-time",
            "Set the maximum idle time for connections, in milliseconds. The true idle timeout is the minimum of this and the client's one. Default: 15000",
            "MAX_IDLE_TIME",
        );

        opts.optopt(
            "",
            "heartbeat-interval",
            "Set the heartbeat interval, in milliseconds. This ensures that the QUIC connection is not closed when there are relay tasks but no data transfer. Default: 10000",
            "HEARTBEAT_INTERVAL",
        );

        opts.optflag("", "reduce-rtt", "Enable 0-RTT QUIC handshake");

        opts.optopt(
            "",
            "local-port",
            "Set the listening port for the local socks5 server",
            "LOCAL_PORT",
        );

        opts.optopt(
            "",
            "local-username",
            "Set the username for the local socks5 server authentication",
            "LOCAL_USERNAME",
        );

        opts.optopt(
            "",
            "local-password",
            "Set the password for the local socks5 server authentication",
            "LOCAL_PASSWORD",
        );

        opts.optflag(
            "",
            "allow-external-connection",
            "Allow external connections for local socks5 server",
        );

        opts.optflag("", "enable-ipv6", "Enable IPv6 support");

        opts.optopt(
            "",
            "max-udp-packet-size",
            "Set the maximum UDP packet size, in bytes. Excess bytes may be discarded. Default: 1536",
            "MAX_UDP_PACKET_SIZE",
        );

        opts.optopt(
            "",
            "log-level",
            r#"Set the log level. Available: "off", "error", "warn", "info", "debug", "trace". Default: "info""#,
            "LOG_LEVEL",
        );

        opts.optflag("v", "version", "Print the version");
        opts.optflag("h", "help", "Print this help menu");

        let matches = opts.parse(args.skip(1))?;

        if matches.opt_present("help") {
            return Err(ConfigError::Help(opts.usage(env!("CARGO_PKG_NAME"))));
        }

        if matches.opt_present("version") {
            return Err(ConfigError::Version(env!("CARGO_PKG_VERSION")));
        }

        if !matches.free.is_empty() {
            return Err(ConfigError::UnexpectedArguments(matches.free.join(", ")));
        }

        let server = matches.opt_str("server");
        let server_port = matches.opt_str("server-port").map(|port| port.parse());
        let token = matches.opt_str("token");
        let local_port = matches.opt_str("local-port").map(|port| port.parse());

        let mut raw = if let Some(path) = matches.opt_str("config") {
            let mut raw = RawConfig::from_file(path)?;

            raw.relay.server = Some(
                server
                    .or(raw.relay.server)
                    .ok_or(ConfigError::MissingOption("server address"))?,
            );

            raw.relay.port = Some(
                server_port
                    .transpose()?
                    .or(raw.relay.port)
                    .ok_or(ConfigError::MissingOption("server port"))?,
            );

            raw.relay.token = Some(
                token
                    .or(raw.relay.token)
                    .ok_or(ConfigError::MissingOption("token"))?,
            );

            raw.local.port = Some(
                local_port
                    .transpose()?
                    .or(raw.local.port)
                    .ok_or(ConfigError::MissingOption("local port"))?,
            );

            raw
        } else {
            let relay = RawRelayConfig {
                server: Some(server.ok_or(ConfigError::MissingOption("server address"))?),
                port: Some(server_port.ok_or(ConfigError::MissingOption("server port"))??),
                token: Some(token.ok_or(ConfigError::MissingOption("token"))?),
                ..Default::default()
            };

            let local = RawLocalConfig {
                port: Some(local_port.ok_or(ConfigError::MissingOption("local port"))??),
                ..Default::default()
            };

            RawConfig {
                relay,
                local,
                ..Default::default()
            }
        };

        if let Some(ip) = matches.opt_str("server-ip") {
            raw.relay.ip = Some(ip.parse()?);
        };

        raw.relay.certificate = matches.opt_str("certificate").or(raw.relay.certificate);

        if let Some(mode) = matches.opt_str("udp-mode") {
            raw.relay.udp_mode = mode.parse()?;
        };

        if let Some(cgstn_ctrl) = matches.opt_str("congestion-controller") {
            raw.relay.congestion_controller = cgstn_ctrl.parse()?;
        };

        if let Some(timeout) = matches.opt_str("max-idle-time") {
            raw.relay.max_idle_time = timeout.parse()?;
        };

        if let Some(interval) = matches.opt_str("heartbeat-interval") {
            raw.relay.heartbeat_interval = interval.parse()?;
        };

        raw.relay.reduce_rtt |= matches.opt_present("reduce-rtt");

        raw.local.username = matches.opt_str("local-username").or(raw.local.username);
        raw.local.password = matches.opt_str("local-password").or(raw.local.password);

        raw.local.allow_external_connection |= matches.opt_present("allow-external-connection");

        raw.enable_ipv6 |= matches.opt_present("enable-ipv6");

        if let Some(max_udp_packet_size) = matches.opt_str("max-udp-packet-size") {
            raw.max_udp_packet_size = max_udp_packet_size.parse()?;
        };

        if let Some(log_level) = matches.opt_str("log-level") {
            raw.log_level = log_level.parse()?;
        };

        Ok(raw)
    }

    fn from_file(path: String) -> Result<Self, ConfigError> {
        let file = File::open(&path).map_err(|err| ConfigError::Io(path, err))?;
        let raw = serde_json::from_reader(file)?;
        Ok(raw)
    }
}

enum CongestionController {
    Cubic,
    NewReno,
    Bbr,
}

impl FromStr for CongestionController {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("cubic") {
            Ok(Self::Cubic)
        } else if s.eq_ignore_ascii_case("new_reno") || s.eq_ignore_ascii_case("newreno") {
            Ok(Self::NewReno)
        } else if s.eq_ignore_ascii_case("bbr") {
            Ok(Self::Bbr)
        } else {
            Err(ConfigError::InvalidCongestionController)
        }
    }
}

impl FromStr for UdpMode {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("native") {
            Ok(Self::Native)
        } else if s.eq_ignore_ascii_case("quic") {
            Ok(Self::Quic)
        } else {
            Err(ConfigError::InvalidUdpRelayMode)
        }
    }
}

fn deserialize_from_str<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: FromStr,
    <T as FromStr>::Err: Display,
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    T::from_str(&s).map_err(DeError::custom)
}

mod default {
    use super::*;

    pub(super) const fn udp_mode() -> UdpMode {
        UdpMode::Native
    }

    pub(super) const fn congestion_controller() -> CongestionController {
        CongestionController::Cubic
    }

    pub(super) const fn max_idle_time() -> u32 {
        15000
    }

    pub(super) const fn heartbeat_interval() -> u64 {
        10000
    }

    pub(super) const fn reduce_rtt() -> bool {
        false
    }

    pub(super) const fn allow_external_connection() -> bool {
        false
    }

    pub(super) const fn enable_ipv6() -> bool {
        false
    }

    pub(super) const fn max_udp_packet_size() -> usize {
        1536
    }

    pub(super) const fn log_level() -> LevelFilter {
        LevelFilter::Info
    }
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("{0}")]
    Help(String),
    #[error("{0}")]
    Version(&'static str),
    #[error("Failed to read '{0}': {1}")]
    Io(String, #[source] IoError),
    #[error("Failed to parse the config file: {0}")]
    ParseConfigJson(#[from] JsonError),
    #[error(transparent)]
    ParseArgument(#[from] Fail),
    #[error("Unexpected arguments: {0}")]
    UnexpectedArguments(String),
    #[error("Missing option: {0}")]
    MissingOption(&'static str),
    #[error(transparent)]
    ParseInt(#[from] ParseIntError),
    #[error(transparent)]
    ParseAddr(#[from] AddrParseError),
    #[error("Invalid congestion controller")]
    InvalidCongestionController,
    #[error("Invalid udp relay mode")]
    InvalidUdpRelayMode,
    #[error("Heartbeat interval must be less than the max idle time")]
    HeartbeatInterval,
    #[error("Failed to load the certificate: {0}")]
    Certificate(#[from] WebpkiError),
    #[error("Username and password must be set together for the local socks5 server")]
    LocalAuthentication,
    #[error(transparent)]
    ParseLogLevel(#[from] ParseLevelError),
}
