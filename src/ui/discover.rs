//! The discover list (left pane).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row, Table};

use super::{ACCENT, pane};
use crate::app::{App, Focus, ListSource};
use crate::format;

const WIDTHS: [Constraint; 5] = [
    Constraint::Length(1),
    Constraint::Fill(3),
    Constraint::Fill(2),
    Constraint::Length(3),
    Constraint::Length(4),
];

pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Discover;
    let title = match &app.source {
        ListSource::Discover => "Discover".to_owned(),
        ListSource::Search(_) => "Search results".to_owned(),
        ListSource::Artist(name) => name.clone(),
    };
    let mut block = pane(&title, focused);
    if app.discover.loading {
        block = block.title(Line::from(" loading… ").right_aligned().fg(ACCENT));
    }
    let inner = block.inner(area);
    // One row is the column header.
    app.page_rows = usize::from(inner.height.saturating_sub(1));

    if app.discover.items.is_empty() {
        let text = match (&app.source, app.discover.loading) {
            (ListSource::Discover, true) => "Loading Discover…",
            (ListSource::Discover, false) => "No releases match these filters.",
            (_, true) => "Loading releases…",
            (_, false) => "No releases.",
        };
        frame.render_widget(block, area);
        let y = inner.y + inner.height / 2;
        frame.render_widget(
            Line::from(text).dim().centered(),
            Rect::new(inner.x, y, inner.width, 1),
        );
        return;
    }

    // Same layout the table computes, so text can be cut with an ellipsis instead of mid-word.
    let columns = Layout::horizontal(WIDTHS)
        .spacing(1)
        .flex(Flex::Start)
        .split(Rect::new(0, 0, inner.width, 1));
    let (title_width, artist_width) =
        (usize::from(columns[1].width), usize::from(columns[2].width));

    let playing_id = app.now_playing.as_ref().map(|a| a.id);
    let rows = app.discover.items.iter().map(|item| {
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
                Span::styled(TAG, Style::new().fg(Color::Yellow)),
                Span::raw(format::truncate(
                    &item.title,
                    title_width.saturating_sub(TAG.len()),
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
        Row::new([
            Cell::from(Span::styled(marker, Style::new().fg(ACCENT))),
            Cell::from(title),
            Cell::from(format::truncate(&item.artist, artist_width))
                .style(Style::new().fg(Color::Gray)),
            Cell::from(Line::from(tracks).right_aligned()),
            Cell::from(Line::from(length).right_aligned()),
        ])
    });
    let header = Row::new([
        Cell::from(""),
        Cell::from("Title"),
        Cell::from("Artist"),
        Cell::from(Line::from("Trk").right_aligned()),
        Cell::from(Line::from("Len").right_aligned()),
    ])
    .style(Style::new().dim().add_modifier(Modifier::UNDERLINED));
    // The selection stays visible when the other pane has focus, but quieter.
    let highlight = if focused {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    };
    let table = Table::new(rows, WIDTHS)
        .header(header)
        .block(block)
        .row_highlight_style(highlight);
    frame.render_stateful_widget(table, area, &mut app.table);
}
