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

#[tokio::test]
async fn simple_query_string_and_nested_queries_support_discover_shapes() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/saved/_doc/1",
        json!({
            "title": "Northwind sales dashboard",
            "references": [
                { "type": "index-pattern", "id": "orders" },
                { "type": "visualization", "id": "sales" }
            ]
        }),
    )
    .await;

    let simple = call(
        &state,
        Method::POST,
        "/saved/_search",
        json!({
            "query": {
                "simple_query_string": {
                    "query": "sales dashboard",
                    "fields": ["title"]
                }
            }
        }),
    )
    .await;
    assert_eq!(simple.status, 200);
    assert_eq!(simple.body.unwrap()["hits"]["total"]["value"], 1);

    let nested = call(
        &state,
        Method::POST,
        "/saved/_search",
        json!({
            "query": {
                "nested": {
                    "path": "references",
                    "query": {
                        "bool": {
                            "filter": [
                                { "term": { "type": "index-pattern" } },
                                { "term": { "id": "orders" } }
                            ]
                        }
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(nested.status, 200);
    assert_eq!(nested.body.unwrap()["hits"]["total"]["value"], 1);

    let path_prefixed_nested = call(
        &state,
        Method::POST,
        "/saved/_search",
        json!({
            "query": {
                "nested": {
                    "path": "references",
                    "query": {
                        "bool": {
                            "filter": [
                                { "term": { "references.type": "index-pattern" } },
                                { "term": { "references.id": "orders" } }
                            ]
                        }
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(path_prefixed_nested.status, 200);
    assert_eq!(
        path_prefixed_nested.body.unwrap()["hits"]["total"]["value"],
        1
    );
}

#[tokio::test]
async fn search_guardrails_reject_expensive_requests_before_scan() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/guarded/_doc/1",
        json!({ "status": "paid" }),
    )
    .await;

    let terms = (0..4097).map(|value| json!(value)).collect::<Vec<_>>();
    let too_many_terms = call(
        &state,
        Method::POST,
        "/guarded/_search",
        json!({ "query": { "terms": { "status": terms } } }),
    )
    .await;
    assert_eq!(too_many_terms.status, 400);
    assert_eq!(
        too_many_terms.body.unwrap()["error"]["type"],
        "x_content_parse_exception"
    );

    let too_deep = call(
        &state,
        Method::POST,
        "/guarded/_search",
        deeply_nested_bool(33),
    )
    .await;
    assert_eq!(too_deep.status, 400);
    assert_eq!(
        too_deep.body.unwrap()["error"]["type"],
        "x_content_parse_exception"
    );

    let msearch = ndjson_call(
        &state,
        Method::POST,
        "/guarded/_msearch",
        r#"{}
{"from":10000,"size":1,"query":{"match_all":{}}}
"#,
    )
    .await;
    assert_eq!(msearch.status, 200);
    assert_eq!(
        msearch.body.unwrap()["responses"][0]["error"]["type"],
        "illegal_argument_exception"
    );
}

fn deeply_nested_bool(depth: usize) -> serde_json::Value {
    let mut query = json!({ "match_all": {} });
    for _ in 0..depth {
        query = json!({ "bool": { "filter": query } });
    }
    json!({ "query": query })
}

#[tokio::test]
async fn basic_metric_and_terms_aggregations_are_returned() {
    let state = ephemeral_state();
    for (id, status, total) in [
        ("1", "paid", 42.0),
        ("2", "paid", 58.0),
        ("3", "refunded", 10.0),
    ] {
        call(
            &state,
            Method::PUT,
            &format!("/orders/_doc/{id}"),
            json!({ "status": status, "total": total }),
        )
        .await;
    }

    let response = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "size": 0,
            "aggs": {
                "by_status": { "terms": { "field": "status" } },
                "total_stats": { "stats": { "field": "total" } },
                "avg_total": { "avg": { "field": "total" } }
            }
        }),
    )
    .await;

    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert_eq!(
        body["aggregations"]["by_status"]["buckets"][0]["key"],
        "paid"
    );
    assert_eq!(
        body["aggregations"]["by_status"]["buckets"][0]["doc_count"],
        2
    );
    assert_eq!(body["aggregations"]["total_stats"]["count"], 3);
    assert_eq!(body["aggregations"]["total_stats"]["sum"], 110.0);
    assert_eq!(body["aggregations"]["avg_total"]["value"], 110.0 / 3.0);
}

#[tokio::test]
async fn search_with_aggregations_still_honors_hit_pagination() {
    let state = ephemeral_state();
    for id in ["1", "2", "3"] {
        call(
            &state,
            Method::PUT,
            &format!("/orders/_doc/{id}"),
            json!({ "status": "paid" }),
        )
        .await;
    }

    let response = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "from": 1,
            "size": 1,
            "query": { "match_all": {} },
            "aggs": {
                "by_status": { "terms": { "field": "status" } }
            }
        }),
    )
    .await;

    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert_eq!(body["hits"]["hits"][0]["_id"], "2");
    assert_eq!(
        body["aggregations"]["by_status"]["buckets"][0]["doc_count"],
        3
    );
}
