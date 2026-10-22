//! The `/` search box, and the search-result and artist lists it opens.

use ratatui::widgets::{ListState, TableState};

use super::{Action, App, DiscoverList, Effect, Focus, ListSource, move_list};
use crate::bandcamp::{Artist, BandId, DiscoverItem, SearchResult};

impl App {
    pub(super) fn next_list_generation(&mut self) -> u64 {
        self.list_generation += 1;
        self.list_generation
    }

    /// Handles `action` while the search box is open; `None` means "handle normally".
    pub(super) fn search_update(&mut self, action: &Action) -> Option<Vec<Effect>> {
        let len = self.search.results.len();
        let mut edited = false;
        match action {
            Action::Up => move_list(&mut self.search.list, len, -1),
            Action::Down => move_list(&mut self.search.list, len, 1),
            Action::PageUp => move_list(&mut self.search.list, len, -10),
            Action::PageDown => move_list(&mut self.search.list, len, 10),
            Action::Edit(edit) => edited = self.search.query.apply(*edit),
            Action::ClosePopup => self.search_open = false,
            Action::Play => return Some(self.open_search_result()),
            Action::Quit => return None,
            _ if action.is_user_input() => {}
            _ => return None,
        }
        if !edited {
            return Some(vec![]);
        }
        self.search.generation += 1;
        self.search.error = None;
        if self.search.query.as_str().trim().is_empty() {
            self.search.results.clear();
            self.search.loading = false;
            return Some(vec![]);
        }
        self.search.loading = true;
        Some(vec![Effect::DebounceSearch(self.search.generation)])
    }

    pub(super) fn search_due(&mut self, generation: u64) -> Vec<Effect> {
        let query = self.search.query.as_str().trim();
        if generation != self.search.generation || query.is_empty() {
            return vec![];
        }
        vec![Effect::FetchSearch {
            query: query.to_owned(),
            generation,
        }]
    }

    pub(super) fn search_loaded(
        &mut self,
        generation: u64,
        result: Result<Vec<SearchResult>, String>,
    ) -> Vec<Effect> {
        if generation != self.search.generation {
            return vec![]; // The query changed since.
        }
        self.search.loading = false;
        match result {
            Ok(results) => {
                self.search.results = results;
                self.search.error = None;
            }
            Err(e) => {
                self.search.results.clear();
                self.search.error = Some(e);
            }
        }
        self.search.list = ListState::default().with_selected(Some(0));
        vec![]
    }

    /// `⏎` in the search box: a release opens all release results in the list; an artist
    /// opens their discography.
    pub(super) fn open_search_result(&mut self) -> Vec<Effect> {
        let Some(chosen) = self.search.selected().cloned() else {
            return vec![];
        };
        self.search_open = false;
        match chosen {
            SearchResult::Release(item) => {
                let releases: Vec<DiscoverItem> = self
                    .search
                    .results
                    .iter()
                    .filter_map(|r| match r {
                        SearchResult::Release(item) => Some(item.clone()),
                        SearchResult::Artist(_) => None,
                    })
                    .collect();
                let index = releases.iter().position(|r| r.id == item.id);
                let query = self.search.query.as_str().trim().to_owned();
                self.show_list(ListSource::Search(query), releases, index, false)
            }
            SearchResult::Artist(artist) => {
                self.pending_artist = Some(artist.band_id);
                let mut effects =
                    self.show_list(ListSource::Artist(artist.name), Vec::new(), None, true);
                effects.push(Effect::FetchArtist(artist.band_id));
                effects
            }
        }
    }

    pub(super) fn artist_loaded(
        &mut self,
        band_id: BandId,
        result: Result<Box<Artist>, String>,
    ) -> Vec<Effect> {
        if self.pending_artist != Some(band_id) {
            return vec![]; // The user moved on.
        }
        self.pending_artist = None;
        match result {
            Ok(artist) => {
                let artist = *artist;
                self.show_list(
                    ListSource::Artist(artist.name),
                    artist.releases,
                    Some(0),
                    false,
                )
            }
            Err(e) => {
                self.discover.loading = false;
                self.error(format!("Couldn't load artist: {e}"));
                self.discover.error = Some(e);
                vec![]
            }
        }
    }

    /// Replaces the release list, keeping Discover aside to come back to.
    pub(super) fn show_list(
        &mut self,
        source: ListSource,
        items: Vec<DiscoverItem>,
        selected: Option<usize>,
        loading: bool,
    ) -> Vec<Effect> {
        if self.source == ListSource::Discover {
            let discover = std::mem::take(&mut self.discover);
            let table = std::mem::take(&mut self.table);
            self.saved_discover = Some((discover, table));
        }
        self.source = source;
        self.autoplay_after = None;
        let generation = self.next_list_generation();
        self.discover = DiscoverList {
            total: items.len() as u64,
            items,
            next_cursor: None,
            loading,
            error: None,
            generation,
        };
        let selected = selected.or((!self.discover.items.is_empty()).then_some(0));
        self.table = TableState::default().with_selected(selected);
        self.focus = Focus::Discover;
        self.selection_changed().into_iter().collect()
    }

    /// `Esc`: back to Discover as it was, without reloading.
    pub(super) fn back_to_discover(&mut self) -> Vec<Effect> {
        if self.source == ListSource::Discover {
            return vec![];
        }
        self.source = ListSource::Discover;
        self.pending_artist = None;
        self.autoplay_after = None;
        if let Some((mut discover, table)) = self.saved_discover.take() {
            // A page that was loading may have been dropped while we were away.
            discover.loading = false;
            self.discover = discover;
            self.table = table;
            let mut effects: Vec<Effect> = self.selection_changed().into_iter().collect();
            effects.extend(self.maybe_load_more());
            effects
        } else {
            vec![self.reload()]
        }
    }
}
