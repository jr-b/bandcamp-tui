//! Rendering. `draw` is a pure function of `App`, apart from recording layout facts the next
//! update needs (table scroll offset, visible rows).

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};

use crate::app::{App, MessageKind};
use crate::bandcamp::{DiscoverItem, Slice};
use crate::format;
use crate::input::HINTS;
use crate::player::PlaybackState;

const ACCENT: Color = Color::Cyan;
/// Below this width the release details pane is hidden.
const DETAILS_MIN_WIDTH: u16 = 90;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, now_playing, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, app);
    if body.width >= DETAILS_MIN_WIDTH {
        let [list, details] =
            Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
                .areas(body);
        draw_discover(frame, list, app);
        draw_details(frame, details, app);
    } else {
        draw_discover(frame, body, app);
    }
    draw_now_playing(frame, now_playing, app);
    draw_footer(frame, footer, app);
}

fn slice_label(slice: Slice) -> &'static str {
    match slice {
        Slice::Top => "best-selling",
        Slice::New => "new arrivals",
        Slice::Random => "surprise me",
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let genres = if app.query.tags.is_empty() {
        "all genres".to_owned()
    } else {
        app.query.tags.join(" › ")
    };
    let left = Line::from(vec![
        Span::styled(
            " bandcamp ",
            Style::new().fg(Color::Black).bg(ACCENT).bold(),
        ),
        Span::raw(" Discover · "),
        Span::styled(genres, Style::new().bold()),
        Span::raw(" · "),
        Span::styled(slice_label(app.query.slice), Style::new().bold()),
    ]);
    let right = if app.discover.total > 0 {
        Line::from(format!(
            "{} releases ",
            format::thousands(app.discover.total)
        ))
        .dim()
    } else {
        Line::default()
    };
    let [l, r] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(u16::try_from(right.width()).unwrap_or(u16::MAX)),
    ])
    .areas(area);
    frame.render_widget(left, l);
    frame.render_widget(right, r);
}

fn draw_discover(frame: &mut Frame, area: Rect, app: &mut App) {
    let mut block = Block::bordered().title(" Discover ");
    if app.discover.loading {
        block = block.title(Line::from(" loading… ").right_aligned().fg(ACCENT));
    }
    let inner = block.inner(area);
    // One row is the column header.
    app.page_rows = usize::from(inner.height.saturating_sub(1));

    if app.discover.items.is_empty() {
        let text = if app.discover.loading {
            "Loading Discover…"
        } else {
            "No releases match these filters."
        };
        let y = inner.y + inner.height / 2;
        frame.render_widget(block, area);
        frame.render_widget(
            Line::from(text).dim().centered(),
            Rect::new(inner.x, y, inner.width, 1),
        );
        return;
    }

    let widths = [
        Constraint::Length(1),
        Constraint::Fill(3),
        Constraint::Fill(2),
        Constraint::Length(3),
        Constraint::Length(4),
    ];
    // Same layout the table computes, so text can be cut with an ellipsis instead of mid-word.
    let columns = Layout::horizontal(widths)
        .spacing(1)
        .flex(ratatui::layout::Flex::Start)
        .split(Rect::new(0, 0, inner.width, 1));
    let (title_width, artist_width) =
        (usize::from(columns[1].width), usize::from(columns[2].width));

    let playing_id = app.now_playing.as_ref().map(|a| a.id);
    let rows = app.discover.items.iter().map(|item| {
        let marker = if Some(item.id) == playing_id {
            "♪"
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
        let artist = format::truncate(&item.artist, artist_width);
        let tracks = item.track_count.map_or(String::new(), |n| n.to_string());
        let length = item.duration.map_or(String::new(), |d| {
            let m = (d.as_secs() + 30) / 60;
            format!("{m}m")
        });
        Row::new([
            Cell::from(Span::styled(marker, Style::new().fg(ACCENT))),
            Cell::from(title),
            Cell::from(artist).style(Style::new().fg(Color::Gray)),
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
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(table, area, &mut app.table);
}

fn draw_details(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" Release ");
    let Some(item) = app.selected() else {
        frame.render_widget(block, area);
        return;
    };
    let text = release_text(item);
    frame.render_widget(
        Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn release_text(item: &DiscoverItem) -> Text<'_> {
    let label = Style::new().dim();
    let mut lines = vec![
        Line::from(item.title.as_str()).bold(),
        Line::from(vec![
            Span::styled("by ", label),
            Span::raw(item.artist.as_str()),
        ]),
    ];
    if item.band_name != item.artist {
        lines.push(Line::from(vec![
            Span::styled("on ", label),
            Span::raw(item.band_name.as_str()),
        ]));
    }
    lines.push(Line::default());
    if let Some(location) = &item.band_location {
        lines.push(Line::from(vec![
            Span::styled("from ", label),
            Span::raw(location),
        ]));
    }
    if let Some(date) = item.release_date {
        lines.push(Line::from(vec![
            Span::styled("released ", label),
            Span::raw(date.to_string()),
        ]));
    }
    let mut size = Vec::new();
    if let Some(n) = item.track_count {
        size.push(format!("{n} track{}", if n == 1 { "" } else { "s" }));
    }
    if let Some(d) = item.duration {
        size.push(format::length(d));
    }
    if !size.is_empty() {
        lines.push(Line::from(size.join(" · ")));
    }
    if item.is_free_download {
        lines.push(Line::from("free download").fg(Color::Green));
    } else if let Some(price) = &item.price {
        lines.push(Line::from(vec![
            Span::styled("digital ", label),
            Span::raw(format::price(price)),
        ]));
    }
    if item.is_preorder {
        lines.push(Line::from("pre-order: not streamable yet").fg(Color::Yellow));
    }
    lines.push(Line::default());
    lines.push(Line::from(item.url.as_str()).fg(ACCENT).underlined());
    Text::from(lines)
}

fn draw_now_playing(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::new()
        .borders(Borders::TOP)
        .title(" Now playing ")
        .border_style(Style::new().dim());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [info, progress] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    let status = &app.player;
    let Some((album, index)) = app.current_track() else {
        let hint = if app.player_available {
            "Nothing playing. Select a release and press ⏎."
        } else {
            "No audio output."
        };
        frame.render_widget(Line::from(format!(" {hint}")).dim(), info);
        return;
    };
    let track = &album.tracks[index];
    let (icon, state) = match status.state {
        PlaybackState::Playing => ("▶", ""),
        PlaybackState::Paused => ("‖", " (paused)"),
        PlaybackState::Buffering => ("…", " (buffering)"),
        PlaybackState::Loading => ("…", " (loading)"),
        PlaybackState::Stopped => ("■", ""),
    };
    let mut line = vec![
        Span::styled(format!(" {icon} "), Style::new().fg(ACCENT).bold()),
        Span::styled(track.title.as_str(), Style::new().bold()),
        Span::raw(" — "),
        Span::raw(track.artist.as_str()),
        Span::styled(format!(" · {}", album.title), Style::new().dim()),
        Span::styled(
            format!(" · {}/{}", index + 1, album.tracks.len()),
            Style::new().dim(),
        ),
    ];
    if !state.is_empty() {
        line.push(Span::styled(state, Style::new().fg(Color::Yellow)));
    }
    frame.render_widget(Line::from(line), info);

    // Prefer the track's API duration; the player only knows it once the track is loaded.
    let duration = if status.duration.is_zero() {
        track.duration
    } else {
        status.duration
    };
    let left = format!(" {} ", format::clock(status.position));
    let right = format!(
        " {}   vol {:>3}% ",
        format::clock(duration),
        volume_percent(status.volume)
    );
    let bar_width = usize::from(progress.width).saturating_sub(left.len() + right.len());
    let ratio = if duration.is_zero() {
        0.0
    } else {
        (status.position.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0)
    };
    frame.render_widget(
        Line::from(vec![
            Span::raw(left),
            progress_bar(bar_width, ratio),
            Span::styled(right, Style::new().dim()),
        ]),
        progress,
    );
}

fn volume_percent(volume: f32) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0..=100
    let percent = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
    percent
}

/// `━━━━━●──────`, `width` cells wide.
fn progress_bar(width: usize, ratio: f64) -> Span<'static> {
    if width < 2 {
        return Span::raw("");
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let filled = ((width - 1) as f64 * ratio).round() as usize;
    let bar = format!("{}●{}", "━".repeat(filled), "─".repeat(width - 1 - filled));
    Span::styled(bar, Style::new().fg(ACCENT))
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let line = match &app.message {
        Some(m) if m.kind == MessageKind::Error => {
            Line::from(format!(" {}", m.text)).fg(Color::Red).bold()
        }
        Some(m) => Line::from(format!(" {}", m.text)).fg(ACCENT),
        None => {
            let mut spans = vec![Span::raw(" ")];
            for (i, (keys, what)) in HINTS.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(*keys, Style::new().fg(ACCENT)));
                spans.push(Span::styled(format!(" {what}"), Style::new().dim()));
            }
            Line::from(spans)
        }
    };
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Left), area);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::{Action, Effect};
    use crate::bandcamp::{DiscoverQuery, parse_album, parse_discover};
    use crate::player::Status;

    fn app_with_page() -> App {
        let (mut app, effects) = App::new(
            DiscoverQuery {
                tags: vec!["electronic".into()],
                ..DiscoverQuery::default()
            },
            true,
        );
        let Some(Effect::FetchDiscover { generation, .. }) = effects.into_iter().next() else {
            panic!("no initial fetch");
        };
        let page = parse_discover(include_str!(
            "../tests/fixtures/discover_electronic_top.json"
        ))
        .unwrap();
        app.update(Action::DiscoverLoaded {
            generation,
            result: Ok(page),
        });
        app
    }

    fn render(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal
    }

    #[test]
    fn discover_with_details() {
        let mut app = app_with_page();
        app.update(Action::Down);
        insta::assert_snapshot!(render(&mut app, 120, 20).backend());
    }

    #[test]
    fn narrow_terminal_hides_details() {
        let mut app = app_with_page();
        insta::assert_snapshot!(render(&mut app, 70, 16).backend());
    }

    #[test]
    fn now_playing_bar() {
        let mut app = app_with_page();
        let album = parse_album(include_str!("../tests/fixtures/tralbum_album.json")).unwrap();
        app.now_playing = Some(Arc::new(album));
        app.update(Action::Player {
            event: None,
            status: Status {
                state: PlaybackState::Paused,
                index: Some(1),
                position: Duration::from_secs(72),
                duration: Duration::from_secs(231),
                volume: 0.8,
            },
        });
        insta::assert_snapshot!(render(&mut app, 100, 16).backend());
    }

    #[test]
    fn loading_and_error_states() {
        let (mut app, _) = App::new(DiscoverQuery::default(), false);
        insta::assert_snapshot!(render(&mut app, 80, 12).backend());
    }

    #[test]
    fn page_rows_follow_the_terminal_height() {
        let mut app = app_with_page();
        render(&mut app, 100, 30);
        // 30 rows - header - now playing (3) - footer - borders (2) - column header.
        assert_eq!(app.page_rows, 22);
    }

    #[test]
    fn progress_bar_widths() {
        assert_eq!(progress_bar(5, 0.0).content, "●────");
        assert_eq!(progress_bar(5, 1.0).content, "━━━━●");
        assert_eq!(progress_bar(5, 0.5).content, "━━●──");
        assert_eq!(progress_bar(1, 0.5).content, "");
    }
}
