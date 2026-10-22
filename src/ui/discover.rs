//! The discover list (left pane).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use unicode_width::UnicodeWidthStr;

use super::theme::{self, theme};
use super::{Metrics, pane};
use crate::app::{AlbumView, App, Focus, ListSource};
use crate::format;

/// Below this inner width the track count column is left out, so titles keep their room.
const TRACKS_COLUMN_MIN_WIDTH: u16 = 50;

/// Marker, title, artist, (track count,) length.
fn widths(show_tracks: bool) -> Vec<Constraint> {
    let mut widths = vec![
        Constraint::Length(1),
        Constraint::Fill(3),
        Constraint::Fill(2),
        Constraint::Length(3),
        Constraint::Length(4),
    ];
    if !show_tracks {
        widths.remove(3);
    }
    widths
}

/// `divider`: another pane sits to the right.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App, metrics: Metrics, divider: bool) {
    let focused = app.focus == Focus::Discover;
    let mut block = pane(&title(app), focused, metrics, divider);
    if app.discover.loading {
        block = block.title(
            Line::from(" loading… ")
                .right_aligned()
                .patch_style(theme().accent),
        );
    }
    app.pane_areas.list = area;
    let inner = block.inner(area);
    // Alone on screen, the list has no album pane beside it: one row says what's selected.
    let strip = !divider && !app.discover.items.is_empty() && inner.height > STRIP_MIN_HEIGHT;
    let [inner, strip_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(u16::from(strip))]).areas(inner);
    // One row is the column header.
    app.page_rows = usize::from(inner.height.saturating_sub(1));

    if app.discover.items.is_empty() {
        frame.render_widget(block, area);
        let text = empty_text(app);
        let paragraph = Paragraph::new(text).centered().wrap(Wrap { trim: true });
        let height = u16::try_from(paragraph.line_count(inner.width))
            .unwrap_or(u16::MAX)
            .min(inner.height);
        let [middle] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(inner);
        frame.render_widget(paragraph, middle);
        return;
    }

    // Same layout the table computes, so text can be cut with an ellipsis instead of mid-word.
    let show_tracks = inner.width >= TRACKS_COLUMN_MIN_WIDTH;
    let widths = widths(show_tracks);
    let columns = Layout::horizontal(widths.clone())
        .spacing(1)
        .flex(Flex::Start)
        .split(Rect::new(0, 0, inner.width, 1));
    let (title_width, artist_width) =
        (usize::from(columns[1].width), usize::from(columns[2].width));

    let playing_id = app.now_playing.as_ref().map(|a| a.id);
    // Only the rows on screen are built: the list can hold a thousand releases.
    let (window, mut state) = visible_window(app);
    let rows = app.discover.items[window].iter().map(|item| {
        let marker = if Some(item.id) == playing_id {
            "♪"
        } else if app.queue.contains_release(item.id) {
            "+"
        } else {
            " "
        };
        // The pre-order tag goes first so it's never the part that gets cut off.
        let title = if item.is_preorder {
            const TAG: &str = "pre ";
            Line::from(vec![
                Span::styled(TAG, theme().warning),
                Span::raw(format::truncate(
                    &item.title,
                    title_width.saturating_sub(TAG.width()),
                )),
            ])
        } else {
            Line::from(format::truncate(&item.title, title_width))
        };
        // Unknown until the details load: a dim dash rather than an empty cell.
        let (tracks, duration) = app.release_size(item);
        let unknown = || Span::styled("–", Style::new().dim());
        let tracks = tracks.map_or_else(unknown, |n| Span::raw(n.to_string()));
        let length = duration.map_or_else(unknown, |d| {
            Span::raw(format!("{}m", (d.as_secs() + 30) / 60))
        });
        let mut cells = vec![
            Cell::from(Span::styled(marker, theme().accent)),
            Cell::from(title),
            Cell::from(format::truncate(&item.artist, artist_width)),
        ];
        if show_tracks {
            cells.push(Cell::from(Line::from(tracks).right_aligned()));
        }
        cells.push(Cell::from(Line::from(length).right_aligned()));
        Row::new(cells)
    });
    let mut header = vec![Cell::from(""), Cell::from("Title"), Cell::from("Artist")];
    if show_tracks {
        header.push(Cell::from(Line::from("Trk").right_aligned()));
    }
    header.push(Cell::from(Line::from("Len").right_aligned()));
    let header = Row::new(header).style(Style::new().dim().add_modifier(Modifier::UNDERLINED));
    // The selection stays visible when the other pane has focus, but quieter.
    let highlight = if focused {
        theme::SELECTED
    } else {
        theme::SELECTED_UNFOCUSED
    };
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(highlight);
    frame.render_widget(block, area);
    frame.render_stateful_widget(table, inner, &mut state);
    if strip {
        frame.render_widget(
            selection_strip(app, usize::from(strip_area.width)),
            strip_area,
        );
    }
}

/// The list needs at least this many inner rows to give one up to the selection strip.
const STRIP_MIN_HEIGHT: u16 = 8;

/// `10 tracks · 38 min · released 16 Oct 2026 · Wuppertal, Germany · #electronic #chill`: what the
/// album pane would say about the selected release, in one dim line.
fn selection_strip(app: &App, width: usize) -> Line<'static> {
    let Some(item) = app.selected() else {
        return Line::default();
    };
    let (tracks, duration) = app.release_size(item);
    let mut facts = Vec::new();
    if let Some(n) = tracks {
        facts.push(format!("{n} track{}", if n == 1 { "" } else { "s" }));
    }
    if let Some(d) = duration {
        facts.push(format::length(d));
    }
    if let Some(date) = item.release_date {
        let verb = if item.is_preorder { "out" } else { "released" };
        facts.push(format!("{verb} {date}"));
    }
    if let Some(location) = &item.band_location {
        facts.push(location.clone());
    }
    let mut line = facts.join(" · ");
    if let AlbumView::Loaded(album) = app.album_view() {
        for tag in album.tags.iter().take(3) {
            line.push_str(" #");
            line.push_str(&tag.norm_name);
        }
    }
    super::fit(Line::from(format!(" {line}")).dim(), width)
}

/// The rows to draw, and a table state for just those rows. Scrolls the way ratatui would:
/// the offset only moves when the selection would leave the screen. Records the new offset.
fn visible_window(app: &mut App) -> (std::ops::Range<usize>, TableState) {
    let len = app.discover.items.len();
    let rows = app.page_rows.max(1);
    let mut offset = app.table.offset().min(len.saturating_sub(rows));
    let selected = app.table.selected().map(|s| s.min(len.saturating_sub(1)));
    if let Some(s) = selected {
        if s < offset {
            offset = s;
        } else if s >= offset + rows {
            offset = s + 1 - rows;
        }
    }
    *app.table.offset_mut() = offset;
    let state = TableState::default().with_selected(selected.map(|s| s - offset));
    (offset..(offset + rows).min(len), state)
}

/// What the list shows, and where the selection is in it: `Discover · 12/1,947,113`.
fn title(app: &App) -> String {
    let name = match &app.source {
        ListSource::Discover => "Discover",
        ListSource::Search(_) => "Search results",
        ListSource::Artist(name) => name,
    };
    match app
        .table
        .selected()
        .filter(|_| !app.discover.items.is_empty())
    {
        Some(selected) => {
            let total = app.discover.total.max(app.discover.items.len() as u64);
            format!(
                "{name} · {}/{}",
                format::thousands(selected as u64 + 1),
                format::thousands(total)
            )
        }
        None => name.to_owned(),
    }
}

/// What the list says while it has no releases: loading, failed, or really empty.
fn empty_text(app: &App) -> Text<'static> {
    let discover = app.source == ListSource::Discover;
    if let Some(error) = &app
        .discover
        .error
        .as_ref()
        .filter(|_| !app.discover.loading)
    {
        let (what, next) = if discover {
            ("Discover", "Press r to retry.")
        } else {
            ("these releases", "Press esc to go back to Discover.")
        };
        return Text::from(vec![
            Line::from(format!("Couldn't load {what}: {error}")).patch_style(theme().error),
            Line::from(next).dim(),
        ]);
    }
    let text = match (discover, app.discover.loading) {
        (true, true) => "Loading Discover…",
        (true, false) => "No releases match these filters.",
        (false, true) => "Loading releases…",
        (false, false) => "No releases.",
    };
    Text::from(Line::from(text).dim())
}
