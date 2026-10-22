//! `App::update` driven with synthetic actions: no terminal, network or audio.

use super::*;

fn page_fixture() -> DiscoverPage {
    let json = include_str!("../../tests/fixtures/discover_electronic_top.json");
    crate::bandcamp::parse_discover(json).unwrap()
}

fn album_fixture(name: &str) -> Box<Album> {
    let json = match name {
        "album" => include_str!("../../tests/fixtures/tralbum_album.json"),
        "preorder" => include_str!("../../tests/fixtures/tralbum_preorder.json"),
        _ => unreachable!(),
    };
    Box::new(crate::bandcamp::parse_album(json).unwrap())
}

/// An app with the fixture page loaded. The first item (Ocean Company, the album fixture)
/// is selected and its details are debounced.
fn loaded() -> App {
    let (mut app, effects) = App::new(DiscoverQuery::default(), true);
    let Some(Effect::FetchDiscover { generation, .. }) = effects.first().cloned() else {
        panic!("expected initial fetch, got {effects:?}");
    };
    app.page_rows = 3;
    let effects = app.update(Action::DiscoverLoaded {
        generation,
        result: Ok(page_fixture()),
    });
    assert!(
        matches!(
            effects.as_slice(),
            [
                Effect::DebounceAlbum(ItemId(3_457_085_593)),
                Effect::FetchDiscover {
                    cursor: Some(_),
                    ..
                }
            ]
        ),
        "{effects:?}"
    );
    app
}

/// `loaded()` plus the first album's details.
fn with_album() -> App {
    let mut app = loaded();
    let id = app.selected().unwrap().id;
    assert!(matches!(
        app.update(Action::AlbumDue(id))[..],
        [Effect::FetchAlbum { .. }]
    ));
    app.update(Action::AlbumLoaded {
        id,
        result: Ok(album_fixture("album")),
    });
    app
}

#[test]
fn startup_fetches_first_page() {
    let (app, effects) = App::new(DiscoverQuery::default(), true);
    assert!(app.discover.loading);
    assert_eq!(
        effects,
        [Effect::FetchDiscover {
            query: DiscoverQuery::default(),
            cursor: None,
            generation: 1
        }]
    );
}

#[test]
fn navigation_clamps_to_the_list() {
    let mut app = loaded();
    assert_eq!(app.table.selected(), Some(0));
    app.update(Action::Up);
    assert_eq!(app.table.selected(), Some(0));
    app.update(Action::Down);
    assert_eq!(app.table.selected(), Some(1));
    app.update(Action::Bottom);
    assert_eq!(app.table.selected(), Some(3));
    app.update(Action::PageUp);
    assert_eq!(app.table.selected(), Some(1));
    app.update(Action::Top);
    assert_eq!(app.table.selected(), Some(0));
}

#[test]
fn appends_next_page_and_stops_at_the_end() {
    let mut app = loaded();
    let generation = app.discover.generation;
    app.update(Action::DiscoverLoaded {
        generation,
        result: Ok(page_fixture()),
    });
    assert_eq!(app.discover.items.len(), 8);
    let effects = app.update(Action::DiscoverLoaded {
        generation,
        result: Ok(DiscoverPage {
            items: vec![],
            total: 8,
            next_cursor: None,
        }),
    });
    assert_eq!(effects, []);
    assert!(!app.discover.has_more());
    app.update(Action::Bottom);
    assert!(!app.discover.loading, "no fetch past the last page");
}

#[test]
fn ignores_responses_for_a_replaced_query() {
    let mut app = loaded();
    let old = app.discover.generation;
    let effects = app.update(Action::CycleSlice);
    assert_eq!(app.query.slice, Slice::New);
    assert_eq!(app.discover.items, []);
    assert!(
        matches!(&effects[..], [Effect::FetchDiscover { query, cursor: None, .. }, Effect::SaveSettings(_)] if query.slice == Slice::New)
    );
    app.update(Action::DiscoverLoaded {
        generation: old,
        result: Ok(page_fixture()),
    });
    assert_eq!(app.discover.items, [], "stale page was applied");
}

#[test]
fn caps_the_list_size() {
    let mut app = loaded();
    let generation = app.discover.generation;
    while app.discover.items.len() < MAX_ITEMS {
        app.update(Action::DiscoverLoaded {
            generation,
            result: Ok(page_fixture()),
        });
    }
    assert_eq!(app.discover.items.len(), MAX_ITEMS);
    assert!(!app.discover.has_more());
}

#[test]
fn details_are_debounced_and_only_fetched_if_still_selected() {
    let mut app = loaded();
    let first = app.selected().unwrap().id;
    // Scroll past the first release before its delay is up.
    let effects = app.update(Action::Down);
    let [Effect::DebounceAlbum(second)] = effects[..] else {
        panic!("{effects:?}");
    };
    assert_eq!(
        app.update(Action::AlbumDue(first)),
        [],
        "moved away; no fetch"
    );
    assert!(matches!(
        app.update(Action::AlbumDue(second))[..],
        [Effect::FetchAlbum { id, .. }] if id == second
    ));
    // In flight: neither selecting it again nor another due fires a second request.
    app.update(Action::Up);
    assert_eq!(app.update(Action::Down), []);
    assert_eq!(app.update(Action::AlbumDue(second)), []);
}

#[test]
fn album_view_follows_loading_states() {
    let mut app = loaded();
    assert!(matches!(app.album_view(), AlbumView::Loading(_)));
    let id = app.selected().unwrap().id;
    app.update(Action::AlbumDue(id));
    app.update(Action::AlbumLoaded {
        id,
        result: Err("boom".into()),
    });
    assert!(matches!(app.album_view(), AlbumView::Failed(_, "boom")));
    assert!(
        app.message.is_none(),
        "background failures stay in the pane"
    );
    // Refresh clears errors so it can be retried.
    app.update(Action::Refresh);
    assert!(app.album_errors.is_empty());

    let app = with_album();
    let AlbumView::Loaded(album) = app.album_view() else {
        panic!();
    };
    assert_eq!(album.title, "Ocean Company");
    assert_eq!(app.album_pane.tracks.selected(), Some(0));
}

#[test]
fn play_from_discover_fetches_immediately_then_queues() {
    let mut app = loaded();
    let effects = app.update(Action::Play);
    let [Effect::FetchAlbum { id, .. }] = effects[..] else {
        panic!("expected album fetch, got {effects:?}");
    };
    let effects = app.update(Action::AlbumLoaded {
        id,
        result: Ok(album_fixture("album")),
    });
    let [Effect::Player(PlayerCommand::PlayQueue(queue, 0))] = &effects[..] else {
        panic!("expected play, got {effects:?}");
    };
    assert_eq!(queue.len(), 10);
    assert_eq!(app.now_playing.as_ref().unwrap().title, "Ocean Company");
}

#[test]
fn play_from_tracklist_starts_at_that_track() {
    let mut app = with_album();
    app.update(Action::ToggleFocus);
    assert_eq!(app.focus, Focus::Album);
    app.update(Action::Down);
    app.update(Action::Down);
    let effects = app.update(Action::Play);
    assert!(
        matches!(&effects[..], [Effect::Player(PlayerCommand::PlayQueue(q, 2))] if q.len() == 10),
        "{effects:?}"
    );
    // Same album again: jump within the queue rather than rebuilding it.
    app.update(Action::Down);
    assert_eq!(
        app.update(Action::Play),
        [Effect::Player(PlayerCommand::PlayIndex(3))]
    );
}

#[test]
fn unplayable_track_shows_an_error() {
    let mut app = loaded();
    let id = app.selected().unwrap().id;
    app.update(Action::AlbumDue(id));
    app.update(Action::AlbumLoaded {
        id,
        result: Ok(album_fixture("preorder")),
    });
    app.update(Action::FocusAlbum);
    assert_eq!(app.update(Action::Play), []);
    let message = app.message.clone().unwrap();
    assert_eq!(message.kind, MessageKind::Error);
    assert!(message.text.contains("isn't available"), "{}", message.text);

    app.update(Action::FocusDiscover);
    assert_eq!(app.update(Action::Play), []);
    assert!(app.message.unwrap().text.contains("pre-order"));
}

#[test]
fn tracklist_follows_the_playing_track() {
    let mut app = with_album();
    app.update(Action::Play);
    let status = |index| Status {
        state: PlaybackState::Playing,
        index: Some(index),
        ..Status::default()
    };
    app.update(Action::Player {
        event: Some(PlayerEvent::TrackChanged(Some(4))),
        status: status(4),
    });
    assert_eq!(app.album_pane.tracks.selected(), Some(4));
    // Not while the user is browsing the tracklist.
    app.update(Action::FocusAlbum);
    app.update(Action::Up);
    app.update(Action::Player {
        event: Some(PlayerEvent::TrackChanged(Some(5))),
        status: status(5),
    });
    assert_eq!(app.album_pane.tracks.selected(), Some(3));
}

#[test]
fn info_view_scrolls_instead_of_moving_tracks() {
    let mut app = with_album();
    app.update(Action::FocusAlbum);
    app.update(Action::ToggleInfo);
    app.update(Action::Down);
    app.update(Action::Down);
    assert_eq!(app.album_pane.info_scroll, 2);
    assert_eq!(app.album_pane.tracks.selected(), Some(0));
    app.update(Action::Top);
    assert_eq!(app.album_pane.info_scroll, 0);
}

#[test]
fn picking_a_genre_reloads_and_saves() {
    let mut app = loaded();
    app.update(Action::OpenPicker(PickerKind::Genre));
    // Keys go to the picker, not the list.
    app.update(Action::Down);
    assert_eq!(app.table.selected(), Some(0));
    for c in "techno".chars() {
        app.update(Action::Edit(Edit::Insert(c)));
    }
    let effects = app.update(Action::Play);
    assert!(app.picker.is_none());
    assert_eq!(app.query.tags, ["electronic", "techno"]);
    assert!(
        matches!(&effects[..], [Effect::FetchDiscover { query, cursor: None, .. }, Effect::SaveSettings(s)]
            if query.tags == ["electronic", "techno"] && s.discover.tags == ["electronic", "techno"]),
        "{effects:?}"
    );
}

#[test]
fn picking_the_current_value_or_cancelling_changes_nothing() {
    let mut app = loaded();
    app.update(Action::OpenPicker(PickerKind::Time));
    assert_eq!(app.update(Action::Play), [], "Any time is already selected");
    app.update(Action::OpenPicker(PickerKind::Location));
    app.update(Action::Edit(Edit::Insert('b')));
    assert_eq!(app.update(Action::ClosePopup), []);
    assert!(app.picker.is_none());
    assert_eq!(app.query.geoname_id, 0);
}

#[test]
fn api_results_still_arrive_while_the_picker_is_open() {
    let mut app = loaded();
    app.update(Action::OpenPicker(PickerKind::Genre));
    let generation = app.discover.generation;
    app.update(Action::DiscoverLoaded {
        generation,
        result: Ok(page_fixture()),
    });
    assert_eq!(app.discover.items.len(), 8);
    assert!(app.picker.is_some());
}

#[test]
fn expired_stream_urls_refetch_the_album_once() {
    let mut app = with_album();
    app.update(Action::Play);
    let id = app.now_playing.as_ref().unwrap().id;
    let expired = |index| Action::Player {
        event: Some(PlayerEvent::TrackFailed {
            index,
            message: "HTTP 403".into(),
            expired: true,
        }),
        status: Status::default(),
    };

    let effects = app.update(expired(3));
    assert!(
        matches!(&effects[..], [Effect::FetchAlbum { id: fetched, .. }] if *fetched == id),
        "{effects:?}"
    );
    assert!(app.message.as_ref().unwrap().text.contains("expired"));
    // Fresh details arrive: resume at the track that failed, with a new queue.
    let effects = app.update(Action::AlbumLoaded {
        id,
        result: Ok(album_fixture("album")),
    });
    assert!(
        matches!(
            &effects[..],
            [Effect::Player(PlayerCommand::PlayQueue(_, 3))]
        ),
        "{effects:?}"
    );

    // Expiring again before anything played: report it rather than loop.
    assert_eq!(app.update(expired(3)), []);
    assert_eq!(app.message.as_ref().unwrap().kind, MessageKind::Error);

    // Once playback works again, a later expiry is retried again.
    app.update(Action::Play);
    app.update(Action::Player {
        event: None,
        status: Status {
            state: PlaybackState::Playing,
            index: Some(0),
            ..Status::default()
        },
    });
    assert!(matches!(
        app.update(expired(0))[..],
        [Effect::FetchAlbum { .. }]
    ));
}

#[test]
fn open_in_browser_uses_the_selected_release() {
    let mut app = loaded();
    app.update(Action::Down);
    let effects = app.update(Action::OpenInBrowser);
    assert!(
        matches!(&effects[..], [Effect::OpenUrl(url)] if url.as_str()
            == "https://n4records.bandcamp.com/album/pete-cannon-from-the-manor-house-n4lp001"),
        "{effects:?}"
    );
}

const SMALL_BOX: ArtBox = ArtBox::new(12, 6, FONT);
const BIG_BOX: ArtBox = ArtBox::new(24, 12, FONT);
/// The cell size `App` starts with.
const FONT: ratatui_image::FontSize = ratatui_image::FontSize::new(10, 20);

/// An app with art on and the cover box as the UI would record it.
fn with_art(art_box: ArtBox) -> App {
    let mut app = loaded();
    app.art_enabled = true;
    app.album_pane.art_box = Some(art_box);
    app
}

fn encoded(art_box: ArtBox) -> Art {
    crate::art::encode(
        &ratatui_image::picker::Picker::halfblocks(),
        &crate::art::tests::test_jpeg(),
        art_box,
    )
    .unwrap()
}

/// Downloads and encodes the selected release's cover at `art_box`.
fn load_cover(app: &mut App, art_box: ArtBox) -> ArtId {
    let item = app.selected().unwrap().clone();
    let art_id = item.art_id.unwrap();
    let effects = app.update(Action::AlbumDue(item.id));
    let Some(&Effect::FetchArt { id, size }) = effects
        .iter()
        .find(|e| matches!(e, Effect::FetchArt { .. }))
    else {
        panic!("{effects:?}");
    };
    assert_eq!(id, art_id);
    let effects = app.update(Action::ArtFetched {
        id,
        size,
        result: Ok(crate::art::tests::test_jpeg().into()),
    });
    assert!(
        matches!(&effects[..], [Effect::EncodeArt { art_box: b, .. }] if *b == art_box),
        "{effects:?}"
    );
    app.update(Action::ArtEncoded {
        id,
        art_box,
        result: Ok(encoded(art_box)),
    });
    art_id
}

#[test]
fn art_is_fetched_with_details_only_when_enabled() {
    // Disabled (the default): no art effects at all.
    let mut app = loaded();
    let id = app.selected().unwrap().id;
    assert!(matches!(
        app.update(Action::AlbumDue(id))[..],
        [Effect::FetchAlbum { .. }]
    ));

    let mut app = with_art(SMALL_BOX);
    let art_id = app.selected().unwrap().art_id.unwrap();
    assert_eq!(app.art_for(Some(art_id)), Some(None), "loading");
    load_cover(&mut app, SMALL_BOX);
    assert!(matches!(app.art_for(Some(art_id)), Some(Some(a)) if a.art_box == SMALL_BOX));

    // Leaving and coming back: details and art are cached, nothing to fetch.
    let item = app.selected().unwrap().id;
    app.update(Action::AlbumLoaded {
        id: item,
        result: Ok(album_fixture("album")),
    });
    app.update(Action::Down);
    assert!(matches!(app.update(Action::Up)[..], []));
    assert_eq!(app.after_draw(), []);
}

#[test]
fn the_download_size_follows_the_box() {
    let mut app = with_art(SMALL_BOX);
    let item = app.selected().unwrap().id;
    let effects = app.update(Action::AlbumDue(item));
    assert!(effects.contains(&Effect::FetchArt {
        id: app.selected().unwrap().art_id.unwrap(),
        size: ImageSize::Px150,
    }));

    let mut app = with_art(BIG_BOX);
    let item = app.selected().unwrap().id;
    let effects = app.update(Action::AlbumDue(item));
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::FetchArt {
                size: ImageSize::Px350,
                ..
            }
        )),
        "{effects:?}"
    );
}

#[test]
fn a_resize_re_encodes_and_shows_the_old_size_meanwhile() {
    let mut app = with_art(BIG_BOX);
    let art_id = load_cover(&mut app, BIG_BOX);

    // Shrinking: the source is big enough, so just re-encode.
    app.album_pane.art_box = Some(SMALL_BOX);
    let effects = app.after_draw();
    assert!(
        matches!(&effects[..], [Effect::EncodeArt { art_box, .. }] if *art_box == SMALL_BOX),
        "{effects:?}"
    );
    // Nothing smaller is encoded yet: the big one doesn't fit, so a placeholder.
    assert_eq!(app.art_for(Some(art_id)), Some(None));
    assert_eq!(app.after_draw(), [], "already encoding");
    app.update(Action::ArtEncoded {
        id: art_id,
        art_box: SMALL_BOX,
        result: Ok(encoded(SMALL_BOX)),
    });

    // Growing back: the big encoding is still cached, so no work at all.
    app.album_pane.art_box = Some(BIG_BOX);
    assert_eq!(app.after_draw(), []);
    assert!(matches!(app.art_for(Some(art_id)), Some(Some(a)) if a.art_box == BIG_BOX));
}

#[test]
fn zooming_re_encodes_for_the_new_cell_size() {
    let mut app = with_art(BIG_BOX);
    let art_id = load_cover(&mut app, BIG_BOX);

    // Zoomed out: the cover spans the same cells, but they're smaller now. The old encoding is
    // too big in pixels, so it's neither reused nor shown while the new one encodes.
    let zoomed = ArtBox::new(24, 12, ratatui_image::FontSize::new(8, 16));
    app.album_pane.art_box = Some(zoomed);
    let effects = app.after_draw();
    assert!(
        matches!(&effects[..], [Effect::EncodeArt { art_box, .. }] if *art_box == zoomed),
        "{effects:?}"
    );
    assert_eq!(app.art_for(Some(art_id)), Some(None));
}

#[test]
fn growing_past_the_source_downloads_a_bigger_one() {
    let mut app = with_art(SMALL_BOX);
    let art_id = load_cover(&mut app, SMALL_BOX);
    app.album_pane.art_box = Some(BIG_BOX);
    let effects = app.after_draw();
    assert_eq!(
        effects,
        [Effect::FetchArt {
            id: art_id,
            size: ImageSize::Px350
        }]
    );
    // While it downloads, the small cover stands in.
    assert!(matches!(app.art_for(Some(art_id)), Some(Some(a)) if a.art_box == SMALL_BOX));
}

#[test]
fn no_room_means_no_cover_and_no_download() {
    let mut app = loaded();
    app.art_enabled = true;
    app.album_pane.art_box = None;
    let item = app.selected().unwrap().clone();
    assert!(matches!(
        app.update(Action::AlbumDue(item.id))[..],
        [Effect::FetchAlbum { .. }]
    ));
    assert_eq!(app.art_for(item.art_id), None);
}

#[test]
fn failed_art_falls_back_to_text_and_isnt_retried() {
    let mut app = with_art(SMALL_BOX);
    let item = app.selected().unwrap().clone();
    let art_id = item.art_id.unwrap();
    app.update(Action::AlbumDue(item.id));
    app.update(Action::ArtFetched {
        id: art_id,
        size: ImageSize::Px150,
        result: Err("404".into()),
    });
    assert_eq!(app.art_for(Some(art_id)), None);
    assert!(app.fetch_art(Some(art_id)).is_none());
    assert_eq!(app.art_for(None), None, "releases without art have none");
}

/// Plays the first release (Ocean Company) and lets it finish.
fn finish_first_album(app: &mut App) -> Vec<Effect> {
    app.update(Action::Play);
    assert_eq!(app.now_playing.as_ref().unwrap().title, "Ocean Company");
    app.update(Action::Player {
        event: Some(PlayerEvent::QueueEnded),
        status: Status::default(),
    })
}

fn fetched_album(effects: &[Effect]) -> Option<ItemId> {
    effects.iter().find_map(|e| match e {
        Effect::FetchAlbum { id, .. } => Some(*id),
        _ => None,
    })
}

/// A page of new releases: the fixture with fresh ids.
fn more_releases(offset: u64) -> DiscoverPage {
    let mut page = page_fixture();
    for item in &mut page.items {
        item.id = ItemId(item.id.0 + offset);
    }
    page
}

#[test]
fn autoplay_continues_with_the_next_release_and_follows_in_the_list() {
    let mut app = with_album();
    let next = app.discover.items[1].id;
    let effects = finish_first_album(&mut app);
    assert_eq!(fetched_album(&effects), Some(next), "{effects:?}");
    assert_eq!(app.table.selected(), Some(1), "selection follows");
    assert!(
        app.message
            .as_ref()
            .unwrap()
            .text
            .starts_with("Up next: Pete Cannon")
    );

    let effects = app.update(Action::AlbumLoaded {
        id: next,
        result: Ok(album_fixture("album")),
    });
    assert!(
        matches!(
            &effects[..],
            [Effect::Player(PlayerCommand::PlayQueue(_, 0))]
        ),
        "{effects:?}"
    );
    assert!(
        app.message.as_ref().unwrap().text.starts_with("Up next"),
        "message kept"
    );
}

#[test]
fn autoplay_skips_preorders_and_unplayable_releases() {
    let mut app = with_album();
    app.discover.items[1].is_preorder = true;
    let (third, fourth) = (app.discover.items[2].id, app.discover.items[3].id);
    let effects = finish_first_album(&mut app);
    assert_eq!(fetched_album(&effects), Some(third), "pre-order skipped");

    // The third has nothing streamable: move on to the fourth.
    let effects = app.update(Action::AlbumLoaded {
        id: third,
        result: Ok(album_fixture("preorder")),
    });
    assert_eq!(fetched_album(&effects), Some(fourth), "{effects:?}");
    assert_ne!(app.message.as_ref().unwrap().kind, MessageKind::Error);
}

#[test]
fn autoplay_does_not_move_the_selection_while_browsing() {
    let mut app = with_album();
    app.update(Action::Play);
    app.update(Action::Bottom);
    let browsing = app.table.selected();
    let effects = app.update(Action::Player {
        event: Some(PlayerEvent::QueueEnded),
        status: Status::default(),
    });
    assert_eq!(fetched_album(&effects), Some(app.discover.items[1].id));
    assert_eq!(app.table.selected(), browsing);
}

#[test]
fn autoplay_loads_the_next_page_at_the_end_of_the_list() {
    let mut app = with_album();
    let generation = app.discover.generation;
    // Finish the last loaded release while the next page is still on its way.
    app.update(Action::Bottom);
    let last = app.selected().unwrap().id;
    app.update(Action::AlbumDue(last));
    let mut album = album_fixture("album");
    album.id = last; // details of the release we asked for
    app.update(Action::AlbumLoaded {
        id: last,
        result: Ok(album),
    });
    app.update(Action::Play);
    let effects = app.update(Action::Player {
        event: Some(PlayerEvent::QueueEnded),
        status: Status::default(),
    });
    assert_eq!(fetched_album(&effects), None, "nothing after it yet");

    let effects = app.update(Action::DiscoverLoaded {
        generation,
        result: Ok(more_releases(1)),
    });
    assert_eq!(
        fetched_album(&effects),
        Some(app.discover.items[4].id),
        "{effects:?}"
    );
}

#[test]
fn autoplay_gives_up_after_repeated_failures() {
    let mut app = with_album();
    let generation = app.discover.generation;
    for offset in [1, 2] {
        app.update(Action::DiscoverLoaded {
            generation,
            result: Ok(more_releases(offset)),
        });
    }
    let mut effects = finish_first_album(&mut app);
    let mut attempts = 0;
    while let Some(id) = fetched_album(&effects) {
        attempts += 1;
        effects = app.update(Action::AlbumLoaded {
            id,
            result: Err("HTTP 500".into()),
        });
    }
    assert_eq!(attempts, 5);
    assert_eq!(app.message.as_ref().unwrap().kind, MessageKind::Error);
    assert!(
        app.message
            .as_ref()
            .unwrap()
            .text
            .contains("Autoplay stopped")
    );
}

#[test]
fn autoplay_off_stops_at_the_end_and_can_be_toggled() {
    let mut app = with_album();
    app.update(Action::ToggleAutoplay);
    assert!(!app.autoplay);
    assert!(
        app.message
            .as_ref()
            .unwrap()
            .text
            .starts_with("Autoplay off")
    );
    assert_eq!(fetched_album(&finish_first_album(&mut app)), None);
    assert!(app.now_playing.is_none());

    app.update(Action::ToggleAutoplay);
    assert!(app.autoplay);
}

#[test]
fn choosing_something_else_cancels_autoplay() {
    let mut app = with_album();
    let next = app.discover.items[1].id;
    finish_first_album(&mut app);
    // While the next release loads, the user picks the third one themselves.
    app.update(Action::Down);
    app.update(Action::Play);
    let effects = app.update(Action::AlbumLoaded {
        id: next,
        result: Err("HTTP 500".into()),
    });
    assert_eq!(
        effects,
        [],
        "a cancelled autoplay attempt doesn't skip onwards"
    );
}

fn search_fixture() -> Vec<SearchResult> {
    let json = include_str!("../../tests/fixtures/search_aphex.json");
    crate::bandcamp::parse_search(json).unwrap()
}

fn artist_fixture() -> Box<Artist> {
    let json = include_str!("../../tests/fixtures/band_aphex.json");
    Box::new(crate::bandcamp::parse_artist(json).unwrap())
}

/// Opens search, types `text`, and lets the results arrive.
fn search_for(app: &mut App, text: &str) {
    app.update(Action::OpenSearch);
    let mut generation = 0;
    for c in text.chars() {
        let effects = app.update(Action::Edit(Edit::Insert(c)));
        let [Effect::DebounceSearch(g)] = effects[..] else {
            panic!("{effects:?}");
        };
        generation = g;
    }
    let effects = app.update(Action::SearchDue(generation));
    assert!(
        matches!(&effects[..], [Effect::FetchSearch { query, .. }] if query == text),
        "{effects:?}"
    );
    app.update(Action::SearchLoaded {
        generation,
        result: Ok(search_fixture()),
    });
}

#[test]
fn search_is_debounced_and_ignores_stale_results() {
    let mut app = loaded();
    app.update(Action::OpenSearch);
    let [Effect::DebounceSearch(first)] = app.update(Action::Edit(Edit::Insert('a')))[..] else {
        panic!();
    };
    let [Effect::DebounceSearch(second)] = app.update(Action::Edit(Edit::Insert('p')))[..] else {
        panic!();
    };
    assert_eq!(
        app.update(Action::SearchDue(first)),
        [],
        "superseded by more typing"
    );
    assert!(app.search.loading);
    // A late answer to the old query is dropped.
    app.update(Action::SearchLoaded {
        generation: first,
        result: Ok(search_fixture()),
    });
    assert_eq!(app.search.results, []);
    assert!(matches!(
        app.update(Action::SearchDue(second))[..],
        [Effect::FetchSearch { .. }]
    ));
    // Keys go to the box: `j` is text, not "move down".
    app.update(Action::Edit(Edit::Insert('j')));
    assert_eq!(app.search.query.as_str(), "apj");
    assert_eq!(app.table.selected(), Some(0));
}

#[test]
fn opening_a_release_shows_the_results_and_esc_restores_discover() {
    let mut app = loaded();
    app.update(Action::Down);
    let discover_items = app.discover.items.len();
    search_for(&mut app, "aphex twin");
    assert_eq!(app.search.results.len(), 7);
    // Results: artist, 4 albums, 2 tracks. Pick the second album (Syro).
    app.update(Action::Down);
    app.update(Action::Down);
    let effects = app.update(Action::Play);
    assert!(!app.search_open);
    assert_eq!(app.source, ListSource::Search("aphex twin".into()));
    assert_eq!(app.discover.items.len(), 6, "releases only, no artists");
    assert_eq!(app.selected().unwrap().title, "Syro");
    assert!(
        matches!(&effects[..], [Effect::DebounceAlbum(id)] if *id == app.selected().unwrap().id),
        "{effects:?}"
    );
    assert!(!app.discover.has_more(), "search has no more pages");

    let effects = app.update(Action::BackToDiscover);
    assert_eq!(app.source, ListSource::Discover);
    assert_eq!(app.discover.items.len(), discover_items);
    assert_eq!(app.table.selected(), Some(1), "selection restored");
    assert!(
        effects
            .iter()
            .all(|e| !matches!(e, Effect::FetchDiscover { cursor: None, .. }))
    );
}

#[test]
fn opening_an_artist_lists_their_releases() {
    let mut app = loaded();
    search_for(&mut app, "aphex twin");
    let effects = app.update(Action::Play); // the artist is first
    let band_id = BandId(1_533_096_991);
    assert_eq!(effects.last(), Some(&Effect::FetchArtist(band_id)));
    assert_eq!(app.source, ListSource::Artist("Aphex Twin".into()));
    assert!(app.discover.loading);

    let effects = app.update(Action::ArtistLoaded {
        band_id,
        result: Ok(artist_fixture()),
    });
    assert_eq!(app.discover.items.len(), 5);
    assert!(!app.discover.loading);
    assert_eq!(app.selected().unwrap().title, "Drukqs");
    assert!(
        matches!(&effects[..], [Effect::DebounceAlbum(_)]),
        "{effects:?}"
    );
}

#[test]
fn a_late_artist_page_is_ignored_after_leaving() {
    let mut app = loaded();
    search_for(&mut app, "aphex twin");
    app.update(Action::Play);
    app.update(Action::BackToDiscover);
    let effects = app.update(Action::ArtistLoaded {
        band_id: BandId(1_533_096_991),
        result: Ok(artist_fixture()),
    });
    assert_eq!(effects, []);
    assert_eq!(app.source, ListSource::Discover);
}

#[test]
fn changing_filters_leaves_search() {
    let mut app = loaded();
    search_for(&mut app, "aphex twin");
    app.update(Action::Down);
    app.update(Action::Play);
    assert_ne!(app.source, ListSource::Discover);
    let effects = app.update(Action::CycleSlice);
    assert_eq!(app.source, ListSource::Discover);
    assert!(matches!(
        &effects[0],
        Effect::FetchDiscover { cursor: None, .. }
    ));
    // Nothing saved to go back to any more.
    assert_eq!(app.update(Action::BackToDiscover), []);
}

#[test]
fn reopening_search_keeps_the_last_query_and_esc_closes_it() {
    let mut app = loaded();
    search_for(&mut app, "aphex twin");
    app.update(Action::ClosePopup);
    assert!(!app.search_open);
    assert_eq!(
        app.source,
        ListSource::Discover,
        "closing doesn't change the list"
    );
    app.update(Action::OpenSearch);
    assert_eq!(app.search.query.as_str(), "aphex twin");
    assert_eq!(app.search.results.len(), 7);
    app.update(Action::Edit(Edit::Clear));
    assert_eq!(app.search.results, []);
}

#[test]
fn open_in_browser_prefers_the_release_page_over_the_artist_page() {
    let mut app = loaded();
    search_for(&mut app, "aphex twin");
    app.update(Action::Play);
    app.update(Action::ArtistLoaded {
        band_id: BandId(1_533_096_991),
        result: Ok(artist_fixture()),
    });
    let item = app.selected().unwrap().clone();
    // Discography entries only know the artist page...
    assert!(
        matches!(&app.update(Action::OpenInBrowser)[..], [Effect::OpenUrl(u)] if *u == item.url)
    );
    // ...until the release's details arrive with its own URL.
    app.update(Action::AlbumDue(item.id));
    let album = album_fixture("album");
    let album_url = album.url.clone();
    app.update(Action::AlbumLoaded {
        id: item.id,
        result: Ok(album),
    });
    assert!(
        matches!(&app.update(Action::OpenInBrowser)[..], [Effect::OpenUrl(u)] if *u == album_url)
    );
}

#[test]
fn list_rows_fall_back_to_album_details_for_size() {
    let mut app = loaded();
    // Discover sometimes sends no track count or duration (e.g. "Declan James - Cycle 2").
    app.discover.items[0].track_count = None;
    app.discover.items[0].duration = None;
    let item = app.discover.items[0].clone();
    assert_eq!(app.release_size(&item), (None, None));

    app.update(Action::AlbumDue(item.id));
    app.update(Action::AlbumLoaded {
        id: item.id,
        result: Ok(album_fixture("album")),
    });
    let (tracks, duration) = app.release_size(&item);
    assert_eq!(tracks, Some(10));
    assert_eq!(duration.map(|d| d.as_secs()), Some(2279));

    // Discover's own numbers win when present.
    let other = app.discover.items[1].clone();
    assert_eq!(
        app.release_size(&other),
        (other.track_count, other.duration)
    );
}

/// Starts playing the first release (Ocean Company) so the queue has something to wait for.
fn playing_first() -> App {
    let mut app = with_album();
    app.update(Action::Play);
    assert!(app.now_playing.is_some());
    app
}

fn queue_ended(app: &mut App) -> Vec<Effect> {
    app.update(Action::Player {
        event: Some(PlayerEvent::QueueEnded),
        status: Status::default(),
    })
}

fn queue_ids(app: &App) -> Vec<ItemId> {
    app.queue.iter().map(|e| e.id).collect()
}

#[test]
fn enqueue_adds_without_interrupting_and_saves() {
    let mut app = playing_first();
    app.update(Action::Down);
    let second = app.selected().unwrap().id;
    let effects = app.update(Action::Enqueue);
    assert!(
        matches!(&effects[..], [Effect::SaveSettings(s)] if s.queue.len() == 1),
        "{effects:?}"
    );
    assert_eq!(queue_ids(&app), [second]);
    assert_eq!(
        app.now_playing.as_ref().unwrap().title,
        "Ocean Company",
        "still playing"
    );
    assert!(
        app.message
            .as_ref()
            .unwrap()
            .text
            .starts_with("Added to queue")
    );

    // E puts it first.
    app.update(Action::Down);
    let third = app.selected().unwrap().id;
    app.update(Action::EnqueueNext);
    assert_eq!(queue_ids(&app), [third, second]);
    assert_eq!(
        app.up_next().map(|(e, more)| (e.id, more)),
        Some((third, 1))
    );
}

#[test]
fn enqueue_with_nothing_playing_starts_it() {
    let mut app = with_album();
    let effects = app.update(Action::Enqueue);
    assert!(
        matches!(&effects[..], [Effect::Player(PlayerCommand::PlayQueue(q, 0))] if q.len() == 10),
        "{effects:?}"
    );
    assert!(app.queue.is_empty());
}

#[test]
fn the_queue_plays_before_autoplay() {
    let mut app = playing_first();
    // Queue the third release; autoplay alone would pick the second.
    app.update(Action::Down);
    app.update(Action::Down);
    let third = app.selected().unwrap().id;
    app.update(Action::Enqueue);
    let effects = queue_ended(&mut app);
    assert_eq!(fetched_album(&effects), Some(third), "{effects:?}");
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::SaveSettings(s) if s.queue.is_empty()))
    );
    assert!(app.queue.is_empty());

    let mut album = album_fixture("album");
    album.id = third;
    let effects = app.update(Action::AlbumLoaded {
        id: third,
        result: Ok(album),
    });
    assert!(
        matches!(
            &effects[..],
            [Effect::Player(PlayerCommand::PlayQueue(_, 0))]
        ),
        "{effects:?}"
    );
    // Queue empty now: the next end falls through to autoplay, which continues after the
    // queued release (so the fourth), not after the one played before it.
    let fourth = app.discover.items[3].id;
    let effects = queue_ended(&mut app);
    assert_eq!(fetched_album(&effects), Some(fourth), "{effects:?}");
}

#[test]
fn a_queued_track_plays_alone() {
    let mut app = playing_first();
    // Queue track 3 from the tracklist (the same album, already cached).
    app.update(Action::FocusAlbum);
    app.update(Action::Down);
    app.update(Action::Down);
    app.update(Action::Enqueue);
    let entry = app.queue.front().unwrap().clone();
    assert_eq!(
        (entry.track, entry.track_title.as_deref()),
        (Some(2), Some("Passage"))
    );

    let effects = queue_ended(&mut app);
    let [
        Effect::Player(PlayerCommand::PlayQueue(queue, 2)),
        Effect::SaveSettings(_),
    ] = &effects[..]
    else {
        panic!("{effects:?}");
    };
    let playable: Vec<usize> = (0..queue.len())
        .filter(|&i| queue[i].url.is_some())
        .collect();
    assert_eq!(playable, [2], "only the queued track is streamable");

    // ⏎ on the album afterwards plays all of it again, not just that track.
    app.update(Action::FocusDiscover);
    app.update(Action::Top);
    let effects = app.update(Action::Play);
    assert!(
        matches!(&effects[..], [Effect::Player(PlayerCommand::PlayQueue(q, 0))]
            if q.iter().filter(|t| t.url.is_some()).count() == 10),
        "{effects:?}"
    );
}

#[test]
fn unplayable_queue_entries_are_skipped() {
    let mut app = playing_first();
    let (second, third) = (app.discover.items[1].id, app.discover.items[2].id);
    app.update(Action::Down);
    app.update(Action::Enqueue);
    app.update(Action::Down);
    app.update(Action::Enqueue);
    let effects = queue_ended(&mut app);
    assert_eq!(fetched_album(&effects), Some(second));
    // The second turns out to be a pre-order: move on to the third.
    let effects = app.update(Action::AlbumLoaded {
        id: second,
        result: Ok(album_fixture("preorder")),
    });
    assert_eq!(fetched_album(&effects), Some(third), "{effects:?}");
}

#[test]
fn queue_popup_edits_and_plays() {
    let mut app = playing_first();
    for _ in 0..3 {
        app.update(Action::Down);
        app.update(Action::Enqueue);
    }
    let ids = queue_ids(&app);
    app.update(Action::OpenQueue);
    // Keys go to the popup: Down moves its selection, not the list's.
    let list_selection = app.table.selected();
    app.update(Action::Down);
    assert_eq!(app.table.selected(), list_selection);
    app.update(Action::QueueMoveUp);
    assert_eq!(queue_ids(&app), [ids[1], ids[0], ids[2]]);
    app.update(Action::Bottom);
    assert!(matches!(
        app.update(Action::QueueRemove)[..],
        [Effect::SaveSettings(_)]
    ));
    assert_eq!(queue_ids(&app), [ids[1], ids[0]]);
    // ⏎ plays the highlighted entry now and closes the popup; the rest stays queued.
    app.update(Action::Top);
    let effects = app.update(Action::Play);
    assert_eq!(fetched_album(&effects), Some(ids[1]));
    assert!(app.queue_view.is_none());
    assert_eq!(queue_ids(&app), [ids[0]]);
    // Transport keys still reach the player with the popup open.
    app.update(Action::OpenQueue);
    assert_eq!(
        app.update(Action::TogglePause),
        [Effect::Player(PlayerCommand::TogglePause)]
    );
    app.update(Action::QueueClear);
    assert!(app.queue.is_empty());
}

#[test]
fn queue_survives_a_restart_in_settings() {
    let mut app = playing_first();
    app.update(Action::Down);
    app.update(Action::Enqueue);
    let saved = app.settings().queue;
    assert_eq!(saved.len(), 1);
    let (mut restored, _) = App::new(DiscoverQuery::default(), true);
    restored.queue = Queue::from_saved(saved);
    assert_eq!(queue_ids(&restored), queue_ids(&app));
    assert!(
        restored.now_playing.is_none(),
        "restoring doesn't start playback"
    );
}

#[test]
fn next_entry_is_prefetched_near_the_end() {
    let mut app = playing_first();
    app.update(Action::Down);
    let next = app.selected().unwrap().id;
    app.update(Action::Enqueue);
    let status = |index, position| Status {
        state: PlaybackState::Playing,
        index: Some(index),
        position: std::time::Duration::from_secs(position),
        duration: std::time::Duration::from_secs(200),
        volume: 1.0,
    };
    // Early on the last track, or late on an earlier one: nothing yet.
    let update = |app: &mut App, index, position| {
        app.update(Action::Player {
            event: None,
            status: status(index, position),
        })
    };
    assert_eq!(update(&mut app, 9, 60), []);
    assert_eq!(update(&mut app, 5, 190), []);
    // The last 30 s of the last track: fetch the next entry's details once.
    assert_eq!(fetched_album(&update(&mut app, 9, 175)), Some(next));
    assert_eq!(update(&mut app, 9, 180), [], "already in flight");
}

#[test]
fn transport_keys_do_nothing_until_something_plays() {
    let mut app = loaded();
    assert_eq!(app.update(Action::TogglePause), []);
    assert_eq!(app.update(Action::NextTrack), []);
    app.now_playing = Some(Arc::from(album_fixture("album")));
    assert_eq!(
        app.update(Action::SeekBackward),
        [Effect::Player(PlayerCommand::SeekBy(-10_000))]
    );
}

#[test]
fn volume_steps_and_clamps() {
    let mut app = loaded();
    let effects = app.update(Action::VolumeDown);
    assert_eq!(effects[0], Effect::Player(PlayerCommand::SetVolume(0.95)));
    assert!(
        matches!(&effects[1], Effect::SaveSettings(s) if (s.volume - 0.95).abs() < 1e-6),
        "{effects:?}"
    );
    for _ in 0..30 {
        app.update(Action::VolumeUp);
    }
    assert!((app.player.volume - 1.0).abs() < f32::EPSILON);
}

#[test]
fn no_audio_output_still_allows_browsing() {
    let (mut app, _) = App::new(DiscoverQuery::default(), false);
    assert_eq!(app.message.as_ref().unwrap().kind, MessageKind::Error);
    let generation = app.discover.generation;
    app.update(Action::DiscoverLoaded {
        generation,
        result: Ok(page_fixture()),
    });
    app.update(Action::Down);
    assert_eq!(app.table.selected(), Some(1));
    assert_eq!(app.update(Action::VolumeUp), []);
}

fn playing_track(app: &mut App, index: usize) {
    app.update(Action::Player {
        event: None,
        status: Status {
            state: PlaybackState::Playing,
            index: Some(index),
            ..Status::default()
        },
    });
}

#[test]
fn listening_view_toggles_keeps_focus_and_is_saved() {
    let mut app = with_album();
    app.update(Action::FocusAlbum);
    let effects = app.update(Action::ToggleListening);
    assert_eq!(app.screen, Screen::Listening);
    assert!(
        matches!(&effects[..], [Effect::SaveSettings(s)] if s.screen == Screen::Listening),
        "{effects:?}"
    );
    app.update(Action::ToggleListening);
    assert_eq!(app.screen, Screen::Browse);
    assert_eq!(app.focus, Focus::Album, "focus survives the round trip");
}

#[test]
fn listening_view_fetches_the_playing_cover_not_the_selected_one() {
    let mut app = playing_first();
    let playing_art = app.now_playing.as_ref().unwrap().art_id.unwrap();
    app.update(Action::Down);
    assert_ne!(app.selected().unwrap().art_id, Some(playing_art));
    app.art_enabled = true;
    app.album_pane.art_box = Some(SMALL_BOX);
    app.update(Action::ToggleListening);
    app.listening.art_box = Some(BIG_BOX);

    // No debounce: the first draw fetches it, at the listening view's size.
    let effects = app.after_draw();
    let wanted = crate::art::source_size(BIG_BOX);
    assert!(
        matches!(&effects[..], [Effect::FetchArt { id, size }] if *id == playing_art && *size == wanted),
        "{effects:?}"
    );
    assert_eq!(app.after_draw(), [], "already in flight");

    let effects = app.update(Action::ArtFetched {
        id: playing_art,
        size: wanted,
        result: Ok(bytes::Bytes::from(crate::art::tests::test_jpeg())),
    });
    assert!(
        matches!(&effects[..], [Effect::EncodeArt { id, art_box, .. }] if *id == playing_art && *art_box == BIG_BOX),
        "{effects:?}"
    );
    app.update(Action::ArtEncoded {
        id: playing_art,
        art_box: BIG_BOX,
        result: Ok(encoded(BIG_BOX)),
    });
    assert!(matches!(app.art_for(Some(playing_art)), Some(Some(_))));
    assert_eq!(app.after_draw(), []);
}

#[test]
fn plays_next_walks_the_album_then_the_queue_then_autoplay() {
    let mut app = playing_first();
    playing_track(&mut app, 0);
    let second_title = app.now_playing.as_ref().unwrap().tracks[1].title.clone();
    assert!(matches!(app.plays_next(), Some(UpNext::Track(t)) if t.title == second_title));

    let last = app.now_playing.as_ref().unwrap().tracks.len() - 1;
    playing_track(&mut app, last);
    assert_eq!(app.plays_next(), Some(UpNext::Autoplay));
    app.autoplay = false;
    assert_eq!(app.plays_next(), None);

    app.update(Action::Down);
    app.update(Action::Enqueue);
    assert!(matches!(app.plays_next(), Some(UpNext::Queue(_, 0))));
}

#[test]
fn listen_on_queue_switches_when_the_queue_starts_a_release() {
    for listen in [false, true] {
        let mut app = playing_first();
        app.listen_on_queue = listen;
        app.update(Action::Enqueue);
        queue_ended(&mut app);
        let expected = if listen {
            Screen::Listening
        } else {
            Screen::Browse
        };
        assert_eq!(app.screen, expected, "listen_on_queue = {listen}");
    }
}

#[test]
fn open_in_browser_in_the_listening_view_opens_what_is_playing() {
    let mut app = playing_first();
    let playing_url = app.now_playing.as_ref().unwrap().url.clone();
    app.update(Action::Down);
    app.update(Action::ToggleListening);
    assert!(
        matches!(&app.update(Action::OpenInBrowser)[..], [Effect::OpenUrl(u)] if *u == playing_url)
    );
}

#[test]
fn info_in_the_listening_view_is_its_own_toggle_and_scrolls() {
    let mut app = playing_first();
    app.update(Action::ToggleListening);
    app.update(Action::ToggleInfo);
    assert!(app.listening.show_info);
    assert!(!app.album_pane.show_info, "the album pane keeps its own");
    app.update(Action::Down);
    app.update(Action::Down);
    assert_eq!(app.listening.info_scroll, 2);
    app.update(Action::Up);
    app.update(Action::Up);
    app.update(Action::Up);
    assert_eq!(app.listening.info_scroll, 0, "never above the top");
    let before = app.selected().map(|i| i.id);
    app.update(Action::Bottom);
    assert_eq!(
        app.selected().map(|i| i.id),
        before,
        "the list doesn't move"
    );
    // Back to browsing: `i` is the album pane's again.
    app.update(Action::ToggleListening);
    app.update(Action::ToggleInfo);
    assert!(app.album_pane.show_info);
    assert!(app.listening.show_info, "remembered for next time");
}

#[test]
fn info_messages_expire_but_errors_stay() {
    let mut app = loaded();
    app.update(Action::Notice("first".into()));
    let first = app.expiring_message().unwrap();
    app.update(Action::Notice("second".into()));
    let second = app.expiring_message().unwrap();
    assert_ne!(first, second);
    app.update(Action::ExpireMessage(first));
    assert!(
        app.message.is_some(),
        "a stale timer leaves the newer message"
    );
    app.update(Action::ExpireMessage(second));
    assert!(app.message.is_none());

    app.update(Action::Notice("info".into()));
    let info = app.expiring_message().unwrap();
    app.update(Action::VolumeUp); // with a player: no error, clears the message anyway
    app.error("broken");
    assert_eq!(app.expiring_message(), None, "errors don't expire");
    app.update(Action::ExpireMessage(info));
    assert_eq!(
        app.message.as_ref().map(|m| m.text.as_str()),
        Some("broken")
    );
}

#[test]
fn jump_to_playing_selects_the_release_and_track() {
    let mut app = loaded();
    app.update(Action::JumpToPlaying);
    assert_eq!(
        app.message.as_ref().map(|m| m.text.as_str()),
        Some("Nothing is playing.")
    );

    let mut app = playing_first();
    playing_track(&mut app, 2);
    let playing = app.now_playing.as_ref().map(|a| a.id);
    app.update(Action::Down);
    app.update(Action::Down);
    assert_ne!(app.selected().map(|i| i.id), playing);
    app.update(Action::JumpToPlaying);
    assert_eq!(app.selected().map(|i| i.id), playing);
    assert_eq!(app.album_pane.tracks.selected(), Some(2));
}
