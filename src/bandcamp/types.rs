//! Small domain types shared by the discover and album APIs.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_type!(
    /// An album or track id (`item_id` in discover, `id` in `tralbum_details`).
    ItemId
);
id_type!(
    /// An artist or label account id.
    BandId
);
id_type!(
    /// A track id.
    TrackId
);
id_type!(
    /// Album cover art id; see [`images::art_url`](super::images::art_url).
    ArtId
);
id_type!(
    /// Any other image (band photos, merch); see [`images::image_url`](super::images::image_url).
    ImageId
);

/// Whether a release is a whole album or a standalone track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TralbumKind {
    Album,
    Track,
}

impl TralbumKind {
    /// The single-letter code Bandcamp uses in query strings (`tralbum_type`).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Album => "a",
            Self::Track => "t",
        }
    }
}

/// A calendar date (UTC). Bandcamp only gives us day precision that matters for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl Date {
    /// Parses the leading `YYYY-MM-DD` of strings like `"2026-10-16 00:00:00 UTC"`.
    #[must_use]
    pub fn parse_ymd_prefix(s: &str) -> Option<Self> {
        let ymd = s.get(..10)?;
        let mut parts = ymd.split('-');
        let year = parts.next()?.parse().ok()?;
        let month: u8 = parts.next()?.parse().ok()?;
        let day: u8 = parts.next()?.parse().ok()?;
        ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some(Self { year, month, day })
    }

    /// Converts a unix timestamp (seconds) to a UTC date.
    #[must_use]
    pub fn from_unix(secs: i64) -> Self {
        // Howard Hinnant's `civil_from_days`, see
        // https://howardhinnant.github.io/date_algorithms.html#civil_from_days
        let z = secs.div_euclid(86_400) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        // The ranges are guaranteed by the algorithm: day 1..=31, month 1..=12.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Self {
            year: year as i32,
            month: month as u8,
            day: day as u8,
        }
    }
}

impl fmt::Display for Date {
    /// Formats like Bandcamp does: `16 Oct 2026`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let month = MONTHS
            .get(usize::from(self.month).wrapping_sub(1))
            .unwrap_or(&"???");
        write!(f, "{} {month} {}", self.day, self.year)
    }
}

/// A price as Bandcamp reports it in discover results: an integer amount in the currency's
/// minor unit (cents for USD, whole yen for JPY).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Price {
    pub amount_minor: u64,
    /// ISO 4217 code, e.g. `USD`.
    pub currency: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_from_unix() {
        assert_eq!(
            Date::from_unix(0),
            Date {
                year: 1970,
                month: 1,
                day: 1
            }
        );
        // 2026-10-16T00:00:00Z, the release date in the album fixture.
        assert_eq!(
            Date::from_unix(1_792_108_800),
            Date {
                year: 2026,
                month: 10,
                day: 16
            }
        );
        // Leap day.
        assert_eq!(
            Date::from_unix(951_782_400),
            Date {
                year: 2000,
                month: 2,
                day: 29
            }
        );
        // Before the epoch.
        assert_eq!(
            Date::from_unix(-86_400),
            Date {
                year: 1969,
                month: 12,
                day: 31
            }
        );
    }

    #[test]
    fn date_parse_prefix() {
        assert_eq!(
            Date::parse_ymd_prefix("2026-10-16 00:00:00 UTC"),
            Some(Date {
                year: 2026,
                month: 10,
                day: 16
            })
        );
        assert_eq!(Date::parse_ymd_prefix("2026-13-01"), None);
        assert_eq!(Date::parse_ymd_prefix("garbage"), None);
        assert_eq!(Date::parse_ymd_prefix(""), None);
    }

    #[test]
    fn date_display() {
        assert_eq!(
            Date {
                year: 2026,
                month: 10,
                day: 16
            }
            .to_string(),
            "16 Oct 2026"
        );
    }
}
