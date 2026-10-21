//! The `/` search popup: a query line and live results (artists, albums, tracks).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem};

use super::ACCENT;
use crate::app::SearchBox;
use crate::bandcamp::{SearchResult, TralbumKind};
use crate::format;

pub(super) fn draw(frame: &mut Frame, area: Rect, search: &mut SearchBox) {
    let width = area.width.saturating_sub(4).clamp(30, 72);
    let height = area.height.saturating_sub(2).clamp(6, 24);
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(popup);

    let mut block = Block::bordered()
        .title(" Search Bandcamp ")
        .title_style(Style::new().bold())
        .border_style(Style::new().fg(ACCENT));
    if search.loading {
        block = block.title(Line::from(" searching… ").right_aligned().fg(ACCENT));
    }
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [input, rule, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    super::draw_input(frame, input, &search.query, "artist, album or track");
    frame.render_widget(Line::from("─".repeat(usize::from(rule.width))).dim(), rule);

    if let Some(error) = &search.error {
        frame.render_widget(
            Line::from(format!("Search failed: {error}")).fg(Color::Red),
            list,
        );
        return;
    }
    if search.results.is_empty() {
        let hint = if search.query.as_str().trim().is_empty() {
            ""
        } else if search.loading {
            "Searching…"
        } else {
            "No matches."
        };
        frame.render_widget(Line::from(hint).dim(), list);
        return;
    }

    // Two-cell gap, a fixed-width type badge, then the name and who it's by.
    let text_width = usize::from(list.width).saturating_sub(2 + 7);
    let items: Vec<ListItem> = search
        .results
        .iter()
        .map(|result| ListItem::new(result_line(result, text_width)))
        .collect();
    let widget = List::new(items)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    frame.render_stateful_widget(widget, list, &mut search.list);
}

fn result_line(result: &SearchResult, width: usize) -> Line<'static> {
    let badge = |label: &str, color| Span::styled(format!("{label:<7}"), Style::new().fg(color));
    match result {
        SearchResult::Artist(artist) => {
            let label = if artist.is_label { "label" } else { "artist" };
            let mut detail = Vec::new();
            if let Some(location) = &artist.location {
                detail.push(location.clone());
            }
            if let Some(genre) = &artist.genre {
                detail.push(genre.to_lowercase());
            }
            let name = format::truncate(&artist.name, width).into_owned();
            let room = width.saturating_sub(unicode_width::UnicodeWidthStr::width(name.as_str()));
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format::truncate(&format!("  {}", detail.join(" · ")), room).into_owned()
            };
            Line::from(vec![
                badge(label, Color::Magenta),
                Span::styled(name, Style::new().bold()),
                Span::styled(detail, Style::new().dim()),
            ])
        }
        SearchResult::Release(item) => {
            let label = match item.kind {
                TralbumKind::Album => "album",
                TralbumKind::Track => "track",
            };
            let title = format::truncate(&item.title, width).into_owned();
            let room = width.saturating_sub(unicode_width::UnicodeWidthStr::width(title.as_str()));
            let by = format::truncate(&format!(" — {}", item.artist), room).into_owned();
            Line::from(vec![
                badge(label, ACCENT),
                Span::raw(title),
                Span::styled(by, Style::new().fg(Color::Gray)),
            ])
        }
    }
}
