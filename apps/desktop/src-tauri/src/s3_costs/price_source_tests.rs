use std::time::Duration;

use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::test_support::TestDir;

const BUNDLED_JSON: &str = include_str!("../../../../../crates/cmdr-s3/src/cost/s3-prices.json");
const HOUR: Duration = Duration::from_secs(60 * 60);

async fn serving(response: ResponseTemplate) -> (MockServer, String) {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(response).mount(&server).await;
    let url = format!("{}/s3-prices/v1", server.uri());
    (server, url)
}

#[tokio::test]
async fn a_served_table_parses_and_comes_back_with_its_json() {
    let (_server, url) = serving(ResponseTemplate::new(200).set_body_string(BUNDLED_JSON)).await;
    let (table, json) = fetch(&url).await.expect("a valid table");
    assert_eq!(table, PriceTable::bundled());
    assert_eq!(json, BUNDLED_JSON);
}

#[tokio::test]
async fn a_table_this_build_cant_read_is_refused() {
    let newer = r#"{"schemaVersion": 2, "providers": {}}"#;
    let (_server, url) = serving(ResponseTemplate::new(200).set_body_string(newer)).await;
    assert!(matches!(fetch(&url).await, Err(FetchError::Table(_))));
}

#[tokio::test]
async fn a_server_error_is_a_refused_request() {
    let (_server, url) = serving(ResponseTemplate::new(503)).await;
    assert!(matches!(
        fetch(&url).await,
        Err(FetchError::Request(ServerRequestError::Refused { status: 503, .. }))
    ));
}

#[test]
fn the_cache_round_trips_and_a_bad_one_is_ignored() {
    let dir = TestDir::new("s3-price-cache");
    let path = dir.join(CACHE_FILE);
    assert!(read_cache(&path).is_none(), "no file yet");

    write_cache(&path, BUNDLED_JSON).expect("written");
    assert_eq!(read_cache(&path), Some(PriceTable::bundled()));

    std::fs::write(&path, "{not json").expect("written");
    assert!(read_cache(&path).is_none(), "a broken cache falls back");
}

#[test]
fn refresh_when_the_cache_is_missing_or_a_day_old_unless_asked_lately() {
    assert!(needs_refresh(None, None), "no cache yet");
    assert!(needs_refresh(Some(25 * HOUR), None), "a day old");
    assert!(!needs_refresh(Some(HOUR), None), "fresh");
    assert!(!needs_refresh(None, Some(HOUR)), "this run asked an hour ago");
    assert!(needs_refresh(None, Some(25 * HOUR)), "and a day ago is long enough");
}
