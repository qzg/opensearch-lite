mod support;

use http::Method;
use serde_json::json;
use std::collections::BTreeSet;
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
async fn multi_index_alias_reads_expand_to_all_targets() {
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
        "/orders-shadow/_doc/2",
        json!({ "status": "pending" }),
    )
    .await;
    let alias = call(
        &state,
        Method::POST,
        "/_aliases",
        json!({
            "actions": [
                { "add": { "indices": ["orders", "orders-shadow"], "alias": "orders-read" } }
            ]
        }),
    )
    .await;
    assert_eq!(alias.status, 200);

    let resolved = call(
        &state,
        Method::GET,
        "/_resolve/index/orders-read",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(resolved.status, 200);
    assert_eq!(
        resolved.body.unwrap()["aliases"][0]["indices"],
        json!(["orders", "orders-shadow"])
    );

    let caps = call(
        &state,
        Method::GET,
        "/orders-read/_field_caps?fields=status",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(caps.status, 200);
    let body = caps.body.unwrap();
    assert_eq!(body["indices"], json!(["orders", "orders-shadow"]));
    assert_eq!(
        body["fields"]["status"]["keyword"]["indices"],
        json!(["orders", "orders-shadow"])
    );

    let search = call(
        &state,
        Method::POST,
        "/orders-read/_search",
        json!({ "query": { "match_all": {} }, "size": 10 }),
    )
    .await;
    assert_eq!(search.status, 200);
    let body = search.body.unwrap();
    assert_eq!(body["hits"]["total"]["value"], 2);
    let hit_indices = body["hits"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|hit| hit["_index"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(hit_indices, BTreeSet::from(["orders", "orders-shadow"]));

    let count = call(
        &state,
        Method::POST,
        "/orders-read/_count",
        json!({ "query": { "match_all": {} } }),
    )
    .await;
    assert_eq!(count.status, 200);
    assert_eq!(count.body.unwrap()["count"], 2);
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
async fn msearch_pit_and_search_after_fail_closed_per_item() {
    let state = ephemeral_state();

    let response = ndjson_call(
        &state,
        Method::POST,
        "/_msearch",
        r#"{}
{"pit":{"id":"pit-missing"},"query":{"match_all":{}}}
{}
{"sort":[{"_id":{"order":"asc"}}],"search_after":["1"],"query":{"match_all":{}}}
"#,
    )
    .await;

    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert_eq!(
        body["responses"][0]["error"]["type"],
        "opensearch_lite_unsupported_api_exception"
    );
    assert_eq!(body["responses"][0]["status"], 501);
    assert_eq!(
        body["responses"][1]["error"]["type"],
        "opensearch_lite_unsupported_api_exception"
    );
    assert_eq!(body["responses"][1]["status"], 501);
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
            "dashboard": {
                "title": "Northwind sales dashboard"
            },
            "references": [
                { "type": "index-pattern", "id": "orders" },
                { "type": "visualization", "id": "sales" }
            ]
        }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/saved/_doc/2",
        json!({
            "dashboard": {
                "title": "Inventory overview"
            },
            "references": []
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

    let default_or_all_fields = call(
        &state,
        Method::POST,
        "/saved/_search",
        json!({
            "query": {
                "simple_query_string": {
                    "query": "sales inventory",
                    "fields": ["*"],
                    "default_operator": "OR"
                }
            }
        }),
    )
    .await;
    assert_eq!(default_or_all_fields.status, 200);
    assert_eq!(
        default_or_all_fields.body.unwrap()["hits"]["total"]["value"],
        2
    );

    let boosted_and_multifield = call(
        &state,
        Method::POST,
        "/saved/_search",
        json!({
            "query": {
                "simple_query_string": {
                    "query": "northwind dashboard",
                    "fields": ["dashboard.title^3", "dashboard.title.raw"],
                    "default_operator": "AND"
                }
            }
        }),
    )
    .await;
    assert_eq!(boosted_and_multifield.status, 200);
    assert_eq!(
        boosted_and_multifield.body.unwrap()["hits"]["total"]["value"],
        1
    );

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
async fn validate_query_analyze_and_explain_are_bounded_local_scaffolds() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/orders/_doc/1",
        json!({ "status": "paid", "title": "Northwind espresso grinder" }),
    )
    .await;

    let valid = call(
        &state,
        Method::POST,
        "/orders/_validate/query?explain=true",
        json!({ "query": { "term": { "status": "paid" } } }),
    )
    .await;
    assert_eq!(valid.status, 200);
    let body = valid.body.unwrap();
    assert_eq!(body["valid"], true);
    assert_eq!(body["explanations"][0]["valid"], true);

    let invalid = call(
        &state,
        Method::POST,
        "/orders/_validate/query",
        deeply_nested_bool(33),
    )
    .await;
    assert_eq!(invalid.status, 400);
    assert_eq!(
        invalid.body.unwrap()["error"]["type"],
        "x_content_parse_exception"
    );

    let analyze = call(
        &state,
        Method::POST,
        "/_analyze",
        json!({ "analyzer": "standard", "text": "Northwind espresso" }),
    )
    .await;
    assert_eq!(analyze.status, 200);
    let tokens = analyze.body.unwrap()["tokens"].as_array().unwrap().clone();
    assert_eq!(tokens[0]["token"], "northwind");
    assert_eq!(tokens[1]["token"], "espresso");

    let unsupported_analyzer = call(
        &state,
        Method::POST,
        "/_analyze",
        json!({ "analyzer": "custom_icu", "text": "hello" }),
    )
    .await;
    assert_eq!(unsupported_analyzer.status, 400);

    let explain = call(
        &state,
        Method::POST,
        "/orders/_explain/1",
        json!({ "query": { "term": { "status": "paid" } } }),
    )
    .await;
    assert_eq!(explain.status, 200);
    let body = explain.body.unwrap();
    assert_eq!(body["matched"], true);
    assert_eq!(
        body["explanation"]["description"],
        "OpenSearch Lite local evaluator match result"
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

#[tokio::test]
async fn sorted_search_returns_sort_values_and_accepts_search_after() {
    let state = ephemeral_state();
    for (id, rank) in [("1", 10), ("2", 20), ("3", 30)] {
        call(
            &state,
            Method::PUT,
            &format!("/orders/_doc/{id}"),
            json!({ "rank": rank }),
        )
        .await;
    }

    let first = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "size": 2,
            "query": { "match_all": {} },
            "sort": [
                { "rank": { "order": "asc" } },
                { "_id": { "order": "asc" } }
            ]
        }),
    )
    .await;
    assert_eq!(first.status, 200);
    let body = first.body.unwrap();
    assert_eq!(body["hits"]["hits"][0]["_id"], "1");
    assert_eq!(body["hits"]["hits"][1]["_id"], "2");
    assert_eq!(body["hits"]["hits"][1]["sort"], json!([20, "2"]));

    let after = body["hits"]["hits"][1]["sort"].clone();
    let second = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "size": 2,
            "query": { "match_all": {} },
            "sort": [
                { "rank": { "order": "asc" } },
                { "_id": { "order": "asc" } }
            ],
            "search_after": after
        }),
    )
    .await;
    assert_eq!(second.status, 200);
    let body = second.body.unwrap();
    assert_eq!(body["hits"]["total"]["value"], 3);
    assert_eq!(body["hits"]["hits"].as_array().unwrap().len(), 1);
    assert_eq!(body["hits"]["hits"][0]["_id"], "3");
}

#[tokio::test]
async fn search_after_with_duplicate_sort_values_returns_equal_sort_hits() {
    let state = ephemeral_state();
    for (id, rank) in [("1", 10), ("2", 10), ("3", 20)] {
        call(
            &state,
            Method::PUT,
            &format!("/orders/_doc/{id}"),
            json!({ "rank": rank }),
        )
        .await;
    }

    let first = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "size": 1,
            "query": { "match_all": {} },
            "sort": [{ "rank": { "order": "asc" } }]
        }),
    )
    .await;
    assert_eq!(first.status, 200);
    let body = first.body.unwrap();
    assert_eq!(body["hits"]["hits"][0]["_id"], "1");
    assert_eq!(
        body["hits"]["hits"][0]["sort"],
        json!([10, "orders\u{1f}1"])
    );

    let second = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "size": 1,
            "query": { "match_all": {} },
            "sort": [{ "rank": { "order": "asc" } }],
            "search_after": body["hits"]["hits"][0]["sort"].clone()
        }),
    )
    .await;
    assert_eq!(second.status, 200);
    let body = second.body.unwrap();
    assert_eq!(body["hits"]["hits"][0]["_id"], "2");
    assert_eq!(
        body["hits"]["hits"][0]["sort"],
        json!([10, "orders\u{1f}2"])
    );
}

#[tokio::test]
async fn descending_sorted_search_after_returns_next_page() {
    let state = ephemeral_state();
    for (id, rank) in [("1", 10), ("2", 20), ("3", 30)] {
        call(
            &state,
            Method::PUT,
            &format!("/orders/_doc/{id}"),
            json!({ "rank": rank }),
        )
        .await;
    }

    let first = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "size": 1,
            "query": { "match_all": {} },
            "sort": [
                { "rank": { "order": "desc" } },
                { "_id": { "order": "asc" } }
            ]
        }),
    )
    .await;
    assert_eq!(first.status, 200);
    let body = first.body.unwrap();
    assert_eq!(body["hits"]["hits"][0]["_id"], "3");
    assert_eq!(body["hits"]["hits"][0]["sort"], json!([30, "3"]));

    let second = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "size": 1,
            "query": { "match_all": {} },
            "sort": [
                { "rank": { "order": "desc" } },
                { "_id": { "order": "asc" } }
            ],
            "search_after": body["hits"]["hits"][0]["sort"].clone()
        }),
    )
    .await;
    assert_eq!(second.status, 200);
    let body = second.body.unwrap();
    assert_eq!(body["hits"]["hits"].as_array().unwrap().len(), 1);
    assert_eq!(body["hits"]["hits"][0]["_id"], "2");
    assert_eq!(body["hits"]["hits"][0]["sort"], json!([20, "2"]));
}
