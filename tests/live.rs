//! Smoke tests against the real Bandcamp. Ignored by default because they need the network
//! and Bandcamp's undocumented API may change. Run with `just test-live`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use bandcamp_tui::bandcamp::{BandcampClient, DiscoverQuery, Slice, TralbumKind};

#[tokio::test]
#[ignore = "hits bandcamp.com"]
async fn discover_then_album_yields_playable_tracks() {
    let client = BandcampClient::new().unwrap();
    let query = DiscoverQuery {
        tags: vec!["electronic".into()],
        slice: Slice::New,
        page_size: 10,
        ..DiscoverQuery::default()
    };

    let page = client.discover(&query, None).await.unwrap();
    assert!(!page.items.is_empty(), "discover returned no items");
    assert!(page.total >= page.items.len() as u64);
    assert!(page.items.iter().all(|i| i.kind == TralbumKind::Album));

    let next = client
        .discover(&query, page.next_cursor.as_ref())
        .await
        .unwrap();
    assert!(!next.items.is_empty(), "second page is empty");
    assert_ne!(page.items[0].id, next.items[0].id, "cursor did not advance");

    // Some new releases are pre-orders with nothing to stream; find one that plays.
    let mut found = false;
    for item in page
        .items
        .iter()
        .chain(&next.items)
        .filter(|i| !i.is_preorder)
    {
        let album = client
            .album(item.band_id, item.id, item.kind)
            .await
            .unwrap();
        assert_eq!(album.id, item.id);
        assert_eq!(album.title, item.title);
        if album.has_playable_tracks() {
            found = true;
            break;
        }
    }
    assert!(found, "no playable album in the first two pages");
}

#[tokio::test]
#[ignore = "hits bandcamp.com"]
async fn stream_url_serves_mp3() {
    let client = BandcampClient::new().unwrap();
    let page = client
        .discover(
            &DiscoverQuery {
                page_size: 10,
                ..DiscoverQuery::default()
            },
            None,
        )
        .await
        .unwrap();
    let item = page.items.iter().find(|i| !i.is_preorder).unwrap();
    let album = client
        .album(item.band_id, item.id, item.kind)
        .await
        .unwrap();
    let url = album
        .tracks
        .iter()
        .find_map(|t| t.stream_url.clone())
        .expect("a best-seller has a playable track");

    // The redirect is followed automatically; ask for the first KiB only.
    let response = client
        .http()
        .get(url)
        .header("range", "bytes=0-1023")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 206);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "audio/mpeg"
    );
}

#[tokio::test]
#[ignore = "hits bandcamp.com"]
async fn refreshes_a_cdn_stream_url() {
    let client = BandcampClient::new().unwrap();
    let page = client
        .discover(
            &DiscoverQuery {
                page_size: 10,
                ..DiscoverQuery::default()
            },
            None,
        )
        .await
        .unwrap();
    let item = page.items.iter().find(|i| !i.is_preorder).unwrap();
    let album = client
        .album(item.band_id, item.id, item.kind)
        .await
        .unwrap();
    let redirect = album
        .tracks
        .iter()
        .find_map(|t| t.stream_url.clone())
        .unwrap();
    // Resolve the redirect without downloading the audio.
    let cdn = client
        .http()
        .get(redirect)
        .header("range", "bytes=0-0")
        .send()
        .await
        .unwrap()
        .url()
        .clone();
    assert!(cdn.path().starts_with("/stream/"), "{cdn}");
    let fresh = client.refresh_stream_url(&cdn).await.unwrap();
    assert_eq!(fresh.path(), cdn.path());
}
