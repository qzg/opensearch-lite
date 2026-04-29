mod support;

use http::Method;
use serde_json::json;
use support::{call, ephemeral_state, ndjson_call};

#[tokio::test]
async fn count_returns_search_count_shape() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/orders/_doc/1",
        json!({ "status": "paid" }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/orders/_doc/2",
        json!({ "status": "refunded" }),
    )
    .await;

    let response = call(
        &state,
        Method::POST,
        "/orders/_count",
        json!({ "query": { "term": { "status": "paid" } } }),
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body.unwrap()["count"], 1);
}

#[tokio::test]
async fn msearch_runs_multiple_queries() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/orders/_doc/1",
        json!({ "status": "paid" }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/orders/_doc/2",
        json!({ "status": "refunded" }),
    )
    .await;

    let response = ndjson_call(
        &state,
        Method::POST,
        "/orders/_msearch",
        r#"{}
{"query":{"term":{"status":"paid"}}}
{}
{"query":{"term":{"status":"refunded"}}}
"#,
    )
    .await;

    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert_eq!(body["responses"][0]["hits"]["total"]["value"], 1);
    assert_eq!(body["responses"][1]["hits"]["total"]["value"], 1);
}

#[tokio::test]
async fn msearch_reports_item_errors_without_failing_whole_request() {
    let state = ephemeral_state();

    let response = ndjson_call(
        &state,
        Method::POST,
        "/_msearch",
        r#"{"index":"missing"}
{"query":{"match_all":{}}}
"#,
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body.unwrap()["responses"][0]["error"]["type"],
        "index_not_found_exception"
    );
}

#[tokio::test]
async fn terms_match_phrase_prefix_and_wildcard_queries_work_for_scalar_scans() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/products/_doc/1",
        json!({ "name": "Northwind espresso grinder", "tags": ["coffee", "hardware"] }),
    )
    .await;

    let terms = call(
        &state,
        Method::POST,
        "/products/_search",
        json!({ "query": { "terms": { "tags": ["coffee"] } } }),
    )
    .await;
    assert_eq!(terms.body.unwrap()["hits"]["total"]["value"], 1);

    let match_phrase_prefix = call(
        &state,
        Method::POST,
        "/products/_search",
        json!({ "query": { "match_phrase_prefix": { "name": "northwind espresso" } } }),
    )
    .await;
    assert_eq!(
        match_phrase_prefix.body.unwrap()["hits"]["total"]["value"],
        1
    );

    let wildcard = call(
        &state,
        Method::POST,
        "/products/_search",
        json!({ "query": { "wildcard": { "name": "*grinder" } } }),
    )
    .await;
    assert_eq!(wildcard.body.unwrap()["hits"]["total"]["value"], 1);
}
