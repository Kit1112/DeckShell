use std::fmt;

/// Errors returned by the bridge. `Unavailable` means Steam / the CEF port is absent
/// (the caller should fall back to `steam-data`).
#[derive(Debug, Clone)]
pub enum Error {
    /// Steam is not running, port closed, or no SharedJSContext target.
    Unavailable(String),
    /// A call exceeded its deadline (<= 2 s).
    Timeout,
    /// The evaluated JavaScript threw.
    Js(String),
    /// Malformed CDP / JSON payload.
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unavailable(s) => write!(f, "steam unavailable: {s}"),
            Error::Timeout => write!(f, "steam bridge timeout"),
            Error::Js(s) => write!(f, "javascript error: {s}"),
            Error::Protocol(s) => write!(f, "protocol error: {s}"),
        }
    }
}
impl std::error::Error for Error {}
