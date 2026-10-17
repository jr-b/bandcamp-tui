//! `POST /api/discover/1/discover_web`: the feed behind <https://bandcamp.com/discover/>.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

use super::types::{ArtId, BandId, Date, ItemId, Price, TralbumKind};

/// Discover sort order (`slice`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
pub enum Slice {
    /// Best-selling.
    #[default]
    #[serde(rename = "top")]
    Top,
    /// New arrivals.
    #[serde(rename = "new")]
    New,
    /// "Surprise me": a random selection, different on every request.
    #[serde(rename = "rand")]
    Random,
}

/// Filters for a discover request. The defaults match the discover page with nothing selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverQuery {
    /// Genre/subgenre/tag norm names, e.g. `["electronic", "techno"]`. Empty means all genres.
    pub tags: Vec<String>,
    pub slice: Slice,
    /// A [`Filters::times`](super::Filters) id. `None` is Bandcamp's default for the slice.
    pub time_facet_id: Option<i64>,
    /// A [`Filters::locations`](super::Filters) geoname id. `0` means anywhere.
    pub geoname_id: i64,
    /// A [`Filters::categories`](super::Filters) id. `0` means all formats.
    pub category_id: i64,
    /// Results per page. The website uses 60.
    pub page_size: u32,
}

impl Default for DiscoverQuery {
    fn default() -> Self {
        Self {
            tags: Vec::new(),
            slice: Slice::default(),
            time_facet_id: None,
            geoname_id: 0,
            category_id: 0,
            page_size: 60,
        }
    }
}

/// Opaque pagination token returned with each page.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cursor(pub(crate) String);

/// One page of discover results.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoverPage {
    pub items: Vec<DiscoverItem>,
    /// Total number of matching releases, e.g. for "Discover (1,959,593)".
    pub total: u64,
    /// Pass this to the next request; `None` means there are no more pages.
    pub next_cursor: Option<Cursor>,
}

/// A release in the discover feed. Contains everything needed to fetch the full album with
/// [`BandcampClient::album`](super::BandcampClient::album).
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoverItem {
    pub id: ItemId,
    pub kind: TralbumKind,
    pub title: String,
    /// The release's artist; for label releases, this differs from `band_name`.
    pub artist: String,
    pub band_id: BandId,
    /// The account (artist or label) that published the release.
    pub band_name: String,
    pub band_location: Option<String>,
    /// Release page, without the `?from=discover_page` tracking parameter.
    pub url: Url,
    pub art_id: Option<ArtId>,
    pub release_date: Option<Date>,
    pub track_count: Option<u32>,
    pub duration: Option<Duration>,
    pub price: Option<Price>,
    pub is_preorder: bool,
    pub is_free_download: bool,
}

// ---- Wire format -------------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct RequestBody<'a> {
    category_id: i64,
    tag_norm_names: &'a [String],
    geoname_id: i64,
    slice: Slice,
    time_facet_id: Option<i64>,
    cursor: &'a str,
    size: u32,
    /// `a` = albums/tracks. We never ask for `s` (merch), which can't be played.
    include_result_types: [&'static str; 1],
    followed_bands: bool,
}

impl<'a> RequestBody<'a> {
    pub(crate) fn new(query: &'a DiscoverQuery, cursor: Option<&'a Cursor>) -> Self {
        Self {
            category_id: query.category_id,
            tag_norm_names: &query.tags,
            geoname_id: query.geoname_id,
            slice: query.slice,
            time_facet_id: query.time_facet_id,
            cursor: cursor.map_or("*", |c| c.0.as_str()),
            size: query.page_size,
            include_result_types: ["a"],
            followed_bands: false,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Response {
    results: Vec<WireItem>,
    #[serde(default)]
    result_count: u64,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireItem {
    item_id: u64,
    item_type: WireItemType,
    title: String,
    item_url: Url,
    band_id: u64,
    band_name: String,
    album_artist: Option<String>,
    band_location: Option<String>,
    primary_image: Option<WireImage>,
    release_date: Option<String>,
    track_count: Option<u32>,
    duration: Option<f64>,
    price: Option<WirePrice>,
    is_album_preorder: Option<bool>,
    is_free_download: Option<bool>,
}

#[derive(Debug, Deserialize)]
enum WireItemType {
    #[serde(rename = "a")]
    Album,
    #[serde(rename = "t")]
    Track,
    /// Merch packages (`p`) and anything Bandcamp adds later.
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct WireImage {
    image_id: u64,
    #[serde(default)]
    is_art: bool,
}

#[derive(Debug, Deserialize)]
struct WirePrice {
    amount: u64,
    currency: String,
}

impl From<Response> for DiscoverPage {
    fn from(r: Response) -> Self {
        Self {
            items: r
                .results
                .into_iter()
                .filter_map(WireItem::into_domain)
                .collect(),
            total: r.result_count,
            next_cursor: r.cursor.filter(|c| !c.is_empty()).map(Cursor),
        }
    }
}

impl WireItem {
    fn into_domain(self) -> Option<DiscoverItem> {
        let kind = match self.item_type {
            WireItemType::Album => TralbumKind::Album,
            WireItemType::Track => TralbumKind::Track,
            WireItemType::Other => return None,
        };
        Some(DiscoverItem {
            id: ItemId(self.item_id),
            kind,
            artist: self
                .album_artist
                .filter(|a| !a.trim().is_empty())
                .unwrap_or_else(|| self.band_name.clone()),
            title: self.title,
            band_id: BandId(self.band_id),
            band_name: self.band_name,
            band_location: self.band_location.filter(|l| !l.is_empty()),
            url: strip_query(self.item_url),
            art_id: self
                .primary_image
                .filter(|i| i.is_art)
                .map(|i| ArtId(i.image_id)),
            release_date: self
                .release_date
                .as_deref()
                .and_then(Date::parse_ymd_prefix),
            track_count: self.track_count,
            duration: self
                .duration
                .filter(|d| d.is_finite() && *d > 0.0)
                .map(Duration::from_secs_f64),
            price: self.price.map(|p| Price {
                amount_minor: p.amount,
                currency: p.currency,
            }),
            is_preorder: self.is_album_preorder.unwrap_or(false),
            is_free_download: self.is_free_download.unwrap_or(false),
        })
    }
}

fn strip_query(mut url: Url) -> Url {
    url.set_query(None);
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> DiscoverPage {
        serde_json::from_str::<Response>(json).unwrap().into()
    }

    #[test]
    fn parses_fixture() {
        let page = parse(include_str!(
            "../../tests/fixtures/discover_electronic_top.json"
        ));
        assert_eq!(page.items.len(), 4);
        assert!(page.total > 1_000);
        assert!(page.next_cursor.is_some());

        let first = &page.items[0];
        assert_eq!(first.id, ItemId(3_457_085_593));
        assert_eq!(first.kind, TralbumKind::Album);
        assert_eq!(first.title, "Ocean Company");
        assert_eq!(first.artist, "Ocean Company");
        assert_eq!(first.band_name, "Hotel Pools");
        assert_eq!(first.band_id, BandId(2_686_449_259));
        assert_eq!(
            first.url.as_str(),
            "https://hotelpoolsmusic.bandcamp.com/album/ocean-company"
        );
        assert_eq!(first.art_id, Some(ArtId(4_072_557_709)));
        assert_eq!(
            first.release_date,
            Some(Date {
                year: 2026,
                month: 10,
                day: 16
            })
        );
        assert_eq!(first.track_count, Some(10));
        assert_eq!(first.duration.map(|d| d.as_secs()), Some(2279));
        assert_eq!(
            first.price,
            Some(Price {
                amount_minor: 1000,
                currency: "USD".into()
            })
        );
    }

    #[test]
    fn artist_falls_back_to_band_name() {
        let page = parse(include_str!(
            "../../tests/fixtures/discover_electronic_top.json"
        ));
        let label_release = page
            .items
            .iter()
            .find(|i| i.band_name == "N4 Records")
            .unwrap();
        assert_eq!(label_release.artist, "N4 Records");
    }

    #[test]
    fn empty_page_has_no_cursor() {
        let page = parse(include_str!("../../tests/fixtures/discover_empty.json"));
        assert_eq!(page.items, []);
        assert_eq!(page.total, 0);
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn skips_merch_and_tolerates_nulls() {
        let page = parse(
            r#"{"results":[
                {"item_id":1,"item_type":"p","title":"T-Shirt","item_url":"https://x.bandcamp.com/merch/t",
                 "band_id":2,"band_name":"X"},
                {"item_id":3,"item_type":"a","title":"LP","item_url":"https://x.bandcamp.com/album/lp",
                 "band_id":2,"band_name":"X","album_artist":"","band_location":null,"primary_image":null,
                 "release_date":null,"track_count":null,"duration":null,"price":null,
                 "is_album_preorder":null,"is_free_download":null,"some_new_field":42}
            ],"result_count":2,"cursor":null}"#,
        );
        assert_eq!(page.items.len(), 1);
        let lp = &page.items[0];
        assert_eq!(lp.artist, "X");
        assert_eq!(lp.art_id, None);
        assert_eq!(lp.duration, None);
        assert!(!lp.is_preorder);
    }

    #[test]
    fn request_body_matches_website() {
        let query = DiscoverQuery {
            tags: vec!["electronic".into()],
            ..DiscoverQuery::default()
        };
        let first = serde_json::to_value(RequestBody::new(&query, None)).unwrap();
        assert_eq!(
            first,
            serde_json::json!({
                "category_id": 0,
                "tag_norm_names": ["electronic"],
                "geoname_id": 0,
                "slice": "top",
                "time_facet_id": null,
                "cursor": "*",
                "size": 60,
                "include_result_types": ["a"],
                "followed_bands": false
            })
        );

        let cursor = Cursor("abc".into());
        let next = serde_json::to_value(RequestBody::new(&query, Some(&cursor))).unwrap();
        assert_eq!(next["cursor"], "abc");
    }
}
