use mumble_protocol::{control::ControlPacket, Serverbound};
use mumlib::error::ConfigError;
use std::fmt;
use tokio::sync::mpsc;

pub(crate) type ServerSendError = mpsc::error::SendError<ControlPacket<Serverbound>>;

#[derive(Debug)]
pub(crate) enum TcpError {
    NoConnectionInfoReceived,
    TlsConnectorBuilderError(native_tls::Error),
    TlsConnectError(native_tls::Error),
    SendError(ServerSendError),

    IOError(std::io::Error),
}

impl fmt::Display for TcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TcpError::NoConnectionInfoReceived => write!(f, "No connection info received"),
            TcpError::TlsConnectorBuilderError(e) => {
                write!(f, "Error building TLS connector: {}", e)
            }
            TcpError::TlsConnectError(e) => write!(f, "TLS error when connecting: {}", e),
            TcpError::SendError(e) => write!(f, "Couldn't send packet: {}", e),
            TcpError::IOError(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl From<std::io::Error> for TcpError {
    fn from(e: std::io::Error) -> Self {
        TcpError::IOError(e)
    }
}

impl From<ServerSendError> for TcpError {
    fn from(e: ServerSendError) -> Self {
        TcpError::SendError(e)
    }
}

#[derive(Debug)]
pub(crate) enum UdpError {
    NoConnectionInfoReceived,
    DisconnectBeforeCryptSetup,

    IOError(std::io::Error),
}

impl From<std::io::Error> for UdpError {
    fn from(e: std::io::Error) -> Self {
        UdpError::IOError(e)
    }
}

#[derive(Debug)]
pub(crate) enum ClientError {
    TcpError(TcpError),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::TcpError(e) => write!(f, "TCP error: {}", e),
        }
    }
}

#[derive(Debug)]
pub(crate) enum AudioStream {
    Input,
    Output,
}

impl fmt::Display for AudioStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioStream::Input => write!(f, "input"),
            AudioStream::Output => write!(f, "output"),
        }
    }
}

#[derive(Debug)]
pub(crate) enum AudioError {
    NoDevice(AudioStream),
    NoConfigs(AudioStream, cpal::SupportedStreamConfigsError),
    NoSupportedConfig(AudioStream),
    InvalidStream(AudioStream, cpal::BuildStreamError),
    OutputPlayError(cpal::PlayStreamError),
    OutputPauseError(cpal::PauseStreamError),
    InputPlayError(cpal::PlayStreamError),
    InputPauseError(cpal::PauseStreamError),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoDevice(s) => write!(f, "No {} device", s),
            AudioError::NoConfigs(s, e) => write!(f, "No {} configs: {}", s, e),
            AudioError::NoSupportedConfig(s) => write!(f, "No supported {} config found", s),
            AudioError::InvalidStream(s, e) => write!(f, "Invalid {} stream: {}", s, e),
            AudioError::OutputPlayError(e) => write!(f, "Playback error: {}", e),
            AudioError::OutputPauseError(e) => write!(f, "Playback error: {}", e),
            AudioError::InputPlayError(e) => write!(f, "Recording error: {}", e),
            AudioError::InputPauseError(e) => write!(f, "Recording error: {}", e),
        }
    }
}

#[derive(Debug)]
pub(crate) enum StateError {
    AudioError(AudioError),
    ConfigError(ConfigError),
}

impl From<AudioError> for StateError {
    fn from(e: AudioError) -> Self {
        StateError::AudioError(e)
    }
}

impl From<ConfigError> for StateError {
    fn from(e: ConfigError) -> Self {
        StateError::ConfigError(e)
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::AudioError(e) => write!(f, "Audio error: {}", e),
            StateError::ConfigError(e) => write!(f, "Config error: {}", e),
        }
    }
}
