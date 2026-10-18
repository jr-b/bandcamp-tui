//! Discover filter metadata: categories, genres, subgenres, slices, time facets and locations.
//!
//! These rarely change, so a snapshot is embedded at compile time. It was taken from the
//! `data-blob` attribute of <https://bandcamp.com/discover/> (`appData.initialState`).

use std::sync::LazyLock;

use serde::Deserialize;

const SNAPSHOT: &str = include_str!("../../assets/discover_filters.json");

static FILTERS: LazyLock<Filters> = LazyLock::new(|| {
    // The snapshot is compiled in and covered by a unit test, so a parse failure is a build bug.
    #[allow(clippy::expect_used)]
    serde_json::from_str(SNAPSHOT).expect("embedded discover_filters.json is valid")
});

/// A selectable filter option.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Facet {
    pub id: i64,
    pub label: String,
    pub slug: String,
}

/// A subgenre, which belongs to a genre (`parent_slug`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subgenre {
    pub id: i64,
    pub label: String,
    pub slug: String,
    pub parent_slug: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Filters {
    /// Formats: all, digital, vinyl, cd, cassette, t-shirt. `id` is the `category_id`.
    pub categories: Vec<Facet>,
    /// Top-level genres; `slug` is a tag norm name.
    pub genres: Vec<Facet>,
    pub subgenres: Vec<Subgenre>,
    /// Sort orders; `slug` is one of `top`, `new`, `rand`. See [`Slice`](super::Slice).
    pub slices: Vec<Facet>,
    /// Time ranges. `id` is the `time_facet_id`.
    pub times: Vec<Facet>,
    /// Curated cities. `id` is the `geoname_id`; 0 means anywhere.
    pub locations: Vec<Facet>,
}

impl Filters {
    /// The embedded snapshot.
    #[must_use]
    pub fn snapshot() -> &'static Self {
        &FILTERS
    }

    /// Subgenres of the genre with the given slug, in Bandcamp's order.
    pub fn subgenres_of<'a>(&'a self, genre_slug: &'a str) -> impl Iterator<Item = &'a Subgenre> {
        self.subgenres
            .iter()
            .filter(move |s| s.parent_slug == genre_slug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_parses() {
        let f = Filters::snapshot();
        assert!(f.genres.len() > 20);
        assert!(f.subgenres.len() > 100);
        assert!(f.genres.iter().any(|g| g.slug == "electronic"));
        let slices: Vec<_> = f.slices.iter().map(|s| s.slug.as_str()).collect();
        assert_eq!(slices, ["top", "new", "rand"]);
        assert_eq!(f.locations.first().map(|l| l.id), Some(0));
    }

    #[test]
    fn every_subgenre_has_a_genre() {
        let f = Filters::snapshot();
        for sub in &f.subgenres {
            assert!(
                f.genres.iter().any(|g| g.slug == sub.parent_slug),
                "{} has unknown parent {}",
                sub.slug,
                sub.parent_slug
            );
        }
        assert!(f.subgenres_of("electronic").any(|s| s.slug == "techno"));
    }
}
