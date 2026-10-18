//! Search (`bcsearch_public_api/1/autocomplete_elastic`, what bandcamp.com's search box uses) and
//! artist pages (`mobile/25/band_details`, for the discography).

use serde::{Deserialize, Serialize};
use url::Url;

use super::discover::DiscoverItem;
use super::types::{ArtId, BandId, Date, ImageId, ItemId, TralbumKind};

/// One search hit.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchResult {
    /// An album or track, ready to show in the release list.
    Release(DiscoverItem),
    Artist(ArtistMatch),
}

/// An artist or label found by search; open it to list its releases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistMatch {
    pub band_id: BandId,
    pub name: String,
    pub location: Option<String>,
    pub genre: Option<String>,
    pub is_label: bool,
    pub image_id: Option<ImageId>,
}

/// An artist's page: who they are and everything they've released.
#[derive(Debug, Clone, PartialEq)]
pub struct Artist {
    pub band_id: BandId,
    pub name: String,
    pub location: Option<String>,
    pub bio: Option<String>,
    pub url: Url,
    /// Newest first, as Bandcamp lists them. Their `url` is the artist page: the discography
    /// doesn't include release URLs (the album's own URL arrives with its details).
    pub releases: Vec<DiscoverItem>,
}

// ---- Wire format: search -----------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct SearchBody<'a> {
    search_text: &'a str,
    /// Empty: albums, tracks and artists together.
    search_filter: &'static str,
    full_page: bool,
    fan_id: Option<u64>,
}

impl<'a> SearchBody<'a> {
    pub(crate) fn new(query: &'a str) -> Self {
        Self {
            search_text: query,
            search_filter: "",
            full_page: false,
            fan_id: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct SearchResponse {
    auto: SearchResults,
}

#[derive(Debug, Deserialize)]
struct SearchResults {
    #[serde(default)]
    results: Vec<WireHit>,
}

#[derive(Debug, Deserialize)]
struct WireHit {
    #[serde(rename = "type")]
    kind: String,
    id: u64,
    name: String,
    art_id: Option<u64>,
    img_id: Option<u64>,
    band_id: Option<u64>,
    band_name: Option<String>,
    item_url_path: Option<Url>,
    item_url_root: Option<Url>,
    location: Option<String>,
    genre_name: Option<String>,
    #[serde(default)]
    is_label: bool,
}

impl From<SearchResponse> for Vec<SearchResult> {
    fn from(r: SearchResponse) -> Self {
        r.auto
            .results
            .into_iter()
            .filter_map(WireHit::into_domain)
            .collect()
    }
}

impl WireHit {
    fn into_domain(self) -> Option<SearchResult> {
        let release_kind = match self.kind.as_str() {
            "a" => TralbumKind::Album,
            "t" => TralbumKind::Track,
            "b" => {
                return Some(SearchResult::Artist(ArtistMatch {
                    band_id: BandId(self.id),
                    name: self.name,
                    location: self.location.filter(|l| !l.is_empty()),
                    genre: self.genre_name.filter(|g| !g.is_empty()),
                    is_label: self.is_label,
                    image_id: self.img_id.map(ImageId),
                }));
            }
            // Fans and anything new.
            _ => return None,
        };
        let band_name = self.band_name.unwrap_or_default();
        Some(SearchResult::Release(DiscoverItem {
            id: ItemId(self.id),
            kind: release_kind,
            title: self.name,
            artist: band_name.clone(),
            band_id: BandId(self.band_id?),
            band_name,
            band_location: None,
            url: self.item_url_path.or(self.item_url_root)?,
            art_id: self.art_id.map(ArtId),
            release_date: None,
            track_count: None,
            duration: None,
            price: None,
            is_preorder: false,
            is_free_download: false,
        }))
    }
}

// ---- Wire format: band_details -----------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct BandResponse {
    id: u64,
    name: String,
    location: Option<String>,
    bio: Option<String>,
    bandcamp_url: Url,
    #[serde(default)]
    discography: Vec<WireRelease>,
}

#[derive(Debug, Deserialize)]
struct WireRelease {
    item_id: u64,
    item_type: String,
    title: String,
    artist_name: Option<String>,
    band_name: Option<String>,
    band_id: Option<u64>,
    art_id: Option<u64>,
    release_date: Option<String>,
}

impl From<BandResponse> for Artist {
    fn from(r: BandResponse) -> Self {
        let url = r.bandcamp_url;
        let releases = r
            .discography
            .into_iter()
            .filter_map(|d| {
                let kind = match d.item_type.as_str() {
                    "album" => TralbumKind::Album,
                    "track" => TralbumKind::Track,
                    _ => return None,
                };
                let band_name = d.band_name.unwrap_or_else(|| r.name.clone());
                Some(DiscoverItem {
                    id: ItemId(d.item_id),
                    kind,
                    title: d.title,
                    artist: d
                        .artist_name
                        .filter(|a| !a.trim().is_empty())
                        .unwrap_or_else(|| band_name.clone()),
                    band_id: BandId(d.band_id.unwrap_or(r.id)),
                    band_name,
                    band_location: r.location.clone(),
                    url: url.clone(),
                    art_id: d.art_id.map(ArtId),
                    release_date: d.release_date.as_deref().and_then(Date::parse_dmy),
                    track_count: None,
                    duration: None,
                    price: None,
                    is_preorder: false,
                    is_free_download: false,
                })
            })
            .collect();
        Self {
            band_id: BandId(r.id),
            name: r.name,
            location: r.location.filter(|l| !l.is_empty()),
            bio: r.bio.filter(|b| !b.trim().is_empty()),
            url,
            releases,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mixed_search_results() {
        let response: SearchResponse =
            serde_json::from_str(include_str!("../../tests/fixtures/search_aphex.json")).unwrap();
        let results: Vec<SearchResult> = response.into();
        assert_eq!(results.len(), 7);

        let SearchResult::Artist(artist) = &results[0] else {
            panic!("{:?}", results[0]);
        };
        assert_eq!(artist.name, "Aphex Twin");
        assert_eq!(artist.band_id, BandId(1_533_096_991));
        assert_eq!(artist.location.as_deref(), Some("UK"));
        assert!(!artist.is_label);

        let SearchResult::Release(album) = &results[1] else {
            panic!("{:?}", results[1]);
        };
        assert_eq!(album.kind, TralbumKind::Album);
        assert_eq!(album.title, "Drukqs");
        assert_eq!(album.artist, "Aphex Twin");
        assert!(album.art_id.is_some());
        assert_eq!(
            album.url.as_str(),
            "https://aphextwin.bandcamp.com/album/drukqs"
        );

        let SearchResult::Release(track) = &results[5] else {
            panic!("{:?}", results[5]);
        };
        assert_eq!(track.kind, TralbumKind::Track);
        assert_eq!(track.band_name, "Curve");
    }

    #[test]
    fn empty_search_and_unknown_types() {
        let response: SearchResponse =
            serde_json::from_str(r#"{"auto":{"results":[{"type":"f","id":1,"name":"a fan"}]}}"#)
                .unwrap();
        assert_eq!(Vec::<SearchResult>::from(response), []);
    }

    #[test]
    fn parses_an_artist_discography() {
        let response: BandResponse =
            serde_json::from_str(include_str!("../../tests/fixtures/band_aphex.json")).unwrap();
        let artist: Artist = response.into();
        assert_eq!(artist.name, "Aphex Twin");
        assert_eq!(artist.releases.len(), 5);
        let drukqs = &artist.releases[0];
        assert_eq!(drukqs.title, "Drukqs");
        assert_eq!(drukqs.artist, "Aphex Twin");
        assert_eq!(drukqs.kind, TralbumKind::Album);
        assert_eq!(drukqs.band_id, BandId(1_533_096_991));
        assert_eq!(
            drukqs.release_date,
            Some(Date {
                year: 2001,
                month: 10,
                day: 22
            })
        );
        assert_eq!(drukqs.url.as_str(), "https://aphextwin.bandcamp.com/");
        assert_eq!(artist.releases[4].kind, TralbumKind::Track);
    }
}
