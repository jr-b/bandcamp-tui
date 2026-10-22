//! The now-playing bar.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use unicode_width::UnicodeWidthStr;

use super::theme::theme;
use crate::app::App;
use crate::format;
use crate::player::{PlaybackState, Status};

pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    const TITLE: &str = " Now playing ";
    let mut right = Vec::new();
    if let Some((next, more)) = app.up_next() {
        let more = if more > 0 {
            format!(" (+{more})")
        } else {
            String::new()
        };
        let label = format::truncate(&next.label(), usize::from(area.width / 3)).into_owned();
        right.push(Span::styled(" next: ", Style::new().dim()));
        right.push(Span::raw(label));
        right.push(Span::styled(format!("{more} ·"), Style::new().dim()));
    }
    let autoplay = if app.autoplay {
        Span::styled(" autoplay on ", theme().accent)
    } else {
        Span::styled(" autoplay off ", Style::new().dim())
    };
    // Up next goes first when the titles would touch: autoplay is a mode, always shown.
    let room = usize::from(area.width).saturating_sub(TITLE.width() + 2);
    if Line::from(right.clone()).width() + autoplay.width() > room {
        right.clear();
    }
    right.push(autoplay);
    let block = Block::new()
        .borders(Borders::TOP)
        .title(TITLE)
        .title(Line::from(right).right_aligned())
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
        // The volume `+`/`-` set, where it shows while playing.
        if app.player_available {
            let volume = format!("vol {:>3}% ", volume_percent(status.volume));
            frame.render_widget(Line::from(volume).right_aligned().dim(), progress);
        }
        return;
    };
    let track = &album.tracks[index];
    let (icon, state) = state_label(status.state);
    // Most important first, so a narrow bar cuts the album, not the state.
    let line = Line::from(vec![
        Span::styled(format!(" {icon} "), theme().accent.bold()),
        Span::styled(track.title.clone(), Style::new().bold()),
        Span::styled(state, theme().warning),
        Span::raw(" — "),
        Span::raw(track.artist.clone()),
        Span::styled(" · ", Style::new().dim()),
        Span::raw(album.title.clone()),
        Span::styled(
            format!(" · {}/{}", index + 1, album.tracks.len()),
            Style::new().dim(),
        ),
    ]);
    frame.render_widget(super::fit(line, usize::from(info.width)), info);

    frame.render_widget(
        progress_line(status, track.duration, usize::from(progress.width)),
        progress,
    );
}

/// The playback state's icon and, unless it's simply playing or stopped, a note like
/// ` (paused)`.
pub(super) fn state_label(state: PlaybackState) -> (&'static str, &'static str) {
    match state {
        PlaybackState::Playing => ("▶", ""),
        PlaybackState::Paused => ("‖", " (paused)"),
        PlaybackState::Buffering => ("…", " (buffering)"),
        PlaybackState::Loading => ("…", " (loading)"),
        PlaybackState::Stopped => ("■", ""),
    }
}

/// ` 1:12 ━━━●──── 3:51   vol  80% `, `width` cells wide. `api_duration` is used until the
/// player knows the track's real length.
pub(super) fn progress_line(
    status: &Status,
    api_duration: Duration,
    width: usize,
) -> Line<'static> {
    let duration = if status.duration.is_zero() {
        api_duration
    } else {
        status.duration
    };
    let left = format!(" {} ", format::clock(status.position));
    let right = format!(
        " {}   vol {:>3}% ",
        format::clock(duration),
        volume_percent(status.volume)
    );
    let bar_width = width.saturating_sub(left.width() + right.width());
    let ratio = if duration.is_zero() {
        0.0
    } else {
        (status.position.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0)
    };
    Line::from(vec![
        Span::raw(left),
        progress_bar(bar_width, ratio),
        Span::styled(right, Style::new().dim()),
    ])
}

fn volume_percent(volume: f32) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=100"
    )]
    let percent = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
    percent
}

/// `━━━━━●──────`, `width` cells wide.
fn progress_bar(width: usize, ratio: f64) -> Span<'static> {
    if width < 2 {
        return Span::raw("");
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "a terminal width, and a ratio in 0..=1"
    )]
    let filled = ((width - 1) as f64 * ratio).round() as usize;
    let bar = format!("{}●{}", "━".repeat(filled), "─".repeat(width - 1 - filled));
    Span::styled(bar, theme().accent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_bar_widths() {
        assert_eq!(progress_bar(5, 0.0).content, "●────");
        assert_eq!(progress_bar(5, 1.0).content, "━━━━●");
        assert_eq!(progress_bar(5, 0.5).content, "━━●──");
        assert_eq!(progress_bar(1, 0.5).content, "");
    }
}
