//! Rendering. `draw` is a pure function of `App`, apart from recording layout facts the next
//! update needs (table scroll offsets, visible rows).

mod album;
mod discover;
mod help;
mod listening;
mod now_playing;
mod picker;
mod queue;
mod search;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Focus, ListSource, MessageKind, Screen};
use crate::bandcamp::{Filters, Slice};
use crate::format;
use crate::input::{HINTS, LISTENING_HINTS, PICKER_HINTS, QUEUE_HINTS};

const ACCENT: Color = Color::Cyan;
/// Below this width only the focused pane is shown.
const TWO_PANE_MIN_WIDTH: u16 = 90;

pub fn draw(frame: &mut Frame, app: &mut App) {
    // The listening view shows the track and progress itself: no now-playing bar.
    let bar_rows = if app.screen == Screen::Listening {
        0
    } else {
        3
    };
    let [header, body, now_playing, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(bar_rows),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, app);
    if app.screen == Screen::Listening {
        listening::draw(frame, body, app);
    } else if body.width >= TWO_PANE_MIN_WIDTH {
        let [list, album] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(body);
        discover::draw(frame, list, app);
        album::draw(frame, album, app, false);
    } else {
        match app.focus {
            Focus::Discover => discover::draw(frame, body, app),
            Focus::Album => album::draw(frame, body, app, true),
        }
    }
    if app.screen == Screen::Browse {
        now_playing::draw(frame, now_playing, app);
    }
    draw_footer(frame, footer, app);
    if let Some(picker) = app.picker.as_mut() {
        picker::draw(frame, body, picker);
    }
    if app.search_open {
        search::draw(frame, body, &mut app.search);
    }
    if app.queue_view.is_some() {
        queue::draw(frame, body, app);
    }
    if app.show_help {
        help::draw(frame, frame.area());
    }
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
    let mut left = vec![Span::styled(
        " bandcamp ",
        Style::new().fg(Color::Black).bg(ACCENT).bold(),
    )];
    let parts = match &app.source {
        ListSource::Discover => {
            left.push(Span::raw(" Discover"));
            let mut parts = crate::picker::describe(Filters::snapshot(), &app.query);
            parts.insert(1, slice_label(app.query.slice).to_owned());
            parts
        }
        ListSource::Search(query) => {
            left.push(Span::raw(" Search"));
            vec![format!("“{query}”")]
        }
        ListSource::Artist(name) => {
            left.push(Span::raw(" Artist"));
            vec![name.clone()]
        }
    };
    for part in parts {
        left.push(Span::styled(" · ", Style::new().dim()));
        left.push(Span::styled(part, Style::new().bold()));
    }
    let left = Line::from(left);
    let right = if app.discover.total > 0 {
        let mut text = format!("{} releases ", format::thousands(app.discover.total));
        if app.source != ListSource::Discover {
            text.push_str("· esc back ");
        }
        Line::from(text).dim()
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
            let hints = if app.picker.is_some() || app.search_open {
                PICKER_HINTS
            } else if app.queue_view.is_some() {
                QUEUE_HINTS
            } else if app.screen == Screen::Listening {
                LISTENING_HINTS
            } else {
                HINTS
            };
            let mut spans = vec![Span::raw(" ")];
            let mut used = 1;
            for (i, (keys, what)) in hints.iter().enumerate() {
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
    fn genre_picker_popup() {
        let mut app = app_with_page();
        app.update(Action::OpenPicker(crate::picker::PickerKind::Genre));
        insta::assert_snapshot!("genre_picker", render(&mut app, 100, 24).backend());
        for c in "tech".chars() {
            app.update(Action::TypeChar(c));
        }
        insta::assert_snapshot!("genre_picker_search", render(&mut app, 100, 24).backend());
    }

    /// Loads a cover for the selected release at whatever size the last draw asked for.
    fn load_cover(app: &mut App, terminal_size: (u16, u16)) -> crate::art::ArtBox {
        render(app, terminal_size.0, terminal_size.1);
        let art_box = app.album_pane.art_box.expect("room for a cover");
        let art_id = app.selected().unwrap().art_id.unwrap();
        let art = crate::art::encode(
            &ratatui_image::picker::Picker::halfblocks(),
            &crate::art::tests::test_jpeg(),
            art_box,
        )
        .unwrap();
        app.update(Action::ArtEncoded {
            id: art_id,
            art_box,
            result: Ok(art),
        });
        art_box
    }

    fn halfblock_cells(terminal: &Terminal<TestBackend>) -> usize {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|c| matches!(c.symbol(), "▀" | "▄"))
            .count()
    }

    #[test]
    fn album_with_cover_art() {
        // Halfblocks only so the snapshot can show the cover; the app itself never uses them.
        let mut app = app_with_page();
        app.art_enabled = true;
        show_album(&mut app, fixture_album("album"));
        // Loading: placeholder of the same size.
        insta::assert_snapshot!("cover_loading", render(&mut app, 120, 24).backend());
        load_cover(&mut app, (120, 24));
        let terminal = render(&mut app, 120, 24);
        assert!(halfblock_cells(&terminal) > 50, "cover not drawn");
        insta::assert_snapshot!("cover_loaded", terminal.backend());
    }

    #[test]
    fn cover_grows_with_the_terminal() {
        let mut app = app_with_page();
        app.art_enabled = true;
        show_album(&mut app, fixture_album("album"));
        let small = load_cover(&mut app, (120, 24));
        let large = load_cover(&mut app, (160, 50));
        assert!(large.rows > small.rows, "{small:?} → {large:?}");
        let terminal = render(&mut app, 160, 50);
        assert!(halfblock_cells(&terminal) > 3 * 50, "large cover not drawn");
        insta::assert_snapshot!("cover_large", terminal.backend());
    }

    #[test]
    fn narrow_layout_keeps_the_cover_compact() {
        let mut app = app_with_page();
        app.art_enabled = true;
        show_album(&mut app, fixture_album("album"));
        app.update(Action::FocusAlbum);
        // 80 columns: single pane. Tall enough that the wide layout would go bigger.
        render(&mut app, 80, 60);
        let art_box = app.album_pane.art_box.unwrap();
        assert!(art_box.rows <= 8, "{art_box:?}");
    }

    #[test]
    fn tiny_panes_drop_the_cover() {
        let mut app = app_with_page();
        app.art_enabled = true;
        show_album(&mut app, fixture_album("album"));
        render(&mut app, 120, 16);
        assert_eq!(app.album_pane.art_box, None);
    }

    #[test]
    fn search_popup_and_results() {
        let mut app = app_with_page();
        app.update(Action::OpenSearch);
        insta::assert_snapshot!("search_empty", render(&mut app, 110, 24).backend());
        for c in "aphex twin".chars() {
            app.update(Action::TypeChar(c));
        }
        let generation = match app.update(Action::SearchDue(10))[..] {
            [Effect::FetchSearch { generation, .. }] => generation,
            ref other => panic!("{other:?}"),
        };
        let results =
            crate::bandcamp::parse_search(include_str!("../../tests/fixtures/search_aphex.json"))
                .unwrap();
        app.update(Action::SearchLoaded {
            generation,
            result: Ok(results),
        });
        insta::assert_snapshot!("search_results_popup", render(&mut app, 110, 24).backend());
        app.update(Action::Down);
        app.update(Action::Play);
        insta::assert_snapshot!("search_results_list", render(&mut app, 110, 24).backend());
    }

    #[test]
    fn queue_popup_and_next_label() {
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
        app.update(Action::Down);
        app.update(Action::Enqueue);
        app.update(Action::Down);
        app.update(Action::Enqueue);
        let terminal = render(&mut app, 120, 24);
        let text = format!("{}", terminal.backend());
        assert!(text.contains("next: Pete Cannon"), "{text}");
        assert!(text.contains("(+1)"), "{text}");
        assert!(text.contains("│+ Pete Cannon"), "queued marker: {text}");
        app.update(Action::OpenQueue);
        insta::assert_snapshot!(render(&mut app, 120, 24).backend());
    }

    #[test]
    fn help_overlay() {
        let mut app = app_with_page();
        app.update(Action::ShowHelp);
        insta::assert_snapshot!(render(&mut app, 100, 36).backend());
    }

    fn listening_to_joyride(app: &mut App, state: PlaybackState) {
        show_album(app, fixture_album("album"));
        app.update(Action::Play);
        app.update(Action::Player {
            event: None,
            status: Status {
                state,
                index: Some(1),
                position: Duration::from_secs(72),
                duration: Duration::from_secs(231),
                volume: 0.8,
            },
        });
        app.update(Action::ToggleListening);
    }

    #[test]
    fn listening_view_nothing_playing() {
        let mut app = app_with_page();
        app.update(Action::ToggleListening);
        insta::assert_snapshot!(render(&mut app, 100, 20).backend());
    }

    #[test]
    fn listening_view_without_the_queue() {
        let mut app = app_with_page();
        listening_to_joyride(&mut app, PlaybackState::Playing);
        let terminal = render(&mut app, 100, 20);
        let text = format!("{}", terminal.backend());
        assert!(!text.contains("Now playing"), "no bar: {text}");
        assert!(text.contains("next: "), "{text}");
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn listening_view_with_queue_and_cover() {
        let mut app = app_with_page();
        app.art_enabled = true;
        listening_to_joyride(&mut app, PlaybackState::Paused);
        // Single-track entry from the last track: "next" comes from the queue.
        app.update(Action::ToggleListening);
        app.update(Action::Down);
        app.update(Action::Enqueue);
        app.update(Action::Down);
        app.update(Action::Enqueue);
        app.update(Action::ToggleListening);
        let last = app.now_playing.as_ref().unwrap().tracks.len() - 1;
        app.update(Action::Player {
            event: None,
            status: Status {
                index: Some(last),
                ..app.player.clone()
            },
        });

        render(&mut app, 100, 40);
        let art_box = app.listening.art_box.expect("room for a cover");
        assert!(art_box.rows >= 24, "{art_box:?}");
        let art_id = app.now_playing.as_ref().unwrap().art_id.unwrap();
        let art = crate::art::encode(
            &ratatui_image::picker::Picker::halfblocks(),
            &crate::art::tests::test_jpeg(),
            art_box,
        )
        .unwrap();
        app.update(Action::ArtEncoded {
            id: art_id,
            art_box,
            result: Ok(art),
        });
        let terminal = render(&mut app, 100, 40);
        assert!(halfblock_cells(&terminal) > 300, "cover not drawn");
        let text = format!("{}", terminal.backend());
        assert!(text.contains("(paused)"), "{text}");
        assert!(text.contains("(+1 queued)"), "{text}");
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn listening_view_info() {
        let mut app = app_with_page();
        app.art_enabled = true;
        listening_to_joyride(&mut app, PlaybackState::Playing);
        app.update(Action::ToggleInfo);
        let terminal = render(&mut app, 100, 30);
        assert_eq!(app.listening.art_box, None, "no cover behind the text");
        let text = format!("{}", terminal.backend());
        assert!(text.contains("i cover"), "{text}");
        assert!(text.contains("About") && text.contains("Artist"), "{text}");
        insta::assert_snapshot!(terminal.backend());

        // Scrolled past the end in a short terminal: clamped to the text.
        app.update(Action::Bottom);
        render(&mut app, 100, 16);
        let scroll = app.listening.info_scroll;
        assert!(scroll > 0 && scroll < u16::MAX, "{scroll}");
    }

    #[test]
    fn listening_view_narrow() {
        let mut app = app_with_page();
        listening_to_joyride(&mut app, PlaybackState::Buffering);
        insta::assert_snapshot!(render(&mut app, 50, 14).backend());
    }

    #[test]
    fn header_shows_all_filters() {
        let mut app = app_with_page();
        app.query.tags = vec!["electronic".into(), "techno".into()];
        app.query.time_facet_id = Some(1);
        app.query.geoname_id = 2_950_159;
        let terminal = render(&mut app, 120, 10);
        let first_line: String = (0..120)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol().to_owned())
            .collect();
        assert!(
            first_line
                .contains("Discover · electronic › techno · best-selling · this week · berlin"),
            "{first_line}"
        );
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
