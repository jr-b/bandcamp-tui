//! Client for Bandcamp's (unofficial, undocumented) web and mobile APIs.
//!
//! Every endpoint used here works without logging in. See `PLAN.md` §1 for how they were found.

mod album;
mod discover;
mod error;
mod filters;
pub mod images;
mod types;

use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use url::Url;

pub use album::{Album, Band, Tag, Track};
pub use discover::{Cursor, DiscoverItem, DiscoverPage, DiscoverQuery, Slice};
pub use error::{Error, Result};
pub use filters::{Facet, Filters, Subgenre};
pub use types::{ArtId, BandId, Date, ImageId, ItemId, Price, TrackId, TralbumKind};

const DEFAULT_BASE_URL: &str = "https://bandcamp.com/";
const USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    "/",
    env!("CARGO_PKG_VERSION"),
    " (+",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-request timeout for API calls. Set per request rather than on the client, so the same
/// connection pool can later be shared with long-running audio streams.
const API_TIMEOUT: Duration = Duration::from_secs(20);

const DISCOVER: &str = "discover_web";
const TRALBUM_DETAILS: &str = "tralbum_details";
const STREAM_REFRESH: &str = "stream refresh";

/// Bandcamp API client. Cheap to clone: clones share one connection pool.
#[derive(Debug, Clone)]
pub struct BandcampClient {
    http: reqwest::Client,
    base_url: Url,
}

impl BandcampClient {
    /// A client for `https://bandcamp.com`.
    pub fn new() -> Result<Self> {
        // The default is a valid constant.
        #[allow(clippy::expect_used)]
        let base = Url::parse(DEFAULT_BASE_URL).expect("default base URL is valid");
        Self::with_base_url(base)
    }

    /// A client for another host. Used by tests to point at a mock server.
    pub fn with_base_url(mut base_url: Url) -> Result<Self> {
        // `Url::join` drops the last path segment unless the base ends with a slash.
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|source| Error::Request {
                endpoint: "client setup",
                source,
            })?;
        Ok(Self { http, base_url })
    }

    /// The underlying HTTP client, for sharing the connection pool (e.g. with the player).
    #[must_use]
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Fetches one page of the discover feed. Pass `None` for the first page, then the
    /// previous page's [`DiscoverPage::next_cursor`].
    pub async fn discover(
        &self,
        query: &DiscoverQuery,
        cursor: Option<&Cursor>,
    ) -> Result<DiscoverPage> {
        let request = self
            .http
            .post(self.endpoint("api/discover/1/discover_web"))
            .json(&discover::RequestBody::new(query, cursor));
        let response: discover::Response = self.send(DISCOVER, request).await?;
        Ok(response.into())
    }

    /// Fetches a release's details and tracklist. `item` usually comes from a
    /// [`DiscoverItem`]: use its `band_id`, `id` and `kind`.
    pub async fn album(&self, band_id: BandId, id: ItemId, kind: TralbumKind) -> Result<Album> {
        let request = self
            .http
            .get(self.endpoint("api/mobile/25/tralbum_details"))
            .query(&[
                ("band_id", band_id.to_string().as_str()),
                ("tralbum_id", id.to_string().as_str()),
                ("tralbum_type", kind.code()),
            ]);
        let response: album::Response = self.send(TRALBUM_DETAILS, request).await?;
        Ok(response.into())
    }

    /// Gets a fresh signed URL for an expired CDN stream URL (`…bcbits.com/stream/…`).
    ///
    /// Only accepts CDN URLs as served after the `stream_redirect` hop; Bandcamp answers
    /// "invalid url" for redirect URLs and for URLs whose signature doesn't verify.
    pub async fn refresh_stream_url(&self, url: &Url) -> Result<Url> {
        #[derive(Deserialize)]
        struct Refreshed {
            url: Url,
        }
        let request = self
            .http
            .get(self.endpoint("api/stream/1/refresh"))
            .query(&[("url", url.as_str())]);
        let refreshed: Refreshed = self.send(STREAM_REFRESH, request).await?;
        Ok(refreshed.url)
    }

    fn endpoint(&self, path: &str) -> Url {
        // `path` is always one of our relative constants.
        #[allow(clippy::expect_used)]
        self.base_url.join(path).expect("endpoint path is valid")
    }

    async fn send<T: DeserializeOwned>(
        &self,
        endpoint: &'static str,
        request: reqwest::RequestBuilder,
    ) -> Result<T> {
        let request_error = |source| Error::Request { endpoint, source };
        let response = request
            .timeout(API_TIMEOUT)
            .send()
            .await
            .map_err(request_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Status { endpoint, status });
        }
        let body = response.bytes().await.map_err(request_error)?;
        decode(endpoint, &body)
    }
}

/// Parses a `discover_web` response body, e.g. one saved to disk.
pub fn parse_discover(json: &str) -> Result<DiscoverPage> {
    decode::<discover::Response>(DISCOVER, json.as_bytes()).map(Into::into)
}

/// Parses a `tralbum_details` response body.
pub fn parse_album(json: &str) -> Result<Album> {
    decode::<album::Response>(TRALBUM_DETAILS, json.as_bytes()).map(Into::into)
}

/// The two error shapes Bandcamp returns, both with HTTP 200:
/// `{"error": true, "error_message": "..."}` (mobile API) and
/// `{"__api_special__": "exception", "error_type": "..."}` (web API).
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    #[serde(default)]
    error: bool,
    error_message: Option<String>,
    #[serde(rename = "__api_special__")]
    api_special: Option<String>,
    error_type: Option<String>,
}

impl ErrorEnvelope {
    fn into_message(self) -> Option<String> {
        if self.error {
            return Some(
                self.error_message
                    .unwrap_or_else(|| "unknown error".to_owned()),
            );
        }
        if self.api_special.as_deref() == Some("exception") {
            return Some(self.error_type.unwrap_or_else(|| "exception".to_owned()));
        }
        None
    }
}

fn decode<T: DeserializeOwned>(endpoint: &'static str, body: &[u8]) -> Result<T> {
    // Error envelopes are tiny, so a failed probe here costs next to nothing; a non-object body
    // falls through to the real decode, which reports it properly.
    if let Ok(envelope) = serde_json::from_slice::<ErrorEnvelope>(body)
        && let Some(message) = envelope.into_message()
    {
        return Err(Error::Api { endpoint, message });
    }
    let de = &mut serde_json::Deserializer::from_slice(body);
    serde_path_to_error::deserialize(de).map_err(|e| Error::Decode {
        endpoint,
        path: e.path().to_string(),
        source: e.into_inner(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_both_error_shapes() {
        let err = decode::<album::Response>(
            TRALBUM_DETAILS,
            include_bytes!("../../tests/fixtures/error_tralbum.json"),
        )
        .unwrap_err();
        assert!(
            matches!(&err, Error::Api { message, .. } if message == "No such tralbum for band 1: 1"),
            "{err:?}"
        );

        let err = decode::<discover::Response>(
            DISCOVER,
            include_bytes!("../../tests/fixtures/error_exception.json"),
        )
        .unwrap_err();
        assert!(
            matches!(&err, Error::Api { message, .. } if message == "Endpoints::MissingParamError"),
            "{err:?}"
        );
    }

    #[test]
    fn decode_error_reports_field_path() {
        let err = decode::<discover::Response>(
            DISCOVER,
            br#"{"results":[{"item_id":"not a number"}],"cursor":null}"#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, Error::Decode { path, .. } if path == "results[0].item_id"),
            "{err:?}"
        );
    }

    #[test]
    fn base_url_without_trailing_slash_keeps_its_path() {
        let client =
            BandcampClient::with_base_url("http://localhost:1234/proxy".parse().unwrap()).unwrap();
        assert_eq!(
            client.endpoint("api/x").as_str(),
            "http://localhost:1234/proxy/api/x"
        );
    }
}
