//! The filter picker popup: a type-to-filter list of genres, time ranges or locations.

use ratatui::widgets::ListState;

use crate::bandcamp::{DiscoverQuery, Filters};
use crate::fuzzy;

/// The "fresh" time facet is rejected by the API without a login.
const FRESH_TIME_FACET: i64 = -1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Genre,
    Time,
    Location,
}

impl PickerKind {
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Genre => "Genre or tag",
            Self::Time => "Released",
            Self::Location => "Location",
        }
    }
}

/// What selecting an item does to the discover query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Tags(Vec<String>),
    Time(Option<i64>),
    Location(i64),
}

impl Choice {
    /// Applies the choice to `query`.
    pub fn apply(&self, query: &mut DiscoverQuery) {
        match self {
            Self::Tags(tags) => query.tags.clone_from(tags),
            Self::Time(id) => query.time_facet_id = *id,
            Self::Location(id) => query.geoname_id = *id,
        }
    }

    fn is_current(&self, query: &DiscoverQuery) -> bool {
        match self {
            Self::Tags(tags) => *tags == query.tags,
            Self::Time(id) => *id == query.time_facet_id,
            Self::Location(id) => *id == query.geoname_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    pub label: String,
    /// Context shown dimmed, e.g. the parent genre of a subgenre.
    pub detail: Option<String>,
    /// Shown indented under the previous top-level item (when not searching).
    pub nested: bool,
    pub choice: Choice,
}

#[derive(Debug)]
pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    items: Vec<PickerItem>,
    /// "Use this as a tag" entry for text that isn't in the list (genre picker only).
    custom: Option<PickerItem>,
    /// Whether `custom` is listed before the matches (no good match) or after them.
    custom_first: bool,
    /// Indices into `items` matching `query`, best first.
    matches: Vec<usize>,
    pub list: ListState,
}

impl Picker {
    #[must_use]
    pub fn new(kind: PickerKind, filters: &Filters, current: &DiscoverQuery) -> Self {
        let items = match kind {
            PickerKind::Genre => genre_items(filters),
            PickerKind::Time => time_items(filters),
            PickerKind::Location => location_items(filters),
        };
        let mut picker = Self {
            kind,
            query: String::new(),
            items,
            custom: None,
            custom_first: false,
            matches: Vec::new(),
            list: ListState::default(),
        };
        picker.refilter();
        // Open on the current setting.
        let current = picker
            .matches
            .iter()
            .position(|&i| picker.items[i].choice.is_current(current));
        picker.list.select(current.or(Some(0)));
        picker
    }

    /// Items to show, in order.
    pub fn visible(&self) -> impl Iterator<Item = &PickerItem> {
        let (before, after) = if self.custom_first {
            (self.custom.as_ref(), None)
        } else {
            (None, self.custom.as_ref())
        };
        before
            .into_iter()
            .chain(self.matches.iter().map(|&i| &self.items[i]))
            .chain(after)
    }

    #[must_use]
    pub fn visible_len(&self) -> usize {
        self.matches.len() + usize::from(self.custom.is_some())
    }

    #[must_use]
    pub fn is_searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub fn push(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.refilter();
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.refilter();
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        let current = self.list.selected().unwrap_or(0);
        self.list
            .select(Some(current.saturating_add_signed(delta).min(len - 1)));
    }

    /// The highlighted item's choice.
    #[must_use]
    pub fn selected(&self) -> Option<&Choice> {
        let index = self.list.selected()?;
        self.visible().nth(index).map(|item| &item.choice)
    }

    fn refilter(&mut self) {
        let query = self.query.trim();
        let mut scored: Vec<(u32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let haystack = match &item.detail {
                    Some(detail) => format!("{} {detail}", item.label),
                    None => item.label.clone(),
                };
                fuzzy::score(query, &item.label)
                    .or_else(|| fuzzy::score(query, &haystack))
                    .map(|s| (s, i))
            })
            .collect();
        if !query.is_empty() {
            // Stable: equal scores keep the list's natural order.
            scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
        }
        let best = scored.first().map_or(0, |(score, _)| *score);
        self.matches = scored.into_iter().map(|(_, i)| i).collect();

        self.custom = None;
        if self.kind == PickerKind::Genre {
            let tag = normalize_tag(query);
            let exact = self.matches.iter().any(
                |&i| matches!(&self.items[i].choice, Choice::Tags(t) if t.last() == Some(&tag)),
            );
            // Offer the typed text as a tag: first if nothing in the list really matches,
            // otherwise after the matches ("tech" should select "techno", not tag "tech").
            self.custom_first = self.matches.is_empty() || best < fuzzy::SUBSTRING_SCORE;
            if !tag.is_empty() && !exact {
                self.custom = Some(PickerItem {
                    label: format!("Tag “{tag}”"),
                    detail: Some("any Bandcamp tag".into()),
                    nested: false,
                    choice: Choice::Tags(vec![tag]),
                });
            }
        }
        // Searching puts the best match first; select it.
        self.list.select(Some(0));
    }
}

/// Turns free text into a Bandcamp tag norm name: `Drum & Bass` → `drum-bass`.
#[must_use]
pub fn normalize_tag(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

fn genre_items(filters: &Filters) -> Vec<PickerItem> {
    let mut items = vec![PickerItem {
        label: "All genres".into(),
        detail: None,
        nested: false,
        choice: Choice::Tags(vec![]),
    }];
    for genre in &filters.genres {
        items.push(PickerItem {
            label: genre.label.clone(),
            detail: None,
            nested: false,
            choice: Choice::Tags(vec![genre.slug.clone()]),
        });
        for sub in filters.subgenres_of(&genre.slug) {
            items.push(PickerItem {
                label: sub.label.clone(),
                detail: Some(genre.label.clone()),
                nested: true,
                choice: Choice::Tags(vec![genre.slug.clone(), sub.slug.clone()]),
            });
        }
    }
    items
}

fn time_items(filters: &Filters) -> Vec<PickerItem> {
    let mut items = vec![PickerItem {
        label: "Any time".into(),
        detail: None,
        nested: false,
        choice: Choice::Time(None),
    }];
    items.extend(
        filters
            .times
            .iter()
            .filter(|t| t.id != FRESH_TIME_FACET)
            .map(|t| PickerItem {
                label: t.label.clone(),
                detail: None,
                nested: false,
                choice: Choice::Time(Some(t.id)),
            }),
    );
    items
}

fn location_items(filters: &Filters) -> Vec<PickerItem> {
    filters
        .locations
        .iter()
        .map(|l| PickerItem {
            label: if l.id == 0 {
                "Anywhere".into()
            } else {
                l.label.clone()
            },
            detail: None,
            nested: false,
            choice: Choice::Location(l.id),
        })
        .collect()
}

/// Human-readable labels for the current filters, for the header.
#[must_use]
pub fn describe(filters: &Filters, query: &DiscoverQuery) -> Vec<String> {
    let mut parts = Vec::new();
    if query.tags.is_empty() {
        parts.push("all genres".to_owned());
    } else {
        let labels: Vec<&str> = query
            .tags
            .iter()
            .map(|tag| {
                filters
                    .genres
                    .iter()
                    .map(|g| (&g.slug, &g.label))
                    .chain(filters.subgenres.iter().map(|s| (&s.slug, &s.label)))
                    .find(|(slug, _)| *slug == tag)
                    .map_or(tag.as_str(), |(_, label)| label.as_str())
            })
            .collect();
        parts.push(labels.join(" › "));
    }
    if let Some(id) = query.time_facet_id
        && let Some(time) = filters.times.iter().find(|t| t.id == id)
    {
        parts.push(time.label.clone());
    }
    if query.geoname_id != 0 {
        let place = filters
            .locations
            .iter()
            .find(|l| l.id == query.geoname_id)
            .map_or_else(
                || format!("location {}", query.geoname_id),
                |l| l.label.clone(),
            );
        parts.push(place);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(kind: PickerKind) -> Picker {
        Picker::new(kind, Filters::snapshot(), &DiscoverQuery::default())
    }

    fn labels(p: &Picker) -> Vec<&str> {
        p.visible().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn genre_list_nests_subgenres_and_opens_on_current() {
        let p = picker(PickerKind::Genre);
        let items: Vec<_> = p.visible().collect();
        assert_eq!(items[0].label, "All genres");
        assert_eq!(items[1].label, "electronic");
        assert!(items[2].nested, "{} should be a subgenre", items[2].label);
        assert_eq!(p.selected(), Some(&Choice::Tags(vec![])));

        let current = DiscoverQuery {
            tags: vec!["electronic".into(), "techno".into()],
            ..DiscoverQuery::default()
        };
        let p = Picker::new(PickerKind::Genre, Filters::snapshot(), &current);
        assert_eq!(
            p.selected(),
            Some(&Choice::Tags(vec!["electronic".into(), "techno".into()]))
        );
    }

    #[test]
    fn typing_filters_and_ranks() {
        let mut p = picker(PickerKind::Genre);
        for c in "techno".chars() {
            p.push(c);
        }
        assert_eq!(labels(&p)[0], "techno");
        assert_eq!(
            p.selected(),
            Some(&Choice::Tags(vec!["electronic".into(), "techno".into()]))
        );
        assert!(p.custom.is_none(), "an exact match needs no custom tag");
        p.backspace();
        assert_eq!(p.query, "techn");
    }

    #[test]
    fn partial_text_prefers_list_matches_over_a_custom_tag() {
        let mut p = picker(PickerKind::Genre);
        for c in "tech".chars() {
            p.push(c);
        }
        assert_eq!(labels(&p)[0], "techno");
        assert_eq!(labels(&p).last(), Some(&"Tag “tech”"));
    }

    #[test]
    fn unknown_text_offers_a_custom_tag() {
        let mut p = picker(PickerKind::Genre);
        for c in "Lo-Fi House".chars() {
            p.push(c);
        }
        assert_eq!(labels(&p)[0], "Tag “lo-fi-house”");
        assert_eq!(
            p.selected(),
            Some(&Choice::Tags(vec!["lo-fi-house".into()]))
        );
    }

    #[test]
    fn subgenres_match_on_their_parent_too() {
        let mut p = picker(PickerKind::Genre);
        for c in "metal doom".chars() {
            p.push(c);
        }
        assert!(labels(&p).contains(&"doom"), "{:?}", labels(&p));
    }

    #[test]
    fn time_picker_skips_fresh() {
        let p = picker(PickerKind::Time);
        assert_eq!(labels(&p)[0], "Any time");
        assert!(!labels(&p).contains(&"fresh"));
        assert!(labels(&p).contains(&"this week"));
    }

    #[test]
    fn location_picker_searches_cities() {
        let mut p = picker(PickerKind::Location);
        assert_eq!(labels(&p)[0], "Anywhere");
        for c in "berl".chars() {
            p.push(c);
        }
        assert_eq!(p.selected(), Some(&Choice::Location(2_950_159)));
        assert!(p.custom.is_none(), "no free-text locations");
    }

    #[test]
    fn movement_is_clamped() {
        let mut p = picker(PickerKind::Time);
        p.move_by(-5);
        assert_eq!(p.list.selected(), Some(0));
        p.move_by(100);
        assert_eq!(p.list.selected(), Some(p.visible_len() - 1));
    }

    #[test]
    fn normalizes_tags_like_bandcamp() {
        assert_eq!(normalize_tag("Drum & Bass"), "drum-bass");
        assert_eq!(normalize_tag("hip-hop/rap"), "hip-hop-rap");
        assert_eq!(normalize_tag("  Lo-Fi  "), "lo-fi");
        assert_eq!(normalize_tag("!!!"), "");
    }

    #[test]
    fn describes_filters_for_the_header() {
        let query = DiscoverQuery {
            tags: vec!["hip-hop-rap".into(), "unknown-tag".into()],
            time_facet_id: Some(1),
            geoname_id: 2_950_159,
            ..DiscoverQuery::default()
        };
        assert_eq!(
            describe(Filters::snapshot(), &query),
            ["hip-hop/rap › unknown-tag", "this week", "berlin"]
        );
        assert_eq!(
            describe(Filters::snapshot(), &DiscoverQuery::default()),
            ["all genres"]
        );
    }
}
