use std::fmt;
use std::io;

/// The error type for encoding and decoding BSUP and for converting between
/// Rust values and super-structured values.
#[derive(Debug)]
pub enum Error {
    /// An I/O error from the underlying reader or writer.
    Io(io::Error),
    /// The input is not valid BSUP.
    Malformed(String),
    /// The input uses a feature of BSUP this crate doesn't support.
    Unsupported(String),
    /// A type name is bound to two different types.
    Redefined(String),
    /// A value can't be encoded or a Rust value can't be converted.
    Message(String),
}

/// A specialized [`Result`](std::result::Result) for this crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn malformed(msg: impl fmt::Display) -> Error {
        Error::Malformed(msg.to_string())
    }

    pub(crate) fn unsupported(msg: impl fmt::Display) -> Error {
        Error::Unsupported(msg.to_string())
    }

    pub(crate) fn msg(msg: impl fmt::Display) -> Error {
        Error::Message(msg.to_string())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(err) => write!(f, "{err}"),
            Error::Malformed(msg) => write!(f, "malformed BSUP: {msg}"),
            Error::Unsupported(msg) => write!(f, "unsupported: {msg}"),
            Error::Redefined(name) => write!(f, "type {name:?} redefined"),
            Error::Message(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}
