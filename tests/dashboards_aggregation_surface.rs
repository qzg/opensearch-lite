mod support;

use http::Method;
use serde_json::json;
use support::{call, ephemeral_state, ndjson_call};

#[tokio::test]
async fn dashboards_visualization_aggregation_subset_is_supported() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/orders",
        json!({
            "mappings": {
                "properties": {
                    "status": { "type": "keyword" },
                    "created_at": { "type": "date" },
                    "total": { "type": "double" },
                    "quantity": { "type": "long" },
                    "customer": { "type": "keyword" },
                    "region": { "type": "keyword" }
                }
            }
        }),
    )
    .await;
    for (id, status, created_at, total, quantity, customer, region) in [
        ("1", "paid", "2026-04-01T10:00:00Z", 42.0, 1, "c1", "north"),
        ("2", "paid", "2026-04-01T12:00:00Z", 58.0, 2, "c2", "north"),
        ("3", "open", "2026-04-02T09:00:00Z", 25.0, 3, "c1", "south"),
        (
            "4",
            "refunded",
            "2026-04-03T09:00:00Z",
            10.0,
            1,
            "c3",
            "south",
        ),
    ] {
        call(
            &state,
            Method::PUT,
            &format!("/orders/_doc/{id}"),
            json!({
                "status": status,
                "created_at": created_at,
                "total": total,
                "quantity": quantity,
                "customer": customer,
                "region": region
            }),
        )
        .await;
    }
    call(
        &state,
        Method::PUT,
        "/orders/_doc/5",
        json!({ "created_at": "2026-04-03T10:00:00Z", "total": 5.0, "quantity": 1 }),
    )
    .await;

    let response = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "size": 0,
            "aggs": {
                "by_status": {
                    "terms": { "field": "status", "size": 10 },
                    "aggs": {
                        "total_sum": { "sum": { "field": "total" } },
                        "sample": {
                            "top_hits": {
                                "size": 1,
                                "sort": [{ "total": { "order": "desc" } }],
                                "_source": ["status", "total"]
                            }
                        }
                    }
                },
                "by_day": {
                    "date_histogram": { "field": "created_at", "calendar_interval": "day" },
                    "aggs": { "quantity": { "sum": { "field": "quantity" } } }
                },
                "total_histogram": { "histogram": { "field": "total", "interval": 25 } },
                "total_ranges": {
                    "range": {
                        "field": "total",
                        "ranges": [
                            { "to": 25 },
                            { "from": 25, "to": 60 },
                            { "from": 60 }
                        ]
                    }
                },
                "status_filters": {
                    "filters": {
                        "filters": {
                            "paid": { "term": { "status": "paid" } },
                            "south": { "term": { "region": "south" } }
                        }
                    }
                },
                "missing_status": { "missing": { "field": "status" } },
                "unique_customers": { "cardinality": { "field": "customer" } },
                "status_count": { "value_count": { "field": "status" } }
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
    assert_eq!(
        body["aggregations"]["by_status"]["buckets"][0]["total_sum"]["value"],
        100.0
    );
    assert_eq!(
        body["aggregations"]["by_status"]["buckets"][0]["sample"]["hits"]["hits"][0]["_source"],
        json!({ "status": "paid", "total": 58.0 })
    );
    assert_eq!(
        body["aggregations"]["by_day"]["buckets"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        body["aggregations"]["total_histogram"]["buckets"][0]["doc_count"],
        2
    );
    assert_eq!(
        body["aggregations"]["total_ranges"]["buckets"][1]["doc_count"],
        3
    );
    assert_eq!(
        body["aggregations"]["status_filters"]["buckets"]["paid"]["doc_count"],
        2
    );
    assert_eq!(body["aggregations"]["missing_status"]["doc_count"], 1);
    assert_eq!(body["aggregations"]["unique_customers"]["value"], 3);
    assert_eq!(body["aggregations"]["status_count"]["value"], 4);
}

#[tokio::test]
async fn unsupported_and_over_limit_aggregations_return_structured_errors() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/orders/_doc/1",
        json!({ "status": "paid" }),
    )
    .await;

    let unsupported = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({ "aggs": { "scripts": { "scripted_metric": {} } } }),
    )
    .await;
    assert_eq!(unsupported.status, 400);
    assert_eq!(
        unsupported.body.unwrap()["error"]["type"],
        "x_content_parse_exception"
    );

    let too_many = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "aggs": {
                "too_many": {
                    "terms": { "field": "status", "size": 10001 }
                }
            }
        }),
    )
    .await;
    assert_eq!(too_many.status, 400);
    assert_eq!(
        too_many.body.unwrap()["error"]["type"],
        "x_content_parse_exception"
    );

    let array_filters = (0..=10_000)
        .map(|_| json!({ "match_all": {} }))
        .collect::<Vec<_>>();
    let too_many_array_filters = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "aggs": {
                "too_many_filters": {
                    "filters": { "filters": array_filters }
                }
            }
        }),
    )
    .await;
    assert_eq!(too_many_array_filters.status, 400);
    assert_eq!(
        too_many_array_filters.body.unwrap()["error"]["type"],
        "x_content_parse_exception"
    );

    let top_hits_over_window = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "aggs": {
                "sample": {
                    "top_hits": { "size": 10001 }
                }
            }
        }),
    )
    .await;
    assert_eq!(top_hits_over_window.status, 400);
    assert_eq!(
        top_hits_over_window.body.unwrap()["error"]["type"],
        "illegal_argument_exception"
    );

    let tags = (0..=10_000)
        .map(|value| json!(format!("tag-{value}")))
        .collect::<Vec<_>>();
    call(
        &state,
        Method::PUT,
        "/orders/_doc/high-cardinality",
        json!({ "tags": tags }),
    )
    .await;
    let discovered_bucket_overflow = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "aggs": {
                "tags": {
                    "terms": { "field": "tags", "size": 1 }
                }
            }
        }),
    )
    .await;
    assert_eq!(discovered_bucket_overflow.status, 400);
    assert_eq!(
        discovered_bucket_overflow.body.unwrap()["error"]["type"],
        "x_content_parse_exception"
    );

    let malformed_filter = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "aggs": {
                "bad_filter": {
                    "filters": {
                        "filters": {
                            "bad": { "unsupported_query": {} }
                        }
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(malformed_filter.status, 400);
    assert_eq!(
        malformed_filter.body.unwrap()["error"]["type"],
        "x_content_parse_exception"
    );

    let malformed_filter_msearch = ndjson_call(
        &state,
        Method::POST,
        "/orders/_msearch",
        r#"{}
{"aggs":{"bad_filter":{"filters":{"filters":{"bad":{"unsupported_query":{}}}}}}}
"#,
    )
    .await;
    assert_eq!(malformed_filter_msearch.status, 200);
    assert_eq!(
        malformed_filter_msearch.body.unwrap()["responses"][0]["error"]["type"],
        "x_content_parse_exception"
    );
}
