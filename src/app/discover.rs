//! The Discover feed: loading pages, and the filter pickers that reload it.

use ratatui::widgets::TableState;

use super::{Action, App, DiscoverList, Effect, Focus, LOAD_MORE_THRESHOLD, ListSource, MAX_ITEMS};
use crate::bandcamp::DiscoverPage;

impl App {
    /// Handles `action` while the picker is open. `None` means "not a picker action": handle
    /// it normally (e.g. API results and player events keep flowing).
    pub(super) fn picker_update(&mut self, action: &Action) -> Option<Vec<Effect>> {
        let picker = self.picker.as_mut()?;
        match action {
            Action::Up => picker.move_by(-1),
            Action::Down => picker.move_by(1),
            Action::PageUp => picker.move_by(-10),
            Action::PageDown => picker.move_by(10),
            Action::Top => picker.move_by(isize::MIN),
            Action::Bottom => picker.move_by(isize::MAX),
            Action::Edit(edit) => picker.edit(*edit),
            Action::ClosePopup => self.picker = None,
            Action::Play => {
                let choice = picker.selected().cloned();
                self.picker = None;
                if let Some(choice) = choice {
                    let mut query = self.query.clone();
                    choice.apply(&mut query);
                    if query != self.query {
                        self.query = query;
                        return Some(vec![self.reload(), self.save_settings()]);
                    }
                }
            }
            Action::Quit => return None,
            _ if action.is_user_input() => {} // other keys are swallowed by the popup
            _ => return None,
        }
        Some(vec![])
    }

    /// Starts a fresh discover load for the current query.
    pub(super) fn reload(&mut self) -> Effect {
        // Filters and refresh always mean Discover.
        self.source = ListSource::Discover;
        self.saved_discover = None;
        self.pending_artist = None;
        let generation = self.next_list_generation();
        self.discover = DiscoverList {
            loading: true,
            generation,
            ..DiscoverList::default()
        };
        self.table = TableState::default();
        self.focus = Focus::Discover;
        // Errors may have been transient; let them be retried.
        self.album_errors.clear();
        // The list autoplay was walking is gone.
        self.autoplay_after = None;
        Effect::FetchDiscover {
            query: self.query.clone(),
            cursor: None,
            generation,
        }
    }

    pub(super) fn discover_loaded(
        &mut self,
        generation: u64,
        result: Result<DiscoverPage, String>,
    ) -> Vec<Effect> {
        if generation != self.discover.generation {
            return vec![]; // Response to a request we've since replaced.
        }
        self.discover.loading = false;
        match result {
            Ok(page) => {
                self.discover.error = None;
                let room = MAX_ITEMS.saturating_sub(self.discover.items.len());
                self.discover
                    .items
                    .extend(page.items.into_iter().take(room));
                self.discover.total = page.total;
                self.discover.next_cursor = page.next_cursor;
                let mut effects = Vec::new();
                if self.table.selected().is_none() && !self.discover.items.is_empty() {
                    self.table.select(Some(0));
                    effects.extend(self.selection_changed());
                }
                // A tall terminal can show the whole first page: keep loading until the
                // selection is far enough from the end (or there's nothing more).
                effects.extend(self.maybe_load_more());
                if let Some(after) = self.autoplay_after.take() {
                    effects.extend(self.autoplay_next(Some(after)));
                }
                effects
            }
            Err(e) => {
                self.autoplay_after = None;
                self.error(format!("Couldn't load Discover: {e}"));
                self.discover.error = Some(e);
                vec![]
            }
        }
    }

    pub(super) fn maybe_load_more(&mut self) -> Option<Effect> {
        let selected = self.table.selected().unwrap_or(0);
        let near_end = selected + LOAD_MORE_THRESHOLD >= self.discover.items.len();
        if !near_end || self.discover.loading || !self.discover.has_more() {
            return None;
        }
        self.discover.loading = true;
        Some(Effect::FetchDiscover {
            query: self.query.clone(),
            cursor: self.discover.next_cursor.clone(),
            generation: self.discover.generation,
        })
    }

    /// Requests the next discover page regardless of where the selection is.
    pub(super) fn load_next_page(&mut self) -> Option<Effect> {
        if self.discover.loading || !self.discover.has_more() {
            return None;
        }
        self.discover.loading = true;
        Some(Effect::FetchDiscover {
            query: self.query.clone(),
            cursor: self.discover.next_cursor.clone(),
            generation: self.discover.generation,
        })
    }
}
