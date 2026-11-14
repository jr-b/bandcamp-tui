use reqwest::StatusCode;

/// Errors returned by [`BandcampClient`](super::BandcampClient).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The request could not be sent or the response body could not be read.
    #[error("request to {endpoint} failed")]
    Request {
        endpoint: &'static str,
        #[source]
        source: reqwest::Error,
    },

    /// The server answered with a non-success HTTP status.
    #[error("{endpoint} returned HTTP {status}")]
    Status {
        endpoint: &'static str,
        status: StatusCode,
    },

    /// Bandcamp reported an error in the response body (it does this with HTTP 200).
    #[error("Bandcamp API error from {endpoint}: {message}")]
    Api {
        endpoint: &'static str,
        message: String,
    },

    /// The response was not in the shape we expected. `path` points at the offending field,
    /// which makes Bandcamp schema changes easy to diagnose from a log line.
    #[error("unexpected response from {endpoint} at `{path}`")]
    Decode {
        endpoint: &'static str,
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

impl Error {
    /// One line for the status bar. A decode error means Bandcamp changed its format, which
    /// only a new version fixes; serde's details are for the log, not the listener.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::Decode { .. } => {
                "Bandcamp changed its reply format; update bandcamp-tui (details in the log)".into()
            }
            _ => crate::format::error_chain(self),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
