//! `GET /api/mobile/25/tralbum_details`: full album (or track) metadata with stream URLs.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use url::Url;

use super::types::{ArtId, BandId, Date, ImageId, ItemId, TrackId, TralbumKind};

/// A release with its full tracklist.
#[derive(Debug, Clone, PartialEq)]
pub struct Album {
    pub id: ItemId,
    pub kind: TralbumKind,
    pub title: String,
    pub artist: String,
    pub band: Band,
    pub url: Url,
    pub art_id: Option<ArtId>,
    pub release_date: Option<Date>,
    /// Free-form description ("about this album").
    pub about: Option<String>,
    pub credits: Option<String>,
    pub label: Option<String>,
    pub tags: Vec<Tag>,
    pub is_preorder: bool,
    pub tracks: Vec<Track>,
}

impl Album {
    /// Total running time of all tracks.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.tracks.iter().map(|t| t.duration).sum()
    }

    /// Whether any track can be played without buying the release.
    #[must_use]
    pub fn has_playable_tracks(&self) -> bool {
        self.tracks.iter().any(Track::is_playable)
    }
}

/// The account (artist or label) that published a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Band {
    pub id: BandId,
    pub name: String,
    pub bio: Option<String>,
    pub location: Option<String>,
    pub image_id: Option<ImageId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// Usable as a discover tag filter.
    pub norm_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: TrackId,
    /// Position on the release, starting at 1.
    pub number: Option<u32>,
    pub title: String,
    /// The track's artist. Differs from the album artist on compilations.
    pub artist: String,
    pub duration: Duration,
    /// 128 kbps MP3 stream. This is a `bandcamp.com/stream_redirect` URL that redirects to the
    /// CDN. Signed and valid for about 24 hours. `None` for tracks that can't be streamed
    /// (pre-orders, tracks held back until release).
    pub stream_url: Option<Url>,
}

impl Track {
    #[must_use]
    pub fn is_playable(&self) -> bool {
        self.stream_url.is_some()
    }
}

// ---- Wire format -------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct Response {
    id: u64,
    #[serde(rename = "type")]
    kind: WireKind,
    title: String,
    tralbum_artist: Option<String>,
    band: WireBand,
    bandcamp_url: Url,
    art_id: Option<u64>,
    release_date: Option<i64>,
    about: Option<String>,
    credits: Option<String>,
    label: Option<String>,
    #[serde(default)]
    tags: Vec<WireTag>,
    #[serde(default)]
    is_preorder: bool,
    #[serde(default)]
    tracks: Vec<WireTrack>,
}

#[derive(Debug, Deserialize)]
enum WireKind {
    #[serde(rename = "a")]
    Album,
    #[serde(rename = "t")]
    Track,
}

#[derive(Debug, Deserialize)]
struct WireBand {
    band_id: u64,
    name: String,
    bio: Option<String>,
    location: Option<String>,
    image_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WireTag {
    name: String,
    norm_name: String,
}

#[derive(Debug, Deserialize)]
struct WireTrack {
    track_id: u64,
    title: String,
    track_num: Option<u32>,
    band_name: Option<String>,
    duration: Option<f64>,
    #[serde(default)]
    is_streamable: bool,
    /// Keyed by encoding; anonymous users only get `mp3-128`.
    streaming_url: Option<HashMap<String, Url>>,
}

const STREAM_ENCODING: &str = "mp3-128";

impl From<Response> for Album {
    fn from(r: Response) -> Self {
        let artist = non_empty(r.tralbum_artist).unwrap_or_else(|| r.band.name.clone());
        let tracks = r
            .tracks
            .into_iter()
            .map(|t| Track {
                id: TrackId(t.track_id),
                number: t.track_num,
                artist: non_empty(t.band_name).unwrap_or_else(|| artist.clone()),
                title: t.title,
                duration: t
                    .duration
                    .filter(|d| d.is_finite() && *d > 0.0)
                    .map(Duration::from_secs_f64)
                    .unwrap_or_default(),
                stream_url: t
                    .is_streamable
                    .then(|| t.streaming_url?.remove(STREAM_ENCODING))
                    .flatten(),
            })
            .collect();

        Self {
            id: ItemId(r.id),
            kind: match r.kind {
                WireKind::Album => TralbumKind::Album,
                WireKind::Track => TralbumKind::Track,
            },
            title: r.title,
            artist,
            band: Band {
                id: BandId(r.band.band_id),
                name: r.band.name,
                bio: non_empty(r.band.bio),
                location: non_empty(r.band.location),
                image_id: r.band.image_id.map(ImageId),
            },
            url: r.bandcamp_url,
            art_id: r.art_id.map(ArtId),
            release_date: r.release_date.map(Date::from_unix),
            about: non_empty(r.about),
            credits: non_empty(r.credits),
            label: non_empty(r.label),
            tags: r
                .tags
                .into_iter()
                .map(|t| Tag {
                    name: t.name,
                    norm_name: t.norm_name,
                })
                .collect(),
            is_preorder: r.is_preorder,
            tracks,
        }
    }
}

/// Bandcamp uses both `null` and `""` for missing text.
fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Album {
        serde_json::from_str::<Response>(json).unwrap().into()
    }

    #[test]
    fn parses_album() {
        let album = parse(include_str!("../../tests/fixtures/tralbum_album.json"));
        assert_eq!(album.id, ItemId(3_457_085_593));
        assert_eq!(album.kind, TralbumKind::Album);
        assert_eq!(album.title, "Ocean Company");
        assert_eq!(album.artist, "Ocean Company");
        assert_eq!(album.band.name, "Hotel Pools");
        assert_eq!(album.band.location.as_deref(), Some("Portland, Oregon"));
        assert_eq!(album.label.as_deref(), Some("Stratford Ct."));
        assert_eq!(album.credits, None);
        assert_eq!(album.art_id, Some(ArtId(4_072_557_709)));
        assert_eq!(
            album.release_date,
            Some(Date {
                year: 2026,
                month: 10,
                day: 16
            })
        );
        assert!(album.tags.iter().any(|t| t.norm_name == "chillwave"));

        assert_eq!(album.tracks.len(), 10);
        assert!(album.tracks.iter().all(Track::is_playable));
        let first = &album.tracks[0];
        assert_eq!(first.number, Some(1));
        assert_eq!(first.title, "Awake");
        assert_eq!(first.duration.as_millis(), 222_334);
        let url = first.stream_url.as_ref().unwrap();
        assert_eq!(url.path(), "/stream_redirect");
        assert_eq!(album.duration().as_secs(), 2279);
    }

    #[test]
    fn preorder_tracks_are_not_playable() {
        let album = parse(include_str!("../../tests/fixtures/tralbum_preorder.json"));
        assert!(album.is_preorder);
        assert!(
            !album.tracks.is_empty(),
            "pre-orders still list their tracks"
        );
        assert!(!album.has_playable_tracks());
        assert!(album.tracks.iter().all(|t| t.duration > Duration::ZERO));
    }

    #[test]
    fn compilation_has_per_track_artists() {
        let album = parse(include_str!(
            "../../tests/fixtures/tralbum_compilation.json"
        ));
        assert_eq!(album.artist, "Various Artists");
        assert_eq!(album.band.name, "Habibi Funk Records");
        assert_eq!(album.label, None);
        assert_eq!(album.tracks[0].artist, "Stephanie Stephanou");
        assert_eq!(album.tracks[1].artist, "Force");
    }

    #[test]
    fn parses_single_track() {
        let track = parse(include_str!("../../tests/fixtures/tralbum_track.json"));
        assert_eq!(track.kind, TralbumKind::Track);
        assert_eq!(track.title, "Windowlicker");
        assert_eq!(track.tracks.len(), 1);
        assert!(track.tracks[0].is_playable());
        assert_eq!(track.band.bio, None);
        assert_eq!(
            track.release_date,
            Some(Date {
                year: 1999,
                month: 3,
                day: 22
            })
        );
    }

    #[test]
    fn streamable_flag_wins_over_url() {
        let album = parse(
            r#"{"id":1,"type":"a","title":"T","tralbum_artist":"","bandcamp_url":"https://x.bandcamp.com/album/t",
                "band":{"band_id":2,"name":"X","bio":"","location":null,"image_id":null},
                "tracks":[
                  {"track_id":10,"title":"a","track_num":1,"is_streamable":false,
                   "streaming_url":{"mp3-128":"https://bandcamp.com/stream_redirect?x"}},
                  {"track_id":11,"title":"b","track_num":2,"is_streamable":true,"streaming_url":{}},
                  {"track_id":12,"title":"c","track_num":null,"is_streamable":true,"duration":null,
                   "streaming_url":{"mp3-128":"https://bandcamp.com/stream_redirect?y"}}
                ]}"#,
        );
        assert_eq!(album.artist, "X");
        assert_eq!(album.band.bio, None);
        let playable: Vec<_> = album.tracks.iter().map(Track::is_playable).collect();
        assert_eq!(playable, [false, false, true]);
        assert_eq!(album.tracks[2].artist, "X");
        assert_eq!(album.tracks[2].duration, Duration::ZERO);
    }
}
