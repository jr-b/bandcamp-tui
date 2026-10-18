//! The listening view (`v`): only what's playing, with the cover as large as it fits.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};

use super::now_playing::{progress_line, state_label};
use super::{ACCENT, pane};
use crate::app::{App, UpNext};
use crate::art::{Art, ArtBox};
use crate::bandcamp::Album;
use crate::format;
use crate::player::PlaybackState;

/// The progress line never gets wider than this, so it stays readable on wide terminals.
const MAX_PROGRESS_COLS: u16 = 72;

pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = pane("Listening", true).title(Line::from(" v browse ").right_aligned().dim());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Recorded for `App::after_draw`, which fetches or re-encodes the cover at this size.
    app.listening_art_box = if app.art_enabled {
        crate::art::listening_layout(inner.width, inner.height, app.art_font)
    } else {
        None
    };

    let Some((album, index)) = app.current_track() else {
        let hint = if app.player_available {
            "Nothing playing. Press v to browse, pick a release and press ⏎."
        } else {
            "No audio output."
        };
        let middle = Rect {
            y: inner.y + inner.height / 2,
            height: 1.min(inner.height),
            ..inner
        };
        frame.render_widget(Line::from(hint).dim().centered(), middle);
        return;
    };
    let track = &album.tracks[index];
    let width = usize::from(inner.width.saturating_sub(2));
    let mut lines = track_lines(album, index, app.player.state, width);
    lines.push(Line::default());
    let progress_width = inner.width.saturating_sub(4).min(MAX_PROGRESS_COLS);
    let progress = progress_line(&app.player, track.duration, usize::from(progress_width));
    let progress_row = lines.len();
    lines.push(progress);
    if let Some(next) = app.plays_next() {
        lines.push(Line::default());
        lines.push(up_next_line(next, width));
    }

    let art = app
        .listening_art_box
        .and_then(|art_box| Some((art_box, app.art_for(album.art_id)?)));
    let text_rows = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let cover_rows = art.map_or(0, |(b, _)| b.rows + 1);
    let top = inner.y + inner.height.saturating_sub(cover_rows + text_rows) / 2;

    if let Some((art_box, art)) = art {
        draw_cover(frame, inner, top, art_box, art);
    }

    let bottom = inner.y + inner.height;
    for (i, line) in lines.into_iter().enumerate() {
        let y = top + cover_rows + u16::try_from(i).unwrap_or(u16::MAX);
        if y >= bottom {
            break;
        }
        let row = if i == progress_row {
            Rect {
                x: inner.x + inner.width.saturating_sub(progress_width) / 2,
                y,
                width: progress_width,
                height: 1,
            }
        } else {
            Rect {
                y,
                height: 1,
                ..inner
            }
        };
        frame.render_widget(line.centered(), row);
    }
}

/// Title (with the playback state), artist, and album with the track number.
fn track_lines(
    album: &Album,
    index: usize,
    state: PlaybackState,
    width: usize,
) -> Vec<Line<'static>> {
    let track = &album.tracks[index];
    let (icon, state) = state_label(state);
    let mut title = vec![
        Span::styled(format!("{icon} "), Style::new().fg(ACCENT).bold()),
        Span::styled(
            format::truncate(&track.title, width).into_owned(),
            Style::new().bold(),
        ),
    ];
    if !state.is_empty() {
        title.push(Span::styled(state, Style::new().fg(Color::Yellow)));
    }
    let position = format!(
        "{} · track {} of {}",
        album.title,
        index + 1,
        album.tracks.len()
    );
    vec![
        Line::from(title),
        Line::from(format::truncate(&track.artist, width).into_owned()),
        Line::from(format::truncate(&position, width).into_owned()).dim(),
    ]
}

/// The cover centred at row `top`, or a placeholder of the same size while it loads.
fn draw_cover(frame: &mut Frame, inner: Rect, top: u16, art_box: ArtBox, art: Option<&Art>) {
    let cover = Rect {
        x: inner.x + inner.width.saturating_sub(art_box.cols) / 2,
        y: top,
        width: art_box.cols.min(inner.width),
        height: art_box.rows.min(inner.height),
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
}

fn up_next_line(next: UpNext<'_>, width: usize) -> Line<'static> {
    let (label, more) = match next {
        UpNext::Track(track) => (track.title.clone(), String::new()),
        UpNext::Queue(entry, more) => (
            entry.label(),
            if more > 0 {
                format!(" (+{more} queued)")
            } else {
                String::new()
            },
        ),
        UpNext::Autoplay => ("next Discover release (autoplay)".to_owned(), String::new()),
    };
    let label = format::truncate(&label, width.saturating_sub(6 + more.len())).into_owned();
    Line::from(vec![
        Span::styled("next: ", Style::new().dim()),
        Span::raw(label),
        Span::styled(more, Style::new().dim()),
    ])
}
