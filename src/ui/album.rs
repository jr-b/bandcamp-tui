//! The album pane (right): release metadata plus the tracklist, or the about/credits/bio text.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Wrap};

use super::theme::{self, theme};
use super::{Metrics, fit, pane};
use crate::app::{AlbumView, App, Focus};
use crate::art::{Art, ArtBox};
use crate::bandcamp::{Album, DiscoverItem};
use crate::format;

/// With at least this many rows beside the cover, the text lists one fact per line.
const TALL_HEADER_ROWS: u16 = 8;

/// `compact`: the narrow single-pane layout, which keeps the cover small.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App, metrics: Metrics, compact: bool) {
    let focused = app.focus == Focus::Album;
    let show_info = app.album_pane.show_info;
    let mut block = pane(
        if show_info { "About" } else { "Album" },
        focused,
        metrics,
        false,
    );
    if matches!(app.album_view(), AlbumView::Loaded(_)) {
        let hint = if show_info { " i tracks " } else { " i about " };
        block = block.title(Line::from(hint).right_aligned().dim());
    }
    app.pane_areas.album = area;
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Recorded for `App::after_draw`, which fetches or re-encodes the cover at this size.
    app.album_pane.art_box = if app.art_enabled {
        crate::art::layout(inner.width, inner.height, app.art_font, compact)
    } else {
        None
    };

    match app.album_view() {
        AlbumView::None => {}
        AlbumView::Loading(item) => {
            let art = art_slot(app, item.art_id);
            let text_width = text_width_beside(art, inner.width);
            let mut text = summary_from_item(item, text_width);
            text.push_line(Line::default());
            text.push_line(Line::from("Loading tracks…").dim());
            let lines = u16::try_from(text.lines.len()).unwrap_or(u16::MAX);
            let (text_area, _) = header_with_art(frame, inner, art, lines);
            frame.render_widget(Paragraph::new(text), text_area);
        }
        AlbumView::Failed(item, error) => {
            let mut text = summary_from_item(item, inner.width);
            text.push_line(Line::default());
            text.push_line(
                Line::from(format!("Couldn't load this release: {error}"))
                    .patch_style(theme().error),
            );
            text.push_line(Line::from("Press ⏎ on it to retry.").dim());
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
        }
        AlbumView::Loaded(album) => {
            let album = std::sync::Arc::clone(album);
            if show_info {
                draw_info(frame, inner, &album, &mut app.album_pane.info_scroll);
            } else {
                draw_tracks(frame, inner, &album, app, focused);
            }
        }
    }
}

/// Title, artist, label and facts, each kept to one row of `width`. With `rows` of room
/// (beside a tall cover) it lists one fact per line and adds more detail.
fn summary(album: &Album, width: u16, rows: u16) -> Text<'static> {
    if rows >= TALL_HEADER_ROWS {
        return tall_summary(album, width, rows);
    }
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
        lines.push(
            Line::from(format::truncate(&tags, width).into_owned()).patch_style(theme().accent),
        );
    }
    if album.is_preorder {
        let playable = album.tracks.iter().filter(|t| t.is_playable()).count();
        lines.push(
            Line::from(format!(
                "pre-order · {playable} of {n} tracks streamable now"
            ))
            .patch_style(theme().warning),
        );
    }
    Text::from(lines)
}

/// The header beside a tall cover: one fact per line, then tags and the start of "about",
/// cut to `rows` lines.
fn tall_summary(album: &Album, width: u16, rows: u16) -> Text<'static> {
    let width = usize::from(width);
    let dim = Style::new().dim();
    let line = |label: &str, value: &str| {
        fit(
            Line::from(vec![
                Span::styled(format!("{label} "), dim),
                Span::raw(value.to_owned()),
            ]),
            width,
        )
    };
    let mut lines = vec![
        Line::from(format::truncate(&album.title, width).into_owned()).bold(),
        line("by", &album.artist),
    ];
    if album.band.name != album.artist {
        lines.push(line("on", &album.band.name));
    }
    if let Some(label) = album.label.as_ref().filter(|l| **l != album.band.name) {
        lines.push(line("label", label));
    }
    let n = album.tracks.len();
    let mut size = format!("{n} track{}", if n == 1 { "" } else { "s" });
    if !album.duration().is_zero() {
        size.push_str(" · ");
        size.push_str(&format::length(album.duration()));
    }
    lines.push(fit(Line::from(size), width));
    if let Some(date) = album.release_date {
        let verb = if album.is_preorder { "out" } else { "released" };
        lines.push(line(verb, &date.to_string()));
    }
    if let Some(location) = &album.band.location {
        lines.push(line("from", location));
    }
    if album.is_preorder {
        let playable = album.tracks.iter().filter(|t| t.is_playable()).count();
        lines.push(
            fit(
                Line::from(format!("pre-order · {playable} of {n} streamable now")),
                width,
            )
            .patch_style(theme().warning),
        );
    }
    if !album.tags.is_empty() {
        let tags = album
            .tags
            .iter()
            .map(|t| format!("#{}", t.norm_name))
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(
            Line::from(format::truncate(&tags, width).into_owned()).patch_style(theme().accent),
        );
    }
    // Fill what's left with the start of the description.
    let room = usize::from(rows).saturating_sub(lines.len() + 1);
    if room > 0
        && let Some(about) = &album.about
    {
        lines.push(Line::default());
        lines.extend(
            about
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(room)
                .map(|l| Line::from(format::truncate(l.trim(), width).into_owned()).dim()),
        );
    }
    lines.truncate(usize::from(rows));
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

/// Where (and whether) to draw the cover: `None` for text-only; otherwise its box and the
/// encoded cover, if it's ready.
type ArtSlot<'a> = Option<(ArtBox, Option<&'a Art>)>;

fn art_slot(app: &App, id: Option<crate::bandcamp::ArtId>) -> ArtSlot<'_> {
    let art_box = app.album_pane.art_box?;
    Some((art_box, app.art_for(id)?))
}

/// Columns left for the text beside the cover.
fn text_width_beside(art: ArtSlot<'_>, width: u16) -> u16 {
    art.map_or(width, |(b, _)| width.saturating_sub(b.cols + 1))
}

/// Lays out the header: cover on the left (or a placeholder while it loads), text on the
/// right. Returns the text area and the area below the header.
fn header_with_art(
    frame: &mut Frame,
    area: Rect,
    art: ArtSlot<'_>,
    text_lines: u16,
) -> (Rect, Rect) {
    let Some((art_box, art)) = art else {
        let [text, rest] =
            Layout::vertical([Constraint::Length(text_lines), Constraint::Min(0)]).areas(area);
        return (text, rest);
    };
    let height = text_lines.max(art_box.rows);
    let [top, rest] =
        Layout::vertical([Constraint::Length(height), Constraint::Min(0)]).areas(area);
    let [cover, _, text] = Layout::horizontal([
        Constraint::Length(art_box.cols),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(top);
    let cover = Rect {
        height: art_box.rows,
        ..cover
    };
    if let Some(art) = art {
        frame.render_widget(ratatui_image::Image::new(art.protocol()), cover);
    } else {
        // Loading: a quiet placeholder of the same size, so nothing jumps when it arrives.
        let middle = Rect {
            y: cover.y + cover.height / 2,
            height: 1,
            ..cover
        };
        frame.render_widget(Line::from("♪").dim().centered(), middle);
    }
    (text, rest)
}

fn draw_tracks(frame: &mut Frame, area: Rect, album: &Album, app: &mut App, focused: bool) {
    let art = art_slot(app, album.art_id);
    let text_width = text_width_beside(art, area.width);
    let header_rows = art.map_or(0, |(b, _)| b.rows);
    let header = summary(album, text_width, header_rows);
    let lines = u16::try_from(header.lines.len()).unwrap_or(0);
    let (text_area, rest) = header_with_art(frame, area, art, lines);
    frame.render_widget(Paragraph::new(header), text_area);
    let [rule, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(rest);
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
            title.push(Span::raw(track.artist.as_str()));
        }
        let duration = if track.duration.is_zero() {
            String::new()
        } else {
            format::clock(track.duration)
        };
        let style = if !track.is_playable() {
            Style::new().add_modifier(Modifier::DIM)
        } else if is_playing {
            theme().accent.bold()
        } else {
            Style::new()
        };
        Row::new([
            Cell::from(Span::styled(marker, theme().accent)),
            Cell::from(Line::from(number).right_aligned()),
            Cell::from(Line::from(title)),
            Cell::from(Line::from(duration).right_aligned()),
        ])
        .style(style)
    });
    let highlight = if focused {
        theme::SELECTED
    } else {
        Style::new()
    };
    let table = Table::new(rows, widths).row_highlight_style(highlight);
    frame.render_stateful_widget(table, list, &mut app.album_pane.tracks);
}

/// About, credits and the artist bio, scrolled by `scroll` (clamped here to the text height).
pub(super) fn draw_info(frame: &mut Frame, area: Rect, album: &Album, scroll: &mut u16) {
    let mut text = summary(album, area.width, 0);
    let heading = |s: &'static str| Line::from(s).patch_style(theme().accent).bold();
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
    text.push_line(
        Line::from(album.url.to_string())
            .patch_style(theme().accent)
            .underlined(),
    );

    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    // Clamp scrolling to the wrapped text height so you can't scroll into blank space.
    let max_scroll = paragraph
        .line_count(area.width)
        .saturating_sub(usize::from(area.height));
    *scroll = u16::try_from(usize::from(*scroll).min(max_scroll)).unwrap_or(u16::MAX);
    frame.render_widget(paragraph.scroll((*scroll, 0)), area);
}
