use serde::export::Formatter;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Serialize, Deserialize)]
pub enum Error {
    DisconnectedError,
    AlreadyConnectedError,
    ChannelIdentifierError(String, ChannelIdentifierError),
    InvalidServerAddrError(String, u16),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::DisconnectedError => write!(f, "Not connected to a server"),
            Error::AlreadyConnectedError => write!(f, "Already connected to a server"),
            Error::ChannelIdentifierError(id, kind) => write!(f, "{}: {}", kind, id),
            Error::InvalidServerAddrError(addr, port) => {
                write!(f, "Invalid server address: {}: {}", addr, port)
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ChannelIdentifierError {
    Invalid,
    Ambiguous,
}

impl Display for ChannelIdentifierError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelIdentifierError::Invalid => write!(f, "Invalid channel identifier"),
            ChannelIdentifierError::Ambiguous => write!(f, "Ambiguous channel identifier"),
        }
    }
}
