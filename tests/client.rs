//! `BandcampClient` against a mock server: request shape, pagination and error handling.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use bandcamp_tui::bandcamp::{
    BandId, BandcampClient, DiscoverQuery, Error, ItemId, Slice, TralbumKind,
};
use serde_json::json;
use wiremock::matchers::{body_json, header, header_regex, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DISCOVER_PATH: &str = "/api/discover/1/discover_web";
const TRALBUM_PATH: &str = "/api/mobile/25/tralbum_details";

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn json_response(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

fn client_for(server: &MockServer) -> BandcampClient {
    BandcampClient::with_base_url(server.uri().parse().unwrap()).unwrap()
}

#[tokio::test]
async fn discover_sends_the_websites_request_and_paginates() {
    let server = MockServer::start().await;
    let query = DiscoverQuery {
        tags: vec!["electronic".into()],
        slice: Slice::New,
        time_facet_id: Some(1),
        page_size: 4,
        ..DiscoverQuery::default()
    };
    let base_body = json!({
        "category_id": 0,
        "tag_norm_names": ["electronic"],
        "geoname_id": 0,
        "slice": "new",
        "time_facet_id": 1,
        "cursor": "*",
        "size": 4,
        "include_result_types": ["a"],
        "followed_bands": false
    });

    Mock::given(method("POST"))
        .and(path(DISCOVER_PATH))
        .and(header("content-type", "application/json"))
        .and(header_regex("user-agent", r"^bandcamp-tui/\d+\.\d+\.\d+ "))
        .and(body_json(&base_body))
        .respond_with(json_response(fixture("discover_electronic_top.json")))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server);
    let first = client.discover(&query, None).await.unwrap();
    assert_eq!(first.items.len(), 4);
    let cursor = first.next_cursor.expect("first page has a cursor");

    // The second request must carry the cursor from the first response.
    let mut next_body = base_body.clone();
    let fixture_cursor: serde_json::Value =
        serde_json::from_str(&fixture("discover_electronic_top.json")).unwrap();
    next_body["cursor"] = fixture_cursor["cursor"].clone();
    assert!(next_body["cursor"].is_string());
    Mock::given(method("POST"))
        .and(path(DISCOVER_PATH))
        .and(body_json(&next_body))
        .respond_with(json_response(fixture("discover_empty.json")))
        .expect(1)
        .mount(&server)
        .await;

    let last = client.discover(&query, Some(&cursor)).await.unwrap();
    assert_eq!(last.items, []);
    assert_eq!(last.next_cursor, None);
}

#[tokio::test]
async fn album_sends_ids_as_query_params() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(TRALBUM_PATH))
        .and(query_param("band_id", "2686449259"))
        .and(query_param("tralbum_id", "3457085593"))
        .and(query_param("tralbum_type", "a"))
        .respond_with(json_response(fixture("tralbum_album.json")))
        .expect(1)
        .mount(&server)
        .await;

    let album = client_for(&server)
        .album(
            BandId(2_686_449_259),
            ItemId(3_457_085_593),
            TralbumKind::Album,
        )
        .await
        .unwrap();
    assert_eq!(album.title, "Ocean Company");
    assert_eq!(album.tracks.len(), 10);
}

#[tokio::test]
async fn api_error_in_200_body_becomes_error_api() {
    let server = MockServer::start().await;
    Mock::given(path(TRALBUM_PATH))
        .respond_with(json_response(fixture("error_tralbum.json")))
        .mount(&server)
        .await;
    Mock::given(path(DISCOVER_PATH))
        .respond_with(json_response(fixture("error_exception.json")))
        .mount(&server)
        .await;
    let client = client_for(&server);

    let err = client
        .album(BandId(1), ItemId(1), TralbumKind::Album)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::Api { endpoint: "tralbum_details", message } if message.contains("No such tralbum")),
        "{err:?}"
    );

    let err = client
        .discover(&DiscoverQuery::default(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(
            &err,
            Error::Api {
                endpoint: "discover_web",
                ..
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn http_error_status_becomes_error_status() {
    let server = MockServer::start().await;
    Mock::given(path(DISCOVER_PATH))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .discover(&DiscoverQuery::default(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::Status { status, .. } if status.as_u16() == 503),
        "{err:?}"
    );
}

#[tokio::test]
async fn html_instead_of_json_becomes_error_decode() {
    let server = MockServer::start().await;
    Mock::given(path(DISCOVER_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>captcha</html>"))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .discover(&DiscoverQuery::default(), None)
        .await
        .unwrap_err();
    assert!(matches!(&err, Error::Decode { .. }), "{err:?}");
}

#[tokio::test]
async fn connection_refused_becomes_error_request() {
    // Bind and release a port so nothing listens on it. (Dropping a `MockServer` doesn't
    // work: wiremock returns it to a pool and keeps it listening.)
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let client =
        BandcampClient::with_base_url(format!("http://127.0.0.1:{port}").parse().unwrap()).unwrap();
    let err = client
        .discover(&DiscoverQuery::default(), None)
        .await
        .unwrap_err();
    assert!(matches!(&err, Error::Request { .. }), "{err:?}");
}

#[tokio::test]
async fn refresh_stream_url_returns_the_new_url() {
    let server = MockServer::start().await;
    let old = "https://t4.bcbits.com/stream/abc/mp3-128/1?p=0&ts=1&t=x&token=1_y";
    Mock::given(method("GET"))
        .and(path("/api/stream/1/refresh"))
        .and(query_param("url", old))
        .respond_with(json_response(
            r#"{"track_id":1,"url":"https://t4.bcbits.com/stream/abc/mp3-128/1?p=0&ts=2&t=z&token=2_w"}"#
                .into(),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/stream/1/refresh"))
        .respond_with(json_response(
            r#"{"error":true,"error_message":"invalid url"}"#.into(),
        ))
        .mount(&server)
        .await;
    let client = client_for(&server);

    let fresh = client
        .refresh_stream_url(&old.parse().unwrap())
        .await
        .unwrap();
    assert!(fresh.as_str().contains("ts=2"), "{fresh}");

    let err = client
        .refresh_stream_url(&"https://bandcamp.com/stream_redirect?x".parse().unwrap())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::Api { message, .. } if message == "invalid url"),
        "{err:?}"
    );
}
