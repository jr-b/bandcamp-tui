//! Rendering. `draw` is a pure function of `App`, apart from recording layout facts the next
//! update needs (table scroll offsets, visible rows).

mod album;
mod discover;
mod now_playing;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Focus, MessageKind};
use crate::bandcamp::Slice;
use crate::format;
use crate::input::HINTS;

const ACCENT: Color = Color::Cyan;
/// Below this width only the focused pane is shown.
const TWO_PANE_MIN_WIDTH: u16 = 90;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, now_playing, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, app);
    if body.width >= TWO_PANE_MIN_WIDTH {
        let [list, album] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(body);
        discover::draw(frame, list, app);
        album::draw(frame, album, app);
    } else {
        match app.focus {
            Focus::Discover => discover::draw(frame, body, app),
            Focus::Album => album::draw(frame, body, app),
        }
    }
    now_playing::draw(frame, now_playing, app);
    draw_footer(frame, footer, app);
}

/// A bordered pane, highlighted when focused.
fn pane(title: &str, focused: bool) -> Block<'static> {
    let block = Block::bordered().title(format!(" {title} "));
    if focused {
        block
            .border_style(Style::new().fg(ACCENT))
            .title_style(Style::new().bold())
    } else {
        block.border_style(Style::new().dim())
    }
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
    let width = u16::try_from(right.width()).unwrap_or(u16::MAX);
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(area);
    frame.render_widget(left, l);
    frame.render_widget(right, r);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let line = match &app.message {
        Some(m) if m.kind == MessageKind::Error => {
            Line::from(format!(" {}", m.text)).fg(Color::Red).bold()
        }
        Some(m) => Line::from(format!(" {}", m.text)).fg(ACCENT),
        None => {
            // Show as many whole hints as fit; they're ordered by importance.
            let mut spans = vec![Span::raw(" ")];
            let mut used = 1;
            for (i, (keys, what)) in HINTS.iter().enumerate() {
                let gap = if i > 0 { 2 } else { 0 };
                let width = gap + keys.chars().count() + 1 + what.len();
                if used + width > usize::from(area.width) {
                    break;
                }
                used += width;
                if gap > 0 {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(*keys, Style::new().fg(ACCENT)));
                spans.push(Span::styled(format!(" {what}"), Style::new().dim()));
            }
            Line::from(spans)
        }
    };
    frame.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::{Action, Effect};
    use crate::bandcamp::{Album, DiscoverQuery, parse_album, parse_discover};
    use crate::player::{PlaybackState, Status};

    fn fixture_album(name: &str) -> Box<Album> {
        let json = match name {
            "album" => include_str!("../../tests/fixtures/tralbum_album.json"),
            "compilation" => include_str!("../../tests/fixtures/tralbum_compilation.json"),
            "preorder" => include_str!("../../tests/fixtures/tralbum_preorder.json"),
            _ => unreachable!(),
        };
        Box::new(parse_album(json).unwrap())
    }

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
            "../../tests/fixtures/discover_electronic_top.json"
        ))
        .unwrap();
        app.update(Action::DiscoverLoaded {
            generation,
            result: Ok(page),
        });
        app
    }

    /// Loads `album` as the details of the selected release (whatever it really is).
    fn show_album(app: &mut App, album: Box<Album>) {
        let id = app.selected().unwrap().id;
        app.update(Action::AlbumDue(id));
        let mut album = album;
        album.id = id;
        app.update(Action::AlbumLoaded {
            id,
            result: Ok(album),
        });
    }

    fn render(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal
    }

    #[test]
    fn album_loading() {
        let mut app = app_with_page();
        insta::assert_snapshot!(render(&mut app, 120, 20).backend());
    }

    #[test]
    fn album_with_tracklist_playing() {
        let mut app = app_with_page();
        show_album(&mut app, fixture_album("album"));
        app.update(Action::Play);
        app.update(Action::Player {
            event: None,
            status: Status {
                state: PlaybackState::Playing,
                index: Some(1),
                position: Duration::from_secs(72),
                duration: Duration::from_secs(231),
                volume: 0.8,
            },
        });
        app.update(Action::FocusAlbum);
        app.update(Action::Down);
        app.update(Action::Down);
        insta::assert_snapshot!(render(&mut app, 120, 24).backend());
    }

    #[test]
    fn compilation_shows_track_artists() {
        let mut app = app_with_page();
        show_album(&mut app, fixture_album("compilation"));
        insta::assert_snapshot!(render(&mut app, 120, 22).backend());
    }

    #[test]
    fn preorder_tracks_are_greyed_out() {
        let mut app = app_with_page();
        show_album(&mut app, fixture_album("preorder"));
        let terminal = render(&mut app, 120, 22);
        // Unplayable rows are dim; check a track title cell's style.
        let buffer = terminal.backend().buffer();
        let row = (0..buffer.area.height)
            .find(|&y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains("Ariel")
            })
            .expect("track row rendered");
        let x = (0..buffer.area.width)
            .find(|&x| buffer[(x, row)].symbol() == "A" && buffer[(x + 1, row)].symbol() == "r")
            .unwrap();
        assert!(
            buffer[(x, row)]
                .modifier
                .contains(ratatui::style::Modifier::DIM),
            "unplayable track should be dim"
        );
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn info_view() {
        let mut app = app_with_page();
        show_album(&mut app, fixture_album("album"));
        app.update(Action::FocusAlbum);
        app.update(Action::ToggleInfo);
        insta::assert_snapshot!(render(&mut app, 120, 22).backend());
    }

    #[test]
    fn narrow_terminal_shows_focused_pane() {
        let mut app = app_with_page();
        show_album(&mut app, fixture_album("album"));
        insta::assert_snapshot!("narrow_discover", render(&mut app, 70, 16).backend());
        app.update(Action::ToggleFocus);
        insta::assert_snapshot!("narrow_album", render(&mut app, 70, 16).backend());
    }

    #[test]
    fn startup_without_audio() {
        let (mut app, _) = App::new(DiscoverQuery::default(), false);
        insta::assert_snapshot!(render(&mut app, 80, 12).backend());
    }

    #[test]
    fn now_playing_bar_paused() {
        let mut app = app_with_page();
        app.now_playing = Some(Arc::from(fixture_album("album")));
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
        let terminal = render(&mut app, 100, 16);
        let text = format!("{}", terminal.backend());
        assert!(text.contains("‖ Joyride — Ocean Company"), "{text}");
        assert!(text.contains("1:12 ━━"), "{text}");
        assert!(text.contains("3:51   vol  80%"), "{text}");
    }

    #[test]
    fn page_rows_follow_the_terminal_height() {
        let mut app = app_with_page();
        show_album(&mut app, fixture_album("album"));
        render(&mut app, 100, 30);
        // 30 rows - header - now playing (3) - footer - borders (2) - column header.
        assert_eq!(app.page_rows, 22);
        assert!(
            app.album_pane.page_rows > 10,
            "{}",
            app.album_pane.page_rows
        );
    }
}
