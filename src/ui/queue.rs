//! The `u` queue popup: what's playing, then what's queued.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem};

use super::theme::{self, theme};
use crate::app::App;
use crate::bandcamp::TralbumKind;
use crate::format;

pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let width = area.width.saturating_sub(4).clamp(36, 76);
    let wanted = u16::try_from(app.queue.len())
        .unwrap_or(u16::MAX)
        .saturating_add(5);
    let height = wanted.clamp(7, area.height.saturating_sub(2).max(7));
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(popup);

    let block = Block::bordered()
        .title(format!(" Queue · {} ", app.queue.len()))
        .title_style(Style::new().bold())
        .border_style(theme().accent);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    let [now, rule, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);

    let now_line = match app.current_track() {
        Some((album, index)) => Line::from(vec![
            Span::styled("Now  ♪ ", theme().accent),
            Span::raw(
                format::truncate(
                    &format!("{} — {}", album.title, album.artist),
                    usize::from(now.width).saturating_sub(20),
                )
                .into_owned(),
            ),
            Span::styled(
                format!("  track {}/{}", index + 1, album.tracks.len()),
                Style::new().dim(),
            ),
        ]),
        None => Line::from("Nothing playing.").dim(),
    };
    frame.render_widget(now_line, now);
    frame.render_widget(Line::from("─".repeat(usize::from(rule.width))).dim(), rule);

    if app.queue.is_empty() {
        let hint = "Empty. Press e on a release (or a track) to add it; E plays it next.";
        frame.render_widget(Line::from(hint).dim(), list);
        return;
    }
    // "› " highlight + "12  " number + "album  " badge.
    let text_width = usize::from(list.width).saturating_sub(2 + 4 + 7);
    let items: Vec<ListItem> = app
        .queue
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let (badge, title) = match (&entry.track_title, entry.kind) {
                (Some(track), _) => ("track", track.as_str()),
                (None, TralbumKind::Track) => ("track", entry.title.as_str()),
                (None, TralbumKind::Album) => ("album", entry.title.as_str()),
            };
            let mut spans = vec![
                Span::styled(format!("{:>2}  ", i + 1), Style::new().dim()),
                Span::styled(format!("{badge:<7}"), theme().accent),
            ];
            spans.extend(super::title_by(title, &entry.artist, text_width));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let widget = List::new(items)
        .highlight_style(theme::SELECTED)
        .highlight_symbol("› ");
    if let Some(view) = app.queue_view.as_mut() {
        frame.render_stateful_widget(widget, list, view);
    }
}
