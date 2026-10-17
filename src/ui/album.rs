//! The album pane (right): release metadata plus the tracklist, or the about/credits/bio text.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Wrap};
use unicode_width::UnicodeWidthStr;

use super::{ACCENT, pane};
use crate::app::{AlbumView, App, Focus};
use crate::bandcamp::{Album, DiscoverItem};
use crate::format;

pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Album;
    let show_info = app.album_pane.show_info;
    let mut block = pane(if show_info { "About" } else { "Album" }, focused);
    if matches!(app.album_view(), AlbumView::Loaded(_)) {
        let hint = if show_info { " i tracks " } else { " i about " };
        block = block.title(Line::from(hint).right_aligned().dim());
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match app.album_view() {
        AlbumView::None => {}
        AlbumView::Loading(item) => {
            let mut text = summary_from_item(item, inner.width);
            text.push_line(Line::default());
            text.push_line(Line::from("Loading tracks…").dim());
            frame.render_widget(Paragraph::new(text), inner);
        }
        AlbumView::Failed(item, error) => {
            let mut text = summary_from_item(item, inner.width);
            text.push_line(Line::default());
            text.push_line(
                Line::from(format!("Couldn't load this release: {error}")).fg(Color::Red),
            );
            text.push_line(Line::from("Press ⏎ on it to retry.").dim());
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
        }
        AlbumView::Loaded(album) => {
            let album = std::sync::Arc::clone(album);
            if show_info {
                draw_info(frame, inner, &album, app);
            } else {
                draw_tracks(frame, inner, &album, app, focused);
            }
        }
    }
}

/// Title, artist, label and facts. `width` is used to keep each line on one row.
fn summary(album: &Album, width: u16) -> Text<'static> {
    let width = usize::from(width);
    let dim = Style::new().dim();
    let mut lines = vec![Line::from(format::truncate(&album.title, width).into_owned()).bold()];

    let mut by = vec![Span::styled("by ", dim), Span::raw(album.artist.clone())];
    if album.band.name != album.artist {
        by.push(Span::styled(" · on ", dim));
        by.push(Span::raw(album.band.name.clone()));
    }
    if let Some(label) = album.label.as_ref().filter(|l| **l != album.band.name) {
        by.push(Span::styled(" · label ", dim));
        by.push(Span::raw(label.clone()));
    }
    lines.push(fit(Line::from(by), width));

    // Most useful first: whatever doesn't fit gets cut from the end.
    let n = album.tracks.len();
    let mut facts = vec![format!("{n} track{}", if n == 1 { "" } else { "s" })];
    if !album.duration().is_zero() {
        facts.push(format::length(album.duration()));
    }
    if let Some(date) = album.release_date {
        let verb = if album.is_preorder { "out" } else { "released" };
        facts.push(format!("{verb} {date}"));
    }
    if let Some(location) = &album.band.location {
        facts.push(location.clone());
    }
    lines.push(fit(Line::from(facts.join(" · ")).dim(), width));

    if !album.tags.is_empty() {
        let tags = album
            .tags
            .iter()
            .map(|t| format!("#{}", t.norm_name))
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(Line::from(format::truncate(&tags, width).into_owned()).fg(ACCENT));
    }
    if album.is_preorder {
        let playable = album.tracks.iter().filter(|t| t.is_playable()).count();
        lines.push(
            Line::from(format!(
                "pre-order · {playable} of {n} tracks streamable now"
            ))
            .fg(Color::Yellow),
        );
    }
    Text::from(lines)
}

/// What we know from the discover feed while the details load.
fn summary_from_item(item: &DiscoverItem, width: u16) -> Text<'static> {
    let width = usize::from(width);
    let dim = Style::new().dim();
    let mut by = vec![Span::styled("by ", dim), Span::raw(item.artist.clone())];
    if item.band_name != item.artist {
        by.push(Span::styled(" · on ", dim));
        by.push(Span::raw(item.band_name.clone()));
    }
    let mut facts = Vec::new();
    if let Some(date) = item.release_date {
        facts.push(format!("released {date}"));
    }
    if let Some(location) = &item.band_location {
        facts.push(location.clone());
    }
    Text::from(vec![
        Line::from(format::truncate(&item.title, width).into_owned()).bold(),
        fit(Line::from(by), width),
        fit(Line::from(facts.join(" · ")).dim(), width),
    ])
}

/// Truncates a styled line to `width` cells, keeping span styles.
fn fit(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let mut remaining = width;
    let mut spans = Vec::new();
    for span in line.spans {
        let w = span.content.width();
        if w < remaining {
            remaining -= w;
            spans.push(span);
        } else {
            let cut = format::truncate(&span.content, remaining).into_owned();
            spans.push(Span::styled(cut, span.style));
            break;
        }
    }
    Line::from(spans).style(line.style)
}

fn draw_tracks(frame: &mut Frame, area: Rect, album: &Album, app: &mut App, focused: bool) {
    let header = summary(album, area.width);
    let header_height = u16::try_from(header.lines.len()).unwrap_or(0) + 1;
    let [top, rule, list] = Layout::vertical([
        Constraint::Length(header_height - 1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(header), top);
    frame.render_widget(Line::from("─".repeat(usize::from(rule.width))).dim(), rule);
    app.album_pane.page_rows = usize::from(list.height);

    let playing = app.playing_index_in(album);
    let widths = [
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(7),
    ];
    let rows = album.tracks.iter().enumerate().map(|(i, track)| {
        let is_playing = playing == Some(i);
        let marker = if is_playing { "♪" } else { " " };
        let number = track.number.map_or(String::new(), |n| n.to_string());
        let mut title = vec![Span::raw(track.title.as_str())];
        if track.artist != album.artist {
            title.push(Span::styled(" — ", Style::new().dim()));
            title.push(Span::styled(
                track.artist.as_str(),
                Style::new().fg(Color::Gray),
            ));
        }
        let duration = if track.duration.is_zero() {
            String::new()
        } else {
            format::clock(track.duration)
        };
        let style = if !track.is_playable() {
            Style::new().add_modifier(Modifier::DIM)
        } else if is_playing {
            Style::new().fg(ACCENT).bold()
        } else {
            Style::new()
        };
        Row::new([
            Cell::from(Span::styled(marker, Style::new().fg(ACCENT))),
            Cell::from(Line::from(number).right_aligned()),
            Cell::from(Line::from(title)),
            Cell::from(Line::from(duration).right_aligned()),
        ])
        .style(style)
    });
    let highlight = if focused {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new()
    };
    let table = Table::new(rows, widths).row_highlight_style(highlight);
    frame.render_stateful_widget(table, list, &mut app.album_pane.tracks);
}

fn draw_info(frame: &mut Frame, area: Rect, album: &Album, app: &mut App) {
    let mut text = summary(album, area.width);
    let heading = |s: &'static str| Line::from(s).fg(ACCENT).bold();
    let mut section = |title: &'static str, body: &str| {
        text.push_line(Line::default());
        text.push_line(heading(title));
        for line in body.lines() {
            text.push_line(Line::from(line.to_owned()));
        }
    };
    if let Some(about) = &album.about {
        section("About", about);
    }
    if let Some(credits) = &album.credits {
        section("Credits", credits);
    }
    let band = &album.band;
    let mut artist = band.name.clone();
    if let Some(location) = &band.location {
        artist.push_str(" · ");
        artist.push_str(location);
    }
    if let Some(bio) = &band.bio {
        artist.push_str("\n\n");
        artist.push_str(bio);
    }
    section("Artist", &artist);
    text.push_line(Line::default());
    text.push_line(Line::from(album.url.to_string()).fg(ACCENT).underlined());

    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    // Clamp scrolling to the wrapped text height so you can't scroll into blank space.
    let max_scroll = paragraph
        .line_count(area.width)
        .saturating_sub(usize::from(area.height));
    let scroll = usize::from(app.album_pane.info_scroll).min(max_scroll);
    app.album_pane.info_scroll = u16::try_from(scroll).unwrap_or(u16::MAX);
    frame.render_widget(paragraph.scroll((app.album_pane.info_scroll, 0)), area);
}
